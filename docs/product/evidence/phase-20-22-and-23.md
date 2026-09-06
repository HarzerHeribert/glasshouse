# Capability evidence — phase 20-22-and-23

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phases 20, 22 and 23 — durable project memory, its lifecycle, and FTS5 search (31 of 34)

Delivered by the `lead-memory` team lead (Claude Code, Opus 5, effort high)
with two Sonnet subcontractors. 30 mutations, every one run by the lead;
**29 killed and one deliberate survivor, reported rather than hidden.**

Contract: Given work worth remembering, Glasshouse stores it in this project's
own SQLite database under one of six kinds and one of the lifecycle statuses,
lets a newer memory retire an older one without deleting it, and finds it again
by free text — never returning history unless history was asked for, and never
reaching another project's database.

State: COMPLETE for 31 lines. Three stay open, and none of the three is a gap
in this work.

Production evidence:
- `database.rs` migration 4 — `memories`, three indexes, the FTS5 index, and
  the same pair of project-isolation triggers migration 2 established for
  `sessions`.
- `memory/store.rs` — `ProjectMemory`, `MemoryStore`, the six kinds, the
  statuses, supersession that records its successor and refuses to name a
  memory that does not exist.
- `memory/policy.rs` — `admit()` refuses raw conversational filler and refuses
  to keep a step-by-step plan as a todo.
- `memory/search.rs` — BM25-ranked FTS5 search, FTS5 operator characters
  sanitized, `SearchScope::Current` by default so history is an explicit ask.
- `memory/snapshot.rs` — a bounded project snapshot whose `omitted` count is
  never silent.
- `cli.rs`/`main.rs` — `glasshouse memory search <query> [--history] [--limit N]`,
  added by the orchestrator (see below).

Regression evidence:
- 870 lib tests and four new integration suites, 0 failures.
- 30 mutations in a private `CARGO_TARGET_DIR` with `touch` before every
  build, `cp` restore, `diff -q` verified, each killing a named test in the
  target that runs it. None reported `could not compile`.
- **The survivor is the useful entry.** Removing `search`'s `project_id`
  filter killed no test — because isolation is structural (one database per
  project, plus triggers), so the filter is redundant defence in depth. The
  lead reported the survival rather than deleting the filter or inventing a
  test that would have proved nothing. A surviving mutation that is explained
  is worth more than a suppressed one.

Platform evidence:
- **No CI** (practice §27). `scripts/ci-local.sh` green on all ten checks —
  five of seven CI jobs. **Windows unexercised.**

Orchestrator work required to land this:
- **The CLI surface.** The lead built `search`, `get` and `snapshot` and could
  reach none of them: `main.rs`'s match over `&cli.command` has no `_` arm, so
  adding a variant to `cli.rs` alone does not compile, and both files were
  forbidden to it. It wrote the patch into its report instead of guessing, and
  asked whether the command should be flat or a subcommand. **Phase 48 answers
  that** — it names `glasshouse memory search <query>` — so it is a
  subcommand. Run against the real binary, not just tested.
- **Four tests in `session/store.rs` reconciled with migration 4.** Three were
  pinned constants. The fourth was not:

  **A real find, and not the one anybody predicted.** Both rollback tests
  simulated an older database by deleting *some* rows from
  `schema_migrations` — `= 3` in one, `IN (2, 3)` in the other. The runner
  resumes from `MAX(version)`, so once migration 4 existed those deletions
  left a **hole**: max was still 4, nothing re-applied, and the tests failed
  much later with `no such column: launch_profile` and `no such table:
  sessions`. Both now roll back a contiguous range and drop what migration 4
  added. The lead had predicted "change 3 to 4" for these two and stopped at
  the version assertion; the second failure only appears once the first is
  fixed. Re-running a worker's decisive observations found it (practice §23).

Missing evidence — the three open lines, and why each is right to be open:
- *Do not store obvious source-code facts* and *prefer storing information
  whose rediscovery would require significant exploration* — not decidable at
  the storage layer. Whether a statement is an obvious source fact is a
  judgment about the project that only the producer can make. A keyword
  heuristic would refuse real memories, admit fake ones, and produce a test
  that passed for the wrong reason. These belong to Phase 21's extractor and
  its evaluation.
- *Avoid returning mutually contradictory current memories without flagging
  the conflict* — half built, and the honest half is the flagging: once two
  memories are marked conflicted neither is returned as current, and a
  mutation proves the test notices if only one side is flagged. Nothing
  **detects** a contradiction yet, so no test can show Glasshouse avoids
  returning an *undetected* one, which is what the sentence promises. Phase
  21E's decision ladder is where the detector belongs.

Phase 26 remains entirely open, deliberately. Every one of its six lines says
**agent**, the operations exist only as a Rust API and a person-facing CLI, and
a property proven of a Rust API is not a property proven of a tool surface that
does not exist. It closes with Phase 43's MCP surface.

