# Design — the Phase 51 evaluation table

> **Status: a design, not a decision.** Written 2026-08-29 by the Opus
> specialist worker `GH-GH-PHASE51-DESIGN`, read-only, no code changed. The
> orchestrator folds it into `docs/product/design-decisions.md` after reading
> it. It is written inside the brief already recorded there — *"Scoping the
> Phase 51 event log: a split verdict, and most of it is not a migration"*
> (`docs/product/design-decisions.md:2302`) — and it contradicts that brief in
> two places, both marked **⚠ DEPARTURE FROM THE BRIEF** and argued.

---

## 1. What the table is for

**Glasshouse can already answer questions about what it *is*; it cannot answer
questions about what it *did*.** A memory's status, a session's mechanism, a
provider's last quota reading — all durable, all queryable. But a retrieval, a
route preference, a tier assignment and a profile override are *events that
leave no trace*: they happen inside one function call, change what the user
gets, and are gone. Phase 51's verb, in 26 of its 37 lines, is **"Measure how
often"**, and you cannot count what was never written down.

`evaluation_observations` is one row per **decision Glasshouse made whose
wisdom is only visible later**, written at the moment of the decision, carrying
what was decided, what it was about, which side of a feature switch was in
force, and — when it is already known — how it turned out. It answers *how
often*, over a window, split by arm. It does **not** answer *how much*: cost,
tokens, and latency belong to `routing_observations`
(`crates/glasshouse/src/database.rs:1123`), and a second column for any of them
here would be a second source of truth for a fact that ledger already models.

**The paragraph a reader can disagree with:** this table is worth its migration
only if you accept that *the count is the product feature*. If evaluation is
something a developer does by hand against a database dump, none of this is
needed — `EventLog::len()` (`events/log.rs:402`) already counts, and
`EvidenceLedger::summarize` already aggregates. The claim here is that the
recorded alpha directive ("built so their usefulness can be measured",
`design-decisions.md:2179-2184`) makes *Glasshouse counting its own decisions*
a shipped capability rather than an analyst's chore, and that a table is the
cheapest honest way to have one.

### The organizing principle, which is sharper than "the memory cluster"

The brief groups Phase 51's lines by subject (memory / routing / gateway
health). The more useful cut is by **where the answer already lives**:

| the answer is… | needs | example |
|---|---|---|
| a property of **durable state** | a read helper, **no migration** | 1824 — a memory's `review_reason` + `status` + `review_marked_at` already record whether a revalidation was borne out (`database.rs:1030-1044`) |
| a property of **rows already written** | a counting read, **no migration** | 1851, 1852 — `gateway_unhealthy` / `gateway_backend_changed` are already in `lifecycle_events` |
| a **quantity on a routed turn** | caller-side wiring on an existing schema | 1850, 1855 — `first_token_at`, `input_tokens`, `cost_confidence` are declared and never set |
| an **ephemeral decision** | this table | 1822 — nothing records that a retrieval happened |
| a **judgment nobody produces** | not schema work at all | 1823, 1825, 1831 |

Only the fourth row is a migration. That is why this design closes far fewer
than 37 lines, and says so in §4.

---

## 2. The schema

Migration **15**. `SUPPORTED_SCHEMA_VERSION` moves from 14 to 15
(`database.rs:80`), and the doc comment above it gains a sentence, as every
migration before it has.

