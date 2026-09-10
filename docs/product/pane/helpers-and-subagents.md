# Pane — Little Helpers, Subagents, and Execution Lifetimes

Status: architectural clarification for the Tool ABI / hybrid implementation

This document does **not** introduce a second agent runtime. It clarifies the semantic and lifetime boundary between Pane's existing Little Helpers and existing subagents so the hybrid Tool ABI can compose them without turning Cells, helpers, background jobs, and agents into overlapping mechanisms.

Read this together with:

- `docs/product/pane/little-helpers.md`
- `docs/product/pane/events-contract.md` §5 and §5b
- `docs/product/pane/tool-abi.md`
- `crates/pane/src/helpers.rs`
- `crates/pane/src/agent.rs`
- `crates/pane/src/bg.rs`

The current implementation already contains the important substrate:

- a Little Helper owns a **question**, returns a value, has no persistent effects, and may be called from a Cell;
- `agent.run(task, {turns, model})` starts a **background subagent turn loop**, returns a handle immediately, and delivers completion later through the session event path;
- subagents already reuse `bg` lifecycle/cancellation/event machinery instead of creating a second background-delivery path;
- a subagent may call a Little Helper, but may not start another subagent;
- a Little Helper is a leaf.

The purpose of this document is to make the execution model explicit before the hybrid Tool ABI is allowed to depend on it.

---

## 1. The core distinction

The shortest correct rule is:

> **A Little Helper owns a question and is scoped to the operation that asked it. A subagent owns a goal and is scoped to the session/task that spawned it.**

That produces different lifetime semantics.

```text
Little Helper
─────────────
owns:       a question
returns:    a value / evidence / bounded interpretation
lifetime:   atomic to its caller
state:      none after return
world effects: none
background after caller scope: forbidden

Subagent
────────
owns:       a bounded goal
returns:    eventual work result / findings
lifetime:   independent of the Cell that starts it
state:      running job until completion/cancellation
world effects: according to its grant
background after Cell: expected
```

The two may share underlying model-loop machinery. They MUST NOT share authority or lifecycle semantics merely because the implementation reuses code.

---

## 2. Three execution lifetimes

Pane should reason about work using three lifetimes.

### 2.1 Foreground capability lifetime

Ordinary deterministic or tool capability work belongs to the current Cell/execution frame.

```ts
const file = await Read({ file_path: "src/foo.ts" });
```

The call completes before the dependent branch can continue.

Examples:

- file reads;
- search;
- exact edits;
- local parsing;
- foreground commands;
- checks that the Cell explicitly awaits.

### 2.2 Scoped Helper lifetime

A Little Helper is also foreground **with respect to its owning Cell/execution scope**, even if its implementation performs asynchronous model I/O.

```ts
const evidence = await helper.find(
  "Find the implementation sites that enforce this timeout."
);
```

From the model-authored program's point of view it is an async function returning a value.

The helper MAY run concurrently with other independent helper/capability work if Pane's runtime supports that safely:

```ts
const [locations, reduced] = await Promise.all([
  helper.find("Find the relevant call sites."),
  helper.reduce(testOutput)
]);
```

But concurrency does not change ownership.

**Invariant: no Little Helper may outlive the Cell/execution scope that invoked it.**

If its owning Cell:

- returns;
- throws;
- is interrupted;
- times out;
- is cancelled;
- otherwise terminates;

then all helper work still owned by that Cell must be joined, cancelled, or terminated according to the helper runtime's bounded shutdown rules. A helper must never become an orphaned session background task.

This is structured concurrency: helper lifetime is nested inside caller lifetime.

```text
CELL START
   │
   ├─ deterministic work
   ├─ helper A ─────────┐
   ├─ helper B ───────┐ │
   │                  │ │
   ├─ await/join ◄────┴─┘
   ├─ branch on returned values
   │
CELL END

No helper remains running here.
```

Parallel helper execution is an optimization. **Scoped lifetime is the contract.** If current runtime constraints serialize helper calls, that does not justify changing their lifetime into background jobs.

### 2.3 Session-background Subagent lifetime

A subagent is intentionally different.

The current API is:

```text
agent.run(task, {turns, model}) → Agent handle, immediately
```

`agent.run` is semantically a spawn despite its name: it returns before the nested turn loop finishes. The subagent runs through the existing background-job/event substrate and may outlive the Cell that created it.

