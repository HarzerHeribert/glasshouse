# Pane Tool ABI and Adaptive Capability Execution

Status: implementation spec

## Thesis

Pane should let a model work with the coding tools and interaction shapes it already knows, while compiling those familiar calls into a substantially stronger Pane execution substrate.

The provider-facing tool is an ABI. It is not the execution plan.

The defining idea is:

> **Pane makes provider-familiar tools composable as code, then executes them through one deterministic-first capability kernel that can escalate to bounded Little Helpers only when semantic work has a clear advantage.**

The model should feel as though it is using unusually good versions of familiar tools. It should not need to learn a second Pane-specific tool universe merely to access Pane's stronger runtime semantics.

```text
                     PROVIDER MODEL
                           │
                  learned tool prior
                           │
             ┌─────────────┴─────────────┐
             │                           │
       familiar direct tools        execute_cell
       cheap / conventional         programmable composition
             │                           │
             └─────────────┬─────────────┘
                           ▼
                    CAPABILITY ABI
                           │
                    canonical intent
                           │
                           ▼
                      CELL IR
               universal execution form
                           │
                           ▼
                 PANE EXECUTION KERNEL
          ┌────────────────┼────────────────┐
          │                │                │
   deterministic       composed         bounded
     primitives          logic       Little Helpers
          │                │                │
          └────────────────┼────────────────┘
                           ▼
                  artifacts / handles
                           ▼
                        ledger
                           ▼
                       verifier
```

The desired property is:

> **Familiar to the model; Pane-grade underneath.**

---

## 1. One architecture, multiple invocation forms

Pane MUST NOT implement a native-tool executor beside a cell executor.

There is one capability system and one execution path.

A provider-native tool call and a model-authored TypeScript cell are merely two ways to produce work for the same capability kernel.

```text
native provider tool call
          │
          └────── compile ──────┐
                                ▼
model-authored TypeScript ──> CELL IR ──> one executor
```

A direct call such as:

```text
Read({ file_path: "src/runtime.rs" })
```

may internally be represented as the equivalent single-operation cell program:

```ts
const result = await Read({ file_path: "src/runtime.rs" });
return result;
```

A composed operation may instead be authored explicitly by the parent model:

```ts
const config = await Read({ file_path: "src/config.ts" });

if (config.content.includes("legacyAuth")) {
  const hits = await Grep({
    pattern: "legacyAuth",
    path: "src"
  });

  if (hits.count > 20) {
    return await Bash({ command: "cargo test auth" });
  }
}

return config;
```

Both forms MUST reach the same capability implementation, the same event ledger, the same evidence system, and the same policy controls.

There must never be a situation where `Read` called directly behaves according to one implementation while `Read` inside a cell behaves according to another.

---

## 2. Hybrid is the product target

The target Pane interface is hybrid from the beginning.

The parent model SHOULD receive:

- provider-familiar direct tools for single-intent operations; and
- `execute_cell` as Pane's composition primitive for dependent work, branching, loops, batching, speculative local execution, local transformations, and custom TypeScript logic.

The distinction is semantic:

```text
simple single intent
    → direct familiar tool

dependent or composed intent
    → execute_cell
```

`execute_cell` is not a legacy escape hatch and MUST NOT be de-emphasized. It is Pane's programmable composition surface.

The model can therefore use familiar operations directly:

```text
Read
Grep
Edit
Bash
```

or compose those same operations in TypeScript:

```ts
const files = await Glob({ pattern: "src/**/*.ts" });

for (const file of files.paths) {
  const source = await Read({ file_path: file });

  if (source.content.includes("oldApi")) {
    await Edit({ /* exact edit intent */ });
  }
}

return await RunTests({ scope: "project" });
```

The provider should not need to learn different semantic names merely because it moved from a direct call into a cell.

---

## 3. Runtime interface modes are visibility policies only

Pane MAY expose runtime modes for benchmarking, compatibility, diagnostics, and ablation tests:

```text
--interface=cells
--interface=tools
--interface=hybrid
```

These modes MUST NOT select different execution architectures.

### `--interface=cells`

The parent sees only `execute_cell`.

The cell runtime still exposes the provider dialect's familiar capability bindings.

### `--interface=tools`

The parent sees only provider-familiar direct tools.

