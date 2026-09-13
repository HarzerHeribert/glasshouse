# Pane smarter-and-cheaper execution roadmap

Status: product direction and measured gap register, 2026-09-13.

This record turns the Terminal-Bench pilot, the Tool ABI design, and the useful
parts of GVS5H's ledger-based orchestration into one product plan. It does not
close capability-map boxes. Every row still needs implementation and evidence
through the normal ledger.

## Product thesis

Pane is not valuable because it renames ordinary coding-agent tools or because
it makes the model learn JavaScript. It is valuable when a model can use the
tool vocabulary it already knows while Pane supplies a better execution
mechanism underneath:

```text
provider-familiar intent
    -> canonical capability
    -> dependable deterministic execution
    -> exact artifact and provenance
    -> deduplication, bounding or cheap semantic reduction
    -> only decision-relevant feedback reaches the expensive parent
```

The practical analogy is an ordinary power tool versus a better professional
one: the operator makes the familiar motion, but the better tool removes more
friction and fatigue over a full day. Pane wins when it preserves competitor
correctness while lowering weighted cost, expensive-parent attention and
avoidable recovery work.

Raw token count alone is not the objective. Cheap-model tokens can be an
excellent trade when they remove expensive parent turns or prevent a failed
task.

## What the 91 `execute_cell` calls mean

The accepted 2026-09-12 Terminal-Bench campaign invoked Pane without an
explicit `--interface` option. Pane therefore used its default `hybrid`
interface, which exposed provider-familiar direct tools and `execute_cell`.

The raw provider conversation contains 91 assistant `tool_use` blocks, all
named `execute_cell`, and no direct `Read`, `Edit`, `Bash`, `Grep` or related
provider tool call. This observation is made before lowering, so it is not the
result of direct tools being represented as Cell IR internally. The model
really selected `execute_cell` for every acting turn.

That fact is not itself a defect. The selected tasks required dependent
compilation, inspection, branching and verification, for which composed cells
are an intended interface. A model preferring cells can be evidence that the
composition surface is useful.

It becomes a concern only when one of these is true:

- the model selected cells because Pane's prompt biased it away from a more
  familiar or cheaper direct call;
- a simple single-intent operation paid cell-language or result-envelope tax;
- the chosen cell exposed Pane-specific syntax, permission or mutation
  failures that a direct familiar call would have avoided;
- composition did not reduce parent turns or context enough to repay that tax;
- the direct translation path remains unexercised and therefore unproven in
  real work.

The pilot cannot decide among those explanations. Direct calls and authored
cells converge on the same kernel by design, but final machine telemetry counts
only cells and primitive tools. The stream event also omitted the frame origin.
The next telemetry revision must separately report:

- provider-native direct tool calls;
- model-authored cells;
- lowered direct-tool frames;
- primitive operations per frame and per parent request;
- single-intent authored cells that could have been direct calls;
- failures and repair turns by invocation origin;
- repeated observation bytes and parent tokens by invocation origin.

Do not force a quota of direct calls. Measure **interface regret** instead: for
the same task and model, did the interface selected in hybrid mode cost more,
fail more, or expose more context than the best supported alternative?

## Terminal-Bench failure audit

The pilot's 13 failed cells were not 13 ordinary bad shell commands. Their
causes were:

| Cause | Cells | Interpretation |
|---|---:|---|
| `gdb` denied before execution | 3 | Policy/capability mismatch |
| `/build` search denied before execution | 3 | Benchmark omitted a task-declared read root |
| TypeScript `as const` rejected | 2 | Advertised TypeScript/runtime mismatch |
| Edit rejected after Pane's own earlier mutation | 2 | Mutation composition/version-chaining gap |
| Write beneath reserved `.pane` denied | 1 | Correct boundary, insufficient steering/scratch affordance |
| Deliberately crashing target surfaced as killed Bash | 1 | Expected diagnostic action counted as a tool failure |
| Headless Vim command exceeded 60 seconds | 1 | One genuine unintended execution failure |

Thus six failures were policy/profile mismatches, five were Cell/Tool ABI
friction, one was the crash the debugging task explicitly asked the agent to
reproduce, and one was a genuine hung command. A cell can also complete useful
earlier operations before a later tool throws; labelling the whole frame
`failed` is truthful about control flow but not equivalent to the per-command
failure display of Claude Code or Codex.

Telemetry must retain the aggregate while splitting it into syntax, denial,
mutation conflict, expected diagnostic non-zero/signal, timeout, transient
infrastructure and genuine command failure. It must also attribute the next
parent request and its usage when that request is actually a repair.

## Current strengths to preserve