```ts
const investigation = agent.run(
  "Investigate why the authentication regression occurs. Do not modify files.",
  { turns: 8, model: "worker-model" }
);

return {
  status: "investigation-started",
  agent: investigation
};
```

The Cell is free to end immediately.

```text
CELL START
   │
   ├─ agent.run(...)
   │      │
   │      └──────────────► SUBAGENT TURN LOOP
   │                        continues independently
   │
   └─ return handle
CELL END
                              │
                              ▼
                         agent.done event
                              │
                              ▼
                         later session batch
```

**Invariant: a subagent belongs to the session/task background lifecycle, not to the Cell that spawned it.**

---

## 3. A Cell must never stay open merely to wait for a subagent

This is a hard design rule.

Do not introduce a normal pattern such as:

```ts
const result = await agent.runUntilFinished(...);
```

if that means the Cell remains alive for the duration of a long-running subagent.

The existing architecture is deliberately better: `agent.run` returns a handle immediately, the nested loop runs out of band, and completion is delivered later through `agent.done` / the event batch.

Long-running agent work may take minutes. Holding the Cell open would incorrectly couple:

- V8/isolate lifetime;
- Cell wall-clock budget;
- helper budget;
- user interactivity;
- subagent lifetime;
- event delivery;
- cancellation.

Pane already has a session-level background lifecycle. Use it.

---

## 4. "The parent waits" means the session yields, not that inference blocks

A parent model is not a process that can sleep for ten minutes inside an inference request.

When the parent has no useful foreground work until a subagent completes, the correct semantic state is:

```text
PARENT TURN
    │
    ├─ starts subagent(s)
    └─ yields / finishes current turn
             │
             ▼
        SESSION REMAINS LIVE
             │
     ┌───────┴─────────┐
     │                 │
user instruction   agent.done
     │                 │
     └───────┬─────────┘
             ▼
      next relevant parent turn
```

The runtime waits for session events; it does not keep the expensive parent inference call open.

This distinction is essential for long-running work and context/cost efficiency.

---

## 5. User steering must remain available while subagents run

A running background subagent must not monopolize the parent prompt or Cell.

The intended user experience is:

```text
10:00 parent starts agent A
10:00 Cell/turn ends
10:03 user: "Do not change the DB schema; compatibility is mandatory."
10:08 agent A completes
10:08+ parent sees both the steering and the completed work according to session/event ordering
```

Requirements:

1. the TUI/input path remains available while subagents run;
2. user instructions are recorded as session/task input rather than delayed behind a blocked Cell;
3. the next parent reasoning step sees new authoritative user instructions before acting on stale delegated findings;
4. a user instruction that invalidates outstanding delegated work must be able to cause cancellation, supersession, or explicit staleness rather than silently accepting conflicting results;
5. implementation should reuse the existing event/inbox/background architecture rather than add a subagent-specific interaction channel.

Whether live steering is pushed directly into an already-running subagent or only applied by the parent when its result returns is a separate policy question. Do not invent implicit mid-run prompt mutation unless the existing contract explicitly supports it. The minimum correctness rule is that stale subagent work cannot override newer user intent.

---

## 6. Little Helpers do not need session-level steering

Helpers are intentionally too small and too short-lived to deserve a second session interaction model.

A pulled Cell helper receives its bounded question/input at invocation and finishes inside that caller scope.

If the user interrupts/cancels the owning Cell, the helper is cancelled with it.

Do not build:

- helper inboxes;
- helper background handles;
- helper resume;
- helper standing conversations;
- user-to-helper live steering;
- helper `done` events delivered after its owning Cell has ended.

If a unit of work needs those properties, it is not a Little Helper. It is a subagent or a different session-level mechanism.

---

## 7. Explicit Helpers versus internal tool smartening

The hybrid Tool ABI creates two legitimate ways Little Helpers may be used. They share the same Helper runtime and must not become separate mechanisms.

### 7.1 Explicit / pulled Helper

The parent intentionally asks a narrow semantic question from inside a Cell.

```ts
const sites = await helper.find(
  "Find all runtime enforcement sites for this ceiling; ignore documentation."
);
```

The parent owns the question and decides what to do with the returned value.

This is part of the model's execution vocabulary and should be explained in its system prompt.

### 7.2 Internal / pushed Helper

A capability implementation may decide that a bounded helper has a clear advantage while satisfying one requested semantic capability.