```sql
CREATE TABLE evaluation_observations (
    seq          INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id   TEXT    NOT NULL,
    observed_at  INTEGER NOT NULL,

    -- What was decided. NOT a SQL vocabulary — see "Why `kind` has no CHECK".
    kind         TEXT    NOT NULL CHECK (kind <> ''),

    -- How it turned out, as far as was known when this row was written.
    -- Never silently absent: migration 11's `context_state` argument.
    outcome      TEXT    NOT NULL DEFAULT 'unknown' CHECK (outcome <> ''),

    -- What it was about, in the vocabulary of `kind`. Free text, never a
    -- count key on its own.
    subject      TEXT,

    -- The session the decision was made for, when it was made for one.
    session_id   TEXT,

    -- The A/B half. Two columns, never one joined string: migration 8's
    -- "remain separately represented" rule (`database.rs:831-836`).
    feature      TEXT,
    arm          TEXT,
    CHECK ((feature IS NULL) = (arm IS NULL)),

    -- Provenance: the row in the ledger that owns the measurement, so this
    -- table never copies one. Bare ids, no REFERENCES — migration 12's rule.
    memory_id    TEXT,
    routing_seq  INTEGER,

    -- The sentence a human reads after a count surprises them. Never parsed,
    -- never a WHERE key. `gateway_cause` (migration 7) is the precedent.
    detail       TEXT
);

-- The one access pattern this table exists for: how many rows of one kind
-- fell in a window. Everything Phase 51 asks is a filter on this index plus
-- a GROUP BY outcome.
CREATE INDEX evaluation_observations_by_kind_time
    ON evaluation_observations (kind, observed_at);

CREATE TRIGGER evaluation_observations_reject_foreign_project_insert
BEFORE INSERT ON evaluation_observations
FOR EACH ROW
WHEN NEW.project_id IS NOT (
    SELECT value FROM project_metadata WHERE key = 'project_id'
)
BEGIN
    SELECT RAISE(ABORT, 'evaluation observation belongs to a different project');
END;

CREATE TRIGGER evaluation_observations_reject_foreign_project_update
BEFORE UPDATE OF project_id ON evaluation_observations
FOR EACH ROW
WHEN NEW.project_id IS NOT (
    SELECT value FROM project_metadata WHERE key = 'project_id'
)
BEGIN
    SELECT RAISE(ABORT, 'evaluation observation belongs to a different project');
END;
```

That is the whole migration. **No `ALTER TABLE`, no rebuild, no existing row
touched, no existing `CHECK` altered.**

### Why `kind` has no `CHECK`, and why that is not a lapse

This is the design's one genuinely contestable choice, so here is the argument
in full.

A `CHECK (kind IN (...))` is what `lifecycle_events` has, and it is exactly why
map lines 310, 327 and 1316 are refused today: SQLite cannot widen a `CHECK` in
place, so an eleventh value cost a full table rebuild (migration 7,
`database.rs:711-717`), and a twelfth is forbidden outright by the house rule
at `database.rs:838-844`. **Phase 51 is the phase whose vocabulary is
guaranteed to grow** — every future measurable feature wants a new kind. Putting
a SQL vocabulary on this column would be manufacturing migration 7's problem
deliberately, in the one table most certain to need widening.

The house already has the answer, in two places:

1. **`LIFECYCLE_EVENT_KINDS` exists because the SQL `CHECK` was not trusted
   alone.** Its doc says so: *"Here rather than only in the SQL so that
   `LifecycleEvent::kind` can be pinned against it by a test"*
   (`database.rs:82-88`). The Rust constant plus an exhaustive `match` plus a
   test that inserts every variant is the mechanism that actually catches
   drift; the `CHECK` is the belt beside it.
2. **`response_profile` gets no `CHECK` at all**, on the stated ground that
   pinning its combinations *"would be a vocabulary this file has no business
   holding"* (`database.rs:869-875`). An unrecognised encoding is reported as
   an error rather than guessed at.

`evaluation_observations.kind` is `response_profile`'s case, not
`lifecycle_events.kind`'s. So:

- the vocabulary is a Rust enum, `EvaluationKind`, with a
  `const EVALUATION_KINDS: [&str; N]` beside it in `database.rs` for the same
  reason `LIFECYCLE_EVENT_KINDS` is there;
- the store encodes through an exhaustive `match`, so a new variant is a
  compile error at the writer;
- one test inserts every `(kind, outcome)` pair the enum can produce, through
  the real schema — the shape
  `every_stored_vocabulary_is_one_the_schema_accepts` already has;
- a reader that meets a `kind` it does not know **reports it** rather than
  bucketing it into a neighbour, because a count that silently absorbs an
  unknown kind is worse than one that refuses.

What is given up: a hand-written `INSERT` at a `sqlite3` prompt can store
nonsense. That is true of `response_profile` today and has not hurt.

`outcome` is the same case for a sharper reason — its vocabulary is *per kind*
(`helped`/`stale` for a retrieval, `preferred`/`displaced` for a route), so a
single global `CHECK` would be two vocabularies in one column, which is the
first objection this design makes to widening `lifecycle_events` at all.

### Why `outcome` is the one column that is `NOT NULL DEFAULT 'unknown'`

Verbatim migration 11's argument for `context_state`
(`database.rs:1102-1110`): every other column's NULL means *"not recorded"*, but
a row that does not say how it turned out must not be countable as *"turned out
badly"*. `DEFAULT 'unknown'` makes that automatic for any future insert path
that forgets to think about it, and it is what lets *"how often retrieved memory
was useful"* report an honest denominator with an honest unknown bucket instead
of a flattering ratio.

