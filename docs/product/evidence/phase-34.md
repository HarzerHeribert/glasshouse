# Phase 34 — Capability registry, 10 of 10 closed

> **Closed by the second package, not the first.** The history below is kept
> deliberately: the registry landed complete and correct in `GH-ROUTING-CAPABILITY`
> and ticked nothing, because nothing consulted it. `GH-ROUTER-TASK-INPUT`
> supplied the one missing argument and all ten closed together. Read both
> halves — the hold is the more useful record.

Capability map lines 1382–1391. Package `GH-ROUTING-CAPABILITY`, worktree
`.worktrees/routing-capability`; full report in
`.agent-runtime/report-routing-capability.md`. **Integrated 2026-08-29 and
deliberately ticked nothing.**

## Read this before re-deriving the phase

The registry is **built, tested, integrated and correct**. Ten boxes are still
open, and that is a ruling rather than an oversight. The worker did what its
packet asked and reported the limit honestly and unprompted; the gap is in what
the packet could ask for, not in what it delivered.

## What landed

- `src/routing/capability.rs` (new) — `CapabilityAxis` (all seven of 1383–1389),
  `axis_for`, `ResourceFacts`, `ResourceCapabilities::describe`, `render`.
- `src/routing/session.rs` — `capability_fit`, the seventh `Contribution`,
  pushed unconditionally in `score()` at `session.rs:1457`;
  `Destination::resource_facts`; `TaskRequirements::hard_capabilities`.
- `tests/routing_capability.rs` — four acceptance tests entered through
  `SessionRouter::choose`.

Three mutations, all KILLED, including the one that matters most — deleting
`capability_fit`'s own call site, which failed all three routing-decision
tests and proves the call site is one no test bypasses (§35/§36).

## Why nothing is ticked

**`capability_fit` cannot change a routing outcome in any execution that can
happen today.** Verified by the orchestrator against the integrated tree, not
taken from the report:

1. `capability_fit` short-circuits on its first statement —
   `if requirements.hard_capabilities.is_empty() { return Contribution::new("capability fit", 0.0, ...) }`.
2. Every production construction of `TaskRequirements` is
   `TaskRequirements::default()` — `main.rs:995`, `main.rs:1172`,
   `main.rs:3516`. There is no other one.
3. Therefore `hard_capabilities` is always empty in production, the
   short-circuit always fires, and **no capability description is ever read by
   shipped code**.

Line 1382 says the resources are described "*with a small set of capabilities
**used for routing***". A registry that no production execution consults is not
used for routing. Closing it would manufacture a fresh instance of exactly the
Cluster B shape this project spent four of batch 51's eight closures removing.

1383–1389 ("include X capability in the registry") are held with it: "in the
registry" is only a capability claim if the registry is the router's, and 1382
is the line that makes it so. 1390 and 1391 are properties of that same
registry.

**`scripts/evidence_from_report.py` refused this report independently**, on
eight of the ten lines, for a different reason: `verdict: closed` with no
killed mutation attached to that line — §14's trap, a closure resting on an
existing test. Two mechanisms disagreeing with the same verdict from different
directions is worth recording.

## What closes all ten, and it is small

One package with `main.rs` in EXPECTED FILES: classify the task, call
`TaskClassification::hard_capabilities()` (`classify.rs:365`), and populate
`TaskRequirements.hard_capabilities` before `SessionRouter::choose` is invoked
at `main.rs:1022`, `1194` and `3518`. That single wire makes every one of these
ten lines true at once, and it retires a long-standing Cluster B symbol: the
**method** `hard_capabilities()` still has exactly one non-test call site,
`classify.rs:623`, inside a `writeln!` — it is printed and never decided on,
which is the gap this phase was opened to close.

It could not be done here: `main.rs` was held by `GH-SESSION-CONTEXT` for the
same round, and the packet forbade it rather than let two workers collide.

## Two facts a successor should not re-derive