Each call still compiles into the same Cell IR / capability kernel.

### `--interface=hybrid`

The parent sees both provider-familiar tools and `execute_cell`.

This is the intended product mode and SHOULD become the default once proven stable.

The runtime flag changes only which entry points are visible to the parent model.

It must not fork execution semantics.

---

## 4. Capability descriptors are the single source of truth

Pane SHOULD define each capability once and generate all model-facing forms from the same descriptor.

Illustrative form:

```ts
interface CapabilityDescriptor<Intent, Result> {
  id: string;

  providerShapes: {
    anthropic?: ProviderToolShape;
    openai?: ProviderToolShape;
    generic?: ProviderToolShape;
  };

  decodeProviderInput(provider: Provider, input: unknown): Intent;
  encodeProviderResult(provider: Provider, result: Result): unknown;

  cellBinding: CellBindingDescriptor;
  validate(intent: Intent): ValidationResult;
  execute(intent: Intent, ctx: ExecutionContext): Promise<Result>;
}
```

From one descriptor Pane should be able to derive:

```text
provider-native JSON/tool schema
TypeScript declaration
cell callable binding
argument validation
canonical intent decoding
result encoding
error encoding
documentation/help text
ledger capability identity
```

There MUST NOT be separately maintained native-tool semantics and cell-tool semantics.

---

## 5. Provider dialects

Pane should deliberately present different familiar façades to different provider families when doing so aligns with their learned tool-use prior.

The internal capabilities remain provider-neutral.

```text
                         Pane capabilities

     ReadArtifact   Search   Edit   Execute   Check   List
          │            │       │       │        │      │
          ├────────────┴───────┴───────┴────────┴──────┤
          │                                             │
   Anthropic dialect                               OpenAI dialect
          │                                             │
       Read / Grep                                  shell-like
       Glob / Edit                                  apply_patch-like
       Write / Bash                                 provider-familiar forms
```

The exact provider shape should follow the public provider protocol and the shapes current models are known to handle well. Pane should not invent gratuitous symmetry between providers.

The provider adapters are skins over the same capabilities, not provider-specific backends.

### Anthropic / Claude-oriented initial dialect

The first-class local coding surface SHOULD cover familiar concepts equivalent to:

```text
Read
Grep
Glob
Edit
Write
Bash
RunTests / Check where Pane can expose a useful explicit verification capability
execute_cell
```

### OpenAI / Codex-oriented initial dialect

The first-class surface SHOULD preserve familiar shell and patch semantics where those are the model's stronger learned interface:

```text
shell
apply_patch
explicit Pane verification/check capability where beneficial
execute_cell
```

Read/search/list operations MAY be expressed through the provider-familiar shell shape when that better matches the model prior, while still being recognized and lowered to canonical Pane capabilities when mechanically safe.

For example:

```text
shell: rg "SessionManager" crates/
        │
        ▼
mechanically recognized safe command shape
        │
        ▼
SearchRepository intent
```

A complex or unrecognized shell expression MUST fall back to normal sandboxed process execution rather than being heuristically reinterpreted.

Rule:

> **Translate when semantics are mechanically provable; otherwise execute the requested generic capability faithfully.**

---

## 6. Canonical Pane Intent IR

All provider-facing forms and all cell bindings compile to provider-neutral semantic intents.

Illustrative set:

```ts
type Intent =
  | ReadArtifact
  | SearchRepository
  | ListArtifacts
  | EditArtifact
  | WriteArtifact
  | ApplyPatch
  | ExecuteCommand
  | RunCheck
  | InspectProcess;
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

The canonical IR is Pane's stable semantic contract.

Provider schemas may change without changing execution.

Execution strategies may improve without changing provider schemas.

---

## 7. Cell IR is the universal execution substrate

All executable work SHOULD lower into the same Cell IR or equivalent existing Pane execution representation before reaching the kernel.

The IR must support at least:

- sequential dependency;
- parallel independent operations;
- `if` / `else` branching;
- loops;
- local variables;
- local deterministic transformation;
- capability invocation;
- assertions / checks;
- artifact handles;
- bounded helper invocation through capabilities;
- early return;
- structured errors;
- execution budgets.

This is how Pane retains the advantage that ordinary provider tool calling cannot offer for dependent operations inside one parent inference turn.

Native parallel tool calls can express independent work. Cells can additionally express dependent control flow:

```text
Read A
  ↓