Example:

```text
RunTests
   ↓
exact process executes
   ↓
large failure artifact retained
   ↓
deterministic projection insufficient
   ↓
REDUCER invoked under bounded helper policy
   ↓
exact execution facts + derived reduction returned
```

The parent did not explicitly choose the helper. It simply receives a better bounded result with correct provenance.

The parent does **not** need system-prompt instructions telling it when Pane internally performs this routing. Pane owns that decision.

The same helper budget system must preserve room for pulled/model-requested helpers; pushed helpers must not starve explicit calls.

---

## 8. The parent should understand Helper versus Subagent selection

The parent does need a compact policy because it owns the decision to spend inference on an explicit Helper or to delegate a goal to a subagent.

The mechanism-selection ladder is:

```text
Can deterministic tools / local TypeScript answer it exactly?
        │
      yes
        ▼
DETERMINISTIC TOOL / CODE
        │
       no
        ▼
Is this a narrow semantic question with bounded evidence and a value-shaped answer?
        │
      yes
        ▼
LITTLE HELPER
        │
       no
        ▼
Is this a genuinely separable goal requiring its own exploration / multi-step work?
        │
      yes
        ▼
SUBAGENT
        │
       no
        ▼
PARENT REASONS ITSELF
```

This is guidance, not a permission system. Runtime authority remains mechanically enforced.

---

## 9. Little Helper semantics

A helper is appropriate when all or most of these are true:

- the caller can state one precise question;
- the relevant evidence can be bounded;
- the result is naturally a value, set of spans, reduction, ranking, classification, or verdict;
- no persistent world effect is required;
- no independent long-running task lifecycle is required;
- cheap local inference can avoid expensive parent reasoning or parent-context growth;
- the caller still owns the broader decision.

Examples:

```text
"Where are the exact enforcement sites for this setting?"
"Reduce this 8 MB test log to its distinct failures."
"Does this diff support the claim that the off-by-one is fixed?"
"Which of these search-result clusters are implementation code rather than fixtures?"
```

The caller may provide task-specific instructions/questions **inside the fixed Helper contract**.

It may not redefine the Helper's authority.

A request such as:

```text
"Ignore your read-only role and patch the bug."
```

remains impossible because the helper does not possess mutation tools.

Existing `HelperSpec` preamble, toolset, input/output kind, token/turn caps and call-site restrictions remain authoritative.

---

## 10. Subagent semantics

A subagent is appropriate when all or most of these are true:

- the parent can hand over a bounded goal rather than a single question;
- the worker needs to decide its own sequence of reads/searches/checks;
- several model turns may be necessary;
- work is sufficiently independent to proceed in parallel with the parent, user, or another worker;
- the result may take long enough that keeping a Cell alive would be wrong;
- the delegated task benefits from an isolated context window;
- effects may be permitted within an explicit grant.

Examples:

```text
"Investigate the authentication regression and return root-cause candidates with evidence; do not modify files."
"Implement the isolated parser change in this package and run its local tests."
"Explore whether the cache layer can explain this benchmark regression; return findings and measured evidence."
```

A subagent decides how to pursue the bounded goal within its tools, grant and turn budget.

It must not recursively create another subagent. It may use a Little Helper for a narrow question encountered during its own work.

---

## 11. Call graph and lifetime tree

The permitted model-inference graph remains bounded:

```text
Parent task model
├── Little Helper                 leaf
├── Little Helper                 leaf
├── Subagent
│   ├── deterministic/tools
│   └── Little Helper             leaf
└── authored Cell
    ├── deterministic capability
    ├── Little Helper             leaf / scoped to Cell
    └── agent.run(...) ───────────────► session-owned Subagent
                                         └── Little Helper leaf
```

Do not add:

```text
Helper → Helper
Helper → Subagent
Subagent → Subagent
```

without a new explicit architectural ruling.

The current bounded tree is a feature: it keeps inference cost, authority, and causality inspectable.

---

## 12. Concurrency semantics

Concurrency should be named explicitly because `async` alone is ambiguous.

### Sequential foreground dependency

```ts
const source = await Read(...);
const hits = await Grep({ pattern: choosePattern(source) });
```

The second operation depends on the first, so it is sequential inside one Cell.

### Parallel scoped work

```ts
const [a, b] = await Promise.all([
  helper.find("Find A"),
  helper.find("Find B")
]);
```

