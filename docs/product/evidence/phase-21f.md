# Capability evidence — phase 21F

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 21F — memory retrieval quality, over a schema that already carried the answers

Contract: Given a project whose durable memories carry authority, validity
conditions and validation timestamps, when an agent or a user retrieves memory
for a task, Glasshouse returns current invariants and constraints distinctly
from historical decisions, ranks a validated memory above an unvalidated one of
equal relevance, and carries each result's authority, validity state, rationale
and invalidation conditions — while never presenting a challenged memory as
settled, and never inventing a judgement about the repository the stored record
does not support.

State: **COMPLETE** for map lines 929, 931, 933, 935, 936, 937 and 938 — seven
of eleven. **NOT STARTED** for 930, 932, 934 and 939, each with its missing
source named below.

**No migration.** Every box here was closed over the existing schema: the
challenge operation reuses `MemoryStatus::NeedsReview` and `review_reason`,
both of which Phase 21C already shipped. `memory/store.rs` is unchanged by this
batch.

Production evidence:

- `memory/search.rs::RetrievalResult` and `MemoryStore::search_grouped`, with
  `is_current_invariant_or_constraint` — line 929's separation, reached through
  **both** real doors: `main.rs::memory_search_grouped` → `render_memory_report`
  (the `glasshouse memory search` CLI) and `api/unix.rs::query_memory` (the
  machine door).
- `memory/policy.rs::phase_penalty`, composed into the existing
  `retrieval_weight` — lines 931 and 933. It reads a memory's **own recorded
  `project_phase`** and whether it has ever been revalidated. It reads no
  repository state at all; see the note on line 932 below.
- `api/unix.rs::memory_result_json` — line 935. Authority, status, `current`,
  `may_constrain_implementation`, `review`, `last_validated_at` and `created_at`
  as **structured JSON fields**, not merely embedded in the rendered `report`
  string.
- `main.rs::constraint_lines` and `api/unix.rs::memory_result_json` — line 936,
  both gated on `MemoryAuthority::is_binding` rather than on whether the field
  happens to be populated, so an `Idea` does not carry invalidation conditions
  and a `Constraint` does.
- `main.rs::memory_challenge` over the existing `MemoryStore::mark_for_review`,
  plus `cli.rs`'s `Challenge` subcommand — lines 937 and 938.

Regression evidence:

- `memory_search.rs::search_grouped_separates_current_invariants_and_constraints_from_everything_else`
- `memory_search.rs::search_grouped_excludes_an_invalidated_invariant_from_the_rules_group_even_under_history`
- `memory_search.rs::a_validated_memory_outranks_an_equally_relevant_equally_authoritative_unvalidated_one`
  — the load-bearing ranking test.
- `memory_search.rs::a_prototype_phase_decision_is_penalized_until_it_is_reaffirmed`
- `policy.rs::decay_tests::{earlier_phases_carry_a_sharper_unvalidated_penalty, a_prototype_phase_unvalidated_decision_decays_faster_than_an_unrecorded_phase_one, reaffirming_a_prototype_phase_decision_clears_the_phase_penalty}`
- `main.rs::a_challenged_memory_drops_out_of_current_search_and_names_why`
- `main.rs::the_report_prints_validity_and_invalidation_conditions_only_for_binding_memories`
- `memory_authority.rs::a_challenge_refuses_a_memory_planted_from_another_project_and_writes_nothing`
- `tests/memory_query_api.rs` (new) — three tests **against the real shipped
  binary and its socket**, because `api/mod.rs:35-37` states this door "is
  proven only by running the shipped binary… never by an in-process unit test,
  which is the right proof for an external door anyway." Modelled on the
  existing `tests/capacity_api.rs` precedent for the same door.

Failure/isolation evidence:

- **The isolation mutation, re-run by the integrator, and the reason it is the
  most valuable thing in this batch.** Deleting `mark_for_review`'s leading
  `self.get(id)?` project-scope guard fails
  `a_challenge_refuses_a_memory_planted_from_another_project_and_writes_nothing`
  with `right: "active"` — i.e. the test read the foreign row back and found it
  had been **flipped to `needs_review`**. The call still returned an error
  either way, because the function's *trailing* `self.get(id)?` re-checks scope.
  **A test asserting only the returned error would have passed against a build
  that silently corrupted another project's memory.** The regression test reads
  the row back at the raw-connection level specifically to catch that. Restored
  byte-identical.