if result contains X
  ├── yes → Search B → maybe Edit C
  └── no  → Run D
```

without requiring the parent model to receive an intermediate observation after each edge.

---

## 8. Three execution tiers

Each capability should follow a deterministic-first escalation model.

```text
Tier 0 — deterministic primitive
Tier 1 — deterministic composed execution / projection
Tier 2 — bounded semantic Little Helper
```

The preference order is:

```text
L0 → L1 → L2
```

not the reverse.

Normative rule:

> **Pane MUST prefer deterministic execution whenever it can satisfy the requested semantic contract adequately. Helper inference MUST require an identifiable expected advantage.**

A Little Helper is escalation, not default behavior.

Valid reasons to escalate include:

- output is too large for useful direct parent consumption;
- semantic relevance cannot be obtained adequately through deterministic projection;
- ambiguous failures require interpretation;
- structured semantic grouping materially reduces parent work;
- the capability contract explicitly requests semantic interpretation.

Invalid reasons include:

- "an LLM might make this nicer";
- arbitrary aesthetic preference;
- replacing cheap exact operations with semantic guesses;
- silently repairing mutation intent.

---

## 9. Little Helpers remain bounded semantic functions

Pane's existing Little Helper / reducer mechanism is a core architectural feature and SHOULD be part of the complete Tool ABI implementation.

Helpers run out-of-turn relative to the parent and may be called from inside capability execution, including while a model-authored cell is running.

They MUST remain bounded to their assigned semantic task and existing anti-scope-creep controls MUST continue to apply.

Conceptually:

```ts
interface HelperInvocation {
  intent: HelperIntent;
  sourceArtifacts: ArtifactRef[];
  requestedOutputSchema: Schema;
  tokenBudget: number;
  wallTimeBudget: number;
  evidenceScope: EvidenceScope;
}
```

A helper result is always derived evidence:

```ts
interface HelperResult<T> {
  source: "derived";
  result: T;
  basedOn: ArtifactRef[];
  helperExecutionId: string;
}
```

A helper may interpret evidence. It does not become ground truth merely because it was invoked inside a tool.

The parent must always be able to distinguish exact evidence from helper interpretation.

---

## 10. Execution and presentation are separate

Pane MUST distinguish:

```text
what happened
```

from:

```text
what was shown to the model
```

Example: `RunTests` may execute a command that produces 20 MB of stdout.

Pane can retain the exact stdout artifact while presenting a much smaller useful view.

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
projection / helper interpretation
      │
      ▼
model
```

The reduction changes the parent working set. It MUST NOT rewrite what Pane actually observed.

---

## 11. Provenance contract

Every non-trivial result MUST expose machine-readable provenance.

Minimum evidence classes:

```ts
type EvidenceClass =
  | "exact"
  | "bounded_exact"
  | "derived";
```

### `exact`

The returned content directly represents the complete observed result within the requested semantic scope.

### `bounded_exact`

The returned content is an exact subset of a larger observation, such as a line range, byte range, or subset of exact search matches.

Omitted data remains addressable through a handle or continuation.

### `derived`

A semantic transformation occurred, including:

- summarization;
- clustering;
- classification;
- diagnosis;
- explanation;
- relevance ranking requiring model judgment;
- any Little Helper output.

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

A derived result MUST NOT be accepted as proof of a stronger exact-content claim without supporting exact or sufficiently bounded exact evidence.

---

## 12. Artifact handles

The Tool ABI MUST preserve Pane's handle/context-efficiency advantage.

Large results should not be forced inline merely because they originated from a provider-native tool call.

A direct tool result may return:

- useful inline exact or bounded exact content;
- provenance metadata; and
- a stable artifact reference to the complete underlying observation.

The artifact identifier is not a fabricated model-local variable name.

The model may later bind it inside a cell explicitly:

```ts
const log = await Artifact.open("artifact://exec/82/stdout");
```

or use the existing Pane handle mechanism that best fits current runtime contracts.

Pane MUST reconcile this with the existing no-server-invented-model-identifier rule rather than creating a parallel naming system.

---