- **The pre-existing `needs_tool_calls` field has the identical shape.**
  `grep needs_tool_calls` shows it set to `true` only in
  `tests/session_router.rs`; every production site uses `::default()`. The new
  field sits at the same maturity as the one beside it. That is context, **not
  a reason to close** — an existing gap is not evidence.
- **`HardConstraint::Capability` is defined and never constructed anywhere**,
  including tests. It is the rejection path for a task whose hard capability a
  resource is *established* to lack. Cluster C shape; it needs the same wiring
  packet before it can have a constructor.

## The three model-only axes

`large-context`, `fast-cheap-analysis` and `repository-review` are
representable and rendered but have **no `HardCapability` variant** to be
reached from a task's side, so `capability_fit` never touches them even in a
test. Reaching them needs a task-side vocabulary that does not exist yet —
Phase 34A's lines 1401–1403, which depend on this registry in turn.


---

# Closure — `GH-ROUTER-TASK-INPUT`, 2026-08-29

## The one missing argument

`glasshouse route` now accepts an optional `--task <TEXT>`.
`main.rs::task_requirements_from_text` (`main.rs:1074`) classifies it once with
`classify_heuristically` and builds a real `TaskRequirements` — **both fields**,
not just the new one:

    let hard_capabilities = classify_heuristically(text).hard_capabilities();
    TaskRequirements { needs_tool_calls: !hard_capabilities.is_empty(), hard_capabilities }

Blank or absent text returns `TaskRequirements::default()`, so today's
behaviour is byte-for-byte the no-`--task` behaviour. The call site is
`main.rs:998`, inside `route_report`, before `SessionRouter::choose`.

**Line 1382 is now true.** The registry is consulted on a real production path
and the decision changes with the task description. That is what the hold above
was waiting for, and 1383–1391 close with it because they were held only
because 1382 was false.

## What this also retires

`TaskClassification::hard_capabilities()` had exactly one non-test call site,
`classify.rs:623`, inside a `writeln!` — printed and never decided on. It now
has a production caller at `main.rs:1083`. That is the Cluster B symbol this
phase was opened to close, and it is closed.

`needs_tool_calls` was `false` in production since it was added, set `true`
only in `tests/session_router.rs`. It is now derived. Wiring the new field and
leaving that one hardcoded would have closed ten boxes while leaving the
identical defect one field to the left.

## Mutations

| mutation | result | killed by |
|---|---|---|
| skip the derivation at the call site (`task_requirements_from_text` → `TaskRequirements::default()`) | **killed** | the `--task` ranking-flip regression |
| hardcode `needs_tool_calls: false` | **killed** | the tool-calls hard-constraint test |

## The orchestrator's ruling on the seven lines the evidence tool refused

`scripts/evidence_from_report.py` refused 1383 and 1386–1391 for §14's reason:
`closed` with no killed mutation on that line's own axis. **The worker flagged
this itself and explicitly declined to rule**, which is the correct behaviour
and is why the ruling can be made accurately.

I close them. 1386–1388 (`large-context`, `fast-cheap-analysis`,
`repository-review`) and 1389 (`MCP`) close on **the axis being in the
registry** — which is what those lines ask — and the registry is now live.
1383, 1390 and 1391 rest on mutation (a)'s coverage of `capability_fit`'s
single call site, which no test bypasses.

## Limits

- The three model-only axes still have **no `HardCapability` variant**, so no
  task can require them. That is Phase 34A's 1401–1403 and needs a task-side
  vocabulary that does not exist. Representable ≠ requestable.
- `launch_session` and `report_task_boundary_routing` still pass
  `TaskRequirements::default()`. Neither has request text, and inventing one
  is a product decision this package did not carry.
- `HardConstraint::Capability` is still never constructed. Capability mismatch
  costs a contribution; it does not reject.
- The ranking-flip test depends on `PROTOCOL_COMPATIBLE_FIT` and
  `CAPABILITY_ESTABLISHED_PRESENT` both being `0.4`. A future packet changing
  either must re-derive it — it fails loudly rather than mis-flipping quietly,
  because the assertions are exact-prefix.
