# Capability evidence — phase 12-18-and-19

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phases 12, 18 and 19 — the event log, portable checkpoints, and the wiring that had no caller (25 of 28)

Delivered by the `lead-record` team lead (Claude Code, Opus 5, effort high)
with two Sonnet subcontractors. 13 mutations, all run by the lead: 12 killed,
**1 confirmed survivor reported and explained**, and — the valuable part —
**three that survived on their first run and exposed claims nothing was
testing.**

Contract: Given anything worth remembering about a session, Glasshouse writes
it to an append-only project-scoped log that cannot be updated or deleted; and
given a session worth continuing, it writes a small portable checkpoint that
can bootstrap a fresh session in a *different* harness.

State: COMPLETE for 25 lines. Three stay open, below.

Production evidence:
- `database.rs` migration 5 — `lifecycle_events` and `checkpoints`, with
  append-only triggers (an `UPDATE` or `DELETE` on a logged event raises).
- `events/log.rs` — `EventLogSink` behind `EventBus::attach_sink`. `record` is
  a `try_send` that **drops and counts** rather than blocking: the publishing
  thread is sometimes draining a pseudo-terminal.
- `checkpoint/store.rs`, `checkpoint/git.rs` — the portable format, its size
  bound, and `GitPosition::detect`, which reads `.git/HEAD` and its ref
  directly, handling linked worktrees and packed refs, **spawning no
  subprocess**.
- `main.rs::report_hook` now calls `session::lifecycle::observe`, which is what
  finally gave `RawObservation` a production caller.
- `shell/mod.rs` — the TUI subscribes to the bus, and the duplicated
  `Stopped`/`Failed` split is gone: `ProcessExit::session_state` is the single
  definition of "did it crash".

Regression evidence:
- 980 tests. 13 mutations in a private `CARGO_TARGET_DIR`, `touch` before every
  build, `cp` restore, `diff -q` proving the restore byte-identical.
- **The three first-survivors are the most useful result in this batch**, and
  each exposed an untested claim rather than a weak mutation:
  - replacing `observe()` with a bare translation killed nothing, because the
    *stored* observation comes from a different line. Only the **debug log**
    was lost — which is the box's actual subject — and nothing tested it. That
    box would have been claimed on a mechanism with no coverage at all.
  - `git: None` killed nothing, because the round-trip test built a
    `Checkpoint` literal with `git` already filled in: it proved the field
    survives storage and nothing about reading a repository.
  - the tail's `observed_harness IS NOT NULL` filter had no test; it is what
    stops the interface showing every in-process event twice.
  All three tests were then written and all three mutations re-run and killed.

Platform evidence:
- **No CI** (practice §27). `scripts/ci-local.sh` green on all ten checks —
  and this is the first batch gated by a version of that script that **can
  actually fail** (practice §31). **Windows unexercised.**
- Driven through the shipped binary, including a checkpoint written while one
  harness was running bootstrapping a session in another — which is the only
  form of evidence that means anything for Phase 19's cross-harness line.

Map/design conflict, decided by the orchestrator:
- *Preserve raw adapter event payloads in debug logs* — the lead built the
  mechanism, refused to log a payload document, and put the reading to me
  rather than choosing. **Closed.** "Payloads" cannot mean the payload
  document: that document carries the user's prompt and the model's last
  message, and a standing decision plus a test forbid it reaching any log.
  Read as "what the adapter reports, raw and untranslated" the line is
  satisfied, the mechanism now has a production caller, and
  `the_debug_log_preserves_the_raw_observation_and_none_of_the_payload` is
  mutation-proven in both directions.

Orchestrator work to land it:
- `tests/memory_store.rs` — the `MAX(version)` rollback trap for the **third**
  time, in a third file, and the lead reported exact replacement values rather
  than touching a file it did not own. It also found the trap's opposite face:
  dropping the migration rows *without* dropping migration 5's tables makes the
  re-run fail with `table lifecycle_events already exists`.

Missing evidence — the three open lines:
- *Deliver lifecycle events to the orchestration layer* — there is no
  orchestration layer; Phase 14 is entirely open. A delivery path with no
  consumer does not deliver, which is the rule that kept the TUI line open
  until this batch gave it one.
- *Record Git commit identifiers associated with memory events* — no memory
  event exists on the lifecycle stream, and `memories.source_commit` belongs to
  Phase 21's extractor. `checkpoint::git::GitPosition::detect` is the cheap
  resolver that line needs; Phase 21 should use it rather than shelling out.
- *Request a checkpoint automatically at selected task boundaries* — **the
  confirmed mutation survivor.** It works end to end (three `task_boundary`
  checkpoints observed from separate hook processes against a running shell),
  but nothing covers the shell's run loop, so the mutation that repointed the
  detector at the wrong event survived a full suite. The lead said it would not
  object to the box staying open. Taken at its word.