## 13. Tool behavior under the hood

The implementation should build the complete core behavior of each supported tool, not a translation-only façade.

The following contracts describe the intended behavior.

### 13.1 Read / ReadArtifact

Ground truth: artifact bytes / text.

Deterministic path:

1. resolve target safely;
2. inspect mechanical size/type facts;
3. read exact requested range or complete artifact where reasonable;
4. retain a handle for larger content;
5. return `exact` or `bounded_exact` data;
6. preserve continuation/addressability for omitted exact content.

Helper escalation MAY add a derived semantic view when there is a clear advantage, especially for very large structured text or logs.

The helper MUST NOT replace exact evidence invisibly.

A huge file may therefore produce:

```text
exact artifact retained
+ bounded_exact excerpt/index
+ optional derived helper interpretation
```

not:

```text
LLM summary pretending to be file contents
```

### 13.2 Grep / SearchRepository

Ground truth: exact search matches.

Deterministic path:

1. select the strongest mechanically appropriate search implementation available (`rg`, index, AST-backed lookup, or equivalent);
2. retain complete exact match evidence when result volume exceeds parent presentation limits;
3. return exact or bounded exact matches plus a handle;
4. expose match count and truncation/projection metadata.

Helper escalation is especially valuable for very large or semantically noisy result sets.

A helper MAY:

- cluster matches by semantic role;
- rank likely implementation relevance;
- separate generated/vendor/test/documentation noise;
- summarize families of matches.

Those outputs MUST be marked `derived` and linked to the exact match artifact.

### 13.3 Glob / ListArtifacts

Ground truth: exact path set for the requested search semantics.

Deterministic path:

- filesystem/index lookup;
- exact path filtering;
- bounded exact presentation when large;
- stable handle to full path set.

Helper escalation should be rare and only used when a huge result set requires semantic grouping or task-relevance classification.

### 13.4 Edit / EditArtifact

Ground truth: requested mutation and observed resulting diff.

Deterministic path:

1. validate target and expected old state;
2. reject stale or ambiguous mutation requests;
3. apply exact requested edit atomically where possible;
4. observe resulting filesystem state/diff;
5. record mutation evidence in the ledger.

A Little Helper MUST NOT silently alter mutation intent.

If an exact edit fails, a helper MAY provide a `derived` repair candidate or diagnosis, but applying a materially different mutation requires a new explicit intent from the parent/model-authored cell logic according to current Pane safety rules.

### 13.5 Write / WriteArtifact

Ground truth: requested content and resulting file state.

Deterministic path:

- validate location/policy;
- perform explicit write;
- observe resulting content/metadata;
- record exact mutation evidence.

Helpers should generally not rewrite requested content behind the parent's back.

### 13.6 apply_patch / ApplyPatch

Ground truth: supplied patch plus observed repository mutation.

Deterministic path:

- parse patch;
- validate target state;
- apply or reject;
- capture exact resulting diff;
- preserve failure diagnostics.

A helper MAY interpret a rejected patch and produce bounded repair advice, but MUST NOT silently mutate the patch into a different change.

### 13.7 Bash / shell / ExecuteCommand

Ground truth: actual process execution, exit status, stdout/stderr, and process metadata.

Deterministic path:

1. apply sandbox/policy controls;
2. recognize mechanically provable specialized command forms where beneficial;
3. lower recognized operations into canonical Pane capabilities when semantics are preserved exactly;
4. otherwise execute the requested command faithfully in the sandbox;
5. retain stdout/stderr artifacts;
6. return useful exact/bounded exact output and execution metadata.

Large or ambiguous failure output is a prime helper candidate.

A helper MAY diagnose failures or group errors. Its interpretation remains `derived` while exit code/stdout/stderr remain exact evidence.

### 13.8 RunTests / RunCheck

Ground truth: observed checker/test execution and its results.

Pane SHOULD integrate with the existing checker/evidence system rather than creating a second test runner.

The result must preserve at least:

```text
requested
executed
fresh vs reused
exit/result status
observed evidence
artifact references
presentation provenance
```

The parent must continue to distinguish:

```text
executed=true, reused=false
```

from:

```text
executed=false, reused=true
```

Large test logs MAY invoke helpers for diagnosis, while the exact execution record remains authoritative.

