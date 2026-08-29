# Phase 34 — Capability registry, 0 of 10 closed — CODE LANDED, BOXES HELD

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
