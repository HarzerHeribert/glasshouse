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