Known limit, recorded rather than fixed:
- `the_project_database_schema_has_nowhere_to_put_a_credential` pins the whole
  schema and asks each new column be confirmed unable to hold a secret. The
  lead **refused to certify that** for `memories.subject` and `memories.body`,
  which are free text, and it was right to. The test's own documentation now
  says what it can and cannot prove: it proves no column exists whose *purpose*
  is a credential and that adding one is a deliberate, recorded act. It cannot
  prove free text is clean. That control belongs to the producer, and is now
  written down as an explicit acceptance condition of Phase 21's extractor
  rather than inherited by assumption.

### Phase 22 line 1063 — contradictory current memories are flagged, and `mark_conflicted` gets its first caller

Contract: Given two current memories that contradict each other, when a
retrieval returns both, Glasshouse marks them `conflicted` and stops presenting
either as a current instruction — while preserving both as history rather than
choosing a winner.

State: **COMPLETE**.

Production evidence:
- `memory/search.rs: MemoryStore::search` → `flag_contradictions` → the store's
  pre-existing `mark_conflicted`. **`mark_conflicted` had zero production
  callers before this** — re-grepped; only tests reached it. So this line closes
  two things: the detector, and the first real caller of a method that had been
  shipped and unreachable.
- Detection is over the memories the call **just matched**, not the whole
  project. Phase 22 asks that a conflict be flagged, not that every retrieval do
  an O(n²) scan of every memory in the database.

**The map's vocabulary had to be reinterpreted, and this is the decision to
re-examine if the box is ever disputed.** The map says "same subject, opposite
disposition". `disposition` is a field `memory::extract::schema` reads from a
model's reply during extraction — and **the `memories` table has no
`disposition` column**; nothing persists it. So `contradicts()` uses the closest
stored proxy: same normalised subject (trimmed, lowercased), with one memory's
`kind` being `Decision`/`Constraint` (the stored equivalent of *adopted*) and
the other's `FailedAttempt` (the stored equivalent of *abandoned*). That is the
same distinction `memory::extract::authority`'s disposition ceilings already
encode on the way in, expressed in the vocabulary the table actually keeps.

Regression evidence:
- `tests/memory_validity.rs::needs_review_and_conflicted_memories_stay_out_of_current_search_but_are_findable_as_history`
  — a `Decision` and a `FailedAttempt` sharing a subject (differing only in
  case) are both moved to `Conflicted` by one `search()` call, then excluded
  from `Current` and found under `Historical`.
- `tests/memory_validity.rs::unrelated_or_agreeing_memories_are_never_flagged_as_conflicted`
  — **the conservative half, and the one that matters.** Two agreeing
  `Decision`s sharing a subject, and a `Decision`/`FailedAttempt` pair with
  *different* subjects, are never flagged.
- `tests/memory_store.rs::contradictory_current_memories_are_flagged_and_leave_normal_retrieval`
  — pre-existing, exercises the manual `mark_conflicted` path, untouched. The
  manual and automatic paths coexist.

Failure/isolation evidence:
- Mutation: deleting `self.flag_contradictions(&mut scored)?;` from `search()`
  killed the first test above (`left: Active, right: Conflicted`).

### Phase 20 lines 828 and 829 — closed 2026-09-06 on the rule the extraction prompt carries, with its limit stated

State: **COMPLETE** — `GH-PROVE-IT-BATCH-2` (Sonnet, Green, tests only; report `.agent-runtime/report-prove-it-batch-2.md`), ruled by the primary on 2026-09-06 (`refusal-register.md`, *Rulings on the census's sixteen disputed rows*). The mechanism Glasshouse has for *what to store* is the extraction model's judgement under `memory::extract::schema::PROMPT_CONTRACT`, whose one sentence carries both lines: *"Emit only what a future agent working on this project would need and could not cheaply rediscover by reading the code"* — the negative clause is 828, the positive clause is 829. Tests: `memory_extract_schema::the_recorded_prompt_carries_the_contract_the_schema_and_the_activity` (extended: the sentence is asserted on the prompt the model actually received through `Extractor::run`); `memory_extract_schema::a_rediscoverable_source_fact_the_prompt_asks_to_omit_is_still_accepted_by_the_validator` (a well-formed memory that is an obvious source fact is stored, not refused). **Limit, and it is the whole point:** the rule is the model's to follow; Glasshouse pins that it is asked, never that it is obeyed — the keyword heuristic that would "enforce" it was refused because it refuses real memories and admits fake ones (the register's standing ruling, unchanged). The section below is the record of that refusal and stays as written.

State before 2026-09-06: NOT STARTED, blocked on a judgement the storage layer cannot make.

*"Do not store obvious source-code facts when rereading the source is cheaper"*
and *"prefer storing information whose rediscovery would require significant
exploration"* are stated in `memory::extract::schema`'s `PROMPT_CONTRACT` and
are **asked for, never validated**. That module's own "what is enforced" table
lists exactly three checkable fields — `support`, `disposition`, `confidence` —
and these two are not among them.

**The worker was asked to close them and declined, correctly.** Deciding whether
a claim is "an obvious source-code fact" is a judgement about the project that
only the producer can make; a keyword heuristic would refuse real memories and
admit fake ones. That is the same limit `memory::policy` already declined to
fake at the storage layer. Recorded here so the next package does not re-derive
it.