### 13.9 execute_cell

`execute_cell` is the composition primitive over the same capabilities.

It MUST support provider-familiar capability bindings inside TypeScript and preserve:

- dependent control flow;
- branching;
- loops;
- batching;
- local data transformation;
- parallel independent calls;
- helper-capable nested capability execution;
- ledger visibility for every nested operation;
- execution/helper budgets.

A cell is allowed to be significantly more expressive than provider-native tool calling. That is the point.

---

## 14. Direct tools and cells share the same familiar vocabulary

Where practical, the provider dialect should be available inside the TypeScript cell using the same semantic names and argument shapes the parent sees directly.

Example for an Anthropic-oriented dialect:

```text
Direct:
Read({ file_path: "src/foo.ts" })

Cell:
await Read({ file_path: "src/foo.ts" })
```

The cell adds ordinary programming constructs without changing the conceptual tools:

```text
if / else
for / while
Promise.all
variables
map/filter/reduce
local parsing
assertions
custom deterministic transformations
```

This turns familiar tools into a programmable tool language instead of forcing the model to learn a separate Pane-specific API for composition.

---

## 15. Little Helpers inside cells

A capability invoked from a cell MAY itself escalate to a Little Helper when the deterministic-first policy permits it.

Example:

```text
Parent inference
      │
      ▼
execute_cell
      │
      ├── deterministic Read
      ├── deterministic Grep
      ├── local if/else
      ├── deterministic Edit
      └── RunTests
              │
              └── huge ambiguous failure log
                       ↓
                  Little Helper
                       ↓
              bounded derived diagnosis
      │
      ▼
Parent inference continues
```

This is nested inference inside deterministic control flow without forcing every intermediate result back through the parent model.

Existing helper scope/budget controls MUST apply equally whether the capability was called directly or from a cell.

---

## 16. Helper budgets and recursion controls

Because cells can invoke helper-capable capabilities in loops, helper execution MUST remain explicitly budgeted.

The existing controls should be reused where available and extended only where necessary to cover at least:

```text
max helper calls per cell / execution frame
max helper token budget
max helper wall time
max helper concurrency
max nested helper depth
scope restrictions
artifact/evidence visibility restrictions
```

A construct such as:

```ts
for (const file of files) {
  await Read({ file_path: file });
}
```

must not accidentally trigger hundreds of uncontrolled LLM calls merely because each read is individually helper-eligible.

Routing should account for aggregate execution context and remaining helper budget.

---

## 17. Deterministic router

Routing decisions should use mechanically observable facts first.

Valid inputs include:

- file size;
- line count;
- match count;
- requested range;
- command shape;
- output size;
- exit status;
- artifact type;
- cache state;
- process state;
- dependency structure;
- prior exact evidence;
- provider context limits;
- configured thresholds;
- remaining cell/helper budget.

Routine routing MUST NOT require an LLM.

A learned router may later optimize cost or relevance, but MUST NOT be necessary for correctness and MUST NOT erase provenance.

Illustrative policy:

```text
Can deterministic primitive satisfy semantic contract adequately?
    yes → use it
    no  ↓

Can deterministic composition/projection satisfy it adequately?
    yes → use it
    no  ↓

Is there a clear semantic advantage to bounded helper inference?
    yes → invoke helper with explicit budget and derived provenance
    no  → expose exact handle / pagination / explicit limitation
```

---

## 18. Tool-call fusion and speculative execution

Pane SHOULD preserve and extend its advantage in reducing parent inference round trips.

Two forms must be distinguished.

### Parallel speculation

Several independent operations can execute together without seeing one another's results.

Native provider tool calling may already express some of this.

### Dependent local control flow

Later operations depend on earlier results:

```text
Read A
  ↓
if X
  ├── Search B
  │      ↓
  │   if Y → Edit C
  └── else → Run D
```

This requires a programmable execution context and is a core purpose of cells.

Pane MAY also fuse independent direct provider calls into one internal execution frame where protocol semantics allow, provided ordering, dependency, error, and evidence semantics remain unchanged.

The model interaction granularity and runtime execution granularity are not required to be identical.

---

## 19. TUI model

The TUI may continue to render all execution using Pane's existing cell-style visual language.

However, UI representation must not imply that every direct provider call was literally authored as JavaScript by the model.

