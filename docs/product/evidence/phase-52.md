# Capability evidence — phase 52

Phase 52 — *criteria before adding semantic/vector retrieval*. Six lines
(1865–1870), and no entry here until 2026-09-02. The census is
`GH-RECON-52-53` (Sonnet high, read-only; `.agent-runtime/report-recon-52-53.md`),
spot-checked by the orchestrator against current source.

**The tree-wide fact every line below rests on:** no vector, embedding or
semantic-retrieval code exists anywhere in `crates/glasshouse/src`. Every grep
hit on those words is prose (`Vec`, `paragraph`, a comment about *not* drawing
a graph). `JobKind::Reranking` (`routing/disposable.rs:64`) is a declared
variant with no production caller — the same fact `phase-24.md` records as
*"no cheap model is wired up in this build"* (`routing/classify.rs:583-586`).
`scripts/cluster-b.py` finds nothing for `vector|embed|semantic|graph|rerank`.

## Censused 2026-09-02 — six lines, three causes

### 1867, 1868, 1869, 1870 — REFUSED, Cluster Q

`refusal-register.md`'s Cluster Q: *a negative requirement over a capability
that does not exist* cannot be closed, because the test would pass on the
feature's absence and nothing would be watched. All four are conditioned on
semantic retrieval existing:

- **1867** *"If semantic retrieval is added, combine it with lexical…"* — no
  second retrieval path exists to combine with the one lexical ranker
  (`memory/search.rs` BM25 + `policy::retrieval_weight`).
- **1868** *"Keep project isolation physically intact when adding embeddings."*
  — no embeddings table or column exists to keep isolated. (Isolation of the
  *existing* `memories` table is real — `database.rs:487-523`'s triggers,
  Phase 21F's isolation mutation — and is a different, closed box.)
- **1869** *"Ensure semantic retrieval respects memory lifecycle status…"* —
  nothing can resurrect a superseded memory because there is no second path
  to disagree with the first. The precedent whoever builds it must copy:
  `SearchScope::Current` admits only `MemoryStatus::Active`
  (`memory/search.rs:44-54`), restated for injection at `memory/inject.rs:41-46`.
- **1870** *"Evaluate semantic retrieval on real Glasshouse queries before
  making it part of the default path."* — reads as an evaluation gate, has
  the restraint's shape: the object of the evaluation does not exist, so no
  evaluation has been skipped.

A source-scanning tripwire would be worth having and **would not tick any of
these** (the 616/622 ruling, `refusal-register.md:520`). No successor until a
semantic-retrieval prototype exists to gate.

### 1866 — REFUSED, successor named

*"Define concrete retrieval cases that lexical search cannot solve before
selecting an embedding system."* Answerable in principle without building
the feature — from recorded examples of queries BM25 under-served — and there
are none: no design document attempts it (`design-decisions.md` has no
`embedding` or `semantic retrieval` entry) and nothing records a lexical
failure. **Not Cluster Q** — it names a reachable question. Successor:
revisit once `GH-RETRIEVAL-CRITERIA`'s miss rows (below) have accumulated in
real projects; only then is there material to define cases from.

### 1865 — PACKAGED: `GH-RETRIEVAL-CRITERIA` (Amber, Sonnet high), dispatched 2026-09-02

*"Do not add vector retrieval until FTS5 retrieval failures are observed and
recorded in real projects."* The restraint half is Cluster-Q-trivial (nothing
added). **The measurement half names a producer this project can build
today, over inputs it already has**, and it did not exist:

- `evaluation::record_memory_retrieval` (`evaluation/mod.rs:1716`) writes one
  `MemoryRetrieved` row per *returned* memory and returns early on an empty
  iterator (`:1729-1731`) — a zero-result search is invisible to the ledger
  by construction, and the variant's own doc says so (`:93-100`).
- Its one production caller, `main.rs::memory_search_grouped` (`:10407`), has
  the empty result in scope at that exact point. The launch-time door,
  `memory/inject.rs::briefing` (`:256`, called from `main.rs:1487` and
  `api/unix.rs:1554`), records nothing on any outcome.
- **No production surface reads any memory-evaluation count.**
  `EvaluationObservations::stale_retrievals` (`evaluation/mod.rs:948`) has
  zero production callers (`cluster-b.py`), which is also why Phase 51's
  1822 and 1826 were re-opened the same day (`phase-51.md`).

**Ruling on the tick condition, made before dispatch.** The line asks that
failures be *observed and recorded*; both verbs are mechanisms. 1865 ticks
when (a) a zero-result search on **every** production door — the
`memory search` command and API query, and the launch-time briefing — writes
a `MemoryRetrievalMiss` row carrying scope and no query text, (b)
`glasshouse memory retrievals` prints the counts, giving `stale_retrievals`
its first production caller in the same stroke, and (c) both are
mutation-proven through the shipped binary. Real-project accumulation is not
a tick condition: a recorder that runs in every project is *in real projects*
from its first run, and the Phase 51 precedent (1822/1826 ticked on producer
plus reader) stands — corrected by the same package so that the reader has a
production caller. The briefing door was made a condition rather than a
recorded limit because it is almost certainly the dominant real-world
retrieval path; a producer that measured only the CLI door would report the
quiet door and miss the busy one.

