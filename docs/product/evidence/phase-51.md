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
