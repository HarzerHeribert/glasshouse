# Glasshouse worker capabilities

> This describes how Glasshouse is built, not what Glasshouse does. Nothing
> here is a product requirement. Capability requirements live only in
> `docs/product/capability-map.md`.

This document defines what each model tier should and should not do. The goal
is to save expensive-model tokens without delegating judgment to a model that
cannot reliably carry it.

## Opus orchestrator

### Do

- Own capability order, behavioral contracts, architecture, and proof
  conditions.
- Create small immutable worker task packets.
- Inspect every diff and independently rerun relevant checks.
- Resolve worker disagreements using code, tests, runtime evidence, and the
  capability map.
- Update the map, evidence ledger, handoff, and Git history.
- Preserve project isolation, lifecycle correctness, security, and product
  scope across batches.

### Do not

- Spend most of the turn on searches or boilerplate that a cheaper worker can
  perform safely.
- Accept summaries without inspecting decisive code or output.
- Check a capability because code exists or a worker says `ACCEPT`.
- deploy, publish a release, or broaden product scope. Pushing to origin to run
  CI is expected of this role rather than withheld from it — see the SDLC.

## Opus specialist worker

Use only when the task is red-risk or architecture remains disputed.

### Do

- Design or review PTY/process lifecycle, signals, job control, terminal
  restoration, session persistence/resume, concurrency, SQLite migrations,
  recovery, security boundaries, and native Windows behavior.
- Implement one bounded high-risk component when the orchestrator has defined
  its contract and ownership.
- Perform independent adversarial review of another worker's risky diff.
- State exact acceptance/rejection evidence and remaining uncertainty.

### Do not

- Perform routine repository inventories, boilerplate, or broad speculative
  rewrites.
- Edit the same files as another active worker.
- Update project-status documents, commit, or make final checkbox decisions.
- Treat model confidence as evidence.

## Sonnet implementer

This is the default implementation tier after design is settled.

### Do

- Implement scoped modules, adapters, configuration, CLI parsing, TUI state,
  deterministic refactors, and regression tests.
- Work in an isolated worktree with explicit expected and forbidden files.
- Preserve public invariants and stop when the task requires new architecture.
- Run focused formatting, compilation, lint, and tests before reporting.
- Report changed files, diffstat, commands, failures, and risks.

### Do not

- Decide ambiguous product behavior or capability-map interpretation.
- Expand into lifecycle, persistence, concurrency, or security architecture
  without escalation.
- Hide missing platform evidence behind injected-platform unit tests.
- Commit, update the map/evidence/handoff, or merge its own work.

## Sonnet verifier

### Do

- Receive and condense Ox leaf reports.
- Review medium-risk diffs against the orchestrator's acceptance matrix.
- Reproduce worker failures and distinguish product defects from flaky tests.
- Check production reachability, test non-vacuity, error paths, and scope.
- Send one concise `ACCEPT` or `REJECT` report to the orchestrator.

### Do not

- Edit the implementation it is reviewing.
- Forward large raw worker transcripts when a short evidence summary suffices.
- Override unresolved Opus-level architecture.
- Commit or update project-status records.

## Ox leaf worker

Ox is a fast junior worker for mechanically decidable tasks.

### Do

- Search and inventory files, call sites, spawn sites, configuration uses, and
  missing tests.
- Run focused tests repeatedly and capture exact failure output.
- Add a small known regression test after the design is fixed.
- Make one-file or two-file mechanical changes, normally under about 150
  changed lines.
- Review a diff against a short explicit checklist.
- Update settled comments or documentation.
- Report no more than five decisive findings.

### Do not

- Own architecture, capability interpretation, product decisions, or final
  verification.
- Design PTY lifecycle, persistence, migrations, concurrency, recovery,
  security, or cross-platform process behavior.
- Perform large refactors or touch unexpected files.
- Continue reasoning indefinitely without producing an artifact.
- Commit, update the map/evidence/handoff, or report directly to the root when
  a verifier tier is assigned.

### Immediate escalation triggers

Stop Ox and promote the task when any of these occurs:

- roughly two minutes of reasoning produces no useful artifact;
- more than two files or about 150 changed lines become necessary;
- requirements change during the turn;
- the worker proposes a new abstraction;
- platform behavior is uncertain;
- lifecycle, isolation, secrets, persistence, or concurrency appears;
- the worker needs repeated corrective follow-ups.

Do not accumulate contradictory prompts in a confused context. Stop it, write a
cleaner task packet, and restart at the appropriate model tier.

## Leaf tier: a fast cheap model works here

The leaf tier is not Ox-only. A fast, cheap, less capable model is exactly
right for mechanically decidable work, and it was measured rather than
assumed.

Gemini 3.7 Flash, driven through the installed Antigravity CLI (`agy`), was
given a bounded inventory task: read the capability map, quote every unchecked
line matching a term list, group by phase, write one report. It returned
**171 quotes, all 171 verbatim against the map, and none of them an
already-checked line** — verified mechanically, not eyeballed.

Use it for the same things as any other leaf worker: inventories, call-site
searches, focused reruns, checklist reviews, settled documentation. Give it
the same bounded packet, and **verify its output mechanically** — diff its
quotes against the source rather than trusting the summary.

Two operational notes. Antigravity declares no automatic-review mode, and its
"always allow" matches on the exact command prefix, so it re-prompts for every
new script; a leaf worker there needs `--dangerously-skip-permissions`, which
the user has accepted for this use. And `--mode accept-edits` is required for
it to write its own report.

## Risk routing

### Green: Ox

- inventories and call-site searches;
- focused stress reproduction;
- small known regression tests;
- path/config literals;
- settled comments and mechanical reviews.

### Amber: Sonnet

- configuration layering;
- integration selection;
- ordinary CLI/TUI state;
- bounded adapters and refactors;
- multi-file implementation with settled ownership.

### Red: Opus specialist

- project/session isolation;
- PTY lifecycle, shutdown, signals, and job control;
- database migrations and concurrent recovery;
- resume identity and durable state;
- secret boundaries and unsafe command construction;
- native platform behavior whose failure could lose state or execute the wrong
  process.

## Worker task packet

Every worker receives this information:

```text
TASK ID:
ROLE / MODEL:
CAPABILITY:
BEHAVIORAL CONTRACT:
OBJECTIVE:

EXPECTED FILES:
FORBIDDEN FILES:

REQUIRED BEHAVIOR:
SECURITY / ISOLATION INVARIANTS:
CROSS-PLATFORM REQUIREMENTS:

ACCEPTANCE TESTS:
VERIFICATION COMMANDS:

STOP CONDITIONS:
- Stop if architecture is ambiguous.
- Stop before expanding beyond expected files.
- Do not edit map, evidence ledger, or handoff.
- Do not commit.
- Report changed files, diffstat, gates, failures, and remaining risks.
```

An Ox completion report should normally be only:

```text
TASK:
STATUS: PASS | FAIL | BLOCKED
WORKTREE:
FILES:
DIFFSTAT:
GATES:
FINDINGS: at most five bullets
```
