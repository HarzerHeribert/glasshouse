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

# Phase 21F — lines 930 and 934, closed behind Phase 27

Package `GH-INJECTION-RECALL`, worktree `.worktrees/injection-recall`; report
in `.agent-runtime/report-injection-recall.md`. Integrated 2026-08-29.

These two were filed as **Cluster D** in the refusal register earlier the same
batch — qualifiers on an injection that did not exist. Phase 27 built it, so
they became packageable. **932 remains open and is Cluster F**, declined four
times; do not re-package it.

## The product decision behind this package

Injection retrieved nothing in production: `sanitize_query` joins quoted
tokens with spaces, which FTS5 reads as implicit AND, so a prose task demanded
one memory contain every one of its words. A read-only recon established that
`sanitize_query` is **one implementation behind three doors** — the CLI search
box, the `query_memory` API verb, and injection.

**The user chose: injection gets its own query construction; CLI and API keep
AND.** Verified on the integrated tree — `sanitize_query`'s body is unchanged,
and `the_search_box_still_requires_every_word_of_a_query_to_match` pins it.

Injection's expression is `("a" "b" "c") OR ({subject} : ("a" OR "b" OR "c"))`.
The left disjunct is `sanitize_query`'s output **verbatim**, so injection's
result set is a superset of the search box's **by construction** rather than by
intention. `MemoryStore::search` was split into `search` and a private
`search_matching`, so both doors share the SQL, project scoping, conflict
flagging, ladder, decay weighting and truncation. **One ranking; only the
expression varies** — which is what keeps this clear of 1129's refusal, whose
objection was to a second *ranking*.

## The measurement that changed the design, and neither option offered was right

The packet asked whether `bm25()` alone suffices under OR or whether a
stop-word/frequency filter is needed. The worker measured both on a synthetic
fifteen-memory corpus driven through `briefing`, and **the answer was neither**:

| task | injected under a bare OR |
|---|---|
| *"Please look at the kestrel export and make sure it cannot write a partial file…"* | secrets, project isolation, pty line limit |
| *"Update the README with the new installation instructions."* | pty line limit, secrets, kestrel export |
| *"make sure it is up to date"* | project isolation, migrations, kestrel export |

The exactly-relevant memory is crowded out of its own task, and three unrelated
tasks return the same three memories.

**Why, and this is the part a threshold could not have fixed.**
`MemoryStore::search` sorts by `LadderRung` **before** the relevance/decay
weight (Phase 21E, lines 916–918), and `briefing` takes the
invariants-and-constraints group first (line 1131). Under AND the candidate set
was already narrow, so ranking by authority was safe. **Under OR, membership
costs one incidental word, so the top results become this project's
highest-authority memories whatever was asked.** The noise was never selected
by score, so no relevance threshold reaches it.

The frequency filter was ruled out by measurement too: the README task injected
three irrelevant memories with **no term above 47% of the corpus**, and in the
first row the noise memory matched on `file` and `look` — genuinely
discriminating words. A minimum-token-length filter keeps `make`, `sure`,
`look`, `when`, `with`, `that`.

**No stop-word list was imported and no threshold was introduced.**

## 930 — scope is the recorded subject, and it is not a threshold in disguise

A memory's scope is the `subject` it recorded, and it overlaps the task when
the task names a word of it — expressed as the `{subject}` column filter on the
added disjunct, so it is **membership in the query, not a filter after it**.
This is already what the store means by scope: `search::contradicts` uses the
subject, and only the subject, to decide two memories are about the same thing.

The test that makes the distinction load-bearing seeds a memory saying
*kestrel export* four times in its **body** under the subject *"billing
invoices"*. It is asserted to be the **stronger** BM25 match, and it is
excluded — while a memory whose body says nothing relevant is kept, because its
subject is *"kestrel export"*. **A threshold keeps the wrong one.**

Measured after the fix, same corpus and tasks: the prose kestrel task injects
*kestrel export* and *kestrel dashboard*; the README task and *"make sure it is
up to date"* inject **nothing**; the windows task injects *windows paths*.

