# Pane Tool ABI — Provider-Familiar Tool Shapes over Pane Execution

Status: proposed implementation spec

## Thesis

Pane should separate the interface presented to a model from the mechanism used to execute that interface.

A provider may see familiar coding-agent tools such as `read_file`, `edit_file`, `grep`, `glob`, `run_command`, and `run_tests`, while Pane compiles those calls into a canonical internal intent and decides how to execute them using Pane primitives, cells, deterministic projections, reducers, and the event ledger.

The model-facing tool schema is an ABI. It is not the execution plan.

```text
Provider-visible tool call
        │
        ▼
Provider Tool ABI
        │
        ▼
Canonical Pane Intent
        │
        ▼
Deterministic Router
        │
        ├── primitive
        ├── composed cell
        ├── deterministic projection
        └── execution + reducer
        │
        ▼
Pane runtime
        │
        ▼
Event ledger + evidence
        │
        ▼
Provider-shaped result
```

The goal is:

> Exploit learned tool-use priors at the model boundary while preserving Pane's deterministic execution, composition, evidence, validation, and reduction semantics underneath.

## Why this exists

Modern coding models have post-training priors around familiar tool interfaces and loops: tool names, argument structures, call sequencing, read-before-edit behavior, search patterns, command failure recovery, filesystem errors, and stopping behavior.

Pane's cell model has different strengths: composition, fewer inference round trips, local control flow, deterministic transformation, validation, and batching.

These are complementary. Pane should not force every provider to learn a novel execution API when the provider already has useful learned priors for conventional coding tools.

The desired property is:

> Familiar to the model; native to Pane underneath.

## Governing invariants

1. **The provider-visible tool is a semantic request, not an implementation choice.**
2. **Provider-specific schemas must compile into provider-neutral Pane intents.**
3. **Routine routing must be deterministic and based on mechanically observable facts.**
4. **Execution semantics and presentation semantics are separate.**
5. **Exact evidence, bounded exact evidence, and derived content must never be conflated.**
6. **A reducer must never masquerade as an exact primitive result.**
7. **Every model-visible tool invocation must be represented in the Pane event ledger.**
8. **Completion checks must use observed execution evidence, not the model's narrative about what happened.**
9. **Cells remain first-class; provider-shaped tools are an additional front end, not a replacement.**
10. **Correctness must not depend on a learned router. Learned routing may optimize later, but the deterministic path remains authoritative.**

## Architecture

### 1. Provider Tool ABI

Each provider adapter may expose tool schemas aligned with the model family's familiar interaction shape.

Examples:

```text
Claude adapter
GPT adapter
Gemini adapter
Generic adapter
```

Adapters may differ in:

- tool names;
- parameter names;
- descriptions;
- optional parameters;
- result formatting;
- error formatting.

They must not create provider-specific execution semantics.

Example:

```text
Claude-shaped edit ─┐
GPT-shaped patch   ─┼──> EditArtifact
Generic replace   ─┘
```

### 2. Canonical Pane Intent IR

All provider calls compile into a small, stable internal representation.

Illustrative shape:

```ts
type Intent =
  | ReadArtifact
  | SearchRepository
  | EditArtifact
  | ExecuteCommand
  | RunCheck
  | ListArtifacts
  | InspectProcess
  | ComposeOperations;
```

Example:

```ts
interface ReadArtifact {
  kind: "read_artifact";
  target: ArtifactRef;
  range?: ByteRange | LineRange;
  semantics: "exact";
}
```

The IR is Pane's internal contract. Provider schemas may change without changing execution. Execution strategies may change without changing provider schemas.

### 3. Deterministic execution router

Pane chooses the execution strategy from canonical intent plus facts available to the host.

Valid routing inputs include:

- file size;
- line count;
- match count;
- requested range;
- command class;
- output size;
- exit status;
- artifact type;
- cache state;
- process state;
- dependency structure;
- previous execution evidence;
- provider context/output limits;
- configured thresholds.