Pane should treat the visual unit as an execution frame/cell card with an origin such as:

```text
direct_tool
authored_cell
little_helper
```

A direct call may render as:

```text
╭─ 42 · Read ─────────────────────────────
│ src/runtime.rs
│ exact · 18.3 KB · 312 lines
╰──────────────────────────────────────────
```

A composed cell may render as:

```text
╭─ 43 · Cell ─────────────────────────────
│ Read × 3
│ Grep × 1
│ branch × 1
│ 4 operations · 1 helper · 0 failures
╰──────────────────────────────────────────
```

Expanded views SHOULD expose nested capability/helper execution and provenance where useful.

This preserves Pane's coherent TUI without creating separate UI architecture for direct tools.

---

## 20. Event ledger requirements

Every provider-visible call and every nested cell capability invocation MUST produce sufficient ledger evidence to establish what actually happened.

Illustrative sequence:

```text
ToolRequestReceived / CellStarted
IntentCompiled
ExecutionStrategySelected
ExecutionStarted
ArtifactObserved
HelperInvoked          # optional
ReductionApplied       # optional
ToolResultReturned
CellCompleted
```

The ledger must distinguish at least:

- requested vs executed;
- direct vs cell-authored origin;
- fresh execution vs reused evidence;
- exact vs bounded exact vs derived result;
- primitive vs composed vs helper execution;
- successful execution vs successful presentation;
- execution failure vs adapter/serialization failure;
- helper scope/budget use;
- underlying exact artifact references for derived views.

The model's narrative about a tool run is never authoritative evidence that the run occurred.

---

## 21. Trust ordering

Pane MUST preserve:

```text
observed execution evidence
        >
deterministic checks
        >
derived helper analysis
        >
model claims
```

Familiar tool ergonomics must never weaken this hierarchy.

A helper result can be excellent analysis and still remain derived.

---

## 22. Errors

Errors should be canonical internally and provider-familiar at the boundary.

Example canonical error:

```ts
{
  kind: "artifact_not_found",
  target: "src/a.ts",
  recoverable: true
}
```

The Anthropic and OpenAI dialects may render this differently if that improves model recovery, but the fact and ledger evidence remain identical.

This allows Pane to exploit learned provider error-recovery behavior without forking execution semantics.

---

## 23. Tool schemas as prompt compression

Operational rules that can be represented by schemas, types, validators, runtime state, or capability behavior SHOULD be removed from prompt folklore when practical.

A provider-aligned schema can provide:

```text
explicit structural guidance
+
learned behavioral prior
```

without repeatedly spending prompt tokens explaining a novel API.

Pane's governing rule remains:

> **A rule belongs outside the prompt whenever Pane can enforce or infer it mechanically.**

The Tool ABI extends that principle by also taking advantage of knowledge already embedded in model weights through familiar tool shapes.

---

## 24. Non-goals

Pane is not trying to clone undocumented internals of Claude Code, Codex, or another harness.

It is not trying to make every provider expose identical visible tools.

It is not trying to replace exact execution with helper inference.

It is not trying to infer multi-step parent intent from a simple direct tool call.

For example, if the parent calls `Read`, Pane may satisfy that read intelligently, but it must not invent a subsequent search/edit/test workflow because it guesses that might be useful.

The parent owns composition intent. Pane owns execution quality within the requested capability contract.

---

## 25. Complete implementation scope

This work should not stop at a minimal translation-layer demo.

The first shippable hybrid implementation should include the complete core semantics necessary for the supported tools to be meaningfully comparable to existing Pane cells:

- hybrid parent exposure;
- provider-specific façades;
- one capability descriptor system;
- direct-tool and TypeScript-cell bindings generated from the same capability definitions;
- canonical intent lowering;
- universal Cell IR / existing equivalent execution substrate;
- deterministic-first routing;
- exact / bounded exact / derived provenance;
- artifact handles for large results;
- existing Little Helper/reducer integration;
- helper scope and budget enforcement;
- direct and nested ledger evidence;
- mutation safety and observed diffs;
- checker reuse/freshness semantics;
- TUI rendering for direct and composed execution;
- benchmark/ablation runtime modes with one execution architecture.