---

### Inject only memories whose scope overlaps the current task. (line 930)

Contract: Given a routed task, when Glasshouse selects memory to inject, Glasshouse injects only memories whose recorded subject the task names, while preserving every memory today's conjunctive query already retrieves.

State: **COMPLETE**

Production evidence:
- `src/memory/search.rs` — `injection_query`
- `src/memory/search.rs` — `MemoryStore::search_grouped_for_injection`
- `src/memory/search.rs` — `SUBJECT_COLUMN`
- `src/memory/inject.rs` — `briefing`

Regression evidence:
- `context_injection::line_930_a_memory_out_of_the_tasks_scope_is_not_injected_though_it_is_retrievable`
- `memory_search::line_930_a_memory_whose_subject_is_about_something_else_is_out_of_scope_for_a_prose_task`
- `context_injection::a_task_written_as_a_sentence_retrieves_the_memory_it_is_about`
- `memory_search::a_keyword_task_retrieves_at_least_everything_it_retrieves_today`
- `memory_search::a_task_of_hundreds_of_distinct_words_is_still_a_query_fts5_accepts`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/search.rs: "({conjunctive}) OR ({{{SUBJECT_COLUMN}}} : ({scoped}))" -> "({conjunctive}) OR ({scoped})" | `drop-930s-scope-predicate` | **killed** | `context_injection::line_930_a_memory_out_of_the_tasks_scope_is_not_injected_though_it_is_retrievable` |
| src/memory/search.rs: .join(" OR "); -> .join(" "); | `revert-the-injection-join-to-and` | **killed** | `context_injection::a_task_written_as_a_sentence_retrieves_the_memory_it_is_about` |
| src/memory/search.rs: "({conjunctive}) OR ({{{SUBJECT_COLUMN}}} : ({scoped}))" -> "({{{SUBJECT_COLUMN}}} : ({scoped}))" | `drop-the-conjunctive-half-that-guarantees-no-recall-is-lost` | **killed** | `memory_search::a_keyword_task_retrieves_at_least_everything_it_retrieves_today` |
| src/memory/search.rs: AND memories.project_id = ?2 \ -> AND (?2 IS NOT NULL) \ | `drop-project-scope-on-the-refactored-injection-query` | **killed** | `context_injection::another_projects_memory_never_reaches_an_injected_block` |

> drop-930s-scope-predicate observed: context_injection.rs:1286 `a memory whose subject is about something else must not be injected` — with the column filter gone the block carried the provider-key invariant, which shares only `the`, `file` and `look` with the task

> revert-the-injection-join-to-and observed: context_injection.rs:1204 `a task written as a sentence must retrieve the memory it is about; the first delivery was: [the raw task]`. First run killed at deliveries()'s own timeout instead (§80 case 5); the test was changed to wait for ONE delivery so it fails on its own assertion, and the re-run is the verdict recorded here

> drop-the-conjunctive-half-that-guarantees-no-recall-is-lost observed: memory_search.rs:1054 `injection must not lose a memory the conjunctive query finds for "kestrel"` — a memory with no recorded subject became unreachable, which is the recall regression the left disjunct exists to make impossible

> drop-project-scope-on-the-refactored-injection-query observed: context_injection.rs:630 `another project's memory must never reach an injected block` — re-run because the SQL moved into search_matching during the refactor; Phase 27's isolation proof still holds on the new path, and only that one test failed

Recorded scope limits — stated by the worker, not discovered later:
- A memory that records NO subject is exempt from the scope rule: it is reachable only through the conjunctive half, which is exactly today's behaviour. Forced by 'strictly more recall, never less'. `subject` is optional in extract::schema, so OR noise can still enter through a subject-less memory.
- The corpus measurement behind this design is synthetic — fifteen memories of Glasshouse-shaped text. It refutes 'bm25 alone suffices' and 'a frequency filter suffices' by counterexample; it is not a claim about behaviour at very large corpora.
- Scope is the SUBJECT column only. A memory whose subject is worded differently from the task (`kestrel exports` vs `kestrel export`) does not overlap: the tokenizer is `unicode61` with no stemming, so this is exact-word overlap.

