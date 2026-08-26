# Glasshouse harness hook protocol

This document specifies a safe completion/reporting bridge between visible
Claude Code and OpenCode/Ox sessions. It is a design contract for project-local
adapters; it does not require either harness to impersonate or replace the
other.

## Goals

- Keep every worker visible in a real native harness TUI.
- Deliver completion durably even if cmux input injection or Enter fails.
- Route cheap leaf reports through a verifier instead of waking the root for
  every event.
- Normalize different native hook events without pretending they have the
  same blocking semantics.
- Prevent worker-controlled text from being executed in a shell.
- Avoid a shared global routing file whose target races between worktrees.
- Remain fail-open for agent execution but fail-closed for integration: a hook
  failure must not crash a worker, and missing evidence must never be treated
  as task completion.

## What a hook can do, and the one thing it cannot

A native harness hook is a **gate, not a proxy**. It can stop a tool call before
it runs, and it can put text in front of the model. It cannot answer on the
harness's behalf: no hook return field carries a substitute tool result.

Verified against the Claude Code hook reference (2026-08-26). A `PreToolUse`
hook returns `permissionDecision` (`allow` / `deny` / `ask`) and
`permissionDecisionReason`; every hook may additionally return `systemMessage`,
`additionalContext` and `terminalSequence`. None of these is the tool's result.
`PostToolUse` observes a call that already ran and does not rewrite what the
model is shown. A third-party project that needed to substitute a result had to
deny the call and hide the answer inside the reason string — because the surface
offers nothing else.

This is a fit, not a limitation, for what this protocol does. Reporting a
completion, waking a parent, and normalizing an event are all within a gate's
power. But it fixes where *other* features must live: anything that substitutes
a harness's behaviour belongs at the **transport** (the gateway) or at the
**executable** (a generated shim), never here. See
`GLASSHOUSE_DESIGN_DECISIONS.md`, "Speculative tool calling is a harness
technique, and a harness hook is a gate, not a proxy."

## Topology

Use project/worktree-local parent routes:

```text
Ox leaf worktree ---------> Sonnet verifier pane
Other leaf worktree ------> Sonnet verifier pane
Sonnet verifier ----------> Opus orchestrator pane
Sonnet implementer -------> Opus orchestrator pane
Opus specialist ---------> Opus orchestrator pane
```

The root receives aggregate verifier results rather than every Ox turn-end.
Direct Opus/Sonnet reports remain appropriate when there is no verifier tier.

## Durable task report

Normal OpenCode TUI sessions do not reliably produce `.runs/` artifacts, so
the protocol must not assume one. Each assigned task gets a worktree-local,
Git-ignored runtime directory such as:

```text
.agent-runtime/tasks/<task-id>/
  route.json
  report.json
  delivery.json
```

`report.json` is the durable source of truth. The wake message only points to
it. Write reports atomically by replacing a temporary file in the same
directory.

Recommended report schema:

```json
{
  "version": 1,
  "task_id": "GH-P01-008-WINPATH",
  "event_id": "session-id:turn-number",
  "role": "ox-leaf",
  "status": "pass",
  "worktree": "/absolute/worktree/path",
  "files": ["crates/glasshouse/src/platform/paths.rs"],
  "diffstat": "1 file changed, 24 insertions(+)",
  "gates": ["cargo fmt: pass", "focused tests: pass"],
  "findings": ["maximum five short, secret-free findings"],
  "updated_at": "RFC3339 timestamp"
}
```

Reports must never contain credentials, environment values, raw prompts,
transcripts, or arbitrary command output. Long diagnostics stay in the visible
worker pane or a separately referenced local artifact.

## Local routing configuration

`route.json` belongs to one worker/worktree and is immutable for the session:

```json
{
  "version": 1,
  "task_id": "GH-P01-008-WINPATH",
  "role": "ox-leaf",
  "parent": {
    "harness": "claude",
    "surface_ref": "surface:19",
    "surface_uuid": "UUID",
    "workspace_ref": "workspace:7"
  },
  "notify": true
}
```

Do not use a single mutable file in `~/.config/opencode` to route several
workers. A global adapter may exist, but it must read routing data relative to
the current project/worktree. Missing, malformed, or changing configuration
means “write the report but do not inject a wake.”

The adapter must compare both cmux short refs and UUIDs to prevent self-wake
loops.

## Native event normalization

Normalize native events into these protocol events:

- `turn.completed` — a model turn became idle; work may or may not be done.
- `task.ready_for_review` — the worker explicitly produced its final report.
- `task.blocked` — the task cannot progress within its task packet.
- `session.ended` — the native session was deleted, archived, or exited.

Claude Code adapters may observe native stop/session hooks. OpenCode adapters
may observe `session.idle`, idle status, deleted, or archived events. Their
semantics differ:

- A Claude Code stop hook may participate in native stop handling according to
  Claude's own hook contract.
- An OpenCode idle event happens after the turn has stopped and cannot be used
  as a blocking enforcement gate.

Therefore, turn-end wakes are advisory. Durable integration enforcement stays
with the verifier/orchestrator reviewing `report.json`, the diff, and tests.

## Safe cmux delivery

Delivery has two independent parts:

1. Write the durable report and delivery status.
2. Attempt a compact wake and optional cmux notification.

The wake text must be a constant template containing only validated identifiers
and a local report path, for example:

```text
[worker-ready GH-P01-008-WINPATH] read <validated-report-path>
```

Never inject arbitrary worker findings, model output, shell fragments, quoted
prompts, or environment data.

Before typing and pressing Enter, verify that the configured parent surface
still hosts the expected harness process. A surface may fall back to a shell
after Claude/OpenCode exits; pressing Enter there could execute injected text.
If the expected harness cannot be confirmed, use `cmux notify` only and record
`wake_skipped_parent_not_running` in `delivery.json`.

If safe injection is possible:

1. send the constant wake text to the configured surface;
2. send Enter as a separate cmux operation;
3. check both command results;
4. retry at most once;
5. record delivery success/failure without spawning a hidden polling loop.

Input injection is never the durable channel. A missed Enter, busy orchestrator,
or active user must not lose the report.

## Deduplication and debounce

- Deduplicate by `task_id + event_id + normalized event`.
- Debounce repeated native idle/status events for the same turn.
- `turn.completed` must not overwrite an explicit `task.ready_for_review` or
  `task.blocked` report.
- A verifier sends one aggregate wake after it has inspected all expected leaf
  reports, not one wake per file or test.

## Security and repository hygiene

- Keep `.agent-runtime/` ignored and uncommitted.
- Commit adapter templates and schemas only when intentionally implemented as
  project tooling; never commit live routes, surface IDs, session IDs, or
  reports.
- Restrict report and route files to the current user where the platform
  supports it.
- Validate task IDs, enum values, paths, and cmux identifiers before use.
- Do not evaluate report fields as shell, JavaScript, or prompt templates.
- Use explicit command arguments rather than shell interpolation in adapters.
- Hook/plugin failures must not terminate the native harness.
- Missing or malformed reports mean “not verified,” never success.

## Implementation sequence

When this protocol is implemented:

1. Define and test the report/route schemas and atomic writer.
2. Implement a common cmux delivery helper with process-presence safety.
3. Add the Claude Code native-hook adapter.
4. Add the OpenCode project-local plugin adapter.
5. Test event normalization and deduplication with captured fixtures.
6. Test a leaf-to-verifier-to-root flow in visible panes.
7. Test parent-exited behavior and prove no text executes in a shell.
8. Document installation and rollback.

Until those tests exist, use manual visible polling rather than an unsafe
partial auto-wake implementation.
