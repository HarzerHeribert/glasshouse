# Glasshouse agent SDLC

This document defines how agent harnesses implement the authoritative
[`GLASSHOUSE_IMPLEMENTATION_CAPABILITY_MAP.md`](GLASSHOUSE_IMPLEMENTATION_CAPABILITY_MAP.md)
without confusing code presence with verified product behavior. It is
phase-independent: the current checkpoint belongs in
[`GLASSHOUSE_HANDOFF.md`](GLASSHOUSE_HANDOFF.md), while evidence for individual
requirements belongs in
[`GLASSHOUSE_CAPABILITY_EVIDENCE.md`](GLASSHOUSE_CAPABILITY_EVIDENCE.md).

## Sources of truth

Use these records for distinct purposes:

1. The capability map defines requirements and implementation order.
2. The evidence ledger explains what observable behavior each requirement
   promises and what proves it.
3. The handoff records current state, commands, workers, loose ends, and the
   next exact action.
4. Git history records coherent integrated batches.

The map always wins if another record contradicts it. Checked code, passing
tests, worker confidence, or a plausible implementation are not substitutes
for capability evidence.

## Behavioral contracts

Before implementing a mandatory checkbox, express it as one plain-language
sentence:

> Given **context**, when **trigger**, Glasshouse **observable behavior**,
> while preserving **an important invariant or failure behavior**.

The sentence should describe what the application does, not which Rust type it
contains. It becomes the stable link between specification, production code,
and regression coverage.

Examples of useful contract properties include:

- the user-visible result;
- project-isolation or secret-handling constraints;
- lifecycle cleanup after success, failure, cancellation, or crash;
- cross-platform behavior;
- a fail-closed outcome for unsafe or ambiguous input.

## Evidence states

Every active ledger entry uses one of these states:

- **NOT STARTED** — no relevant implementation or proof.
- **SCAFFOLDED** — supporting code exists, but production behavior is absent or
  unproven.
- **PARTIALLY VERIFIED** — at least one contract clause has real production and
  regression evidence, while another required clause is missing.
- **LOCALLY VERIFIED** — production behavior and local regression evidence are
  present for the whole contract, but required external/platform evidence is
  missing.
- **CI VERIFIED** — required platform CI has passed, but another contract
  condition is still incomplete.
- **COMPLETE** — every applicable production, failure, isolation, lifecycle,
  and platform claim has concrete evidence.

Only **COMPLETE** may justify checking the authoritative capability-map box.

## What counts as regression evidence

A regression test counts only when it:

- exercises the production path or the nearest deterministic production seam;
- asserts observable behavior, not merely type or function existence;
- would fail if the required behavior were removed;
- cannot pass solely through test-only scaffolding;
- covers negative or fail-closed behavior when security or isolation applies;
- does not silently skip on every relevant platform;
- is paired with real platform CI when the contract makes an OS-specific claim.

Evidence is many-to-many. One test may prove several contracts, and one
contract may require several tests. Do not manufacture trivial one-test-per-box
coverage. If behavior is inherently manual or external, record the exact
runtime probe or CI evidence instead of pretending it is unit-tested.

For high-risk contracts, reviewers should perform a non-vacuity check: identify
the smallest relevant production mutation or removal that would make the test
fail. This need not become permanent mutation-testing infrastructure, but the
reasoning must be credible.

## Capability implementation loop

Work from the first unchecked mandatory capability after reconciling existing
evidence.

### 1. Reconcile

Inspect Git status and history, worker worktrees, uncommitted diffs, the
handoff, the capability map, the current evidence entry, relevant tests, and
current CI. Preserve valid progress and separate complete work from scaffolding
or generated noise.

### 2. Define proof before code

Quote the exact checkbox, write its behavioral contract, and list applicable
positive, negative, isolation, lifecycle, security, and platform evidence.
Record missing evidence before assigning implementation.

### 3. Inventory cheaply

