# Capability evidence — phase 29

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 29 — memory commits, 7 of 8 (lines 1147–1154); 1152 open

Package `GH-MEMORY-COMMITS`, 2026-08-31, Opus at high. A *memory commit* is the existing extraction with its trigger named on every memory it produces: a person (`glasshouse memory commit`), a completed task, a Git commit landing (`sessions.last_seen_commit` compared with HEAD at every turn end — no Git hook), and the harness's pre-compaction event. **Migration 21** adds `sessions.last_seen_commit` and `memories.extraction_trigger`; the worker built it as 19 from a stale read of its peers and the orchestrator renumbered it at integration (19 assumption-guardrails, 20 cmux, 21 this). Nine mutations, nine killed. The worker also found and fixed a pre-existing red on its base — `memory_provenance.rs` and `memory_store.rs` pinning `version, 17` against a `SUPPORTED_SCHEMA_VERSION` of 18 — which `dfaf27f` had since fixed on main.

### Define a lightweight memory commit operation that extracts durable project knowledge from recently completed work. (line 1147)

Contract: Given a project with completed work, when a memory commit is triggered, Glasshouse runs the one extraction pipeline it already has over the recently completed work and records what survives its contract, while preserving that there is exactly one extractor, one credential screen and one duplicate check.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/main.rs` — `run_extraction`
- `src/memory/extract/mod.rs` — `ExtractionTrigger`
- `src/memory/extract/mod.rs` — `Extractor::run`

Regression evidence:
- `memory_commits::a_manual_commit_extracts_and_a_second_run_adds_nothing`
- `memory_commits::the_pre_compaction_event_triggers_extraction_and_the_post_event_does_not`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/extract/mod.rs: `.with_extraction_trigger(Some(trigger))` -> `.with_extraction_trigger(None::<String>)` | `skip-state-update` | **killed** | `memory_commits::a_manual_commit_extracts_and_a_second_run_adds_nothing` |

> skip-state-update observed: panicked at crates/glasshouse/tests/memory_commits.rs:433: assertion `left == right` failed: every trigger names itself on the memory it produced

Recorded scope limits — stated by the worker, not discovered later:
- This is a naming and a trigger set over Phase 21's pipeline. It does not prove the extraction itself is good, only that all four triggers reach the same one.


---


### Allow a memory commit to be triggered manually. (line 1148)

Contract: Given a session with recorded activity, when a person runs `glasshouse memory commit`, Glasshouse asks the configured extraction model and records the memories stamped `manual`, while preserving that failure never reaches the person's shell as an error.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/cli.rs` — `MemoryCommand::Commit`
- `src/main.rs` — `memory_commit`
- `src/main.rs` — `run_extraction`

Regression evidence:
- `memory_commits::a_manual_commit_extracts_and_a_second_run_adds_nothing`
- `memory_commits::a_commit_with_no_session_named_takes_the_most_recently_active_one`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/extract/mod.rs: `.with_extraction_trigger(Some(trigger))` -> `.with_extraction_trigger(None::<String>)` | `skip-state-update` | **killed** | `memory_commits::a_manual_commit_extracts_and_a_second_run_adds_nothing` |

> skip-state-update observed: panicked at crates/glasshouse/tests/memory_commits.rs:433: assertion `left == right` failed: every trigger names itself on the memory it produced

Recorded scope limits — stated by the worker, not discovered later:
- The default-session ordering is `last_activity_at DESC, id ASC`. At second resolution two sessions touched in the same second tie and the tiebreak is a random identifier; the test ages one session deliberately rather than pretending the ordering is finer than it is.


---


### Allow a memory commit to be triggered after a successful Git commit. (line 1149)