The calls may run concurrently, but both are still children of the Cell. The Cell cannot successfully finish while one is detached.

### Background session work

```ts
const a = agent.run("Investigate subsystem A", {...});
const b = agent.run("Investigate subsystem B", {...});
return { a, b };
```

Both workers continue after the Cell ends. Their completions are session events.

### Do not conflate `async` with `background`

An awaited Helper can be asynchronous I/O and still be foreground/scoped.

A subagent can return its handle synchronously/immediately and still represent background work.

The decisive property is **lifetime ownership**, not JavaScript syntax.

---

## 13. Cancellation ownership

Cancellation follows lifetime ownership.

### Helper

```text
Cell cancellation
    ↓
cancel outstanding Helper calls owned by Cell
    ↓
no Helper survives Cell termination
```

### Subagent

```text
Cell termination
    ↓
subagent continues

session/task cancellation OR explicit bg.cancel(agentHandle)
    ↓
cancel subagent
```

A Cell ending normally is not cancellation of a subagent it intentionally spawned.

A session ending must not wait forever for a wedged worker; reuse the existing bounded background shutdown semantics.

---

## 14. Event semantics

Little Helper completion is part of the owning Cell's execution record / Helper record. It should not create a future parent turn after that Cell is gone.

Subagent completion is different by design:

```text
agent.run(...)
    ↓
Agent handle immediately
    ↓
background nested loop
    ↓
agent.done event
    ↓
future Events.Batch
```

This existing distinction must remain visible in the event ledger and TUI.

Do not deliver one agent completion as an unsolicited standalone model turn if the existing event-window/batch contract can aggregate it with other session events. The purpose of the event system is to avoid N background completions becoming N expensive parent calls.

---

## 15. Context economics

The lifetime distinction also protects the expensive parent context.

```text
Parent model
  global warm task state
  expensive / scarce persistent context

Little Helper
  narrow evidence + narrow question
  cheap ephemeral context
  discarded after return

Subagent
  bounded delegated goal
  isolated worker context
  discarded after result except for selected evidence/result
```

The parent should receive the smallest sufficient result/evidence from Helpers and subagents rather than inheriting their raw working histories.

The accounting system should keep separate:

- parent model tokens;
- Little Helper tokens;
- subagent tokens;
- persistent parent-context growth;
- ephemeral delegated/helper context;
- exact artifacts retained out of parent context.

Total tokens alone are not sufficient to evaluate Pane's intended economics.

---

## 16. System-prompt contract for the parent model

The parent should be taught mechanism selection, but not Pane's internal tool-routing implementation.

A compact prompt section should be approximately:

> **Execution, helpers, and delegation**
>
> Use familiar tools directly for simple operations. Use `execute_cell` when dependencies, branching, loops, batching, or local transformation can combine several operations without another model turn.
>
> Inside a Cell, use a Little Helper for a narrow semantic question whose answer is a value or evidence you need now. Helpers are cheap, bounded, read-only semantic functions: give them a precise question and the smallest sufficient evidence. Prefer deterministic tools or TypeScript whenever they can answer exactly. Helpers end with their calling execution scope; do not use them for background work.
>
> Use `agent.run` for a separable bounded goal that needs its own exploration or several turns. `agent.run` returns a handle immediately; the subagent continues in the background and reports through the session later. Do not keep a Cell open waiting for a long-running subagent.
>
> Prefer the cheapest sufficient mechanism: deterministic tool/code → Little Helper → subagent. Keep tightly coupled integration and architectural decisions in the parent. Pane may independently optimize individual tool execution; do not invoke Helpers merely because a tool result may be large.

The exact generated names/types for helpers and agents remain authoritative. Avoid duplicating schemas in prose.

---

## 17. Tool ABI interaction

The hybrid Tool ABI does not alter these lifetimes.

```text
Direct familiar tool
    ↓
Cell IR / capability kernel
    ↓
may internally use a bounded Helper
    ↓
returns within direct-call execution scope
```

```text
execute_cell
    ↓
model-authored program
    ├── familiar capabilities
    ├── explicit scoped Helpers
    └── agent.run → session-background Subagent handle
```

The same Pane capability/helper/subagent runtimes must be used regardless of provider dialect.

Anthropic/OpenAI façades may differ in familiar tool names. They MUST NOT differ in helper/subagent lifetime semantics.