These mechanisms are aligned with the product thesis and should be hardened,
not replaced:

- one canonical capability kernel for direct calls and cells;
- provider-dialect façades that lower rather than implement separate tools;
- exact artifact handles and provenance;
- bounded exact versus derived evidence classes;
- deterministic-first semantic command lifting;
- cheap, bounded and separately metered Little Helpers;
- durable event and rollout ledgers;
- sandbox admission, credential stripping and cancellation;
- context compaction and resumable sessions.

## Build, debug and redesign register

`Debug` means the intended feature exists but behaved incorrectly. `Redesign`
means its present semantics work against the product thesis. `Build` means the
capability is materially missing. `Prove` means implementation exists but no
representative measurement supports the claim yet.

| Priority | Feature | Work | Current evidence or problem | Required outcome |
|---|---|---|---|---|
| P0 | Interface-origin telemetry | Build | The final result cannot distinguish authored cells from lowered direct calls | Machine output records provider selection, frame origin, primitive operations and failures by origin |
| P0 | Hybrid interface choice | Prove | Hybrid exposed both paths, but the model selected 91 authored cells and zero direct calls | Matched tools/cells/hybrid runs establish whether model choice reduces turns, failures and weighted cost; no arbitrary direct-call quota |
| P0 | Familiar Tool ABI correctness | Debug | Real work did not exercise direct provider façades | OpenAI- and Anthropic-shaped calls pass representative read/search/edit/shell tasks through the shared kernel |
| P0 | TypeScript Cell contract | Debug | `as const` failed twice although Pane specifies model-authored TypeScript | Every advertised erasable construct runs; unsupported constructs receive an accurate diagnostic before execution |
| P0 | Capability/environment manifest | Build | The model repeatedly selected task-advertised `/build` and `gdb` capabilities that Pane denied | Before acting, the parent sees effective readable/writable roots, available commands, reserved paths and unavailable capabilities |
| P0 | Benchmark container profile | Debug | Outer-container mode still conflicted with task-required reads and debugging | The adapter grants declared in-container capabilities while normal host security remains unchanged |
| P0 | Tool outcome semantics | Redesign | One late denial throws the whole composed cell after useful earlier work | Each call has a typed outcome; a frame aborts only when safe continuation is impossible, while partial effects remain explicit |
| P0 | Transactional mutation composition | Redesign | Two edits were stale because Pane itself changed the file first | Multi-hunk and sequential same-file edits compile into one checked atomic mutation; only external races are stale |
| P0 | Observation delta | Redesign | 678 handle rows were emitted over 91 cell results, mean 7.45 and maximum 26 | Return new/changed handles, dependencies and unresolved failures rather than the full repeated inventory |
| P0 | Handle lifecycle | Redesign | Resolved intermediate handles accumulate in the active result surface | Retire them from active context while preserving exact ledger access and explicit pinning |
| P0 | Semantic read/search lifting | Debug and prove | The recognizer exists, but the pilot did not isolate its context or round-trip effect | Equivalent familiar shell/direct/cell reads converge, deduplicate and return the smallest exact sufficient observation |
| P0 | Adaptive result reduction | Debug and build | Only one pushed reducer ran in 12 trials, on a 403-line result | Large noisy observations are reduced when the expected parent-attention saving exceeds cheap-helper cost; raw evidence stays addressable |
| P0 | Structured task capsule | Build | The active conversation retains repeated handles and resolved diagnostics | Maintain a bounded goal, current state, verified facts, evidence references, unresolved risks and one next action |
| P0 | Evidence-gated completion | Build | Three incorrect final filesystem states were reported as complete | Fresh contradictory evidence overrides `done`; completion requires the task's applicable acceptance contract |
| P0 | Final-state contract checker | Build | Two stray binaries and one misplaced gcov tree caused all genuine failures | Check required paths, forbidden artifacts, directory contents, tests, installation state and working-tree state before completion |
| P1 | Fresh independent checker | Build | A checker inheriting the working narrative can inherit its mistaken assumptions | A cheap fresh context sees the original request, current diff/state and exact evidence, but not the parent's rationale |
| P1 | Preflight Helper | Redesign | Some preflights tried to complete an executable task and merely reported that they could not run commands | Preflight scouts constraints, files, tests, capabilities and risks; it never impersonates the acting parent |
| P1 | No-progress guard | Build | Denied or ineffective actions can recur without changing strategy | Detect repeated intent, denial, unchanged diff and unchanged failing evidence, then choose another method or escalate clearly |
| P1 | Verified checkpoint | Build | Later work can disturb a previously correct intermediate state | Preserve the newest verified state and prevent an unverified finalizer from silently replacing it |
| P1 | Cut-off salvage | Build | A timeout or output limit can discard useful partial reasoning | A cheap bounded summarizer writes established facts and unresolved work into the task capsule without claiming completion |
| P1 | Adaptive orchestration | Redesign | Every benchmark task paid for preflight whether or not it helped | Direct execution remains the fast path; helpers and workers activate from complexity, uncertainty, failure or verification signals |
| P0 | Recovery-cost attribution | Build | Failure counts do not reveal the Sol usage actually caused by recovery | Attribute parent requests, tokens and wall time to implementation, exploration, verification and repair causes |
| P0 | Interface ablation runner | Build | One hybrid campaign cannot prove the Tool ABI's advantage | Run the same model, task, limits and environment in tools-only, cells-only and hybrid modes |

