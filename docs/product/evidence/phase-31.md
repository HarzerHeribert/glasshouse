---

# Line 1171 — closed 2026-08-30

Package `GH-COMPACTION-CHECKPOINT`. Phase 31 was 7 open / 0 closed and had no
ledger entry; this file is its first.

The line: *"Prefer creating or refreshing a portable checkpoint before
intentional compaction when practical."*

State: **COMPLETE**

## Why this one and none of the other six

`GH-COMPACTION-RECON` scoped the phase. **1171 is the only line of the seven
with all four Phase −1 links standing and no ruling in front of it.** Three of
the rest — 1169, 1172, 1173 — are Cluster Q: **Glasshouse never compacts
anything**, so a rule forbidding or preferring a compaction it does not perform
cannot be violated. See `docs/process/refusal-register.md`.

## The chain

- **Producer** `harness/codex.rs` — `PreCompact` is in production
  `REPORTED_EVENTS`, and `session/select.rs:1102-1108` asserts it reaches the
  `hooks.json` Codex actually reads, not merely the adapter's constant.
- **Caller** the `precedes_native_compaction` arm in `main.rs`, which already
  counted the compaction and ran extraction and did nothing about checkpoints.
- **Propagation** `checkpoint_before_compaction` → `Checkpoint::capture` →
  `CheckpointStore::save`.
- **Consumer** `glasshouse checkpoint list` and `resolve_bootstrap_prompt`,
  both production.

## The ruling that made this need no migration

`CheckpointReason` has exactly two variants and `database.rs:588` pins them
with `CHECK (reason IN ('manual', 'task_boundary'))`. So:

- **Stamping `TaskBoundary` would be false** — its own doc says *"Glasshouse
  took it because a turn ended"*, and a compaction is not a turn ending.
- **Adding a `BeforeCompaction` variant needs a table rebuild**, since SQLite
  cannot widen a `CHECK`.

The map line's verb is *"creating **or refreshing**"*, and a refresh needs no
new reason: what moves is `created_at` and the Git position.
`checkpoint_before_compaction` therefore **preserves
`previous.checkpoint.reason`**. No migration, no new variant, and
`checkpoint_after_turn` is untouched.

## "When practical" was already implemented

`store.latest_for(id)?` returning `None` → early return. A session that never
had a checkpoint does not get one invented at compaction time. No time window,
no staleness threshold, no config key was added.

## Gating

Behind `automatic_checkpoint_enabled`, the same independent switch
`checkpoint_after_turn` answers to — and deliberately **not** behind
`memory_extraction_enabled`. The compaction **count** stays outside both, as
its own comment already required, so a user who turns checkpoints off still
gets an honest count.

## Regression and mutation

`tests/session_context.rs`, four tests, driven through the shipped binary's
`glasshouse hook --event PreCompact` — the same entry point a real Codex hook
invokes. `18 passed; 0 failed`.

| mutation | result | killed by |
|---|---|---|
| delete the `checkpoint_before_compaction` call from the `PreCompact` arm | **KILLED** | `a_precompact_hook_refreshes_an_existing_checkpoints_created_at` |
| `previous.checkpoint.reason` → `CheckpointReason::TaskBoundary` | **KILLED** | `a_precompact_hook_preserves_a_manual_checkpoints_reason` |

The first is the characteristic one (§35): it lands on the **call**, so a test
that had entered below the hook would not have caught it. Observed:
*"assertion left == right failed: compaction must never restamp a manual
checkpoint as a task boundary."*

## Limits

- **Codex only.** Claude Code emits no compaction event at all
  (`harness/claude_code.rs:28-38`), so this behaviour is unreachable there —
  which is also why map line 310 is refused rather than open.
- No real Codex binary fired a live `PreCompact` in this package; the shipped
  `glasshouse hook` entry point was driven directly.
- Says nothing about `PostCompact`, which this line does not concern.

---


### Line 1174 — a hook's lost extraction is announced, never silent

