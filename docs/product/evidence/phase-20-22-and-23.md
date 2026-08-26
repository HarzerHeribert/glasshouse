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