### Outcomes learned later are new rows, never an `UPDATE`

A retrieval is recorded when it happens; whether it helped may only be knowable
a turn later. The answer is **a second row** with the same `memory_id` and a
later `observed_at`, not an edit. This is migration 11's *"append-oriented is a
property of the code as much as the schema"* (`database.rs:1061-1070`): the
store offers `record` and reads, and no method that edits a recorded
observation. A measurement edited in place is a falsified measurement.

There is deliberately **no** self-referencing `evaluates_seq` column.
`memory_id` + `session_id` + time ordering already answers every count in §4,
and migration 12's *"One direction only"* rule (`database.rs:1214-1220`) plus
the `checkpoints` rule that a column nothing queries is *"a liability with no
use"* (`database.rs:447-451`) both say not to carry it until a query needs it.

### One index, and the second one is an experiment, not an omission

`(kind, observed_at)` serves the shape every Phase 51 line reduces to. An A/B
split adds `feature`/`arm` to the `WHERE`, which this index does not cover.
**Do not add `(feature, arm, kind, observed_at)` on speculation.** The
implementing packet should measure it: fill the table to its retention ceiling
(§6), run the arm-split count, and read `EXPLAIN QUERY PLAN`. Add the index if
and only if the plan is a scan and the scan is slow at the ceiling. Same for an
index on `memory_id` — the 1822/1826 join is against `memories`, whose own
`memories_by_status_updated` index (`database.rs:302`) may already carry it.

---

## 3. How it relates to what already exists

**It is a new table.** Not a widening of `lifecycle_events`, not a view.

### Why not `lifecycle_events`

Four reasons, the first two from the brief and the last two added here.

1. **Vocabulary mismatch.** All eleven values in `LIFECYCLE_EVENT_KINDS`
   (`database.rs:89-101`) are things that happened *to a session's process or
   its harness* — a turn, a keystroke, a process exit, a backend going
   unhealthy. Phase 51's subjects are decisions *Glasshouse* made. The module
   doc scopes the stream explicitly and two tests exist to keep it narrow —
   `no_harness_is_named_in_the_core_event_stream` and
   `turn_completion_is_minted_in_exactly_one_place` (`events/mod.rs:1-40`).
2. **It is migration 11's own argument, one level up.** `routing_observations`
   is a separate table rather than columns on `sessions` because *"a dedicated
   table with its own `seq` is migration 4's own argument for
   `lifecycle_events` over a column on `sessions`, applied here for the same
   reason"* (`database.rs:1050-1059`). Applied a third time here.
3. **The rebuild is forbidden, and the risk is specific.** Widening
   `lifecycle_events.kind` is a third rebuild of the one table
   `memories.source_event_first`/`_last` reference by `seq` — the hazard
   migration 7 documents at length (`database.rs:718-733`) and the house rule
   at `database.rs:838-844` refuses.
4. **⚠ `lifecycle_events` can never be pruned, and this table must be.** Its
   three triggers `RAISE(ABORT)` on every `UPDATE` and every `DELETE`
   (`database.rs:490-512`). Anything folded into it is permanent by
   construction. An evaluation ledger that grows per decision and can never be
   trimmed is the "defect with a delay" §6 is about — so the fold is not merely
   awkward, it is unimplementable within the retention policy this design owes.

### Why not a view

A view computes from stored rows. The rows this phase needs — *a retrieval
happened*, *this route was preferred over that one* — **are not stored
anywhere**. `memory_search_grouped` (`main.rs:2915`, called from `main.rs:2948`
and `api/unix.rs:1348`) returns its result and forgets. There is nothing to
project. Where a view *would* work — 1824, 1851, 1852 — this design says use a
read helper and no migration at all (§4).

### Why not `routing_observations`

Because it is the wrong grain and the right neighbour. It is one row per
*routed turn through the gateway*; its sole writer is inside
`gateway/session.rs`, so a native subscription session produces zero rows. A
memory retrieval is not a routed turn. The relationship is `routing_seq`: an
evaluation row **points at** the observation that measured the turn instead of
copying its cost, which is what structurally enforces the brief's *"must not
duplicate"* rule rather than leaving it to a future author's discipline.

