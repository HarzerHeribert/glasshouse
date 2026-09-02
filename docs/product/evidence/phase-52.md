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