Package `GH-HOOK-EXTRACTION-DETACH`, 2026-08-31, Opus 5 at high (Red). Opened when the prove-it for this line flaked under load (~1 run in 3 with six cargo processes). The packet's hypothesis — a detached extraction outliving the hook — was **refuted with measurement**: `run_extraction`'s thread is awaited by `recv_timeout(EXTRACTION_BOUND)` and `Extractor::run` calls `store.record` before it sends its outcome. Two real defects were found instead: the reproduction's fake model dropped connections under load because on macOS an accepted socket inherits the listener's `O_NONBLOCK` (a fixture defect, fixed in the test), and — the production defect — a hook whose extraction recorded nothing said so only through `tracing`, whose sink is disabled unless a `--log-*` flag or `GLASSHOUSE_LOG` is given, which a harness never passes: exit 0, empty stderr, no memory. `hook_extraction` now writes one warning to stderr naming the trigger and the reason, following `run`'s own precedent for the overridden safety refusal; the exit code stays 0 so the coding session is unaffected (Phase 21's non-fatal rule).

Follow-up worth a Green packet: `memory/extract/model.rs` maps a refused connection to `ModelError::Unavailable`, so the notice tells a user who *has* configured a model that *no extraction model is available* — honest about the loss, misleading about the cause; a one-arm change (a non-timeout `Error::Io` → `Failed { phrase }` naming an unreachable model) would fix the wording.

### Record enough pre-compaction durable memory that important project decisions do not depend solely on a lossy native compact summary. (line 1174)

Contract: Given a harness about to natively compact a session, when Glasshouse's PreCompact hook runs extraction and the extraction records no memory -- because the model cannot be reached, refuses, answers off-contract, or is cut off at EXTRACTION_BOUND -- Glasshouse writes a warning naming the trigger and the reason to the hook's stderr, so the person does not compact believing decisions were captured that were not, while preserving the hook's zero exit code and leaving the coding session unaffected.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's artifacts (a refuted packet hypothesis with the measurement that refuted it; two KILLED mutations with killing tests and output quoted; `test result:` lines with counts; 20 of 20 serial runs under load average 23–36 after the fix, against 6 of 20 before) and from reading the decision — `lost_extraction_notice`'s four cases, two deliberately silent — in the diff. The blast radius the report left `PENDING` was run by `integrate.sh` on the merged tree (see the commit).

Production evidence:
- `crates/glasshouse/src/main.rs` — `hook_extraction`
- `crates/glasshouse/src/main.rs` — `lost_extraction_notice`
- `crates/glasshouse/src/main.rs` — `report_hook_with (PreCompact arm, BeforeCompaction trigger)`

Regression evidence:
- `precompact_memory::a_precompact_hook_that_records_nothing_says_so_with_no_logging_configured`
- `precompact_memory::a_precompact_hook_leaves_a_memory_stamped_before_compaction`
- `precompact_memory::a_completed_turn_stamps_a_different_trigger_than_a_compaction_does`
- `glasshouse::tests::a_failed_extraction_is_reported_with_its_trigger_and_its_reason`
- `glasshouse::tests::a_compaction_with_no_session_activity_is_not_reported_as_a_loss`
- `glasshouse::tests::an_extraction_that_never_produced_an_outcome_is_reported_as_a_loss`
- `glasshouse::tests::a_run_that_stored_a_memory_is_silent_even_when_it_also_rejected_one`
- `glasshouse::tests::a_run_whose_every_memory_was_rejected_is_reported_as_a_loss`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| main.rs PreCompact arm: `hook_extraction(runtime, &id, model(&id), ExtractionTrigger::BeforeCompaction);` -> `let _ = model(&id);` | `delete-production-call` | **killed** | `precompact_memory::a_precompact_hook_leaves_a_memory_stamped_before_compaction` |
| main.rs hook_extraction: both `eprintln!` lines -> `let _ = notice;` | `remove-user-visible-report` | **killed** | `precompact_memory::a_precompact_hook_that_records_nothing_says_so_with_no_logging_configured` |

> delete-production-call observed: assertion `left == right` failed: the harness's own approaching compaction must run extraction exactly once -- left: 0, right: 1; and a_precompact_hook_that_records_nothing_says_so_with_no_logging_configured FAILED alongside it

> remove-user-visible-report observed: panicked at crates/glasshouse/tests/precompact_memory.rs:516:5: a compaction that recorded nothing must say so where the person can read it; stderr was:  <-- and nothing followed, which is exactly the silent loss

Recorded scope limits — stated by the worker, not discovered later:
- does not prove the notice is legible in Claude Code's UI; it proves the hook writes it to stderr with logging off
- the O_NONBLOCK accept inheritance is BSD/macOS behaviour; the flake was never reproduced on Linux and nothing here ran on Linux or Windows
- does not prove extraction succeeds under a real provider; the model is a loopback fake and a closed port
- does not make the loss durable or queryable -- a migration was forbidden, so the record is the stderr line and the existing routing-observation row

---