**The cost of that pointer, stated plainly:** it puts `routing_observations`
into the same category `lifecycle_events` is already in — a table whose `seq`
a future migration may not renumber. `AUTOINCREMENT` already guarantees no
reuse after pruning (`database.rs:1067-1070`), so pruning is safe; a *rebuild*
is not. The implementing packet owes a test in migration 7's shape —
`an_evaluation_rows_provenance_survives_a_routing_observations_rebuild`,
exercised against a deliberately naive rebuild — or the pointer is a trap
already laid.

### Why not `EvidenceLedger` or `GatewayQuotaCache`

`EvidenceLedger` covers gateway-forwarded turns only and structurally cannot
see memory operations. `GatewayQuotaCache` is one JSON file per provider
holding only the most recent reading, overwritten each time — a snapshot, not a
history — and growing it into a second history mechanism is exactly what the
brief forbids.

---

## 4. Which of the 37 lines this design unblocks — and which it does not

Phase 51 is `docs/product/capability-map.md:1820-1856`, 37 open lines, verified
with `scripts/discover.py --phase 51`.

### A. Closed by this table plus a producer that already exists — **3 lines**

| line | text | the producer |
|---|---|---|
| 1822 | *"Measure how often stale or incorrect memory is retrieved."* | `main.rs:2931` (`store().search_grouped`) writes one row per returned memory with `memory_id`. "Stale" is then **not a judgment**: it is a join to `memories.status = 'superseded'` / `review_reason IS NOT NULL`, columns migration 10 already added (`database.rs:1030-1044`). |
| 1826 | *"Measure how often superseded memories are incorrectly resurfaced as current guidance."* | Same producer, same join, narrower predicate (`status = 'superseded'`). |
| 1856 | *"Keep evaluation data local and project-scoped unless the user explicitly exports it."* | The two triggers in §2, plus the absence of any export function in the new module — structurally identical to how `routing_observations` carries map line 1343. Closes in the same package that lands the table, not before. |

1822 and 1826 are the design's strongest claim because **neither needs a new
signal**. Everything the count needs is already durable; the only missing fact
is *that a retrieval happened at all*, which is precisely and only what this
table adds.

### B. Unblocked by the table, conditional on a Phase −1 check the design cannot do read-only — **4 lines**

Each names a producer site that plausibly exists and that **this worker did not
verify carries the required input**. Packaging any of them without that check
is the failure `assurance-economics.md` Phase −1 exists to prevent.

