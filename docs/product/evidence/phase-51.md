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