The supported core tool set should be completed coherently rather than shipping one nominal tool whose behavior is not representative of the final architecture.

However, completeness has a boundary: this does **not** require solving every future search backend, every provider in existence, or every conceivable model failure before the architecture can ship.

The initial complete scope should target the currently supported Anthropic and OpenAI provider families and the local coding capabilities listed in this document.

---

## 26. Implementation order without architectural staging

Implementation may proceed internally in dependency order, but intermediate steps are not separate product architectures.

Recommended order:

1. inventory current Pane primitive/cell/helper/checker/ledger boundaries;
2. define provider-neutral capability descriptors and canonical intents;
3. make descriptors generate both provider tool schemas and cell bindings;
4. lower both direct tools and authored cells into the same existing execution substrate;
5. implement full read/search/list behavior with handles and provenance;
6. implement full mutation/patch behavior with exact validation and observed diffs;
7. implement shell/process behavior and mechanically safe specialized lowering;
8. integrate checks/tests and freshness/reuse evidence;
9. integrate Little Helpers under deterministic-first routing for every capability where they have a justified use;
10. enforce helper aggregate budgets and scope inside cells;
11. integrate TUI execution-frame rendering;
12. implement `cells`, `tools`, and `hybrid` visibility modes over the same kernel;
13. run full existing Pane gates plus dedicated cross-interface contract tests;
14. benchmark cells-only, tools-only, and hybrid behavior on representative tasks.

No intermediate step should create a second executor intended to be removed later.

---

## 27. Benchmark design

The benchmark should test the actual complete architecture rather than a crippled translation-only mode.

At minimum compare:

```text
A — cells-only
B — provider-familiar tools-only
C — hybrid direct tools + cells
```

Because Little Helpers are a core Pane mechanism, the main product comparison SHOULD leave adaptive helper execution enabled under the same deterministic-first policy in all modes where the relevant capability is available.

For architectural attribution, additionally support ablations such as:

```text
hybrid + helpers enabled
hybrid + helpers disabled
```

and, where useful:

```text
tools-only + mechanical execution
tools-only + adaptive helper execution
```

These are benchmark switches, not product forks.

Measure at least:

- task success;
- provider requests;
- total input/output tokens;
- parent inference turns;
- Little Helper calls and tokens;
- failed capability calls;
- repair turns;
- direct tool calls;
- authored cells;
- operations per parent inference;
- wall time;
- human intervention;
- unsupported completion claims;
- helper/reducer use;
- exact vs derived evidence consumption;
- repeated observations;
- mutation failures/staleness;
- verification freshness/reuse correctness.

Where possible, compare the same underlying model and task across interface modes to isolate harness/interface effects from base model quality.

Do not claim efficiency superiority without a measured before/after baseline.

---

## 28. Acceptance criteria

The hybrid architecture is complete only when all of the following hold for the supported provider families and core capabilities:

1. `--interface=hybrid` exposes provider-familiar direct tools and `execute_cell` simultaneously.
2. `--interface=cells` and `--interface=tools` alter visibility only and do not select separate execution engines.
3. Direct tool calls and cell-bound calls compile through the same capability descriptor and canonical intent.
4. Direct tool calls lower into the same Cell IR / existing Pane execution substrate as authored cells.
5. Provider-specific schema/result/error code remains confined to thin adapter/dialect boundaries.
6. Anthropic-oriented and OpenAI-oriented façades can differ without changing canonical execution semantics.
7. Familiar capability names/shapes are available inside cells where practical, so the model can compose known tools with TypeScript control flow.
8. Read/search/list operations preserve exact ground truth, bounded exact views, and stable access to omitted content.
9. Large provider-native tool results retain Pane's artifact/handle context-efficiency advantage.
10. `exact`, `bounded_exact`, and `derived` are real production states, not test-only types.
11. Existing Little Helpers/reducers can be invoked from capability execution when deterministic execution is inadequate and a clear semantic advantage exists.
12. Helper outputs are always marked derived and linked to source evidence.
13. Helper scope, token, call-count, wall-time, concurrency, and nesting controls prevent runaway nested inference.
14. Mutation tools never silently use helpers to change requested mutation intent.
15. Shell/Bash execution mechanically lowers recognizable safe operations where semantics are exact and faithfully falls back to sandboxed process execution otherwise.
16. Test/check capabilities preserve fresh-vs-reused evidence semantics and parent interpretation remains independent of checker prose.
17. Every direct tool, nested cell capability, helper call, artifact observation, mutation, and check produces sufficient ledger evidence for completion verification.
18. The TUI can render direct tool execution and authored cells coherently without requiring separate UI stacks.
19. Existing Pane gates remain green.
20. Dedicated tests prove semantic equivalence between direct and cell invocation of the same capability.
21. Dedicated tests prove dependent multi-operation logic works inside one parent-authored cell, including branching on earlier capability results.
22. Dedicated tests prove helper-capable operations can run inside a cell without escaping scope/budget boundaries.
23. Dedicated tests prove a derived result cannot substantiate an exact-content claim by itself.
24. A recorded benchmark exists for cells-only, tools-only, and hybrid modes using the same execution kernel.
25. Benchmark reporting separates parent-model tokens/turns from out-of-turn Little Helper tokens/calls.