---

## 1865 — CLOSED 2026-09-02 (`GH-RETRIEVAL-CRITERIA`, Amber, Sonnet high) — and 1822, 1826 re-closed with it (`phase-51.md`)

One mechanism, both directions. `EvaluationKind::MemoryRetrievalMiss` and
`record_memory_retrieval_miss` beside the retrieval producer; a third
`RetrievalScope`, `Injection`, so the launch-time door is its own word;
`briefing` returns a `BriefingOutcome` (`Injected`, `NothingMatched`,
`NothingNew`) so its two callers can tell *the search found nothing* from
*it found something and rightly withheld all of it*; and
`glasshouse memory retrievals --hours N` prints returned, stale,
stale-under-history, unresolved and missed. §65 is kept on every door (the
memory handle is dropped before the ledger opens; the report names each
drop). One scope overflow, disclosed: `tests/file_memory_lookup.rs`'s six
call sites gained `.into_injection()`, no assertion changed.

**Phase 52 stands at 1 of 6.** 1866 waits on this producer's rows; the
other four are Cluster Q (above).

### Do not add vector retrieval until FTS5 retrieval failures are observed and recorded in real projects. (line 1865)

Contract: Given a memory search that matched nothing, on any production door (the `memory search` command and API query, and the launch-time briefing), when it completes, Glasshouse records one retrieval-miss row in the project's evaluation ledger; and given a reader asking how retrieval has been doing, `glasshouse memory retrievals` prints returned, stale, stale-under-history, unresolved and missed counts for a window.

State: **COMPLETE** — ruled 2026-09-02 against the tick condition recorded above: the miss is written on every production door (the search core shared by the CLI and the API, and both briefing callers), carries scope and no query text, and is read by `glasshouse memory retrievals`; all three mutations are on the decisions the line names and are KILLED through the shipped binary. Two decisions the worker made and disclosed stand as scope, not defects: *nothing matched* is decided from the raw text search before any filter, so an all-excluded briefing is never a miss — and a text-search miss that the path-keyed file-observed route still serves is *not* recorded, because that is evidence a non-semantic mechanism covered it, which is the question this count exists to answer.

Production evidence:
- `evaluation/mod.rs` — `EvaluationKind::MemoryRetrievalMiss`
- `evaluation/mod.rs` — `record_memory_retrieval_miss`
- `evaluation/mod.rs` — `RetrievalScope::Injection`
- `main.rs` — `memory_search_grouped`
- `memory/inject.rs` — `BriefingOutcome`
- `memory/inject.rs` — `briefing`
- `main.rs` — `estimated_project_memory_tokens`
- `api/unix.rs` — `select_memory`
- `cli.rs` — `MemoryCommand::Retrievals`
- `main.rs` — `memory_retrievals_report`
- `main.rs` — `render_memory_retrievals`

Regression evidence:
- `evaluation_observations::a_search_that_returns_nothing_records_one_miss_row_and_nothing_else`
- `evaluation_observations::a_search_that_matches_leaves_no_miss_row`
- `evaluation_observations::a_recorded_retrieval_stores_no_memory_content`
- `evaluation_observations::glasshouse_memory_retrievals_keeps_stale_and_stale_under_history_disjoint`
- `evaluation_observations::glasshouse_memory_retrievals_prints_every_figure_for_the_window`
- `evaluation_observations::glasshouse_memory_retrievals_on_an_empty_window_prints_zeros_not_an_error`
- `main.rs::tests::a_briefing_that_matches_nothing_records_one_miss_row_under_injection_scope`
- `main.rs::tests::a_briefing_whose_matches_are_all_excluded_records_no_miss_row`
- `context_injection (15 tests, unchanged, end-to-end proof of select_memory)`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| main.rs: `if grouped...is_empty() && grouped.other.is_empty() {` -> `if false && grouped...is_empty() && grouped.other.is_empty() {` | `skip-state-update` | **killed** | `evaluation_observations::a_search_that_returns_nothing_records_one_miss_row_and_nothing_else` |
| main.rs render_memory_retrievals: `"stale-under-history", counts.stale_under_history` -> `"stale-under-history", 0` | `drop-the-reader` | **killed** | `evaluation_observations::glasshouse_memory_retrievals_keeps_stale_and_stale_under_history_disjoint` |
| main.rs estimated_project_memory_tokens: merged `Some(BriefingOutcome::NothingNew) | None => None` into the `NothingMatched` miss-recording arm | `conflate-outcomes` | **killed** | `main.rs::tests::a_briefing_whose_matches_are_all_excluded_records_no_miss_row` |