Contract: Given a session whose project is a Git repository, when a turn ends with HEAD at a commit this session has not seen, Glasshouse runs a memory commit triggered by that commit and stores the new position, while preserving that no Git hook is installed and no `git` subprocess is spawned.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/main.rs` — `note_head_commit`
- `src/main.rs` — `report_hook_with (TurnEnded arm)`
- `src/session/store.rs` — `SessionStore::record_seen_commit`
- `src/database.rs` — `MIGRATIONS[18] (sessions.last_seen_commit)`
- `src/memory/extract/mod.rs` — `ExtractionTrigger::GitCommit`

Regression evidence:
- `memory_commits::a_new_head_at_turn_end_is_a_code_change_boundary_and_the_commit_is_recorded_on_the_memory`
- `memory_commits::an_unchanged_head_triggers_only_the_task_completion_extraction`
- `database::tests::the_memory_commit_migration_adds_its_two_columns_and_undoes_cleanly`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/main.rs: `if previous == Some(position.commit.as_str()) {` -> `if previous != Some(position.commit.as_str()) {` | `invert-condition` | **killed** | `memory_commits::a_new_head_at_turn_end_is_a_code_change_boundary_and_the_commit_is_recorded_on_the_memory` |
| src/main.rs: `previous.is_some().then_some(position.commit)` -> `Some(position.commit)` | `widen-condition` | **killed** | `memory_commits::a_new_head_at_turn_end_is_a_code_change_boundary_and_the_commit_is_recorded_on_the_memory` |
| src/main.rs: `let landed = note_head_commit(runtime, &store, &id, record.last_seen_commit.as_deref());` -> `let landed: Option<String> = None;` | `delete-production-call` | **killed** | `memory_commits::a_new_head_at_turn_end_is_a_code_change_boundary_and_the_commit_is_recorded_on_the_memory` |

> invert-condition observed: panicked at crates/glasshouse/tests/memory_commits.rs:528: assertion `left == right` failed: the first completed turn records where HEAD stood

> widen-condition observed: panicked at crates/glasshouse/tests/memory_commits.rs:535: assertion `left == right` failed: a first sighting of HEAD is not a commit landing

> delete-production-call observed: panicked at crates/glasshouse/tests/memory_commits.rs:528: assertion `left == right` failed: the first completed turn records where HEAD stood

Recorded scope limits — stated by the worker, not discovered later:
- `GitPosition::detect` reads `.git/HEAD` and the ref it names. It does not notice a commit that landed and was then reset back to the same object within one turn, and it cannot see a commit at all in a project that is not a repository (asserted as the discriminating half).
- A commit that lands while no turn ends is not observed until the next `TurnEnded`. The boundary is the turn, not the commit's own instant.


---


### Allow a memory commit to be triggered after a task-completion event. (line 1150)

Contract: Given a harness reporting a completed turn, when no commit has landed since this session last looked, Glasshouse runs a memory commit triggered by the task completion, while preserving that the harness's own verdict is recorded first and never lost to work Glasshouse chose to do about it.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/main.rs` — `report_hook_with (TurnEnded arm, `(None, true)`)`
- `src/main.rs` — `run_extraction`

Regression evidence:
- `memory_commits::an_unchanged_head_triggers_only_the_task_completion_extraction`
- `memory_extract_triggers::a_completed_task_asks_the_configured_model_and_stores_what_it_answered`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/main.rs: `match (landed, completed) {` -> `match (landed, false) {` (the `(None, true)` arm holding the TaskCompleted call becomes unreachable; §35 mutates the call, not the callee) | `delete-production-call` | **killed** | `memory_commits::an_unchanged_head_triggers_only_the_task_completion_extraction` |

> delete-production-call observed: panicked at crates/glasshouse/tests/memory_commits.rs:599 and memory_commits.rs:534; both tests FAILED in one run

Recorded scope limits — stated by the worker, not discovered later:
- The trigger predates this package (batch 51). What is new is the §35 mutation proving the production call and the trigger stamped on the memory; the wiring itself was already there.
- On a completed turn that also landed a commit, the recorded trigger is `git_commit`, not `task_completed` — one boundary, the more specific description. A reader counting `task_completed` rows is counting turns with no commit, not turns.


---


### Allow a memory commit to be triggered before an intentional native prompt compaction. (line 1151)

Contract: Given a harness that reports it is about to compact its own context, when that event arrives, Glasshouse runs a memory commit triggered by the imminent compaction, while preserving that the post-compaction event triggers nothing and that no `lifecycle_events` row is written for either.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/session/lifecycle.rs` — `precedes_native_compaction`
- `src/main.rs` — `report_hook_with (unrecognised-event arm)`
- `src/harness/codex.rs` — `REPORTED_EVENTS`

Regression evidence:
- `memory_commits::the_pre_compaction_event_triggers_extraction_and_the_post_event_does_not`
- `memory_extract_triggers::a_harness_about_to_compact_runs_extraction_and_records_no_lifecycle_event`
- `memory_extract_triggers::an_unrecognised_event_that_is_not_a_compaction_asks_no_model`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/session/lifecycle.rs: `matches!(event, "PreCompact")` -> `matches!(event, "PreCompact" | "PostCompact")` | `widen-condition` | **killed** | `memory_commits::the_pre_compaction_event_triggers_extraction_and_the_post_event_does_not` |

> widen-condition observed: panicked at crates/glasshouse/tests/memory_commits.rs:683: assertion `left == right` failed: the post-compaction event must run nothing

Recorded scope limits — stated by the worker, not discovered later:
- CODEX ONLY. `grep -n 'ompact'` over harness/{claude_code,cursor,opencode,antigravity,hermes,pi}.rs finds nothing — six of seven adapters report no compaction event of any kind, so for those harnesses this line is open on the harness, not on Glasshouse. If the box requires all seven, it stays open.


---


### Separate durable project memories from transient session checkpoints during a memory commit. (line 1152)

Contract: Given a project holding both, when a memory commit runs, Glasshouse writes to the memory store and not to the checkpoint store, and when a checkpoint is taken it writes to the checkpoint store and not to the memory store, while preserving that each remains reachable through its own command.

State: NOT STARTED — worker reports the line still open

Production evidence:
- `src/memory/store.rs` — `MemoryStore::record`
- `src/checkpoint/store.rs` — `CheckpointStore::save`
- `src/main.rs` — `memory_commit`

Regression evidence:
- `memory_commits::memories_and_checkpoints_never_write_into_each_other`

Recorded scope limits — stated by the worker, not discovered later:
- NO MUTATION, and this is the weakest line in the package. The claim is an absence — 'no row appeared in the other table' — and a mutation that made a memory commit write a checkpoint would be inventing a feature rather than removing one. The test counts rows in both tables in both directions through the built binary, which is evidence, but it is not mutation-proven and should not be read as if it were.
- It proves the two stores do not cross-write. It does not prove their lifetimes differ; that is a property of the pruning code, which this package did not touch.


---


### Record the relevant Git commit with memories produced from a code-change boundary. (line 1153)

Contract: Given a memory commit triggered by a landed commit, when memories are stored, each carries that commit as its source commit, while preserving that a memory from any other trigger records no commit it did not learn from.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/main.rs` — `run_extraction (chunk_for_session(.., trigger.commit(), ..))`
- `src/memory/extract/mod.rs` — `ExtractionTrigger::commit`
- `src/memory/extract/mod.rs` — `Extractor::store_one (with_source_commit)`
- `src/database.rs` — `MIGRATIONS[18] (memories.extraction_trigger)`

Regression evidence:
- `memory_commits::a_new_head_at_turn_end_is_a_code_change_boundary_and_the_commit_is_recorded_on_the_memory`
- `memory_commits::an_unchanged_head_triggers_only_the_task_completion_extraction`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/main.rs: `chunk_for_session(id, &events, trigger.commit(), ChunkLimits::default())` -> `chunk_for_session(id, &events, None, ChunkLimits::default())` | `drop-carried-value` | **killed** | `memory_commits::a_new_head_at_turn_end_is_a_code_change_boundary_and_the_commit_is_recorded_on_the_memory` |

> drop-carried-value observed: panicked at crates/glasshouse/tests/memory_commits.rs:560: assertion `left == right` failed: line 1153: the memory carries the commit that made the boundary, not the one before it

Recorded scope limits — stated by the worker, not discovered later:
- The commit stored is the trigger's own payload, read once when the boundary was detected — never a second reading taken during the run. That is deliberate; it also means a commit landing *during* extraction is not what gets recorded.
- `memories.source_commit` alone cannot say a memory came from a boundary: `glasshouse memory extract` fills it too. `memories.extraction_trigger` is what distinguishes them, which is why both columns exist.


---


### Make memory commits idempotent enough that rerunning one does not create uncontrolled duplicate knowledge. (line 1154)

Contract: Given a memory commit already run over a session's work, when the same commit is run again over the same work and the model answers the same thing, Glasshouse stores nothing new, while preserving that a genuinely different memory from the same session is still stored.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts.

Production evidence:
- `src/memory/extract/mod.rs` — `Extractor::store_one (seen.contains)`
- `src/memory/extract/mod.rs` — `Extractor::existing_bodies`
- `src/memory/extract/mod.rs` — `normalize`

Regression evidence:
- `memory_commits::a_manual_commit_extracts_and_a_second_run_adds_nothing`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/extract/mod.rs: `if seen.contains(&key) {` -> `if false {` | `disable-guard` | **killed** | `memory_commits::a_manual_commit_extracts_and_a_second_run_adds_nothing` |

> disable-guard observed: panicked at crates/glasshouse/tests/memory_commits.rs:442: assertion failed on `second.contains("stored 0, 1 duplicate")`

Recorded scope limits — stated by the worker, not discovered later:
- The dedupe needed no fix — it already held. The test asserts the STORE does not grow (`SELECT COUNT(*) FROM memories`), and separately asserts the model really was asked twice, so the count is not an artefact of nothing having happened.
- Normalized equality only: case, whitespace runs and trailing punctuation. Two genuinely different sentences saying the same thing are two memories, deliberately — see `normalize`'s own doc.
- 'Uncontrolled' is the map's word and the floor is what is proven. A model that paraphrases itself on every run will still accumulate near-duplicates.