Do not route on vague concepts such as "this looks complicated" unless a semantic operation explicitly requires model reasoning.

Example:

```text
ReadArtifact
    │
    ├── small file
    │      └── exact primitive
    │
    ├── medium file
    │      └── bounded exact projection + continuation handle
    │
    └── very large file
           └── exact artifact handle + deterministic projection/index
```

The router should optimize execution and context use without changing the semantic contract requested by the model.

## Critical separation: execution vs presentation

Pane must distinguish:

```text
what happened
```

from:

```text
what was shown to the model
```

Example: `run_tests` produces 20 MB of exact stdout.

Pane may execute the command exactly, retain the full artifact, and present only the failure-relevant portion to the model.

```ts
{
  execution: {
    exact: true,
    exitCode: 1,
    stdoutHandle: "artifact://exec/82/stdout"
  },
  presentation: {
    source: "derived",
    reducer: "test-output-v2"
  }
}
```

```text
EXACT EXECUTION
      │
      ▼
20 MB observed output
      │
      ├──────────────> ledger / artifact store
      │
      ▼
derived presentation
      │
      ▼
model
```

The reducer changes the model's view. It must not rewrite history about what Pane observed.

## Provenance contract

Every non-trivial result must expose machine-readable provenance.

Minimum classes:

```ts
type EvidenceClass =
  | "exact"
  | "bounded_exact"
  | "derived";
```

### `exact`

The returned content directly represents the observed result.

### `bounded_exact`

The returned content is an exact subset of a larger observed result, for example a line range, byte range, or first N matches. Omitted data remains addressable through a handle or continuation.

### `derived`

A semantic transformation occurred, for example summarization, classification, grouping, explanation, or model-generated reduction.

Example envelope:

```ts
{
  source: "derived",
  content: "...",
  evidence: {
    handle: "artifact://a81",
    exactBytes: 18422931,
    reducer: "test-log-reducer-v2"
  }
}
```

The model must always be able to distinguish ground truth from derived interpretation.

## Reducer invariant

A reducer must never silently satisfy a stronger semantic request with weaker evidence.

Invalid:

```text
Model: read_file("architecture.md")
Pane:  <LLM-generated summary presented as file contents>
```

Valid:

```text
Model: read_file("architecture.md")
Pane:  source=derived
       exact artifact=artifact://f19
       representation=<semantic reduction>
```

Prefer bounded exact data where it preserves the requested semantics:

```text
source=bounded_exact
lines=1-500
continuation=artifact://f19#501
```

Semantic reduction should be used only when it provides material value and its provenance remains explicit.

## Cells remain first-class

Provider-shaped tools must not replace `execute_cell` or equivalent Pane composition.

Tool-shaped mode optimizes for:

- learned provider familiarity;
- low instruction burden;
- conventional error recovery;
- simple routine actions.

Cell mode optimizes for:

- composition;
- local branching/control flow;
- fewer inference round trips;
- deterministic transformation;
- multi-operation execution.

The intended architecture is a superset:

```text
provider-familiar tools + Pane cells
```

not one or the other.

## Tool-call fusion

Pane should eventually separate model interaction granularity from runtime execution granularity.

If a provider emits several independent conventional tool calls:

```text
read_file(A)
read_file(B)
grep(C)
```

Pane may compile them into one internal execution unit where the provider protocol and dependency graph permit it:

```text
Cell 814
 ├── read(A)
 ├── read(B)
 └── grep(C)
```

This preserves familiar interaction while recovering cell-level efficiency.

Fusion is an optimization. It must preserve ordering, dependency, error, and evidence semantics.

## Adaptive execution examples

A stable model-facing capability may use different native mechanisms underneath.

```text
SearchRepository
       │
       ├── literal symbol
       │      └── rg
       │
       ├── language symbol
       │      └── AST / language index
       │
       ├── filename query
       │      └── fd / index
       │
       └── huge ambiguous result
              └── exact search artifact + reduction
```

The provider sees a stable search capability. Pane is free to improve implementation underneath it.