## GVS5H concepts worth adopting

[GVS5H](https://github.com/slee-persis/GVS5H) studies a deliberately simple
ledger-based manager/worker scaffold. Its reported gains are research evidence,
not a Pane result: it evaluates LiveCodeBench algorithm problems, adds
substantial test-time compute, and reports cases where orchestration is flat or
harmful. The transferable concepts are nevertheless well aligned with Pane:

| GVS5H mechanism | Pane-native form |
|---|---|
| Shared `task.md`, `plan.md`, `notes.md`, current solution | Typed task capsule backed by the existing ledger and artifact handles |
| Fresh model context for each worker | Narrow Helper/subagent context containing the capsule and one task, not the transcript |
| Manager curates exactly one next task | Cheap curator updates one bounded next-action record after meaningful state changes |
| Public test verdict overrides a worker's `done` | Fresh Pane evidence overrides unsupported completion claims |
| Repeated-task no-progress guard | Compare canonical intent, diff hash and evidence outcome before retrying |
| Cut-off attempt summarizer | Salvage only established facts and unfinished work into the capsule |
| Finalizer skipped when a usable solution is already accepted | Never let a redundant final pass overwrite a verified state |
| Proposed fresh-perspective worker | Independent task-plus-diff checker without inherited notes or rationale |

Do not copy the Markdown-file protocol or make every task a manager loop. Pane
already has typed state, exact evidence, deterministic tools, Helpers and
subagents. Adopt the control principles inside those mechanisms. The intended
flow is:

```text
familiar action
    -> canonical execution
    -> exact evidence
    -> compact task-state update
    -> cheap independent verification when warranted
    -> expensive parent receives only the next real decision
```

## Economic definition of success

Use observed subscription credit debits when the provider exposes them. When
it does not, report a declared weighted model-equivalent estimate separately
from raw tokens:

```text
weighted spend = Sol spend + rL * Luna spend + rT * Terra spend
quality-adjusted spend = weighted spend / verified successful trials
```

`rL` and `rT` are the observed or explicitly assumed credit ratios relative to
Sol. Cache-read, ordinary input and output classes remain separate because they
need not have the same price. Never present an assumed ratio as a provider bill.

The product scorecard is:

| Dimension | Success condition |
|---|---|
| Correctness | Predeclared non-inferiority comparison on matched tasks with enough trials; no aggregate-to-subset substitution |
| Expensive-parent attention | Lower Sol requests, new input and recovery usage per verified pass |
| Weighted economics | Lower declared weighted spend per verified pass, even when cheap-helper raw tokens rise |
| Context hygiene | Fewer repeated observation bytes and no resolved diagnostic replay |
| Tool dependability | Zero predictable capability denials and zero runtime-caused mutation conflicts after preflight |
| Completion integrity | No false completion on the three known final-state failure fixtures |
| Wall time | Competitive task latency after verification, reported independently of summed worker time |

An initial engineering target is a 25--30% reduction in Sol usage per verified
pass without reducing correctness. It is a target for the next matched
experiment, not a present product claim.

## Work and measurement order

1. Repair TypeScript, capability-profile, direct-facade and mutation semantics.
2. Add invocation-origin and recovery-cost telemetry before optimizing.
3. Replace repeated handle inventories with delta observations and lifecycle.
4. Add the task capsule, evidence-gated completion and final-state checker.
5. Add fresh cheap checking, no-progress handling and adaptive orchestration.
6. Replay the known polyglot and gcov failures as targeted canaries.
7. Run matched tools-only, cells-only and hybrid trials.
8. Start a larger competitive benchmark only after the canaries and interface
   measurement are sound.

This order preserves the core Tool ABI thesis: the parent may choose familiar
direct tools or composed cells, but Pane must make either choice smarter than
the raw provider interaction and must prove the saving in expensive attention.
