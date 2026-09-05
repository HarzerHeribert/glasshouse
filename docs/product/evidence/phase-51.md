# Phase 51 — evaluation hooks

### Lines 1822, 1826, 1856 (batch 51). Migration 15 and the `evaluation` module.

State: **COMPLETE** for all three — orchestrator ruling.

Design: `docs/product/design-phase51-event-log.md`. It unblocks 7 of Phase 51's
37 lines and closes these 3; twenty have no producer and are not schema work.

Production: migration 15 (`evaluation_observations`, `CREATE TABLE` + index +
migration 11's two project triggers, `SUPPORTED_SCHEMA_VERSION` 14→15);
`src/evaluation/mod.rs`; and the producer at `main.rs:2945` inside
`memory_search_grouped` — **the shared core both `glasshouse memory search` and
`api::unix::query_memory` pass through**, so the machine door is counted by the
same row without touching `api/unix.rs`.

Regression: `tests/evaluation_observations.rs`, driving the shipped binary as a
process against three planted memories (current, superseded, needs-review).
Mutation `drop-the-retrieval-producer` re-run by the orchestrator in the
integrated tree: **KILLED** by three tests.

**A product insight the design missed, and the line needed it.** A search run
with `--history` is *asking* for superseded memories, so counting those as
"incorrectly resurfaced as current guidance" would report the tool's own history
command as a defect — a metric that gets worse the more correctly the feature is
used. `subject` therefore carries retrieval scope (`current`/`historical`) and
`stale_under_history` is reported separately.

`subject` carries scope and **not query text**: the query is the user's own words
about their project, this ledger has shorter retention than the memories it
points at, and no Phase 51 count needs it.
`a_recorded_retrieval_stores_no_memory_content` reads every text cell of every
stored row and fails if a body, subject line or query string appears.

"Stale" is not judged — it is a `LEFT JOIN` on `status = 'superseded'` or
`review_reason IS NOT NULL`, and unresolved rows are reported as `unresolved`
rather than dropped, so no number is a fraction of an unstated denominator.

1856 is proven both ways: a foreign-project row is refused by migration 15's
triggers on `INSERT` and on an `UPDATE` that would move a stored row, and
`the_evaluation_module_has_no_path_out_of_the_project` pins the absence of an
export.

**Three corrections the implementation made to the design**, each argued in
code rather than worked around:
1. The design's `CREATE TABLE` **did not parse** — a table constraint sat above
   a column definition and SQLite accepts no column after the first table
   constraint (`near "memory_id": syntax error`, verified with `sqlite3`). Moved
   to the only legal position; nothing else changed. The design doc is fixed.
2. Retention had to key off `seq`, not an in-memory counter. The design said
   "every 256th insert"; a per-process counter would mean **the trim never
   runs**, because `glasshouse memory search` appends a few rows and exits.
3. The rollback-undo constant had to be extended, which is the §69 blast radius.

**An orchestrator packet defect worth generalising.** The packet forbade
`session/store.rs` *and* told the worker to fix what the blast radius names. A
migration necessarily touches every rollback fixture and four live in that file,
so both rules could not hold. The worker took ownership as binding, wrote and
verified the fix, **restored the file byte-identically**, and left the patch
beside its report — which the orchestrator applied. **Every future migration
packet must include the rollback fixtures or declare them a shared file.**

---

# Lines 1829 and 1830 — closed 2026-08-30

Package `GH-ROUTING-OVERRIDE-SIGNAL`, scoped by `GH-PHASE51-RECON`.

- **1829** *"Measure how often automatic routing is overridden by the user."*
- **1830** *"Measure how often warm-session reuse is chosen over fresh-session
  creation."*

State: **COMPLETE** (both)

## Why these two are one package

They are two fields of the same value. `SessionRouter::choose` returns a
`Routed` that already carries both: `Routed::overrode()` is 1829 in the
codebase's own words — *"the `Destination::id` the ranking would have chosen,
when a user override changed the answer"* — and `Destination::is_fresh()` is
1830. Splitting them would have meant two workers building one producer call.

`routing/session.rs` contains **zero** `#[cfg(test)]`, so every line of the
producer is production.

## The gap was not a mechanism, it was a write

**Both numbers were already computed and already shown to the user, then
discarded.** The resume path prints *"the ranking would have chosen `X`"* and
the launch path prints *"continuing session N … rather than starting a new
one"*. Neither was recorded. `record_routing_decision`
(`evaluation/mod.rs`), called from `launch_session`, records them — copying
`record_disposable_route`'s established producer shape exactly.

**No migration.** `evaluation_observations.kind` carries only
`CHECK (kind <> '')`, deliberately, because `database.rs:139-146` says *"this
is a vocabulary that will grow"*; `evaluation/mod.rs` prescribes *"One variant
per landed producer."* Two variants were added and pinned in
`EVALUATION_KINDS`.

## The two honesty properties, both pinned by mutation

1. **`overrode()` returning `None` means the automatic answer stood** — it is
   not "no override was offered", and it is recorded as `automatic` rather
   than omitted. Mutating `if overrode.is_some()` to `if true` dies on
   *"no override was asked for, so `overrode()`'s `None` must be recorded as
   the automatic answer standing, not as an override."*
2. **`glasshouse route` records nothing.** It reports without acting, so
   recording there would make the counts answer a different question than
   1829 and 1830 ask. Pinned by
   `glasshouse_route_reports_without_acting_and_records_nothing`.

The characteristic mutation — deleting the `record_routing_decision` call from
`launch_session` — is **KILLED** by four tests. It lands on the **call** (§35),
so a test that had entered at the producer would not have caught it.

## Consumer

`EvaluationObservations::recent_of_kind` and `::count(kind, from, to)`, both
already production and already rendered by `build_route_decision_table` in the
shell's routing-decisions view. A person reads that table today.

## Limits

- No mutation isolates the freshness fact independently of the shared call
  site; `Destination::is_fresh()` lives in a file this package was forbidden to
  edit.
- The ledger-open-failure arm is inspection-verified against a byte-identical
  sibling (`record_memory_retrieval`), not independently test-verified.
- These count decisions on the **launch** path only. The resume path
  (`main.rs`) computes the same `Routed` and still records nothing; that is a
  separate package, not a gap in these two lines.

---

# `GH-ROUTING-OUTCOME` — 2026-08-31: a routing decision's outcome is the harness's own verdict

Opus specialist at high effort. **One line closed (1835); four left open, each
with its missing link named precisely enough to package.** The mechanism the
register's RC-B held twelve lines behind — *how does Glasshouse learn whether a
routing decision was good?* — is now real, wired on both production paths, and
rendered to a person. The ruling it implements is in `design-decisions.md`,
*"Phase 51: a routing decision's outcome is the harness's own verdict, and
nothing else."*

Contract: Given a session Glasshouse routed, when the harness itself reports
that a turn ended, Glasshouse records that turn's outcome against the routing
decision that put the work there, so the evaluation ledger can answer how often
a route succeeded — while preserving that a quiet or exited process is never
read as success, and that no recorded observation is ever edited in place.

**What landed.** Three new `EvaluationKind`s (`database::EVALUATION_KINDS`
extended; the in-module pin `every_kind_the_type_can_produce_is_one_the_schema_constant_declares`
updated — `blast-radius.sh` caught it red via `--lib` when the packet's own
targeted command had passed):

| kind | `subject` | `session_id` | `detail` | written by |
|---|---|---|---|---|
| `routing_cost_class_observed` | `free` / `metered` / `unknown` | the routed session | destination id | `launch_session`, both routed exits |
| `routing_evidence_observed` | `observed` / `absent` | the routed session | destination id | same call (`record_routed_session`) |
| `routing_outcome_observed` | `completed` / `failed` | the session whose turn ended | the destination its decision chose | `glasshouse hook`, on `TurnEnded` (`events::task_outcome`'s first production caller) |

**The link is a third row, not a rewrite.** `record_routing_decision` runs
before a fresh launch has minted a session id, and its absent `session_id` is
deliberate — a launch refused while resolving its profile has made a routing
decision and never reaches a session record, so moving the call would change
what 1829/1830 count. `record_routed_session` records what the decision
*became*, on both routed exits of `launch_session` (warm continuation, where
the destination id already is the session id; fresh, just after
`store.create`). Nothing is `UPDATE`d.

**The reader is a section of `glasshouse route`** ("Past routes in this
project, last 30 days"): by cost class, by pairing class, by evidence held —
with **two denominators kept apart on purpose**: `completed`/`failed` count
*turns*, `sessions` counts *decisions*; a reader dividing completions by
sessions would print a rate above 1 on any project that works for an
afternoon. A session whose harness never reported a turn end is its own bucket,
never a success or a failure.

## 1835 — CLOSED

State: **COMPLETE**. *"Measure how often a low-cost or free route succeeds
compared with the premium route it displaced."*

**The finding that mattered: the packet's stated producer was a constant.**
`main.rs::destination_backend` builds every session-router destination with
`Cost::Metered`, hardcoded, and says so in its doc comment; `ResourceFacts` has
no cost field. Recording that would have given the line one bucket for ever.
The class is read instead from `ProviderConfig::cost_of` — the lookup
`disposable_candidates` and `gateway_upstream` already use — applied to the
chosen destination's own provider and model, project layer over user layer, in
`main.rs::routed_cost_class` (the only crate that may import `config`). A
destination naming no configured provider records `unknown`, counted in its
own bucket, never folded into `metered`.

Production: `main.rs :: record_routed_session, routed_cost_class`; the hook
path's `TurnEnded` arm → `evaluation::record_routing_outcome`;
`evaluation/mod.rs :: EvaluationObservations::{routed_destination,
route_outcomes_by, route_outcomes_by_pairing_class}`, `RouteOutcomeCounts`;
`main.rs :: render_route_outcomes` in `glasshouse route`.

Regression (`tests/routing_outcome.rs`, 4 tests, **every one through the
shipped binary**: `glasshouse launch` with a fake harness and two real
direct-provider profiles, one model in `free_models`; then `glasshouse hook` as
a separate process with a payload on stdin; then `glasshouse route` to read the
numbers back — there is no seam short of the process, so §35 cannot apply):
`a_completed_turn_records_the_outcome_against_the_decision_that_routed_it`,
`a_failed_turn_records_failed_and_a_silent_exit_records_nothing`,
`free_and_metered_route_success_is_reported_with_denominators`,
`a_session_with_no_routing_decision_records_no_outcome`;
`evaluation_observations` 22 (new kinds in the vocabulary; foreign-project rows
refused); `session_hook` 19.

Mutations — six, six KILLED (`mutate.sh --script`): the hook-path write dropped
(*"one turn end, one outcome row"*); a failed turn recorded as completed
(*"`StopFailure` is the harness stating a turn that ended badly, and recording
it as `completed` would make every success ratio here a fabrication"*); the
denominator dropped from the rendering; **the constant cost recorded instead of
the configured one** (*"the route this launch took is the free one, from the
provider's own `free_models`"*); the evidence state inverted; the route not
attributed to the session.

Limits: no Windows leg; two SQLite handles are briefly live at once on the hook
path (session store + ledger) — §65's hazard is Windows-specific; hook latency
argued, not measured; `glasshouse resume` attributes no route, only `launch`
does.

## The four left OPEN — each with its link

- **1834** — needs **two** links: the classified tier (landing with
  `launch-classifier`'s `TaskRequirements::minimum_tier`) **and** the
  escalation flag — in this codebase *escalation* is `WorkloadTier::escalate`,
  fired by `TaskClassification` when confidence is `Low`
  (`routing/classify.rs:366-374`). Both are decision-time facts for the row; and
  the tier exists only for a launch that named `--task` — whether a task-less
  launch records `unknown` or nothing is a decision to make before packaging.
- **1845** — *task success* by pairing class is produced and printed, keyed by
  `sessions.pairing_class` through a `LEFT JOIN` (the module's rule against
  duplicating a fact the database already holds). The other five quantities
  (usable tool calls, repair loops, effective TTFC, reliability, user
  overrides) are RC-C: columns since migration 11, no writer. The register's
  note stands: *"three producers, not a join."*
- **1854** — the *sparse* word is recorded (`observed`/`absent`: whether the
  health pool held a reading for the chosen destination — strictly more than
  the `unknown` the packet allowed, same row). *Stale* is not derivable:
  `GatewayHealthReading` has no `observed_at_unix` (a change to the health
  cache file's format, not to this ledger). *Incorrectly segmented* has no
  fact behind it on the launch path at all.
- **1851** — no prevention is *decided* anywhere in production:
  `failure_domain_contribution` (`routing/interactive.rs:987`) is a −1.0
  scoring term, never a rejection, by design decision 1 (*additive, never a
  filter*). **Successor package, not a refusal**: the prevention is derivable
  as *"the candidate that would have won without the failure-domain term did
  not win with it"* — one comparison inside `best()`/`on_provider_failure` plus
  one producer call from `gateway/session.rs`.

---

## From `GH-LAUNCH-CLASSIFIER` (2026-08-31)

The launch-path classifier package (router request schema, classification on the acting path) touched this phase's lines 1849 (closed — routing latency measured decision-start → decision-end). The full entry — production sites, regression names, the 23 killed mutations, the one honestly-survived one, and the missing producer for 1516/1517/1531 — is in `phase-34d.md`, *Phase 34D — router request schema* and *lines outside Phase 34D*, because the mechanism lives there.

### Phase 51 — four measured quantities (lines 1832, 1833, 1834, 1851); 1854 open

Package `GH-EVALUATION-PRODUCERS`, 2026-08-31, Opus at high. Six mutations, six killed. 1854 stays open by the worker's own reasoning: `PersistedGatewayHealth.observed_at_unix` has been written on every store since the format existed and `load`/`load_all` dropped it; copying it per entry would be a second source of truth, so `GatewayHealthCache::load_all_dated()` was added instead and `ObservedHealth` in `main.rs` now carries the dated readings the router's explanation reads. The routing-off branch yields an empty `ObservedHealth` rather than opening the three stores — the orchestrator's one edit at integration, where this package met the `--no-routing` restructure that landed after its base.

### Worker-reported packet errors and gates (transcribed at closure)

**Packet errors the worker reported — read these BEFORE its results.**
Thirteen consecutive rounds a worker corrected its packet and was right:
- 1854's producer already existed. `PersistedGatewayHealth.observed_at_unix` has been written on every store since the format was created; `load`/`load_all` drop it on the way out. Adding a per-entry `observed_at_unix` to `GatewayHealthReading` would have been the same number copied N times (a second source of truth for a fact the file holds), and would have broken ten struct literals in six files outside EXPECTED FILES. Added `GatewayHealthCache::load_all_dated()` instead, leaving `load_all` alone.
- 1833's request figure was already rendered — `"{tokens} over {N} classification calls"` and `"... over {N} other calls"` were both in the block and `RoutingOverhead` already had both `*_requests` fields. What was actually missing was the separation, which is what this package adds.
- 1851's row cannot be written from `gateway/session.rs` `the way record_routing_observation writes`: that method opens nothing, it takes `ledger: &EvidenceLedger` from `gateway/mod.rs`'s accept loop, and the evaluation ledger is a different store. `gateway/session.rs` has no `Runtime` and no paths, by design. See scope_overflow.
- 1834's `subject = <tier>` / `detail = escalated|not-escalated` split would have needed a second reader duplicating `route_outcomes_by`'s SQL, because that reader groups on `subject` alone and line 1834's question is about the pair. `subject` carries the pair as one closed eleven-word vocabulary (`RoutingTier::as_str`, exhaustive match); `detail` carries the tier the classifier stated.
- EXPECTED FILES listed `tests/routing_outcome.rs`; it needed no change and was not modified.
- NOT A PACKET ERROR, A BASE ERROR, recorded here because the gate will hit it: `tests/memory_provenance.rs` is red on `58e4d2c` itself. Migration 18 landed in `46b96ef` and that file was last touched in `ac9f0f5`, which predates it, so `a_memorys_provenance_survives_the_seq_rebuild` (line 1031) and `a_version_five_database_migrates_forward_keeping_its_memories` (line 760) still pin `version, 17` against a `SUPPORTED_SCHEMA_VERSION` of 18. This package adds no migration and does not touch the constant. The fix is two literals and two message strings, `17` -> `18`; I did not apply it because the file is outside EXPECTED FILES and the regression belongs to another package.

**Files touched outside EXPECTED FILES** — disclosed, not hidden:
- `crates/glasshouse/src/gateway/mod.rs` — Line 1851's row must be written from the thread that ranked the failover, and the only way into that thread is this file's accept loop. Eight lines: one more optional argument (`prevention_sink: Option<session::FailoverPreventionSink>`) threaded through `start_if_required_with_degrade_sink` -> `Gateway::start_with_degrade_sink` -> `accept_loop` -> `observe_exchange`, mirroring the `degrade_sink` beside it including its `None` reproduces the previous behaviour exactly contract. The alternative needing no extra file — a setter on `SessionRouting` called after the gateway is already accepting connections — was rejected as an unreachable-today race.
- `crates/glasshouse/tests/gateway_degrade.rs` — The one existing caller of the public door whose signature grew. One `None` argument with a comment saying why.
- `crates/glasshouse/tests/classification_call.rs` — Found by `blast-radius.sh`, not by the packet. `a_classification_call_is_recorded_under_its_purpose_and_extraction_is_not` asserted that extraction rows carry no purpose — true when written, and exactly what map line 1832 changes. The assertion was inverted (now `Some("memory-extraction")` plus an `assert_ne!` against the classification row) and the test renamed `a_classification_call_and_an_extraction_call_are_recorded_under_their_own_purposes`; its real subject, that two calls to the same endpoint in the same project are told apart, is unchanged. The half of its old claim that is still true — no back-fill of rows written before the stamp — is asserted in `evaluation_producers::extraction_rows_are_stamped_and_old_rows_are_not_relabelled`, which can actually produce such a row, and both doc comments now point at each other.

Gates the worker ran (re-run the decisive ones yourself):
- cargo fmt --all -- --check: clean
- cargo clippy --workspace --all-targets --all-features -- -D warnings: clean, exit 0
- cargo test -p glasshouse --lib: test result: ok. 1619 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --bin glasshouse: test result: ok. 48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --test evaluation_producers: test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --test classification_call: test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --test evaluation_observations: test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --test routing_outcome: test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --test gateway_retry_after: test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --test routing_economics: test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo doc -p glasshouse --no-deps: clean after fixing three intra-doc links to private items that the first blast radius caught
- scripts/mutate.sh --script <6 mutations> --allow-dirty: 6 KILLED, 0 SURVIVED; git diff byte-identical before and after. None of the six mutated lines changed after the batch — the later edits were the rendering early-return in `route_outcomes_section` and two test files — so the verdicts stand as run.
- scripts/blast-radius.sh: NOT exit 0, and every red is attributed in the report body. Two were mine and are fixed (rustdoc intra-doc links to private items; `classification_call` asserting extraction is unstamped, which is what line 1832 changes). One is red on the base commit and not this package's (`memory_provenance`, migration 18's unbumped `version, 17` pins). Three are PTY/process-timing targets that fail only under load — `canonical_line_limit` and `worker_access` are ok alone (6 and 19 passed), and `session_supervision::a_harness_that_never_came_up_is_not_restarted` is 4/4 alone and 2/4 as a whole target, splitting on wall clock (fails at 39.7s and 33.3s, passes at 18.1s and 15.5s) on a box at load 9.37 with other workers running. This package touches no session, supervision or lifecycle code. Re-check those three on a quiet machine before the gate.

---

### Measure memory-extraction cost separately from interactive coding cost. (line 1832)

Contract: Given a memory extraction that reached a provider, when Glasshouse records what that call cost, it stamps the row with the purpose the call was made for, so extraction spend can be counted apart from interactive coding spend — while preserving that no row already on disk is re-labelled and that an unstamped row is rendered as unstamped rather than folded into somebody else's bucket.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/main.rs` — `record_extraction_observation`
- `src/routing/evidence.rs` — `EXTRACTION_PURPOSE`
- `src/routing/evidence.rs` — `RoutingOverhead::from_consumption`
- `src/main.rs` — `render_routing_economics`

Regression evidence:
- `evaluation_producers::extraction_rows_are_stamped_and_old_rows_are_not_relabelled`
- `evaluation_producers::resources_separates_extraction_and_routing_consumption_by_tokens_and_calls`
- `classification_call::a_classification_call_and_an_extraction_call_are_recorded_under_their_own_purposes`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/main.rs: `.with_purpose(Some(EXTRACTION_PURPOSE))` -> `.with_purpose(None::<&str>)` | `skip-state-update` | **killed** | `evaluation_producers::extraction_rows_are_stamped_and_old_rows_are_not_relabelled` |

> skip-state-update observed: assertion `left == right` failed: the row this extraction wrote must say what the call was for: [(None, "older-build", Some(11)), (None, "extractor", Some(120))]

Recorded scope limits — stated by the worker, not discovered later:
- It does not prove anything about extraction rows written by earlier builds beyond that they keep their NULL: their real purpose is unrecoverable and is not guessed at.
- No Windows leg.

---

### Measure routing-model cost and request consumption separately from interactive coding cost. (line 1833)

Contract: Given a window of this project's routing evidence, when a person asks `glasshouse resources` what routing cost, it prints the routing model's own consumption — tokens and calls — in its own bucket beside memory extraction, the coding agent's relayed exchanges, and rows no producer stamped, so routing spend is separable from interactive coding spend — while preserving that an uncounted token figure renders as `tokens not counted` and never as zero, and that map line 1466's own denominator keeps its previous meaning.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/routing/evidence.rs` — `RoutingOverhead (extraction_*, routing_latency_*, coding_agent_*, unstamped_*)`
- `src/routing/evidence.rs` — `ROUTING_LATENCY_PURPOSE`
- `src/routing/evidence.rs` — `add_consumption`
- `src/main.rs` — `render_routing_economics`

Regression evidence:
- `evaluation_producers::resources_separates_extraction_and_routing_consumption_by_tokens_and_calls`
- `routing_economics::routing_overhead_is_read_with_its_denominators_and_never_from_an_uncounted_side`
- `routing_economics::resources_reports_routing_overhead_with_denominators_and_warns_past_the_fraction`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/routing/evidence.rs: `add_consumption(named, group.sample_count, tokens)` -> `add_consumption(named, 0, tokens)` | `drop-denominator` | **killed** | `evaluation_producers::resources_separates_extraction_and_routing_consumption_by_tokens_and_calls` |

> drop-denominator observed: panicked at crates/glasshouse/tests/evaluation_producers.rs:564 — every rendered `over N ... calls` denominator went to zero

Recorded scope limits — stated by the worker, not discovered later:
- The coding-agent bucket has a real request count and no token count, and will until something parses a relayed body — which `gateway::ingress` is designed never to do. The separation is real; one side of it is honestly uncounted.
- `RoutingOverhead::fraction()` still divides classification tokens by `task_*`, which today is dominated by extraction. Unchanged deliberately.
- A `purpose` a later build writes and this one does not know lands in `unstamped_*`. Visible degradation, not a wrong attribution.

---

### Measure how often workload-tier classification predicts successful execution without escalation. (line 1834)

Contract: Given a launch Glasshouse routed, when it attributes that route to the session it produced, it also records the workload tier the decision used and whether the conservative rule escalated it — and records `unclassified` for a launch that stated no task — so `glasshouse route` can report completed and failed turns by tier-and-escalation bucket with both denominators, while preserving that nothing is inferred, that a task-less launch is its own bucket rather than an absence, and that no percentage is printed without the counts it came from.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/evaluation/mod.rs` — `EvaluationKind::RoutingTierObserved`
- `src/evaluation/mod.rs` — `RoutingTier, RoutingTier::as_str, RoutingTier::stated_tier, unescalate`
- `src/evaluation/mod.rs` — `record_routed_session (third row)`
- `src/main.rs` — `routed_tier`
- `src/main.rs` — `launch_session, both routed exits`
- `src/main.rs` — `route_outcomes_section (by-tier table)`
- `src/database.rs` — `EVALUATION_KINDS`

Regression evidence:
- `evaluation_producers::a_classified_launch_records_its_tier_and_escalation_and_an_unclassified_one_says_so`
- `evaluation_observations::the_new_kinds_are_in_the_vocabulary_and_foreign_project_rows_are_refused`
- `glasshouse::evaluation::tests::every_kind_the_type_can_produce_is_one_the_schema_constant_declares`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/evaluation/mod.rs: `ledger.record_all(&[class, evidence, tier_row], observed_at_unix)` -> `ledger.record_all(&[class, evidence], observed_at_unix)` | `skip-state-update` | **killed** | `evaluation_producers::a_classified_launch_records_its_tier_and_escalation_and_an_unclassified_one_says_so` |
| src/main.rs: `escalated: answer.required_tier() != answer.stated_tier()` -> `escalated: false` | `invert-decision` | **killed** | `evaluation_producers::a_classified_launch_records_its_tier_and_escalation_and_an_unclassified_one_says_so` |

> skip-state-update observed: assertion `left == right` failed: exactly one `routing_tier_observed` row must name session `a622166ea7da812cad52ed557556336c`

> invert-decision observed: assertion `left == right` failed: an uncertain classification is escalated, the row says so, and the `detail` keeps the tier the classifier itself stated — without which nobody could tell what was escalated from

Recorded scope limits — stated by the worker, not discovered later:
- The escalation measured is decision-time (`WorkloadTier::escalate`, fired at `Confidence::Low`), not a runtime one. Nothing in this build escalates a session that turned out to need more than its tier.
- `glasshouse resume` attributes no route, so it records no tier either. Only `launch` does.
- `escalated` means the tier actually moved, not that the conservative rule fired — the two differ at `Frontier`, where `escalate` is a fixed point. Stated in the type's own doc comment.

---

### Measure how often failure-domain evidence prevents a failover onto the same unhealthy upstream. (line 1851)

Contract: Given a gateway failover whose candidates were ranked with a failure-domain diversity term, when that ranking's winner differs from the winner of the same ranking with the term removed, Glasshouse records that the term steered the failover off a candidate sharing the failed backend's provider, and records `not-prevented` when it did not — so a person can read how often failure-domain evidence prevented a failover onto the same unhealthy upstream, with its denominator — while preserving that the term stays additive and never a filter (design decision 1), that the row carries ids and vocabulary only, and that nothing on the exchange path holds a database handle it is not using.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/routing/interactive.rs` — `FailureDomainEffect, best, argmax, failure_domain_magnitude, FAILURE_DOMAIN_TERM`
- `src/routing/interactive.rs` — `FailureResponse::FailOver { domain_effect }`
- `src/gateway/session.rs` — `FailoverPreventionSink, SessionRouting::observe_exchange`
- `src/gateway/mod.rs` — `start_if_required_with_degrade_sink, Gateway::start_with_degrade_sink, accept_loop`
- `src/main.rs` — `failover_prevention_sink, launch_session, resolve_resume_overlay`
- `src/evaluation/mod.rs` — `EvaluationKind::FailoverPrevented, FailoverPrevention, record_failover_prevention, counts_by_subject`
- `src/main.rs` — `render_failover_preventions`

Regression evidence:
- `evaluation_producers::a_failover_the_domain_term_prevented_is_counted_and_one_it_did_not_is_not`
- `evaluation_producers::the_failover_prevention_ratio_is_printed_with_its_denominator_and_never_over_nothing`
- `glasshouse::tests::every_gateway_the_binary_starts_is_told_where_to_report_a_prevented_failover`
- `evaluation_observations::the_new_kinds_are_in_the_vocabulary_and_foreign_project_rows_are_refused`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/routing/interactive.rs: `explanation.total() - failure_domain_magnitude(explanation)` -> `explanation.total()` | `collapse-comparison` | **killed** | `evaluation_producers::a_failover_the_domain_term_prevented_is_counted_and_one_it_did_not_is_not` |

> collapse-comparison observed: panicked at crates/glasshouse/tests/evaluation_producers.rs:1008 — with the two rankings made identical no failover is ever `prevented`, and the `prevented[0].0` assertion fails

Recorded scope limits — stated by the worker, not discovered later:
- The two `main.rs` call sites are proved structurally, not behaviourally: no test in this crate can drive a *launch* that fails over (it needs a gateway-backed profile, a provider that answers badly, and a harness process that talks to the gateway). The behaviour is proved through `start_if_required_with_degrade_sink`, the door both sites call.
- `OfferMigration` computes the same effect and records nothing: a migration is offered, never taken, and counting it would put a move nobody made in the denominator.
- The row carries no session id. The gateway holds none.
- No Windows leg; the sink opens one SQLite handle on a gateway exchange thread, which is the thing a `--windows-vm` run should look at.

### Line 1852 — how often a route correlation changed a failover

Package `GH-ROUTE-CORRELATION`, 2026-08-31, Fable 5 at xhigh. The reader `phase-33c.md:101` said was missing now exists: `correlate_routes` over `RoutingObservation` rows joins routes by overlapping failure windows and matching class, and yields `RouteCorrelations` with a `CorrelationVerdict` per pair. `CORRELATION_OVERLAP_TOLERANCE_SECONDS = 60` is argued from the conservative side — a missed overlap lands on `InsufficientEvidence`, line 1378's safe side, while an invented one penalises a route that did nothing wrong. `MIN_CORRELATION_SAMPLE` is deliberately `MIN_SAMPLE_FOR_SUMMARY` (5): the ledger keeps one answer to *how many observations before a figure is trusted*. `CORRELATION_PURPOSE` rows are excluded from the reader's own input so it cannot read its consequence back as evidence. The production caller is `gateway/session.rs:604` (`observe_exchange`, the only caller of `on_provider_failure`), which the packet's EXPECTED FILES had omitted and the worker added with its reason. Eleven mutations, eleven killed; 61-target blast radius, exit 0.

### Measure how often nominally different routes provide separate quota capacity but not independent failure resilience. (line 1852)

Contract: Given a gateway failover the correlation term changed the winner of, when the failover is taken, Glasshouse records one routing_observations row under CORRELATION_PURPOSE naming the route it steered off, and `glasshouse route` counts those rows back with an honest zero — while preserving that line 1851's count means what it prints, that the row is never read as an exchange or as spend, and that correlate_routes never reads it back as evidence.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts and the diff of the decision.

Production evidence:
- `src/routing/interactive.rs` — `best (third ranking)`
- `src/routing/interactive.rs` — `FailureDomainEffect::correlation_displaced`
- `src/routing/evidence.rs` — `CORRELATION_PURPOSE`
- `src/routing/evidence.rs` — `RoutingOverhead::from_consumption (continue arm)`
- `src/main.rs` — `failover_prevention_sink`
- `src/main.rs` — `record_correlation_steer`
- `src/main.rs` — `route_correlations_section`

Regression evidence:
- `routing::interactive::tests::on_provider_failure_steers_off_a_measured_correlation_and_names_the_route`
- `gateway::session::tests::observe_exchange_steers_a_real_failover_off_a_route_the_ledger_shows_failing_with_it`
- `tests::a_correlation_steered_failover_is_recorded_by_purpose_and_never_as_an_exchange (bin)`
- `routing::evidence::correlation_tests::from_consumption_leaves_correlation_rows_out_of_every_bucket`
- `routing::evidence::correlation_tests::a_correlation_row_and_an_unjudged_row_are_not_evidence`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| let correlation_displaced = (without_correlation_index != best_index) -> let correlation_displaced = false | `freeze-value` | **killed** | `routing::interactive::tests::on_provider_failure_steers_off_a_measured_correlation_and_names_the_route` |
| .with_purpose(Some(glasshouse::routing::evidence::CORRELATION_PURPOSE)) -> .with_purpose(None::<&str>) | `skip-state-update` | **killed** | `tests::a_correlation_steered_failover_is_recorded_by_purpose_and_never_as_an_exchange` |
| if let Err(err) = ledger.record(row, now_unix) { -> if let Err(err) = { let _ = (&ledger, row); Ok::<i64, EvidenceLedgerError>(0) } { | `skip-state-update` | **killed** | `tests::a_correlation_steered_failover_is_recorded_by_purpose_and_never_as_an_exchange` |

> freeze-value observed: line 1852: the route the correlation steered off is named

> skip-state-update observed: assertion `left == right` failed at main.rs:9987 — purpose read back None

> skip-state-update observed: one steered failover, one row: []

Recorded scope limits — stated by the worker, not discovered later:
- The two-line call from the sink closure to record_correlation_steer is not under a test; deleting it would survive. The effect reaching a sink (mutation 9) and the write itself (mutations 10, 11) are proven; the closure is built only by launch_session/resume, which no test drives to a correlated failover — the same gap 1851 was ruled COMPLETE across
- The row appears in observed_identities under route = None, as ROUTING_LATENCY rows already do

---

## 1822 and 1826 — RE-OPENED 2026-09-02: the measure has no production reader

The first entry above ticked both on a producer
(`memory_search_grouped`'s `record_memory_retrieval`, real, driven through the
shipped binary) and a reader, `EvaluationObservations::stale_retrievals`
(`evaluation/mod.rs:948`). The reader is called only from
`tests/evaluation_observations.rs`: `scripts/cluster-b.py` lists it among the
zero-caller symbols, and no `glasshouse` command or shell view prints a stale
count, a retrieved count, or any memory-evaluation figure. That is the shape
the 1276 ruling rejected the same day (*a reader with zero production callers
is not a tick*) and §90's *recorded limit that is the defect*. **Un-ticked**,
state **LOCALLY VERIFIED**: producer real, measure computable, nothing in the
product observes it.

Re-closed by `GH-RETRIEVAL-CRITERIA` (dispatched 2026-09-02, `phase-52.md`):
`glasshouse memory retrievals` prints retrieved, stale, stale-under-history,
unresolved and missed counts for a window, giving `stale_retrievals` its
production caller; the 1826 distinction this entry records — a `--history`
search is *asking* for superseded memories — is one of its acceptance tests.

---

## 1822 and 1826 — RE-CLOSED 2026-09-02 (`GH-RETRIEVAL-CRITERIA`)

The reader the re-open above asked for exists: `glasshouse memory retrievals --hours N` (`main.rs::memory_retrievals_report`) prints retrieved, stale, stale-under-history, unresolved and missed for the window, and is `stale_retrievals`' first production caller. Mechanism and the 1865 half are in `phase-52.md`'s entry of the same day.

### Measure how often stale or incorrect memory is retrieved. (line 1822)

Contract: phase-51.md's 2026-09-02 re-open: `EvaluationObservations::stale_retrievals` measures how often stale or incorrect memory is retrieved, and now has a production caller (`glasshouse memory retrievals`), re-closing the line the 1276 ruling and practice §90 required re-opening it for.

State: **COMPLETE** — re-closed 2026-09-02. The measure now has a production reader (`memory_retrievals_report`, behind `glasshouse memory retrievals`), which is the caller `stale_retrievals` lacked when the box was un-ticked; the `drop-the-reader` mutation is KILLED through the shipped binary.

Production evidence:
- `main.rs` — `memory_retrievals_report`
- `main.rs` — `render_memory_retrievals`
- `evaluation/mod.rs` — `EvaluationObservations::stale_retrievals (pre-existing, unchanged)`

Regression evidence:
- `evaluation_observations::glasshouse_memory_retrievals_prints_every_figure_for_the_window`
- `evaluation_observations::glasshouse_memory_retrievals_on_an_empty_window_prints_zeros_not_an_error`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| same mutation as 1865 above -- the reader is the shared new surface both lines close on | `drop-the-reader` | **killed** | `evaluation_observations::glasshouse_memory_retrievals_keeps_stale_and_stale_under_history_disjoint` |

> drop-the-reader observed: assertion `left == right` failed (at evaluation_observations.rs:630)

Recorded scope limits — stated by the worker, not discovered later:
- re-closes the same 'reader with zero production callers' defect phase-51.md's 2026-09-02 entry names; the orchestrator's ruling, not this worker's, decides whether the tick is re-applied

---

### Measure how often superseded memories are incorrectly resurfaced as current guidance. (line 1826)

Contract: phase-51.md's 2026-09-02 re-open, 1826's own half: the report distinguishes a --history search's stale hits (stale-under-history) from an unasked-for stale hit (stale), so a --history search is never counted as the defect it exists to avoid.

State: **COMPLETE** — re-closed 2026-09-02. The readout keeps the entry's own argument visible at the one surface a person reads: a `--history` search is asking for superseded memories, so `stale` is printed net of `stale-under-history` (the struct's inclusive count is untouched and its existing test still pins it), and the disjoint pair is the acceptance test that kills `drop-the-reader`.

Production evidence:
- `main.rs` — `render_memory_retrievals`

Regression evidence:
- `evaluation_observations::glasshouse_memory_retrievals_keeps_stale_and_stale_under_history_disjoint`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| same mutation as 1865/1822 above | `drop-the-reader` | **killed** | `evaluation_observations::glasshouse_memory_retrievals_keeps_stale_and_stale_under_history_disjoint` |

> drop-the-reader observed: assertion `left == right` failed (at evaluation_observations.rs:630)

Recorded scope limits — stated by the worker, not discovered later:
- same re-close ruling note as 1822 above

---

---

---

---

## Lines 1823 and 1825 CLOSED, 1821, 1824 and 1831 OPEN — 2026-09-02 (`GH-MEMORY-RATING`, Amber, Sonnet high): the memory half of RC-B gets its explicit signal, and the proxy finds no row to join

Implements the user's ruling of 2026-09-02 (`design-decisions.md`, *an explicit rating when given, a labelled proxy otherwise*). `glasshouse memory rate <memory-id> <verdict> [--session <id>] [--note <text>]` (`cli.rs` `MemoryCommand::Rate`, a `value_parser` over the one list `MEMORY_RATING_VERDICTS` that refuses an unknown word and `unknown` itself by name), resolved through `MemoryStore::resolve_id` — the same project-isolation gate `memory challenge` uses — and recorded by `evaluation::record_memory_rating` as one `EvaluationKind::MemoryRated` observation whose outcome is the verdict (`EvaluationOutcome` gained the design's eight words, spelled once, round-trip pinned); no retrieval or memory row is ever edited. `glasshouse memory retrievals` prints a *Memory quality* section: explicit / proxy / unknown with denominators for 1821 and 1831, explicit-only with the words *no proxy: nothing observed bears on this* for 1823, 1824 and 1825. Eight shipped-binary tests (`tests/memory_rating.rs`), 4/4 mutations KILLED with output quoted (`drop-the-proxy-label`, `merge-proxy-into-explicit`, `silence-is-success`, `rate-writes-unknown`), `evaluation_observations` 26/26 unchanged, targeted blast green, rustdoc clean.

**The finding that decides the verdicts.** The proxy the design defines joins a `MemoryRetrieved` row to its session's `RoutingOutcomeObserved` row — and **no row this build writes carries both a `memory_id` and a `session_id`**: `record_memory_retrieval`'s only caller, `memory_search_grouped`, has no session parameter; the launch-time briefing door (`api/unix.rs::select_memory` / `deliver_memory`), which does hold a `SessionId`, records only misses and tracks injections in an in-memory set. The query is right and is exercised by tests that plant the join's rows directly (disclosed in the test file's header), and in production it is `0 of 0` today. Of the design's four negative signals only *override* has a row shape with a session id; `FailoverPrevented` carries none, nothing records a retry, and `TurnOutcome` has two values so *early abandonment* is not distinguishable from silence — all three omitted from the join by name, none invented.

### Measure how often an old decision causes an agent to add unnecessary implementation complexity. (line 1823)

Contract: Given memories of kind `decision` retrieved in the window, when a person or agent rates one `caused-complexity`, Glasshouse counts explicit ratings over the number of retrieved decision memories and prints both, while preserving that no proxy is offered for a judgement nothing in the build observes.

State: **COMPLETE** — ruled 2026-09-02. The explicit signal is the user's ruling; the denominator is real (`MemoryRetrieved` rows joined to `memories.kind = 'decision'`), the readout prints `explicit caused-complexity c of D retrieved-decision-memories` and says in words that there is no proxy; proven through the shipped binary (`caused_complexity_counts_over_retrieved_decision_memories`) with `rate-writes-unknown` KILLED on the recorder every reader depends on.

### Measure how often agents challenge a remembered decision and whether the challenge was justified. (line 1825)

Contract: Given memories marked for review in the window, when a person or agent rates a challenge `challenge-justified` or `challenge-unjustified`, Glasshouse counts both over the number of challenges and prints them, while preserving that an unrated challenge stays unknown.

State: **COMPLETE** — ruled 2026-09-02. Denominator `memories.review_marked_at` in the window (the column `memory challenge` writes); explicit counts printed over it; proven through the shipped binary (`challenge_accuracy_counts_over_memories_marked_for_review`). Recorded limit, the worker's: `memory revalidate … needs-review` writes the same column, so a revalidation that re-flags an already-challenged memory is indistinguishable from a fresh challenge — the same shape as 1822's recorded limit.

Production evidence (both lines):
- `crates/glasshouse/src/cli.rs` — `MemoryCommand::Rate`, `MEMORY_RATING_VERDICTS`
- `crates/glasshouse/src/main.rs` — `memory_rate`, `memory_retrievals_report` (the *Memory quality* section)
- `crates/glasshouse/src/evaluation/mod.rs` — `EvaluationKind::MemoryRated`, the eight `EvaluationOutcome` verdicts, `record_memory_rating`, the quality readers

Regression evidence (both lines):
- `memory_rating::a_rated_memory_appears_as_explicit_in_the_retrievals_readout`, `memory_rating::caused_complexity_counts_over_retrieved_decision_memories`, `memory_rating::challenge_accuracy_counts_over_memories_marked_for_review`, `memory_rating::an_unknown_verdict_is_refused_by_name`, `memory_rating::a_memory_from_another_project_is_refused`
- `evaluation::tests` round-trip of the verdict vocabulary

**1821 and 1831 — OPEN.** The explicit halves are real and proven (rate → readout; 1831's own denominator, retrievals of `memories.kind = 'failed_attempt'`, is real and tested); the proxy halves have the query and no producer for their denominator. **1824 — OPEN**: `memory revalidate`'s four outcomes share no column meaning *a revalidation happened*, so there is no honest denominator; the explicit counts print with the readout saying so. **Successor, named: `GH-RETRIEVAL-ATTRIBUTION` (Amber)** — thread the session id into `record_memory_retrieval` from `memory_search_grouped` and its two callers, record a successful injection at `deliver_memory` as a `MemoryRetrieved` row with the session, and record `memory revalidate` as its own evaluation row (`MemoryRevalidated`, no migration) so 1824 has a denominator; 1821, 1831 and 1824 tick on its landing with the readers already written.

---

## Line 1824 CLOSED, 1821 and 1831 STILL OPEN — 2026-09-02 (`GH-RETRIEVAL-ATTRIBUTION`, Amber, Sonnet high): the retrieval rows gain their session, and the proxy's other half turns out to have no producer on the same session

Implements the successor the entry above named. `evaluation::record_memory_retrieval` gained `session_id: Option<&str>`, threaded from `main.rs::memory_search_grouped` (both its callers pass `None` — see the packet error below); `api::unix::deliver_memory` records one `MemoryRetrieved` row per memory actually delivered at launch, with the session it briefed and `RetrievalScope::Injection`, after the send succeeded and never for a memory `select_memory`'s dedup set suppressed; `main.rs::memory_revalidate` records one `EvaluationKind::MemoryRevalidated` row (`subject` = the outcome word, `memory_id` set, `outcome` left `Unknown` because this row says *a revalidation happened*, not whether it was right) after the store write, and `revalidation_accuracy()` counts those rows as 1824's denominator with an `unknown` line in `glasshouse memory retrievals`. The two proxy tests no longer plant the retrieval row: they spawn a real session through `glasshouse api serve`'s Unix-socket door (a `#[cfg(unix)] mod door` fixture modelled on `tests/context_injection.rs`) and read the rows production wrote. 3/3 mutations KILLED with output quoted; `memory_rating` 9/9, `evaluation_observations` 26/26, `context_injection` 15/15, targeted blast green, rustdoc clean; one PTY red in `worker_access` under load re-run alone green (§34).

**The finding that decides two of the three verdicts, and it is structural.** The proxy joins a session-attributed `MemoryRetrieved` row to that session's `RoutingOutcomeObserved` row. This package makes the first real. The second is written only by `evaluation::record_routing_outcome`, which refuses to write for a session with no routed destination, and the only production caller that records a routed destination is `main.rs::launch_session` — the CLI launch. `deliver_memory` is reached only from the door's `spawn_session`/`send_message`, which never route a session; and `launch_session` never calls `select_memory`/`deliver_memory` (the `glasshouse route` diagnostic's injection-scope record at `main.rs::estimated_project_memory_tokens` measures a would-be briefing and records only its miss). **So no single production session can carry both rows today.** One proxy test therefore still plants exactly one `RoutingOutcomeObserved` row, disclosed at the line. The denominator for 1821 and 1831 is `0 of 0` in production, as before, but the reason has moved: from *no producer attaches a session* to *the two producers never meet on one session*. Recorded in the refusal register (*Phase 51's memory proxy*), with the successor named there.

**Two packet errors, both the worker's corrections.** (1) The packet said `api::unix::query_memory` "knows which session" it serves; `Request::QueryMemory` (`api/protocol.rs:458–466`) carries no session field, unlike `SendMessage` or `RecordAssumption`, and the MCP tool that builds it (`api/mcp.rs:636–653`) has none — so gap 1 is correct plumbing with no caller yet supplying `Some`. (2) `database.rs::EVALUATION_KINDS` and the pinning test beside it already omitted `MemoryRated` (landed by `GH-MEMORY-RATING`) despite the constant's *one entry per landed producer* rule; `database.rs` was this packet's forbidden file, so the worker left `MemoryRevalidated` out beside it and said so. **Fixed at integration** by the orchestrator: both kinds added to the constant and to `every_kind_the_type_can_produce_is_one_the_schema_constant_declares` (the constant is documentation and a pin, not a `CHECK`, so no migration).

### Measure how often revalidation correctly identifies a decision whose original assumptions no longer hold. (line 1824)

Contract: Given `glasshouse memory revalidate <id> <outcome>`, when the store has written the outcome, Glasshouse records that a revalidation happened as its own evaluation row, so explicit `revalidation-correct`/`revalidation-wrong` ratings print over a real count of revalidations with the unrated remainder shown as unknown, while preserving that no memory row is edited, that a ledger failure never fails the command, and that the readers' output for existing rows is unchanged.

State: **COMPLETE** — ruled 2026-09-02. The denominator is a row the command itself writes, proven through the shipped binary, with the recorder mutation KILLED.

Production evidence:
- `crates/glasshouse/src/evaluation/mod.rs` — `EvaluationKind::MemoryRevalidated`, `record_memory_revalidation`, `EvaluationObservations::revalidation_accuracy`, `RevalidationAccuracyCounts { revalidations, unknown }`
- `crates/glasshouse/src/main.rs` — `memory_revalidate` (the call after the store write), `render_memory_quality` (the 1824 section)

Regression evidence:
- `memory_rating::a_revalidation_gives_1824_a_denominator` — `memory revalidate … reaffirmed` then the readout prints `of 1 revalidations`; one `MemoryRevalidated` row

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `glasshouse::evaluation::record_memory_revalidation(` → `let _ = (` (`main.rs`, `memory_revalidate`) | `skip-the-revalidation-row` | **killed** | `memory_rating::a_revalidation_gives_1824_a_denominator` |

> skip-the-revalidation-row observed: panicked at crates/glasshouse/tests/memory_rating.rs:357:5: assertion `left == right` failed: [] (no MemoryRevalidated row)

Recorded limit, the worker's: the denominator counts every `memory revalidate` call regardless of outcome; the explicit correct/wrong ratings are a separate count over `MemoryRated` rows in the same window — the same explicit-over-window shape 1823 uses — not a per-event join.

### Lines 1821 and 1831 — the retrieval half is now real; the join still finds no session with both rows

Production evidence (the half this package built):
- `crates/glasshouse/src/api/unix.rs` — `deliver_memory` (the `MemoryRetrieved` row with the session, after a successful send)
- `crates/glasshouse/src/evaluation/mod.rs` — `record_memory_retrieval` (`session_id`), the reader block's doc comment stating the remaining gap
- `crates/glasshouse/src/main.rs` — `memory_search_grouped` (`session: Option<&str>`, both callers `None`)

Regression evidence (through the shipped binary and the Unix-socket door):
- `memory_rating::a_retrieval_delivered_by_the_briefing_door_with_no_turn_end_counts_as_unknown` — the delivered memory's row carries the spawned session's id; a repeat send with the same task records no second row
- `memory_rating::a_retrieval_delivered_by_the_briefing_door_into_a_completed_session_counts_as_proxy` — with one planted `RoutingOutcomeObserved` row (disclosed at the line, for the reason above) the readout shows `proxy useful 1 of 1`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `Some(session.as_str())` → `None` in `deliver_memory`'s record (`api/unix.rs`) | `drop-the-session-id` | **killed** | both door tests (`memory_rating.rs:618` and `:676`, the session-id assertions) |
| `if seen.len() < MAX_REMEMBERED_INJECTIONS {` → `if false {` in `deliver_memory` (`api/unix.rs`) | `record-every-injection-twice` | **killed** | `a_retrieval_delivered_by_the_briefing_door_with_no_turn_end_counts_as_unknown` (`:631`, the row-count assertion) |

> drop-the-session-id observed: panicked at crates/glasshouse/tests/memory_rating.rs:618:5: assertion `left == right` failed

> record-every-injection-twice observed: panicked at crates/glasshouse/tests/memory_rating.rs:631:5: a dedup-suppressed repeat must not record a second retrieval

State for both: **PARTIALLY VERIFIED** — the explicit halves complete (entry above), the retrieval half of the proxy complete here, the outcome half has no producer on a briefed session. **Successor, named: `GH-TURN-OUTCOME-FOR-BRIEFED-SESSIONS`** — a design ruling first, then Amber: either the door's spawn records the routing decision it embodies (so `record_routing_outcome` has a destination to attribute the turn's outcome to), or the harness-reported turn outcome becomes a row that does not require a routed destination for the memory proxy alone (the proxy's definition is about the *session's* turn, not the *route's*). The register row says which facts decide it.

---

## Lines 1821 and 1831 CLOSED — 2026-09-02 (`GH-TURN-OUTCOME-ROW`, Amber, Sonnet high): the proxy's two rows finally meet on one session

Implements the orchestrator's ruling (register, *Phase 51's memory proxy*, option (b)): the harness's turn verdict becomes a row that needs no route. `EvaluationKind::TurnOutcomeObserved`, written by `record_turn_outcome` in the hook's `TurnEnded` arm for every session — routed or not — beside `record_routing_outcome`, which is unchanged and still refuses an unrouted session (`routing_outcome::a_session_with_no_routing_decision_records_no_outcome` proves it, and the `route-the-unrouted` mutation is killed by the routing readers' own tests). `usefulness()` (1821) and `prevented_repetition()` (1831) join `MemoryRetrieved.session_id` to the new row; the override signal's clause is untouched. `EVALUATION_KINDS` is thirteen with the pinning test. The proxy test no longer plants anything: a door-spawned session is briefed, its turn ends through the real hook, `RoutingOutcomeObserved` stays empty for it, one `TurnOutcomeObserved` row lands, and the shipped binary prints `proxy useful 1 of 1`. A new test proves a session that is both routed and briefed counts once (its routing row is planted, disclosed at the line, because the door never routes and a CLI launch never briefs — the latter is `GH-LAUNCH-BRIEFING`, live under the user's ruling). 3/3 mutations KILLED; `memory_rating` 10/10, `evaluation_observations` 26/26, `evaluation_producers` 6/6, `tier_outcomes` 2/2, `routing_outcome` 4/4, `--lib evaluation` 6/6, `--lib database` 45/45, targeted blast green.

### Measure how often retrieved memory is actually useful to the receiving agent. (line 1821)

Contract: Given a memory delivered into a session, when that session's harness reports its turn ended, Glasshouse counts the delivery as a proxy hit when the turn completed with no override, failover or retry recorded against it, beside the explicit ratings, while preserving that a session with no turn end counts as unknown and that no session is ever given a routed destination it was not routed to.

State: **COMPLETE** — ruled 2026-09-02. The explicit half (`memory rate`) and the retrieval half (`deliver_memory` with the session id) were already production; this package supplies the outcome half through production rows. The one remaining reach limit — a plain CLI launch is not briefed today — is the user's ruling and `GH-LAUNCH-BRIEFING`, live; the readers need no change when it lands.

Production evidence: `evaluation/mod.rs` — `EvaluationKind::TurnOutcomeObserved`, `record_turn_outcome`, `EvaluationObservations::usefulness`; `main.rs` — the hook's `TurnEnded` arm; `database.rs` — `EVALUATION_KINDS`.

Regression evidence: `memory_rating::a_retrieval_delivered_by_the_briefing_door_into_a_completed_session_counts_as_proxy` (no plant), `memory_rating::a_routed_and_briefed_session_counts_the_proxy_once`, `memory_rating::a_retrieval_delivered_by_the_briefing_door_with_no_turn_end_counts_as_unknown`, `routing_outcome::a_session_with_no_routing_decision_records_no_outcome`.

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `record_turn_outcome` returns before writing | `skip-the-turn-row` | **killed** | `a_retrieval_delivered_by_the_briefing_door_into_a_completed_session_counts_as_proxy` |
| `usefulness()` joins `RoutingOutcomeObserved` again | `join-the-routing-row` | **killed** | the same test (the unrouted door session's proxy collapses to 0 of 1) |

> skip-the-turn-row observed: assertion `left == right` failed: [] (memory_rating.rs:730, zero TurnOutcomeObserved rows)

> join-the-routing-row observed: panicked at memory_rating.rs:736 (`proxy useful 1 of 1` absent)

### Measure how often memory prevents repetition of a recorded failed approach. (line 1831)

Contract: Given a failed-approach memory delivered into a session, when that session's turn ends, Glasshouse counts the delivery as a proxy hit under the same rule, over the failed-approach memories retrieved, while preserving the same invariants as 1821.

State: **COMPLETE** — ruled 2026-09-02; the denominator (`memories.kind = 'failed_attempt'` retrievals) was already real; this package changed only which row the completed-turn join reads.

Production evidence: `evaluation/mod.rs` — `EvaluationObservations::prevented_repetition`, `record_turn_outcome`.

Regression evidence: `memory_rating::prevented_repetition_counts_over_retrieved_failed_approach_memories`, `memory_rating::a_retrieval_delivered_by_the_briefing_door_into_a_completed_session_counts_as_proxy`, `routing_outcome::a_session_with_no_routing_decision_records_no_outcome`.

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `record_turn_outcome` also writes a `RoutingCostClassObserved` row | `route-the-unrouted` | **killed** | `routing_outcome::a_session_with_no_routing_decision_records_no_outcome` (and three more routing-reader tests) |

> route-the-unrouted observed: routing_outcome.rs:469 — an unrouted session now had a cost-class row that must not exist

Recorded limit: macOS only.

---


---

## Lines 1821 and 1831 — the proxy's producer now covers manual launches (2026-09-02, `GH-LAUNCH-BRIEFING`)

Not a state change. `GH-TURN-OUTCOME-ROW` closed both lines above with the verdict row; this package widens the *retrieval* side: `glasshouse launch` and `run` now brief a session and record `MemoryRetrieved` with its id (`main.rs::brief_launch_session`, `phase-27.md`'s last entry), where before only a door-spawned session produced that row. The join in `EvaluationObservations::usefulness` is unchanged and now finds both rows on a manually launched session too; `every_delivered_memory_records_a_memory_retrieved_row_with_the_new_sessions_id` pins the producer. What a real reading needs is still real use.

## 1836, 1855 and 1854 — CLOSED 2026-09-03 (`GH-PHASE51-JOINS`, Amber, Sonnet high): the router's own estimates measured against what happened

**Why now.** The register's 2026-08-30 census refused these on missing producers; the producers landed this week (migration 24's `session_id`, wave 103's rationale row, the estimator, the `RoutingEvidenceObserved` row with its staleness horizon). Three readers over rows that exist, one small producer, no migration.

**1836 — the estimator replayed.** `EvidenceLedger::headroom_replay(provider, now, window)` replays `estimate_subscription_headroom` (untouched) against every `Throttle`/`ExhaustedQuota` row for one provider using only the rows that preceded it: `warned` (`Low`/`Exhausted`), `missed` (`Ample`/`Moderate`), `unestimable` (the estimator's own `None`), plus the *observed reset lag* — the median seconds from a throttle to the provider's next accepted row — with its sample. `entitlement_facets` prints it after the headroom line (`headroom estimate vs throttles (1836): warned N / missed M / unestimable U of K throttles; observed reset lag median Ss over R`), *not enough throttles to score* below the floor. **Ruling:** the line's *resets* clause closes on the observed lag — `RoutingObservation` carries no provider-stated wait (the stated wait lives only in the quota cache's newest reading), and the readout never implies a comparison it cannot make. Five tests; two mutations KILLED (`replay-sees-future`, `warned-and-missed-swapped`).

**1855 — expected versus actual output.** `EvaluationKind::RoutingConsumptionEstimated` (subject = the task class word, detail = the median output tokens as decimal text, `session_id`) is written by `main.rs::record_consumption_estimate` at both routed exits of `launch_session`, beside `record_session_route`, **only** when `comparable_output_tokens` holds a real median for the class — a launch with no comparable rows writes nothing. `EvidenceLedger::output_estimate_accuracy` joins each estimate to the sum of the session's later `output_tokens` and prints, per class, the median of actual ÷ estimated over ≥ `MIN_SAMPLE_FOR_SUMMARY` pairs with `pending` counted, in `route_outcomes_section`. **Ruling:** the line is *token or request consumption*; it closes on the token disjunct, and the request half is recorded open — no request-count estimate exists at launch anywhere in this build. Three tests; two mutations KILLED (`estimate-written-as-zero`, `sum-not-joined-by-session`).

**1854 — the proof, and the ruling.** No production change: `route_outcomes_by(RoutingEvidenceObserved, …)` already separates `observed-fresh`, `observed-stale` and `absent` with their own success counts (wave 103's `RoutingEvidence::from_observation` and its horizon). The new test drives a real launch against a genuinely stale gateway-health reading and one with none, and asserts both buckets carry their counts; `stale-horizon-dropped` KILLED. The worker returned `open` on the third facet — *incorrectly segmented* — and was right to defer: nothing on the launch path segments evidence, so there is no fact to be wrong about. **Ruling: CLOSED.** The line lists three causes to measure; two are measured end to end and the third names a mechanism this build does not have (the same reading as 1333's *only when exposed* and 1247's *or*). The register's row says so.

**Packet errors, all right:** the packet named `FailureClass::CadenceThrottled`/`QuotaExhausted`; the enum's variants are `Throttle`/`ExhaustedQuota`. The estimator has no row-count floor of its own (`None` only when nothing at all is available), and its pressure signals read `Throttle` rows only — a run of pure exhaustions reads `Moderate`; the tests use the honest scenario. Scope overflow: one string appended to `database::EVALUATION_KINDS` (the vocabulary pin the objective itself required; no schema change).

**Recorded limits** (the worker's): the replay narrows to no credential and no live `seconds_until_reset`/session count, which the historical record never held; the median is computed in integer thousandths to reuse the module's `median`; macOS only.

State: **COMPLETE** for 1836, 1855 (token half) and 1854 (sparse and stale). Phase 51 stands at 20 of 37.

> **2026-09-03, the wave-107 trailing sweep:** `tests/entitlement_broker.rs`'s two view tests read each account's block by position — name, facets, `served:` — and the 1836 line, joined into the facets string with a newline, had landed between them. The targeted gate never traced that file. Fixed forward in `main.rs`: `headroom_replay_facet` is its own line, printed after `served:` by `entitlements` and as the facets line's continuation by `status`; `phase51_joins`'s own assertions are `contains` and stand. The 1836 evidence above is unchanged.

## 1845 and 1850 — CLOSED 2026-09-03 (`GH-RESPONSIVENESS-TERMS`, Amber, Sonnet high): the pairing-class join and the separation readout

**1845.** The *by pairing class* block of the route-outcomes section (`main.rs :: route_outcomes_section`) drops its *task success only* caveat and prints, per pairing class, task success (as before), usable tool calls (the share of the class's routing rows with `tool_rounds > 0`), repair loops (mean `repairs` per row carrying one), effective TTFC (`RouteResponsiveness` over the class's rows), reliability (`1 − p`), and user overrides (`RoutingOverrideDecided` rows with subject `overridden`, read from `sessions.pairing_class` — the field `route_outcomes_by_pairing_class` already joins through), each with its sample and *not enough* below the floor. The register's *three producers, not a join* stood in August; the producers landed in waves 102–106 and this is the join.

**1850.** `EvidenceLedger::responsiveness_separation`: for each of raw TTFC, effective TTFC, TTFT and decode tokens/s over exchanges with a usable-turn verdict (`effort_shadow`'s subquery — the session's next `TurnOutcomeObserved` at or after the exchange), the median among usable and among unusable turns and `|median_unusable − median_usable| / median_all` as the separation, printed per measure with both sample counts under `responsiveness vs usable turns (1850):` in `routing-cost` — *separates*, never *predicts*. `usable-verdict-ignored` KILLED (the first attempt, redirecting only the catch-all arm, SURVIVED and the test was strengthened — §80).

Gates and limits: the `phase-33b.md` entry for this package. Scope overflow: `tests/routing_economics.rs` (two `PurposeConsumption` fields), `tests/routing_outcome.rs` (the pairing block's text changed by the caveat's removal).

State: **COMPLETE** for 1845 and 1850. Phase 51 stands at 22 of 37.

## 1837 — CLOSED 2026-09-05 (`GH-RESERVE-AVAILABILITY`, Amber, Sonnet high): protected quota's availability, recorded when a high-tier task is routed

**Why now, and why it was never RC-B.** The register filed 1837 under *no outcome is ever learned* because its verb is *measure … when needed*. Read again after the 2026-09-03 answer, the line needs no outcome: whether protected quota remained available for a high-tier task is decided when the task is routed, from two facts the router already held and nobody wrote together — the task's tier (`routed_tier`, the same value `RoutingTierObserved` records) and the chosen destination's capacity band (`Destination::capacity_facts().band`, computed under the resource's own reserve thresholds). RC-A's shape. Design of record: `design-decisions.md`, *Protected quota's availability is recorded when a high-tier task is routed*.

### Measure how often protected quota remains available for high-tier tasks when needed. (line 1837)

Contract: Given a launch classified above the routine tier, when the router picks a destination, Glasshouse records the destination's capacity band as that task's protected-quota reading and `glasshouse route` reports how often such tasks found quota available — while preserving that a routine-tier launch writes no row, a destination with no reading records `unknown` rather than a band, and the ledger schema is unchanged.

Production: `evaluation/writer.rs :: record_reserve_availability` (writes only for `WorkloadTier::Heavy` or `Frontier`; subject the band in `CapacityBand`'s spelling or `unknown`; detail the tier word; the handle opened and dropped there), called at both routed exits of `commands/launch.rs` beside `record_routed_session`; `EvaluationKind::ReserveAvailabilityObserved` (`kinds.rs`, plus the `EVALUATION_KINDS` pin — no migration); `commands/route.rs :: route_outcomes_section` prints *protected quota for high-tier tasks (1837): available N · at reserve R · exhausted E · unknown U of K high-tier launches*, or *not enough high-tier launches* below `MIN_SAMPLE_FOR_SUMMARY`, counted by `counts_by_subject`.

Regression (all through the shipped binary): `routing_outcome::a_heavy_tier_launch_records_reserve_availability_as_unknown_with_no_reading`, `routing_outcome::a_standard_or_unclassified_launch_writes_no_reserve_availability_row`, `routing_outcome::route_outcomes_section_prints_the_protected_quota_line`.

| mutation | change | result | killed by |
|---|---|---|---|
| tier-filter-dropped | `record_reserve_availability` writes for every tier | KILLED | `routing_outcome::a_standard_or_unclassified_launch_writes_no_reserve_availability_row` (routing_outcome.rs:579) — a first attempt whose replacement left a variable unused failed to compile and read as KILLED; the worker discarded it and re-ran (§80 case 4) |

Packet error (the orchestrator's): the inverse of `as_str` is `from_stored`, not `from_str`; `EVALUATION_KINDS` carries an explicit `[&str; N]` length that moved 15 → 16. Scope overflow, accepted: `evaluation/tests.rs`'s kind-count pin. Limits: the fixture drives a destination with **no** capacity reading (`unknown`) — a real band word on the row is proven by the writer's unit path, not by a launch against a metered destination; `available` sums every band above `Reserve` by definition, not by observation.

State: **COMPLETE** for 1837. Phase 51 stands at 23 of 37.