## Event ledger requirements

Every provider-visible tool invocation must correspond to ledger events sufficient to establish what really happened.

Illustrative sequence:

```text
ToolRequestReceived
IntentCompiled
ExecutionStrategySelected
ExecutionStarted
ArtifactObserved
ReductionApplied        # optional
ToolResultReturned
```

The ledger must make it possible to distinguish at least:

- requested vs executed;
- fresh execution vs reused evidence;
- exact vs bounded vs derived output;
- successful execution vs presentation success;
- execution failure vs adapter/serialization failure.

The model's claim that a tool ran is never authoritative evidence that it ran.

## Provider-aware façades

Pane may expose different façades per model family:

```text
                    Pane Capability ABI
                           │
             ┌─────────────┼──────────────┐
             │             │              │
         Claude ABI      GPT ABI       Generic ABI
             │             │              │
          Claude          GPT-X         Any model
```

All must compile into the same canonical Pane IR.

Provider adapters should remain thin. Provider-specific behavior belongs at the boundary, not inside core execution.

## Capability negotiation

Pane should expose only capabilities that are actually available in the current runtime.

Conceptually:

```text
Runtime capabilities
        +
Provider adapter
        +
Policy
        =
Model-visible schema
```

Example native capability set:

```text
filesystem.read
filesystem.edit
repo.search
process.exec
checks.run
```

The generated provider schema should be derived from this set rather than maintained as a separate universal prompt contract.

## Tool schemas as prompt compression

Operational knowledge expressible as typed schemas should not be repeated as natural-language folklore in the system prompt.

A provider-aligned tool schema can provide both:

```text
explicit structural guidance
+
learned behavioral prior
```

while consuming far fewer prompt tokens than prose instructions describing the same calling convention.

## Error semantics

Errors should be canonical internally and may be rendered in a provider-familiar form at the boundary.

Canonical example:

```ts
{
  kind: "artifact_not_found",
  target: "src/a.ts",
  recoverable: true
}
```

Adapters may format this differently, but the underlying fact and ledger evidence remain identical.

This matters because provider post-training may include recovery behavior for familiar failure classes, not only successful tool calls.

## Trust ordering

Pane must preserve this authority order:

```text
observed execution evidence
        >
deterministic checks
        >
derived analysis
        >
model claims
```

Provider familiarity must never weaken this hierarchy.

## Non-goal

Pane is not attempting to impersonate another product's undocumented implementation.

The objective is not:

```text
pretend to be Claude Code / Codex / another harness
```

The objective is:

```text
present stable, recognizable capability schemas
while compiling them into Pane semantics
```

The architecture must remain valid even when no provider-specific learned prior exists.

## Example end-to-end flow

Model emits:

```json
{
  "name": "read_file",
  "input": {
    "path": "target/test.log"
  }
}
```

Provider adapter compiles:

```text
ProviderReadFile
      ↓
ReadArtifact(path=target/test.log, semantics=exact)
```

Router observes:

```text
size = 18.4 MB
kind = textual log
direct display limit = 128 KB
```

Execution obtains the exact artifact:

```text
artifact://93
```

Presentation policy may then choose:

```text
exact artifact retained
+ deterministic failure extraction
+ semantic reducer only if needed
```

The model receives, for example:

```text
source: derived
underlying: artifact://93
size: 18.4 MB
representation: 21 relevant failures with surrounding context
```

The ledger records the exact observation and the fact that the visible representation was derived.

If the model later claims "the full log contains only those 21 failures", Pane can mark that claim unsupported because the model did not inspect exhaustive exact evidence.

## Routing policy

Initial routing should remain simple and deterministic:

```text
IF result <= direct_limit:
    exact
ELSE IF bounded exact projection preserves requested semantics:
    bounded_exact
ELSE IF operation naturally produces large output:
    exact execution
    + persistent artifact
    + deterministic reduction
ELSE IF semantic reduction was explicitly requested:
    derived reducer
ELSE:
    expose handle / pagination
```

