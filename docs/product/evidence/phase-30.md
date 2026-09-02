# Phase 30 — Session context metadata, 7 of 8 closed, 1 refused

Capability map lines 1158–1165. Package `GH-SESSION-CONTEXT`, worktree
`.worktrees/session-context`; report in `.agent-runtime/report-session-context.md`.
Integrated 2026-08-29 with `GH-WORKLOAD-TIERS` in one `integrate.sh` run.
**Migration 16.**

## UN-TICKED 2026-08-30 — 1161–1165 REFUTED by `GH-PHASE30-AUDIT`

**All five boxes were ticked over a mechanism nothing in the shipped binary
runs.** `SessionStore::context` (`session/store.rs:2020`) is the sole
construction site of every value these lines ask for, and it has **zero
production callers**: all fourteen are in `tests/session_context.rs`.
`SessionContext` appears in `crates/glasshouse/src/` only at its definition
(`:802`), that constructor (`:2060`), a doc comment (`:1999`), the re-export
(`session/mod.rs:61`) and one unrelated comment (`database.rs:1633`).
Independently confirmed by the orchestrator before un-ticking, along with two
sharper facts: `session_detail` (`main.rs:5795-5857`) renders eighteen fields
and never calls `store.context`, and `CheckpointRecency::is_current`
(`store.rs:727`) — the method whose own doc comment calls itself *"Line 1164's
question in one word"* — **has no caller anywhere in the repository, not even a
test.**

**The decisive argument, of the two the audit was required to make.** A
defensible reading says *track* means *persist the inputs*, and on that reading
1161 is honest because `last_activity_at` is durably written. It fails on one
point: **that is line 1160's production evidence, not 1161's.** 1160 is *"Track
the most recent request or turn time"* and is ticked on exactly that write. If
persisting `last_activity_at` also closes 1161, the two lines are one line — and
this entry's own per-line evidence shows they were not believed to be, since it
names `AdvisoryCacheState::estimate` for 1161 and
`SessionStore::write_lifecycle_locked` for 1160. The reading also proves too
much: any pure function over a persisted column would close any box phrased
"track X", with no wiring at all.

For 1164 and 1165 the inputs genuinely are written in production
(`checkpoints.created_at`; `turn_ended` rows) — but both lines ask for a
**derived verdict**, and a timestamp is not a *"whether"* and a table of rows is
not a *"flag"*. The derivation is the requirement and the derivation never runs.
For 1162 and 1163 the reading does not apply at all: they constrain a
representation and a treatment, and both need a value to exist first.

