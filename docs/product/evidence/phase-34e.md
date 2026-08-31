---

# Line 1464 — closed 2026-08-30

Package `GH-ROUTING-SPEND-DOOR`. Phase 34E was 9 open / 0 closed and had **no
ledger entry at all**; this file is its first.

The line: *"Measure routing-model token and request consumption separately from
coding-agent consumption."*

State: **COMPLETE**

## Why this was closeable now and was not last week

Two producers landed on 2026-08-30 and 1464 sits exactly on top of them.
`parse_usage` (`memory/extract/model.rs:495-501`) reads `prompt_tokens` /
`completion_tokens` / `prompt_tokens_details.cached_tokens` out of a reply
document Glasshouse already deserializes — **the first thing in this build that
counts tokens**. And `record_classification_observation` (`main.rs:3213-3232`)
stamps `purpose = "classification"` on the row a real classification call
produces.

What was missing was a **consumer**: nothing anywhere aggregated the ledger by
purpose. `glasshouse routing-cost [--hours N]` is that consumer.

## The hazard this line invites, and how it was actually avoided

**"Separately from coding-agent consumption" is the kind of qualifier that
passes vacuously**, which is precisely why 1455 and 1456 were un-ticked earlier
the same day. If coding-agent consumption were absent from the ledger, a report
"separating" routing consumption from it would be separating it from nothing.

It is not absent. The orchestrator verified before dispatch that
`routing_observations` has **three** production writers:

| writer | `purpose` | `harness` | tokens |
|---|---|---|---|
| classification — `main.rs:3229` | `"classification"` | NULL | **present** |
| extraction — `main.rs:3760` | NULL | NULL | **present** |
| gateway relay — `gateway/session.rs:358` (that file's `#[cfg(test)]` is `:739`) | NULL | **set** | **NULL by design** |

The gateway rows **are** the coding-agent exchanges, one per real relayed
exchange, and they carry genuine request counts. So the separation is over real
data on both sides.

**The packet as dispatched would have got this wrong**, and it was corrected
mid-flight: it said to group by `purpose` alone, which folds extraction's real
token counts together with the gateway's genuinely-uncounted rows. The worker
verified the correction against source itself before applying it and regrouped
on `(purpose, harness IS NOT NULL)`.

## The property that carries the line, and it is structural

A group whose every row has `NULL` tokens must report **"not counted"**, never
`0`. A reader who cannot tell *"nothing was consumed"* from *"nobody counted"*
has been handed a fabrication — the same failure `RetrievalResult.relevances`
is private to prevent.

This is not defended by Rust code that could drift. It falls out of SQL:
`SUM(x)` skips `NULL` inputs and returns `NULL` only when **every** input was
`NULL`. The aggregate is

    SELECT purpose,
           (harness IS NOT NULL) AS harness_recorded,
           COUNT(*) AS sample_count,
           SUM(input_tokens) AS input_tokens, ...
      FROM routing_observations
     WHERE project_id = ?1 AND observed_at >= ?2 AND observed_at <= ?3
     GROUP BY purpose, harness_recorded

`sample_count` is a real `COUNT(*)` and is always defined, so a group can report
**requests without tokens** — which is exactly the coding-agent group's honest
shape.

## Production

- `routing/evidence.rs :: EvidenceLedger::consumption_by_purpose`
- `routing/evidence.rs :: PurposeConsumption`, `row_to_purpose_consumption`
- `main.rs :: routing_cost_report`, `render_routing_cost`,
  `purpose_group_label`, `render_token_count`
- `cli.rs :: Command::RoutingCost`

The ledger handle is opened **only inside `routing_cost_report`** (practice
§65): an open SQLite handle on a path with nothing to do blocks a later writer
under Windows' mandatory `LockFileEx` while being invisible under POSIX
advisory locks.

## Regression — `tests/routing_cost.rs`, 6 tests, all through the real binary

`running 6 tests` … `test result: ok. 6 passed; 0 failed` (§68: a whole-target
run, not a name filter).

The two that carry the line:

- `the_classification_group_is_attributed_its_own_tokens_and_no_others` —
  asserts the exact counts land under `classification` and never under the
  coding-agent group.
- `a_group_with_no_counted_tokens_never_renders_a_digit_for_them` — the hazard
  test. Exact string equality on `"not counted"`, **plus** an assertion that the
  rendered value contains no ASCII digit at all.

`coding_agent_rows_and_other_unpurposed_rows_are_never_merged` pins the
mid-flight correction: an extraction-shaped row and a gateway-shaped row land in
different groups.

**The isolation test is worth naming.** Two projects sharing one `--data-dir`
already get **separate `glasshouse.db` files**, so a naive two-fixture test
would pass even with the `WHERE project_id = ?1` clause deleted — there would be
nothing in the same file to leak. The test therefore drops
`routing_observations_reject_foreign_project_insert`, plants a foreign-project
row **inside beta's own database file** under the same purpose, and recreates
the trigger. Only the SQL `WHERE` clause can keep the totals apart. That is the
same pattern `tests/memory_project_scope.rs::plant_foreign_memory` uses, and it
is the difference between an isolation test and an isolation-shaped test.

## Mutations

| change | result | killed by |
|---|---|---|
| `SUM(input_tokens)` → `COALESCE(SUM(input_tokens), 0)` | **KILLED** | `a_group_with_no_counted_tokens_never_renders_a_digit_for_them` (and `coding_agent_rows_and_other_unpurposed_rows_are_never_merged`) |
| `SUM(output_tokens)` → `COALESCE(SUM(output_tokens), 0)` | **KILLED** | re-run by the orchestrator at integration; the worker had flagged it as untested |

Observed on the first: `assertion left == right failed: a group with no counted
rows must say so, never a number: "    input tokens        : " was "0"`.

§80's checklist was applied to both: a real `test result:` line with real
counts, killing tests inside the named target, the mutated line on the path the
killing tests exercise, and a genuine assertion failure rather than a compile
break.

## Limits

- **Coding-agent token consumption is not counted anywhere**, and this line does
  not require it to be. `gateway::ingress` relays a body it is designed never to
  parse. The report says so in a fixed closing line, so a reader is never left
  to infer that "not counted" means zero.
- The `(no purpose or harness recorded)` group is extraction today. Nothing
  prevents a future producer from leaving both unset while being neither
  extraction nor coding-agent; this aggregate does not claim to identify a
  fourth kind of spend that does not exist yet.
- No currency figure is rendered anywhere. See 1465 below.

---

# Line 1465 — REFUSED, and it is not a near miss

*"Track routing-model spend separately from productive task spend."*

State: **NOT STARTED**, and blocked on a producer that does not exist.

`ObservedCost` has **no production producer**. Its only two assignments,
`routing/evidence.rs:1674` and `:1723`, both fall after that file's
`#[cfg(test)]` at `:1355`. `cost_micro_usd` is therefore `NULL` on every row
this build can write, `evidence.rs:65-67` names the four columns "not supplied",
and `provider/resources.rs:952-954` prints *"Glasshouse does not count spend
against this"* to the user.

**The distinction from 1464 is the whole point and must not be blurred.** 1464
says *token and request consumption*, which this build now measures. 1465 says
*spend*, which is a money figure, and no money figure exists anywhere in
Glasshouse. `routing-cost` therefore renders no price, no currency amount, and
no spend estimate — deliberately, and the packet forbade it explicitly.

This is Cluster M in `docs/process/refusal-register.md`.

---

# Line 1463 — open, untouched

*"Measure the number of routing decisions made per interactive hour."*

The ledger can count rows and knows `observed_at`, so decisions per *elapsed*
hour is available. **"Interactive hour" is the blocker**: nothing in this build
measures interactive time, and a proxy would be a fabricated denominator. Left
open rather than closed against a substituted quantity.


---

## From `GH-ROUTING-ECONOMICS` (2026-08-31)

The routing-model selector package closed this phase's lines 1463, 1465, 1466; the full entry — production sites, regression names, the 22 killed mutations and the four refusals with their producers — is in `phase-34c.md` under *Package GH-ROUTING-ECONOMICS*, because the mechanism (`DisposableRouting::choose_for_automatic_classification`) lives in that phase.


---

## From `GH-LAUNCH-CLASSIFIER` (2026-08-31)

The launch-path classifier package (router request schema, classification on the acting path) touched this phase's lines 1467, 1468, 1470, 1471 (closed). The full entry — production sites, regression names, the 23 killed mutations, the one honestly-survived one, and the missing producer for 1516/1517/1531 — is in `phase-34d.md`, *Phase 34D — router request schema* and *lines outside Phase 34D*, because the mechanism lives there.