---

### Avoid injecting old ideas merely because they mention the same subsystem. (line 934)

Contract: Given a task that names a subsystem an old idea mentions, when Glasshouse selects memory to inject, Glasshouse omits an Idea-authority memory nothing has reaffirmed, while preserving injection of the same idea once MemoryStore::reaffirm has recorded it.

State: **COMPLETE**

Production evidence:
- `src/memory/inject.rs` — `is_unreaffirmed_idea`
- `src/memory/inject.rs` — `briefing`

Regression evidence:
- `context_injection::line_934_an_unreaffirmed_idea_is_not_injected_until_it_is_reaffirmed`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/inject.rs: record.authority == Some(MemoryAuthority::Idea) && record.last_validated_at.is_none() -> false | `drop-934s-authority-and-validation-predicate` | **killed** | `context_injection::line_934_an_unreaffirmed_idea_is_not_injected_until_it_is_reaffirmed` |

> drop-934s-authority-and-validation-predicate observed: context_injection.rs:1373 `an idea nobody has reaffirmed must not take an injection slot merely because the task names its subsystem` — the parquet idea took a slot beside the control memory

Recorded scope limits — stated by the worker, not discovered later:
- 'Old' is read as `last_validated_at.is_none()`, following inject::standing (line 1132) and policy::phase_penalty (line 933). A brand-new idea nobody has reaffirmed is therefore excluded too. The alternative reading of 'merely because they mention' needs the relevance signal Phase 27 refused for 1129.
- Scoped to MemoryAuthority::Idea alone. Hypothesis and Preference are not excluded — the line names ideas, and policy::retrieval_weight already demotes unreaffirmed exploratory-phase memories of every class in the ranking this selection inherits.
- Proven only at the door, in tests/context_injection.rs, which is #![cfg(unix)]. This predicate has no Windows-runnable test; the 930 and query-semantics mechanisms do, in tests/memory_search.rs.

---

### Worker-reported packet errors and gates (transcribed at closure)

**Packet errors the worker reported — read these BEFORE its results.**
Thirteen consecutive rounds a worker corrected its packet and was right:
- ACCEPTANCE TEST 1 asks for a reproduction to be written before the fix; Phase 27 already wrote it, and its doc comment instructs the next worker to INVERT it rather than add another. I inverted it and added the third case the recon flagged as missing (an unrelated sentence of common words asserting NO injection) — without that case a bare OR join passes the inverted test.
- docs/product/evidence/phase-27.md cites `context_injection::a_task_written_as_a_sentence_retrieves_nothing_because_the_search_ands_its_terms` as regression evidence for line 1126. That test is now `a_task_written_as_a_sentence_retrieves_the_memory_it_is_about` (the rename its own doc comment asked for). docs/product/evidence/** is FORBIDDEN, so the ledger edit and 1126's recorded scope limit are the orchestrator's to close.
- phase-27.md cites sanitize_query at search.rs:150-170; current source has it at 162-174. The recon flagged this and it is confirmed.
- The packet's open question offered two answers — bm25 alone, or a bounded frequency/length filter. Measured, NEITHER is sufficient: the noise is selected by LadderRung before relevance is consulted, so no cut on a score removes it. See §2 of the report.

Gates the worker ran (re-run the decisive ones yourself):
- cargo build: clean
- cargo fmt --all -- --check: clean
- cargo clippy --all-targets --all-features -- -D warnings: clean
- cargo doc --no-deps: clean
- cargo test --test context_injection: 13 passed
- cargo test --test memory_search: 23 passed
- cargo test --test memory_query_api: 9 passed
- cargo test --test project_isolation: 7 passed
- scripts/blast-radius.sh: every traced target passed (26 targets)
- scripts/check-doc-boundary.sh: clean
- Windows: NOT RUN — cargo check --target aarch64-pc-windows-msvc fails with std missing for the target, as in phase-27. src/memory/** diff has zero cfg, path, OS-string or line-ending constructs.