A learned router may later optimize routing, but must not be required for correctness.

## MVP

Implement the smallest falsifiable version first.

Expose six familiar capabilities:

```text
read_file
edit_file
grep
glob
run_command
run_tests
```

Implement only:

```text
Provider ABI
    ↓
Canonical Intent IR
    ↓
Existing Pane primitives/cells
    ↓
Event ledger
    ↓
Provider-shaped result
```

For the first experiment, do **not** add a semantic router. The purpose of MVP is to isolate whether familiar tool shapes themselves reduce invalid calls, repair turns, and instruction cost.

Then add, separately and measurably:

1. bounded exact results;
2. artifact handles;
3. deterministic projections/reduction;
4. semantic reduction with provenance;
5. tool-call fusion;
6. provider-specific façades beyond the first adapter.

Each step should have evidence independent of the previous one.

## Benchmark hypothesis

Primary hypothesis:

> Provider-familiar tool façades reduce invalid calls, repair turns, and instruction tokens compared with Pane-native cells alone, while preserving Pane's execution and evidence guarantees.

Compare at least:

```text
A — Pane cells only
B — provider-shaped tools only
C — hybrid provider tools + cells
D — hybrid + adaptive presentation/reduction
```

Measure:

- task success;
- provider requests;
- total input/output tokens;
- instruction/system tokens where observable;
- failed calls;
- repair turns;
- tool calls;
- cells executed;
- operations per inference turn;
- wall time;
- human intervention;
- unsupported completion claims;
- reducer use;
- exact vs derived evidence consumption.

The likely target architecture is C or D, not B.

Where possible, compare the same underlying model with and without the provider-familiar Pane ABI so the harness effect is isolated from model quality.

## Acceptance criteria for the first implementation slice

The MVP is complete only when all of the following are demonstrated:

1. A provider-facing `read_file` call compiles to a canonical Pane read intent and executes through the existing runtime.
2. At least one mutating tool (`edit_file`) compiles through the same boundary without introducing a second execution path.
3. At least one process/check tool (`run_tests` or `run_command`) records request, execution, result, and provenance in the event ledger.
4. Provider-specific schema code does not leak into core Pane execution logic.
5. Existing cell execution remains available and unchanged as a first-class path.
6. The parent can distinguish fresh execution from reused evidence.
7. The parent can distinguish exact, bounded exact, and derived results.
8. A derived result cannot be accepted as proof of an exact-content claim without additional evidence.
9. Existing Pane gates remain green.
10. A recorded comparison exists between cells-only and hybrid mode on at least the current independent oracle/fixture suite, with provider requests, tokens, failures, and wall time captured.

## Implementation guidance for Claude Code

Treat this document as a design constraint, not permission for a broad rewrite.

Before implementation:

1. Read `CLAUDE.md`.
2. Read `docs/product/pane/model-contract.md`.
3. Read `docs/product/pane/runtime-contract.md`.
4. Read `docs/product/pane/events-contract.md`.
5. Read `docs/product/pane/improvement-register.md`.
6. Locate the current provider adapter boundary, cell execution entry point, primitive dispatch, checker/evidence path, and event ledger implementation.
7. Reconcile this spec with existing contracts. Prefer the smallest extension that preserves current semantics.

Implementation rules:

- do not build a second execution engine for familiar tools;
- compile provider calls onto existing Pane capabilities;
- keep the canonical IR provider-neutral;
- keep provider adapters thin;
- do not silently summarize or reduce exact reads;
- do not let a reducer's prose become evidence;
- keep exact artifacts reachable when a derived view is returned;
- route from mechanical facts first;
- preserve cell composition;
- add tests at the contract boundary, not only unit tests of helper functions;
- record benchmark evidence before claiming efficiency improvement;
- if existing architecture already provides an equivalent abstraction, adapt this spec to it rather than duplicating it.

The first implementation should optimize for proving or falsifying the architecture, not completing every provider adapter or every optimization in this document.