---

## 29. Implementation guidance for Claude Code

Treat this document as the target architecture, not as permission to create a parallel tool runtime.

Before implementation:

1. Read `CLAUDE.md`.
2. Read `docs/product/pane/model-contract.md`.
3. Read `docs/product/pane/runtime-contract.md`.
4. Read `docs/product/pane/events-contract.md`.
5. Read `docs/product/pane/improvement-register.md`.
6. Read `docs/product/pane/little-helpers.md`.
7. Read the current checker/evidence and handle contracts relevant to fresh/reused execution and artifact identity.
8. Locate the existing provider adapter boundary, cell execution entry point, primitive dispatch, Little Helper/reducer path, checker/evidence path, artifact/handle layer, event ledger, and TUI cell rendering.
9. Reconcile this spec with those existing contracts before changing code.

Implementation rules:

- **Do not create a native-tool executor beside the cell executor.**
- **Do not implement direct tools as a temporary path intended to be rewritten later.**
- Direct tool calls and cell capability calls must converge immediately onto one canonical execution path.
- Treat `hybrid` as the target product interface, not a later optional experiment.
- Preserve `cells` and `tools` as ablation/compatibility visibility modes only.
- Generate native tool schemas and TypeScript cell bindings from the same capability definitions where feasible.
- Preserve provider-specific familiar shapes at the boundary rather than forcing artificial cross-provider symmetry.
- Keep provider adapters thin.
- Prefer deterministic primitives and deterministic composition/projection.
- Invoke Little Helpers only when there is an explicit semantic advantage.
- Reuse Pane's existing Little Helper implementation and scope controls; do not build a second nested-agent mechanism.
- Never let helper analysis masquerade as exact evidence.
- Never let a helper silently rewrite mutation intent.
- Keep exact artifacts reachable whenever a bounded or derived view is returned.
- Preserve handle efficiency for provider-native results.
- Keep every nested capability/helper call observable in the ledger.
- Preserve existing verifier authority over model claims.
- Preserve cell composition and dependent control flow as a first-class Pane capability.
- Reuse current primitives, handles, reducers, checkers, event contracts, and TUI abstractions where they already satisfy this design.
- If an existing abstraction already represents `CapabilityDescriptor`, `Cell IR`, or an execution frame under another name, extend it rather than duplicating it.
- Complete the core tool behavior coherently before treating the feature as benchmark-ready.
- Do not wait for every future provider or edge case before shipping the supported coherent architecture.
- Record evidence before claiming correctness or efficiency improvements.

The implementation is successful when the parent model can work in its familiar coding-tool dialect, compose those same tools as TypeScript when it needs dependent logic, receive smarter bounded responses when Pane's Little Helpers have a justified advantage, and still have every operation execute through one observable Pane kernel.

---

## 30. Product summary

Pane's value is not that it renames ordinary tool calls.

The model gets familiar tools, but Pane upgrades what those tools mean operationally:

```text
familiar provider ergonomics
        +
programmable TypeScript composition
        +
deterministic-first execution
        +
bounded Little Helper intelligence
        +
lazy artifact/handle materialization
        +
execution ledger
        +
claim/evidence verification
```

A conventional harness gives the model a tool.

Pane should give it the same familiar handle to a much stronger machine.