- Binary run, reproduced independently by the integrator against a scratch
  SQLite-backed project: the grouped render, a rejected review reason
  (`vibes` → the six valid reasons), a successful challenge, the challenged
  decision **absent** from `memory search` and **present and flagged** under
  `--history` (`challenged production_incident — not returned as settled until
  resolved`), preserving Phase 21C line 892's rule that invalidated decisions
  stay searchable as history.

Gates run by the integrator on the combined tree: `cargo fmt --all -- --check`
clean; clippy zero diagnostics; `cargo doc` clean; memory lib 78 passed;
`--test memory_search` 11, `--test memory_authority` 26, `--test
memory_query_api` 3, `--test project_isolation` 6 — all 0 failed.

**Line 938 is closed on the retrieval half, and the injection half is not
lost.** The line reads *"before further automatic injection into the same
task"*. There is no automatic injection anywhere in this build — Phase 27 is
0/11 — so there is nothing on that side to gate. Applying §33's test (ask the
capability as a question a user would ask): *"can Glasshouse stop presenting a
challenged memory as settled?"* — yes, demonstrated in the binary above. The
injection-side obligation is already owned by a **different, still-open box**,
Phase 27 line 1132 (*"do not inject stale ordinary decisions as binding
instructions when their original assumptions have not been validated"*), so
ticking 938 does not retire the requirement — it leaves it where a reader will
actually look for it.

**Why four stay open.**

- **930 and 934** are injection lines, and Phase 27 does not exist. Retrieval's
  half of 934 is already closed as line 904.
- **932** ("penalize memories whose assumptions conflict with current repository
  state") asks the storage layer for a judgement it cannot make, and this
  project has now settled the identical question four times — map lines 828,
  829, 862, and here. A keyword heuristic refuses real memories and admits fake
  ones. The worker was instructed not to re-derive it and did not. **Line 931 is
  deliberately *not* this line**: `phase_penalty` reads a memory's own recorded
  `project_phase` and its validation history, never the repository.
- **939** ("record false-positive or harmful memory retrievals") was the
  package's stretch box and was not attempted once the seven above were proven.
  It needs a retrieval-feedback record and is a clean small package on its own.

**A design caveat the worker raised and the integrator is recording rather than
fixing.** `search_grouped` groups what `search()` already fetched, rather than
fetching invariants/constraints and everything-else as two independently limited
pools. On a project with many binding rules, a small `limit` could see the rules
group starved by higher-ranking decisions in the raw fetch. No map line asks for
per-group limits, `overfetch_limit` already widens the SQL fetch beyond `limit`,
and redesigning the fetch strategy without an explicit requirement is the
speculative complexity Phase 21H exists to discourage. Recorded here so the next
person meets the decision rather than the surprise.

**A latent hazard this batch surfaced in code it did not touch, for the next
round — and the existing triggers do not cover it.** Six `UPDATE memories …
WHERE id = ?1` statements in `memory/store.rs` carry **no `project_id` in their
own WHERE clause**; each is project-scoped only by a leading `self.get(id)`
guard inside its own function. Checked one by one: `supersede`, `set_status`,
`mark_for_review`, `reaffirm` and `set_authority` **all currently have that
guard**, so isolation holds today and **there is no live defect.**

The obvious objection is that `database.rs` already defends this table
structurally — `memories_reject_foreign_project_insert` and
`memories_reject_foreign_project_update` both exist, and `database.rs:248`'s own
comment makes exactly this argument: *"a query can forget to filter by
`project_id`; a `BEFORE INSERT` / `BEFORE UPDATE` guard cannot be forgotten."*
**That reasoning does not reach this case.** The update trigger is `BEFORE UPDATE
OF project_id`, so it fires only when the binding column itself is written. A
status-only write to a foreign row never touches `project_id`, so no trigger
fires and the row is flipped. The triggers protect *the project binding*; they
do not decide *who may write the row*.

The mutation above shows the failure mode precisely: remove one leading guard
and a foreign row is written while the caller still receives a correct-looking
error, because the trailing `self.get(id)` re-checks scope after the damage.

Making each `UPDATE` carry `AND project_id = ?` — or widening the update trigger
beyond `OF project_id` — is a small, well-specified **red-tier** package: six
call sites, one behaviour, a precedent already in the file, and Phase 46's
contamination tests are where its evidence belongs. It is also the closest
existing work to Phase 1 line 110 (*"keep cross-project memory retrieval
disabled by design rather than relying only on query filters"*), which remains
unstarted and has no ledger entry.
