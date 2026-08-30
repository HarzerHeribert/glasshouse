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