> skip-state-update observed: assertion `left == right` failed: a zero-result search must leave exactly one row: []

> drop-the-reader observed: assertion `left == right` failed (at evaluation_observations.rs:630)

> conflate-outcomes observed: panicked at crates/glasshouse/src/main.rs:16365:9 (assertion failure: a miss row was recorded for the excluded-but-matched case)

Recorded scope limits — stated by the worker, not discovered later:
- api/unix.rs::select_memory has no dedicated mutation of its own miss-recording arm (private, unreachable from main.rs's test module); proven end-to-end by tests/context_injection.rs (15 passing) and structurally identical to the mutation-proven main.rs caller
- estimated_project_memory_tokens records an injection-scope miss on every zero-match `glasshouse route` call, not only real launches
- stale_retrievals' own struct contract (stale is inclusive of stale_under_history) is unchanged; the reader renders them disjoint by subtraction, not by changing the underlying count

---

---

## 1866–1870 — CLOSED 2026-09-06 by the user's decision: seven experiment gates adopted as standing rules

The Cluster Q refusals above are superseded. Each line below is a rule the user adopted on 2026-09-06; the reasoning for treating a decided rule as a closed line is in `design-decisions.md`, *Seven experiment gates adopted as standing rules*.

### Define concrete retrieval cases that lexical search cannot solve before selecting an embedding system. (line 1866)

Contract: Given a proposal to add semantic retrieval, when its Phase −1 is written, it names concrete retrieval cases lexical search fails on — sourced from the misses `phase-52.md` 1865 records on every production search door — before an embedding system is selected.

State: **COMPLETE** — ruled 2026-09-06 by the user ("i agree with you on all 7 items"), recorded in `design-decisions.md`, *Seven experiment gates adopted as standing rules*. Ticked on the decision, not on a mechanism: no embedding index or graph store exists in this build, so there is nothing to mutate and no test names it. The named successor 1866 once carried (a recorded-miss reader) is the source of that list.

Limits: a standing rule, not a verified behaviour. Any package that adds what this line governs must cite this entry in its Phase −1 and carry the rule's own test; if such a package lands without one, this box comes back off.

### If semantic retrieval is added, combine it with lexical retrieval rather than replacing lexical retrieval. (line 1867)

Contract: Given semantic retrieval exists, when a query runs, lexical retrieval still runs and its results are combined, never replaced.

State: **COMPLETE** — ruled 2026-09-06 by the user ("i agree with you on all 7 items"), recorded in `design-decisions.md`, *Seven experiment gates adopted as standing rules*. Ticked on the decision, not on a mechanism: no embedding index or graph store exists in this build, so there is nothing to mutate and no test names it. 

Limits: a standing rule, not a verified behaviour. Any package that adds what this line governs must cite this entry in its Phase −1 and carry the rule's own test; if such a package lands without one, this box comes back off.

### Keep project isolation physically intact when adding embeddings. (line 1868)

Contract: Given embeddings exist, when they are stored, they live per project under the same physical isolation the SQLite store keeps by trigger — never a shared index.

State: **COMPLETE** — ruled 2026-09-06 by the user ("i agree with you on all 7 items"), recorded in `design-decisions.md`, *Seven experiment gates adopted as standing rules*. Ticked on the decision, not on a mechanism: no embedding index or graph store exists in this build, so there is nothing to mutate and no test names it. 

Limits: a standing rule, not a verified behaviour. Any package that adds what this line governs must cite this entry in its Phase −1 and carry the rule's own test; if such a package lands without one, this box comes back off.

### Ensure semantic retrieval respects memory lifecycle status and does not resurrect superseded knowledge as current truth. (line 1869)

Contract: Given a memory whose lifecycle status is superseded or retired, when semantic retrieval runs, that memory is never presented as current truth.

State: **COMPLETE** — ruled 2026-09-06 by the user ("i agree with you on all 7 items"), recorded in `design-decisions.md`, *Seven experiment gates adopted as standing rules*. Ticked on the decision, not on a mechanism: no embedding index or graph store exists in this build, so there is nothing to mutate and no test names it. 

Limits: a standing rule, not a verified behaviour. Any package that adds what this line governs must cite this entry in its Phase −1 and carry the rule's own test; if such a package lands without one, this box comes back off.

### Evaluate semantic retrieval on real Glasshouse queries before making it part of the default path. (line 1870)

Contract: Given semantic retrieval is built, when it is proposed as a default path, its evaluation on real Glasshouse queries is recorded first.

State: **COMPLETE** — ruled 2026-09-06 by the user ("i agree with you on all 7 items"), recorded in `design-decisions.md`, *Seven experiment gates adopted as standing rules*. Ticked on the decision, not on a mechanism: no embedding index or graph store exists in this build, so there is nothing to mutate and no test names it. 

Limits: a standing rule, not a verified behaviour. Any package that adds what this line governs must cite this entry in its Phase −1 and carry the rule's own test; if such a package lands without one, this box comes back off.