**The review this entry itself asked for was never done.** Its REVIEW section
says, for each of these five: *"verdict `closed`. Re-run one decisive mutation
yourself, then rule (§79: a worker's packet does not bind the integrator)."*
That step is where this would have been caught, and it was skipped.

**And the test that would have caught it was in the same report.** This
package's own `packet_errors` correctly found that `SessionStore::touch` has no
production caller — *"all four call sites are tests … `touch` is a Cluster B
candidate"* — applying exactly the right test to code it inherited, and never
turning it on the function it was shipping. No reviewer turned it there either,
because the reasoning read as diligence.

**The repair is small and is not a redesign: ≈30 lines across two files.** One
`store.context(&id)` call in `session_detail` plus four render lines, and
`Display` impls for `CheckpointRecency` and `TaskContinuity` (`AdvisoryCacheState`
already has one at `:689`, and it prints `"hot (estimated)"`, which carries 1163
into the output by itself). No new type, no schema change, no migration. The
regression test belongs in `tests/session_model.rs`, which already drives the
real binary and has a `field(&report, label)` helper for
`glasshouse sessions show` — deleting the `store.context` call must fail it,
which is the §35 shape: the mutation lands on the **call**.

**Caution for that package:** production prompt-cache reasoning already exists
under a *different* vocabulary — `routing::session::prompt_cache_state`
(`routing/session.rs:800`, called at `:1459`, reached from `main.rs:1354`) speaks
`Preserved`/`Lost`/`LikelyLost` about a comparison *between two backends*, for
lines 1596/1597. The repair must not introduce a second user-visible cache
vocabulary that disagrees with it.

The code these lines describe is careful, well-reasoned, and correct in what it
computes. It is only not run. **SCAFFOLDED** is the honest state:
*"supporting code exists, but production behavior is absent or unproven."*

## RE-CLOSED 2026-08-30, same day — the repair was one call

`GH-SESSION-CONTEXT-DOOR`. The five lines were un-ticked hours earlier because
`SessionStore::context` had fourteen callers and all fourteen were tests. **The
code was never wrong; it was never run.** It runs now.

`session_detail` calls `store.context(&id)` and renders four labelled lines
alongside the eighteen it already printed. `CheckpointRecency` and
`TaskContinuity` gained `Display`; `AdvisoryCacheState` already had one printing
`"{} (estimated)"`, and that word is what carries **1163** to a user. No new
type, no schema change, no migration — 34 lines in `main.rs` and 32 in
`store.rs`.

**The regression evidence is the part that matters, because it is what was
wrong before.** The tests live in `tests/session_model.rs`, which drives the
**real binary** through `glasshouse sessions show` and reads values back with
its `field(&report, label)` helper. The old tests entered at
`store.context(&id)` — the seam nothing in the shipped binary reached — which
is exactly why they passed while the product did nothing.

**The characteristic mutation is the one that proves the repair:** delete the
`store.context(&id)` call from `session_detail`. It is **KILLED** by
`session_show_reports_cache_checkpoint_and_task_continuity_honestly` and
`an_unlaunched_sessions_context_reads_as_absence_not_a_guess`, observing *"the
cache line must say it is an estimate (line 1163), got `-`"*. Under the old
arrangement that deletion changed nothing any test could see. It lands on the
**call** now (§35).

A second mutation renders `CheckpointRecency::Never` as `Stale`, and dies on
*"no checkpoint exists, so this must read `never` — not `stale` and not a
date."*

Absent values render `-`, matching the eighteen lines above them: a session
with no context row reads as absence, never as a fabricated `0` or `hot`.

**The other cache vocabulary was left alone.**
`routing::session::prompt_cache_state` speaks `Preserved`/`Lost`/`LikelyLost`
about a comparison between two backends, for lines 1596/1597. It is a different
quantity and was neither renamed nor unified.

## The ruling that makes this entry short

The packet asked for migration 16 and left the per-line shape open. The worker
read the schema first and concluded that **Phase 30 needed exactly one new
column**, because seven of its eight facts were already durable or had no
producer at all. I verified that against source before accepting it:

| line | fact | where it lives |
|---|---|---|
| 1158 | estimated context size | **nowhere — refused, see below** |
| 1159 | observed compactions | **`sessions.observed_compactions`, migration 16's one column** (`database.rs:1667`) |
| 1160 | most recent request or turn time | `sessions.last_activity_at` |
| 1161–1163 | prompt-cache state | derived; a stored copy is stale the moment after it is written |
| 1164 | recent portable checkpoint | `checkpoints.created_at` |
| 1165 | task-continuity flag | `lifecycle_events` `turn_ended` rows |

Adding a column for any of the last four would have been a **second source of
truth for a fact this schema already holds exactly once** — migration 15's own
recorded objection to copying a token count out of `routing_observations`.
A stored `prompt_cache_state` is worse: its only input is elapsed time, so it
is wrong the minute after it is written.

The deliverable is therefore **one column plus one reader**:
`SessionStore::context(&SessionId) -> Option<SessionContext>`
(`session/store.rs:802`, `:2020`), which answers all seven together so that no
caller re-derives that "recent checkpoint" is a comparison against
`last_activity_at`, or that a cache estimate must never be a function of
resumability.

## 1158 — refused, and the source says why

The line's own condition is *"when the harness exposes enough information"*.
Both channels that could carry a context size are empty, and I confirmed both:

- **The hook** is the only way a harness reports anything. Its payload is
  drained into `io::sink()` **unread** — `main.rs:2539`,
  `std::io::copy(&mut std::io::stdin(), &mut std::io::sink())` — with the
  comment at `main.rs:2569` stating that nothing downstream can see it, and a
  test at `main.rs:5482` pinning that behaviour.
- **The gateway** owns the only token counts in this schema
  (`routing_observations`, migration 11), and `routing::evidence` documents
  them as *not supplied*, because filling them means parsing a response body
  `gateway::ingress` is forbidden to parse.

Recording this as a refusal rather than an estimate is the point. A fabricated
context size would have been read as telemetry by every future router.

## A pre-existing defect found on the way

`session/store.rs`'s rollback constant was named `UNDO_MIGRATION_FOURTEEN`
while already undoing migration 15 as well — **wrong by two migrations before
this package touched it**. Renamed to `UNDO_MIGRATIONS_ABOVE_THIRTEEN` to match
its twin in `database.rs`. This is exactly the class of drift that made
migration 15 break rollback fixtures in three files.

## Scope beyond EXPECTED FILES, and why it was owed

The package touched `events/mod.rs`, `session/mod.rs`, `session/recovery.rs`,
`shell/state.rs`, `shell/view.rs` and four test files. Every one is a rollback
fixture or a schema-version assertion that a bump to 16 invalidates — which
**ACCEPTANCE TEST 2 explicitly required finding all of**. This is the ripple
migration 15 paid for by discovering it three different ways; here it was
found once, up front.

## Verification note (§85)

The combined tree's blast radius reported one FAILURE:
`session::api::tests::interrupting_through_the_api_is_recorded_as_machine_initiated`,
a live-PTY test with a 45-second deadline, run while an Opus worker was
compiling (load average 11.53). **Re-run alone it passes in 0.32 s.** Recorded
because a load-induced FAIL that is not re-run is how a real defect gets
dismissed later by the same reflex.

---

### Track an estimated context-size value for a session when the harness exposes enough information. (line 1158)

Contract: Given a session, when a caller asks for its estimated context size, Glasshouse answers nothing, because no harness adapter and no gateway producer supplies a token count or a context reading at all.

State: NOT STARTED — worker refused the line; see its reason

Production evidence:
- `src/session/store.rs` — `SessionContext (the field is deliberately absent; the type doc carries the refusal)`

Regression evidence:
- `session_context::a_context_size_has_no_producer_in_this_build`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| no production behaviour was added for this line, so there is nothing to mutate | `none-applicable` | not run | `` |

Recorded scope limits — stated by the worker, not discovered later:
- This refuses the line for THIS build. A harness that later reports a token count, or a body-aware gateway layer, would make it packageable.
- The proof is an absence: the test shows the token columns and the event rows are empty after a full turn plus a compaction. It cannot prove no future producer exists.

---

### Track the number of observed compactions for a session when known. (line 1159)

Contract: Given a harness that says it is about to compact its own context, when the hook reports it, Glasshouse increments a durable per-session counter, while preserving the event log's eleven-value `kind` vocabulary and writing no lifecycle_events row.

State: **COMPLETE**

Production evidence:
- `src/database.rs` — `MIGRATIONS (migration 16, sessions.observed_compactions)`
- `src/session/store.rs` — `SessionStore::record_observed_compaction`
- `src/main.rs` — `report_hook_with (the precedes_native_compaction arm)`

Regression evidence:
- `session_context::an_observed_compaction_is_counted_by_the_shipped_binary_and_writes_no_event`
- `session_context::a_schema_fifteen_database_migrates_forward_and_its_sessions_read_as_uncounted`
- `session_context::a_session_this_build_starts_counts_from_a_measured_zero`
- `session_context::a_session_recorded_before_the_migration_starts_counting_at_its_first_compaction`
- `session_context::the_compaction_count_survives_a_write_a_read_and_a_reopen`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `ADD COLUMN observed_compactions INTEGER` -> `ADD COLUMN observed_compactions INTEGER NOT NULL DEFAULT 0` | `migration-defaults-unknown-to-zero` | **killed** | `session_context::a_schema_fifteen_database_migrates_forward_and_its_sessions_read_as_uncounted` |
| `if let Err(err) = store.record_observed_compaction(&id) {` -> `if let Err(err) = store.get(&id) {` | `delete-the-production-call` | **killed** | `session_context::an_observed_compaction_is_counted_by_the_shipped_binary_and_writes_no_event` |

> migration-defaults-unknown-to-zero observed: assertion `left == right` failed: a session recorded before migration 16 must read as `nobody was counting`, never as a measured zero

> delete-the-production-call observed: assertion `left == right` failed: the one production site that sees a compaction must count it

Recorded scope limits — stated by the worker, not discovered later:
- For a row recorded before migration 16 the count is a LOWER BOUND: compactions before the upgrade were observed by nothing and are unrecoverable. For a row this build created it is exact.
- It counts what a harness ANNOUNCES. A harness that compacts without a PreCompact event is invisible here, and Claude Code's observed catalogue has no compaction event at all.

---

### Track the most recent request or turn time for a session. (line 1160)

Contract: Given a session, when a harness reports a prompt or a turn ending, Glasshouse records the time on the session's existing last_activity_at, while preserving the single-UPDATE rule that no second writer moves a session's lifecycle.

State: **COMPLETE**

Production evidence:
- `src/session/store.rs` — `SessionStore::write_lifecycle_locked (already stamped it)`
- `src/session/store.rs` — `SessionContext::last_activity_at`
- `src/main.rs` — `report_hook_with (calls set_lifecycle on every translated event)`

Regression evidence:
- `session_context::a_request_and_a_turn_ending_each_move_the_existing_activity_stamp`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `last_activity_at = CASE WHEN ?4 THEN ?3 ELSE last_activity_at END` -> `CASE WHEN ?4 THEN last_activity_at ELSE ?3 END`, inverting which changes stamp | `a-lifecycle-change-no-longer-counts-as-activity` | **killed** | `session_context::a_request_and_a_turn_ending_each_move_the_existing_activity_stamp` |

> a-lifecycle-change-no-longer-counts-as-activity observed: the request assertion failed with the stamp still at the session's creation time, and a_checkpoint_written_after_the_last_activity_is_current_and_an_older_one_is_not failed beside it because nothing overtook the checkpoint any more

Recorded scope limits — stated by the worker, not discovered later:
- `may_apply` requires the state to CHANGE, so two consecutive UserPromptSubmit events with no intervening Stop move the stamp once. A Stop always intervenes in real traffic.
- Whole seconds. Two events inside one second are indistinguishable here, as everywhere else in this schema.

---

### Track an estimated prompt-cache state independently from session resumability. (line 1161)

Contract: Given a session, when Glasshouse estimates its prompt-cache state, the estimate is a function of elapsed time alone, while preserving complete independence from whether the session can be resumed.

State: **COMPLETE**

Production evidence:
- `src/session/store.rs` — `AdvisoryCacheState::estimate`
- `src/session/store.rs` — `SessionStore::context`

Regression evidence:
- `session_context::a_prompt_cache_estimate_is_independent_of_whether_a_session_can_be_resumed`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `if idle_seconds < 0 {` -> `if false {` | `ignore-a-backwards-clock` | **killed** | `session_context::every_prompt_cache_state_the_map_requires_is_reachable` |

> ignore-a-backwards-clock observed: assertion `left == right` failed, at the Unknown case: a clock that stepped backwards was reported as Hot

Recorded scope limits — stated by the worker, not discovered later:
- The independence is structural (the inputs do not overlap) and the test proves the two answers disagree in both directions. It does not prove the estimate is ACCURATE, which nothing in this build could.
- A backend change also loses a provider cache (CacheLocality). This per-session estimate does not model that; it is one session's decay, not a comparison.

---

### Represent prompt-cache state as at least hot, warm, cold, or unknown. (line 1162)

Contract: Given a prompt-cache estimate, when it is represented, Glasshouse offers hot, warm, cold and unknown as distinct states, while preserving unknown as a real answer rather than a stand-in for cold.

State: **COMPLETE**

Production evidence:
- `src/session/store.rs` — `CacheState`
- `src/session/store.rs` — `AdvisoryCacheState::estimate`
- `src/session/store.rs` — `AdvisoryCacheState::unknown`

Regression evidence:
- `session_context::every_prompt_cache_state_the_map_requires_is_reachable`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `if idle_seconds < 0 {` -> `if false {` | `ignore-a-backwards-clock` | **killed** | `session_context::every_prompt_cache_state_the_map_requires_is_reachable` |

> ignore-a-backwards-clock observed: the Unknown state became unreachable and the assertion for it failed

Recorded scope limits — stated by the worker, not discovered later:
- The three time thresholds are reasoning from published TTLs, not measurement. The measurement that would change them is a provider that reports a cache hit; none does.

---

### Treat cache-state estimates as advisory when the provider does not expose authoritative cache telemetry. (line 1163)

Contract: Given that no provider exposes authoritative cache telemetry, when Glasshouse produces a cache-state estimate, the value is advisory in its type, while preserving the impossibility of any caller asserting a cache state it did not estimate.

State: **COMPLETE**

Production evidence:
- `src/session/store.rs` — `AdvisoryCacheState (private field; estimate and unknown are the only constructors; no From<CacheState>)`
- `src/session/store.rs` — `impl Display for AdvisoryCacheState`

Regression evidence:
- `session_context::a_cache_estimate_says_it_is_an_estimate_when_it_is_printed`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `write!(f, "{} (estimated)", self.0)` -> `write!(f, "{}", self.0)` | `a-rendered-estimate-does-not-say-it-is-one` | **killed** | `session_context::a_cache_estimate_says_it_is_an_estimate_when_it_is_printed` |

> a-rendered-estimate-does-not-say-it-is-one observed: assertion `left == right` failed: the rendered value was `hot` where the contract requires `hot (estimated)`

Recorded scope limits — stated by the worker, not discovered later:
- The structural half is proven by the COMPILER, not by the suite. If a future edit made the field public or added From<CacheState>, no test here would notice. A trybuild compile-fail case would close that; this crate has no trybuild.

---

### Track whether a session has a recent portable checkpoint. (line 1164)

Contract: Given a session, when a caller asks whether it has a recent portable checkpoint, Glasshouse answers from the newest stored checkpoint's time against the session's own last activity, while preserving the checkpoint document as the single source of what a checkpoint contains.

State: **COMPLETE**

Production evidence:
- `src/session/store.rs` — `CheckpointRecency`
- `src/session/store.rs` — `SessionStore::context`

Regression evidence:
- `session_context::a_checkpoint_written_after_the_last_activity_is_current_and_an_older_one_is_not`
- `session_context::a_checkpoint_written_in_the_same_second_as_the_last_activity_is_current`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `Some(at) if at >= record.last_activity_at` -> `Some(at) if at > record.last_activity_at` | `tie-goes-to-the-activity` | **killed** | `session_context::a_checkpoint_written_in_the_same_second_as_the_last_activity_is_current` |

> tie-goes-to-the-activity observed: assertion `left == right` failed: within one second the checkpoint is at least as new as the activity, and reporting it stale would cost a user a checkpoint they have

Recorded scope limits — stated by the worker, not discovered later:
- No timestamp is copied onto `sessions`: this reads `checkpoints` each time, so nothing can drift, and equally nothing is indexed for it. Fine at the row counts a session accumulates; not a claim about a table with millions.
- 'Recent' means 'not overtaken by recorded activity'. A session whose harness reports no events never overtakes its checkpoint, so its checkpoint stays Current however old it is.

---

### Track a lightweight task-continuity score or flag describing whether the session is still working on the same task. (line 1165)

Contract: Given a session, when a caller asks whether it is still working on the same task, Glasshouse answers with a three-state flag counting the completed task boundaries it observed, while preserving the rule that no transcript content and no task identity enters a session record.

State: **COMPLETE**

Production evidence:
- `src/session/store.rs` — `TaskContinuity`
- `src/session/store.rs` — `SessionStore::context`

Regression evidence:
- `session_context::task_continuity_separates_nothing_observed_from_one_task_from_several`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `(0, _) => TaskContinuity::Unknown,` -> `(0, _) => TaskContinuity::OneTask,` | `collapse-unknown-into-one-task` | **killed** | `session_context::task_continuity_separates_nothing_observed_from_one_task_from_several` |

> collapse-unknown-into-one-task observed: assertion `left == right` failed: a session whose harness has reported nothing has told us nothing

Recorded scope limits — stated by the worker, not discovered later:
- It counts boundaries; it does not COMPARE tasks. Two consecutive turns on one feature are indistinguishable from two on unrelated ones. Phase 36's same-task signal still has no producer.
- It depends on the harness reporting turn events. A harness that reports none reads Unknown for ever, which is honest but is not a signal.

---

### Worker-reported packet errors and gates (transcribed at closure)

**Packet errors the worker reported — read these BEFORE its results.**
Thirteen consecutive rounds a worker corrected its packet and was right:
- The packet framed the phase as needing several columns ('every new column is nullable or defaulted'). Current source says only 1159 lacks a durable producer: 1160 is sessions.last_activity_at (store.rs), 1164 is checkpoints.created_at (migration 5), and 1165 is the turn_ended rows migration 5 already stores with turn_outcome. Columns for those would be second sources of truth, which is migration 15's own stated objection. One column was added, not seven.
- The packet said `store.touch(&record.id)` is 'the established advance-this-session's-activity write, exercised at store.rs:2553'. store.rs:2553 is a test, and `touch` has NO production caller at all — all four call sites are tests (session/api.rs:433, store.rs:2553/2575/2587). What actually moves last_activity_at in production is `write_lifecycle_locked`, reached through `set_lifecycle` from main.rs's hook handler. The line still closes; the named producer was the wrong one. `touch` is a Cluster B candidate.
- The packet located the compaction call site at src/main.rs:2587. At 86c8a7f it is at main.rs:2589 (`if session::lifecycle::precedes_native_compaction(event)`). Same site, two lines off.
- `session/store.rs`'s rollback constant was named UNDO_MIGRATION_FOURTEEN while already undoing migration 15 as well — wrong by two migrations before this package touched it. Renamed to UNDO_MIGRATIONS_ABOVE_THIRTEEN to match the twin in database.rs.

**Files touched outside EXPECTED FILES** — disclosed, not hidden:
- `crates/glasshouse/src/events/mod.rs` — one `observed_compactions: None,` line in a #[cfg(test)] SessionRecord literal; a non-Default field on SessionRecord breaks every literal in the crate (batch 47's shape)
- `crates/glasshouse/src/session/recovery.rs` — same: one line in a #[cfg(test)] SessionRecord literal
- `crates/glasshouse/src/shell/state.rs` — same: four #[cfg(test)] SessionRecord literals, one line each
- `crates/glasshouse/src/shell/view.rs` — same: one #[cfg(test)] SessionRecord literal
- `crates/glasshouse/tests/gateway_degrade.rs` — same: one SessionRecord literal in an integration-test helper
- `crates/glasshouse/tests/memory_provenance.rs` — two rollback fixtures had to drop migration 16's column and two schema-version assertions had to move to 16; ACCEPTANCE TEST 2 required finding every one of these
- `crates/glasshouse/tests/memory_store.rs` — same: one rollback fixture and one version assertion
- `crates/glasshouse/tests/evaluation_observations.rs` — same: one rollback fixture and two version assertions, one of which asserts the database still claims the current schema version

Gates the worker ran (re-run the decisive ones yourself):
- cargo build: clean
- cargo clippy --all-targets --all-features -- -D warnings: clean
- cargo fmt --all -- --check: clean
- cargo test --test session_context: 14 passed, 0 failed
- cargo test --test memory_store: 17 passed; --test events_lifecycle: 5 passed
- cargo test --test memory_provenance: 13 passed; --test evaluation_observations: 22 passed
- cargo test --lib: 1518 passed; --bin glasshouse: 42 passed
- scripts/blast-radius.sh: every traced target passed
- cargo test --workspace --all-targets: 59 targets, 2227 tests, 0 failures (run in full because blast-radius green is not sufficient evidence for a schema change)
- scripts/mutate.sh (7 mutations): 7 KILLED, 0 SURVIVED, tree restored byte-identically; one eighth verdict was discarded as a false KILLED under practice §80 and re-run — see the report body