| line | what it needs | the site to check |
|---|---|---|
| 1835 | the route chosen **and the route displaced**, in one row | the router's selection point. Verify it *knows* what it displaced; if it selects without ever materialising the alternative, the line stays refused and no schema fixes it. |
| 1846 | the pairing prediction, so a later outcome can be compared to it | `harness::pairing::classify` (`harness/pairing.rs:935`, production — the file's `#[cfg(test)]` starts at `:1507`). Verify it is on the launch path, not only in a report renderer. |
| 1836, 1837 | a **history** of quota readings, not the latest one | `GatewayQuotaCache` is a per-provider snapshot. These need each reading written as a row here at the moment it is read. Verify a reading site exists on a path that runs. |

**⚠ DEPARTURE FROM THE BRIEF (1836, 1837).** `design-decisions.md:2322-2325` assigns
1836 and 1837 to the *zero-migration gateway-health cluster*, to be answered by
a counting read over `lifecycle_events`. `design-decisions.md:2369-2372` then says
*"the quota-history lines must route through the new table."* Both cannot hold:
1836 is *"accuracy of estimated subscription headroom against observed
throttling and resets"* and 1837 is *"how often protected quota remains
available"* — both are questions about **quota readings over time**, and
counting `gateway_unhealthy` rows measures a different thing entirely (how often
a backend broke). **Ruling: 1836 and 1837 belong to this table, not to step 1.**
The brief's step-1 cluster is 1851 and 1852 only, and it shrinks from four lines
to two.

### C. Not this table — answerable with **no migration at all** — **3 lines**

| line | where the answer already is |
|---|---|
| 1851 | `gateway_unhealthy` + `gateway_backend_changed` rows already written by a real production caller in `gateway/session.rs`. A counting read over `lifecycle_events`. |
| 1852 | Same rows, correlated across routes. |
| 1824 | **⚠ DEPARTURE FROM THE BRIEF.** *"how often revalidation correctly identifies a decision whose original assumptions no longer hold"* is answerable from `memories` alone: `mark_for_review` has a production caller at `main.rs:3227`, `review_marked_at` records when, `review_reason` survives resolution (`main.rs:3043-3046`), and `status` records what happened next. **A read helper, not an event.** The brief's memory-evaluation cluster (1820-1826, 1831) should lose it. |

**On the counting primitive itself:** the brief states that today *"the only
readers are `EventLog::all()` and `for_session()`, both full scans"*
(`design-decisions.md:2326-2328`). That is not current. `events/log.rs` has six
readers, including `recent`, `recent_for_session`, `observed_since`, `head`, and
**`len()`, which is already a `COUNT(*)` in SQL** (`log.rs:402-414`). The claim
*"Glasshouse cannot count occurrences over time"* is therefore too strong as
written; the accurate form is **"cannot count by kind within a window"**, which
is one method — `count_by_kind(kind, from, to)` — and not a migration. Step 1 is
even cheaper than the brief thought.

### D. Not this table — caller-side wiring on `routing_observations` — **7 lines**

1832, 1833, 1845, 1847, 1849, 1850, 1855. Every one measures a **quantity on a
routed turn**, and `routing_observations` already declares the columns:
`input_tokens`, `output_tokens`, `cached_input_tokens`, `cost_micro_usd`,
`cost_confidence`, `tool_rounds`, `retries`, `repairs`, `failovers`, `purpose`,
and the four timestamps (`database.rs:1123-1163`). The single production writer
sets none of them. **A Phase 51 table with its own cost or latency column would
be a second source of truth for a fact that ledger already models**, which is
why this design has no `magnitude`/`unit` pair despite four lines wanting one.

Two caveats. 1847 is split: *output-token reduction* and *profile overrides* are
reachable (the former here, the latter as an evaluation row), while *missing
caveats* and *additional steering* are judgments with no producer — so 1847 does
not close on wiring alone. 1849 (*"routing latency added before interactive task
execution"*) is a **decision** duration, and `routing_observations` has
`dispatched_at` but nothing marking when routing began; it needs one nullable
`ALTER TABLE routing_observations ADD COLUMN decided_at INTEGER` — migration 3's
shape, no rebuild — and that is a different, much smaller migration than this
one.

### E. Not unblocked by anything schema-shaped — **20 lines**

These have **no producer, and inventing one would be fabrication.** Grouped by
what is actually missing:

- **The signal is outside Glasshouse's process boundary** — 1820. *"Repository
  exploration operations"* are the agent's own tool calls inside the harness.
  Glasshouse hosts a PTY and sees bytes; no `LIFECYCLE_EVENT_KINDS` value
  reports a tool call. Refusal-register Cluster F shape.
- **The verdict is a human/agent judgment nothing emits** — 1821 (*"actually
  useful"*), 1823 (*"unnecessary implementation complexity"*), 1825
  (*"whether the challenge was justified"*), 1831 (*"prevents repetition"*),
  1854 (*"causes a poor routing decision"*). This table gives each of them a
  home; none of them has a producer, and a home is not a producer.
  **⚠ 1831 is in the brief's memory-evaluation cluster; on this reading it does
  not belong there** — it is a judgment line, not a retrieval-count line.
- **The feature does not exist** — 1838, 1839, 1840, 1841, 1842, 1843.
  `Guardrail` has **zero matches** anywhere in `crates/glasshouse/src`
  (verified by grep, 2026-08-29); Phase 21K is unbuilt. 1839 and 1840 also
  measure amounts, not counts, so they would not be this table's shape even
  once the feature lands.
- **The mechanism has no production caller** — 1828 and 1844. `recovery::` has
  **zero references outside `session/recovery.rs`** (verified by grep).
- **The discriminating input is never set** — 1829. `user_override_signal` is
  assigned `None` at both of its production sites (`config/pairing.rs:348`,
  `routing/evidence.rs:1208`) and nowhere else. Refusal-register Cluster J
  shape.
- **Nothing escalates at runtime** — 1834. `WorkloadTier::escalate` is reached
  only through `conservative_workload_tier`, whose single production call site
  (`routing/classify.rs:615`, above the file's `#[cfg(test)]` at `:641`) is a
  **report renderer** that writes the value to a string. A classification that
  is printed and never acted on produces no escalation to count.
- **Nothing probes** — 1853. Recorded already at
  `design-decisions.md:2219` — every probe path is user-invoked; there is no
  automatic prober whose capacity consumption could be measured.
- **Depends on a line above** — 1830 (needs a warm-session *reuse decision
  point*; `warm_context()` exists in classification but no reuse selector was
  found — **verify before packaging**), 1848 (a grouping over 1847's data),
  1827 (*"production-aware checks"* — no such feature).

### The tally

| bucket | lines | count |
|---|---|---|
| **A** — closed by this table + an existing producer | 1822, 1826, 1856 | **3** |
| **B** — this table, pending a Phase −1 producer check | 1835, 1836, 1837, 1846 | **4** |
| **C** — no migration; a read helper | 1824, 1851, 1852 | **3** |
| **D** — `routing_observations` wiring (1849 also a 1-column ALTER) | 1832, 1833, 1845, 1847, 1849, 1850, 1855 | **7** |
| **E** — no producer; not schema work | the twenty above | **20** |

**This design unblocks 7 of 37 lines** (A + B), and closes 3 of them outright.
It explicitly does not touch 30. A design claiming more would be a wish.

---

## 5. The migration's shape, and the `LIFECYCLE_EVENT_KINDS` question

### It is `CREATE TABLE` only

Migration 15 creates one table, one index, two triggers. It runs through the
same `execute_batch` ladder as every migration before it (`database.rs:1713-1718`),
appended to `MIGRATIONS` with `SUPPORTED_SCHEMA_VERSION` moved to 15. Nothing
existing is renamed, copied, dropped or altered. **No table is rebuilt. No
`CHECK` is widened. The house rule at `database.rs:838-844` is not approached,
let alone tested.**

### **This design needs no new `LIFECYCLE_EVENT_KINDS` value. Explicitly.**

Stated plainly because the packet is right that a design which quietly assumed
one away would be worse than none: `LIFECYCLE_EVENT_KINDS` stays at 11 entries
(`database.rs:89`), `lifecycle_events` is not touched, and **map lines 310, 327
and 1316 remain refused on exactly the ground the refusal register gives them**
(Cluster G, `docs/process/refusal-register.md:128-129`). Nothing here helps
them. A compaction event is a thing that happened to a harness process; it
belongs in `lifecycle_events` and cannot be admitted there.

### One observation Cluster G should have, which this design does *not* act on

If the project ever pays for a `lifecycle_events` rebuild, **the rebuild should
drop the `kind` `CHECK` rather than add a twelfth value to it.** The cost is
identical — SQLite cannot alter a `CHECK`, so removing one and widening one are
the same rename-copy-drop-recreate — but removing it is paid once, while
widening it is paid again at every future value. The safety that is lost is
already provided in Rust by `LIFECYCLE_EVENT_KINDS` plus its pinning test,
which is where `database.rs:82-88` says the real guarantee lives.

**This is a decision for the orchestrator and the user, not for this design,
and it must not be smuggled in as a side effect of migration 15.** It carries
migration 7's whole `seq` hazard (`database.rs:718-733`) and would need
`a_memorys_provenance_survives_the_seq_rebuild` re-run against it. Recorded here
only because Cluster G currently reads as permanently blocked, and it is not —
it is blocked on a decision nobody has been asked to make.

---

## 6. What it costs, and what prunes it

### Nothing in Glasshouse prunes anything today

Verified, not assumed: a grep for `fn prune`, `retention`, `VACUUM` and
`DELETE FROM` across `crates/glasshouse/src` returns **no production retention
path of any kind** — only test-side migration undos and `project_metadata`
deletes. `memories` grows forever (rejection is a status, not a delete),
`lifecycle_events` grows forever *and cannot be trimmed even deliberately*
(`database.rs:500-512`), and `routing_observations` grows forever while its own
doc comment anticipates *"some future retention policy"* that was never written
(`database.rs:1067-1070`).

**So this table would be the fourth unbounded ledger, and it is the one with
the highest write rate.** The packet's line — *an event table nobody prunes is a
defect with a delay* — is the accurate reading of the current state of the
schema, and this design treats retention as part of the migration's contract
rather than as future work.

### Rows per session, and the arithmetic

| producer | rate | source |
|---|---|---|
| memory retrieval | ~1 row per returned memory, per `memory search` / API query — human-driven | `main.rs:2948`, `api/unix.rs:1348` |
| memory extraction | ~1 row per **completed turn** — extraction runs after every one | `events/log.rs:322-325` |
| routing decision | ~1 row per routed turn | the router's selection point |
| quota reading (1836/1837) | ~1 row per reading | a gateway quota read |

**~2–5 rows per turn.** A heavy interactive day of 300 turns is ~1,000 rows. At
an estimated ~150 bytes per row plus one index, that is **~200 KB/day heavy,
~5 MB/month, ~60 MB/year, unbounded** — not catastrophic, and exactly the shape
of thing that is invisible for six months and then is somebody's problem.

### The retention policy, which is part of the migration, not a follow-up

**Two bounds, whichever binds first: 90 days, and 100,000 rows.** At the
estimate above that is a ceiling near 15 MB and a window comfortably longer
than any A/B comparison the alpha directive asks for. Phase 51's questions are
*rate* questions; a rate needs a window, not a history.

**Enforced in the writer's own transaction, on a cadence, with no new
mechanism.** The store's `record` runs the trim every *N*th insert (N = 256,
cheap and self-limiting) inside the same transaction as the append:

```sql
DELETE FROM evaluation_observations
 WHERE observed_at < :cutoff
    OR seq <= (SELECT MAX(seq) FROM evaluation_observations) - :max_rows;
```

Three properties this shape has and a background pruner does not:

1. **No new path.** Practice §65 is the reason: a resource acquired on a path
   nobody asserts about is free on the developer's machine and billed on
   Windows. A pruner thread would open a second SQLite handle for the life of
   the process — the precise shape that hung six tests for 37 minutes. The trim
   runs on a connection that is already open and already writing.
2. **Pruning cannot corrupt a provenance pointer.** `AUTOINCREMENT` means a
   `seq` is never reused after a delete (`database.rs:1067-1070`), so a pruned
   row's identity can never come to mean a different row.
3. **No append-only `DELETE` trigger.** Migration 5's three triggers are
   deliberately **not** copied — they are what makes `lifecycle_events`
   unprunable. Migration 11's two are copied exactly, and they are the only
   ones. *This is the load-bearing difference between the two precedents, and
   it is why the table is named `evaluation_observations` and not
   `evaluation_events`*: the name should pull a future author toward migration
   11's prunable-ledger pattern and away from migration 5's permanent stream.

### What a count means once rows are pruned

**A count over a window older than the retention bound is wrong, and must not
be silently returned.** The read API therefore refuses a `from` earlier than the
oldest retained row rather than reporting a small number — the same
visible-degradation rule the enum columns follow. The implementing packet owes
one test: `a_count_over_a_pruned_window_refuses_rather_than_undercounting`.

---

## 7. Experiments the implementing packet should run *before* writing the table

Per the packet's instruction to write down the experiment rather than prove it
in code:

1. **Do step 1 first, and confirm the counting shape against real data.** Add
   `EventLog::count_by_kind(kind, from, to)` and answer 1851/1852 with it. It is
   one method, zero migration, and it tests the whole `(kind, window, count)`
   query shape against rows that already exist. Committing a schema before this
   is designing against guesses — the brief's own warning, and it is right.
2. **Phase −1 on the four bucket-B producers** (§4B). Each needs a producer, a
   caller that carries the input, a propagation path, and a consumer. 1835 and
   1830 are the likeliest to fail it.
3. **Measure the index question, do not guess it** (§2). Fill to 100,000 rows,
   run the arm-split count, read `EXPLAIN QUERY PLAN`, and add the second index
   only if the plan scans and the scan is slow.
4. **Measure the row estimate.** Instrument a real session end-to-end and count
   the rows it would have produced. If it is 20 per turn rather than 2–5, the
   retention bounds in §6 are wrong by an order of magnitude and should move
   before they are shipped, not after.
5. **Write the rebuild-provenance test with the pointer, in the same change**
   (§3) — `routing_seq` is a promise about `routing_observations`' future, and
   an unpromised promise is how migration 7's hazard was created the first time.

## 8. What this design does not decide

- **Whether the table is worth its migration at all.** §1's paragraph is the
  arguable one. If the answer is "measure by hand", the honest move is to do
  step 1, wire `routing_observations`' unset fields, close bucket C and D, and
  leave the table unwritten — that is 10 of 37 lines with **no migration**, and
  it is a defensible place to stop.
- **Whether to free `lifecycle_events.kind`** (§5). Cluster G's fate, and a
  question for the user.
- **The exact `EvaluationKind` variants.** They should be introduced one per
  producer as producers land, not enumerated up front — an enum written before
  its writers is the same mistake as a table written before its counts.