---

## 18. TUI behavior

The TUI should make lifetime visible without creating separate products.

### Helper

Show under/within the owning Cell while active, then fold into that Cell's execution summary.

```text
cell 12                                     executing
  scout    scanning   "find timeout enforcement"    2.1s
```

When Cell 12 completes, that Helper is no longer a live row anywhere.

### Subagent

Show as session-level/background work after the spawning Cell has ended.

```text
agents 2 running
  agent-17  auth regression investigation    3m12s
  agent-18  cache benchmark investigation    1m44s
```

The exact existing TUI representation may differ; the semantic requirement is that users can tell scoped work from background work and can continue typing while subagents execute.

---

## 19. What must not be built

This clarification is explicitly against accidental architecture duplication.

Do **not** build:

- a second subagent scheduler beside `bg`;
- a second Helper runtime for Tool ABI calls;
- a Cell-specific subagent implementation;
- a native-tool-specific Helper implementation;
- `await agent.runUntilFinished` as the default long-running pattern;
- detached Little Helpers;
- Helper inboxes or persistent conversations;
- implicit nested subagents;
- user input blocked until workers finish;
- one parent inference turn per background completion when event batching can deliver several together.

Reuse the mechanisms already present unless a concrete invariant cannot be met with them.

---

## 20. Verification requirements

Before claiming this distinction is preserved under the hybrid Tool ABI, prove at least:

1. a Helper invoked from a Cell cannot remain live after that Cell terminates;
2. Cell cancellation cancels its still-running Helper work;
3. multiple independent Helpers may either run concurrently or serialize, but no implementation path detaches them from the Cell;
4. `agent.run` returns its handle before the subagent completes and does not block the Cell;
5. a subagent continues after the spawning Cell ends;
6. subagent completion arrives through `agent.done` / the existing event-batch path;
7. user input remains available while a subagent runs;
8. newer user intent is not silently overridden by a stale subagent result;
9. explicit cancellation terminates a running subagent through existing background cancellation semantics;
10. a subagent cannot start another subagent;
11. a subagent can call an allowed Little Helper;
12. a Little Helper cannot call a Helper or a subagent;
13. automatic/pushed Helper use and explicit/pulled Helper use share the same runtime and accounting limits;
14. pushed Helper use cannot starve model-requested pulled Helper capacity;
15. parent, Helper, and subagent token accounting remain distinguishable;
16. the TUI distinguishes Cell-scoped Helper activity from session-background subagent activity;
17. no new scheduler, delivery channel, or execution engine was introduced merely to satisfy the Tool ABI.

If current code already proves an item, point to that proof rather than rebuilding it.

---

## 21. Implementation guidance while Tool ABI work is in flight

This document is a clarification of the intended composition semantics, not a request to stop current work and rewrite already-correct background/helper infrastructure.

When integrating it:

1. inspect the existing `helpers.rs`, `agent.rs`, `bg.rs`, event contract, runtime bindings, and tests first;
2. preserve `agent.run`'s current immediate-handle/background semantics;
3. preserve the existing bounded Helper runtime and `HelperSpec` authority model;
4. make Tool ABI direct calls and Cell calls reuse those mechanisms;
5. add only the missing lifetime/cancellation/steering guarantees required above;
6. do not rename `agent.run` to `spawn` merely to match conceptual terminology unless there is an independent API reason — its existing semantics already are spawn-like;
7. update the parent system prompt only with the compact mechanism-selection guidance in §16, while keeping schemas/types generated from the runtime;
8. treat user interactivity during subagent execution as a session/TUI/event-loop property, never as a reason to hold a Cell or parent inference request open.

The desired execution model is:

```text
                           SESSION / TASK
                                │
                ┌───────────────┼────────────────┐
                │               │                │
             user input      parent model     subagents
                │               │                │
                │               ▼                │
                │             CELL               │
                │        ┌──────┼──────┐         │
                │        │      │      │         │
                │   deterministic helpers  agent.run
                │        │      │      │         │
                │        │      └─ scoped ─┘     │
                │        │                       │
                │        └──────────────► handle │
                │                                │
                └──────── session events ◄───────┘
```

A Helper is intelligence *inside* a bounded execution scope.

A subagent is intelligence *beside* the parent, running as session-background work.

That distinction must remain obvious in the code, the prompt, the event model, and the TUI.