Use bounded Ox tasks for searches, call-site inventories, focused test runs,
and platform-gap identification. Have a verifier condense leaf reports into an
acceptance matrix. The orchestrator confirms decisive claims directly.

### 4. Settle design at the right model tier

The Opus orchestrator settles ordinary architecture. A separate Opus
specialist handles disputed or red-risk design. Once the design is stable,
Sonnet performs most implementation. Ox handles only mechanically decidable
leaf work. Detailed boundaries live in
[`GLASSHOUSE_WORKER_CAPABILITIES.md`](GLASSHOUSE_WORKER_CAPABILITIES.md).

### 5. Implement in isolation

Editing workers use separate Git worktrees and branches. Use one primary
implementation path. Parallelize only disjoint modules, tests, investigation,
documentation, or review. Never allow concurrent edits to the same files.

### 6. Review independently

The orchestrator inspects every worker diff. Ox may perform mechanical
checklists; Sonnet aggregates medium-risk review; large or red-risk changes
receive an independent Opus review. Resolve disagreements with repository
evidence, runtime probes, and the map—not model confidence.

### 7. Verify proportionally

Choose applicable checks from:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
rustup run 1.85.0 cargo check --locked --workspace --all-targets
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps
git diff --check
```

Also use focused stress runs, failure injection, executable probes, and
Linux/macOS/Windows CI when the contract requires them. Distinguish new
failures from explicitly recorded pre-existing debt. A narrow local test cannot
prove broad cross-platform behavior.

### 8. Record and integrate

Update the evidence ledger, check the map only at **COMPLETE**, update the
handoff, and create one accurate coherent commit. The Opus orchestrator is the
only role that integrates, commits, or changes project-status records. Leave
main clean at coherent boundaries. Push to origin when a contract's evidence
needs CI: cross-platform claims are unprovable without real runners, and a
green local suite is precisely the state in which a platform-specific defect
stays hidden. Pushing for CI is the orchestrator's job and nobody else's.

### 9. Continue or checkpoint

Proceed checkbox-by-checkbox while safe work remains. Stop only for a genuine
product decision, unavailable mandatory dependency, missing authority for a
material external action, or unsafe ambiguity. At a harness/context boundary,
finish or safely checkpoint the current batch, verify proportionally, update
the records, and leave a precise next-agent prompt.

## Visible harness operation

Keep native harness sessions visible and steerable. Use compact cmux
workspaces with at most four panes each:

- **Glasshouse control:** Opus orchestrator, optional Opus specialist, Sonnet
  verifier, and a plain diagnostic shell.
- **Glasshouse build:** up to two disjoint Sonnet implementers and up to two
  Ox leaf workers.

Do not fill every pane without useful independent work. Start Ox normally by
running `ox` in a terminal and entering its task in the normal TUI. Never use
`ox run`, a headless agent loop, or an invisible worker.

Every editing worker receives an isolated worktree. Read-only reviewers may
inspect main. The preferred report path is:

```text
Ox leaves -> Sonnet verifier -> Opus orchestrator
Sonnet implementers ---------> Opus orchestrator
Opus specialist -------------> Opus orchestrator
```

The completion transport and safety rules are defined in
[`GLASSHOUSE_HARNESS_HOOK_PROTOCOL.md`](GLASSHOUSE_HARNESS_HOOK_PROTOCOL.md).

## Product invariants

All work must preserve these principles:

- Glasshouse orchestrates real installed harnesses; it does not hide or
  replace them.
- Every interactive session is backed by a real native harness.
- Providers and gateways are backends selected through isolated launch
  profiles.
- Memory, session state, logs, and runtime artifacts are project-scoped.
- Cross-project access is disabled structurally where possible.
- PTY/process ownership and cleanup are correct on every claimed platform.
- Secrets never enter logs, Debug output, diagnostics, snapshots, fixtures, or
  commits.
- Telemetry measures outcomes and evidence, not token/spend vanity metrics.
- Same-vendor pairing is an initial prior that local measured evidence may
  override.
- Experimental capabilities remain out of scope unless mandatory work depends
  on them.
