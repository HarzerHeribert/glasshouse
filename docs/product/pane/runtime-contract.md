# pane — the runtime contract

Unblocks **61E**. What a tool result becomes, what the model is shown of it,
how long it lives, how it survives `pane resume`, and what happens when a
program throws. Every sentence here is a decision; where a decision is the
user's to overturn it is marked `DECISION PARKED` and the default is taken.

The vocabulary is Phase 57's, deliberately reused: *passthrough*,
*eligibility*, *provenance*, *raw ref*. The firewall already reduces a tool
result on its way into a conversation
(`crates/glasshouse/src/firewall/mod.rs:284`); pane's difference is that the
result never enters the conversation at all, so the same words describe a
different mechanism and must not drift.

## 1. A cell is a turn's program; a task is a sequence of cells

The model acts by emitting exactly one TypeScript program per turn. That
program is a **cell**. All cells of one task share **one persistent module
scope in one V8 isolate**, REPL-style: a top-level `const`, `let`, `function`
or `class` declared in cell *n* is in scope in cell *n+1*.

A cell ends one of two ways, and the difference is the whole control flow:

- it **falls off the end** → the runtime **yields**. It renders the handle
  table, the model gets another turn, the isolate stays warm.
- it executes a top-level `return` → the task **ends** with that value as the
  result. Nothing further is asked of the model.

This is the `Yielded` / `Result` distinction Codex's `code-mode-protocol`
ships (`codex-rs/code-mode-protocol/src/runtime.rs`, `RuntimeResponse`), taken
because it makes `return` a keyword with one meaning instead of a convention.

## 2. A handle is a binding the model itself named

**Naming rule.** A handle's name *is* the top-level binding name in the
model's own program. `const hits = await grep(...)` produces the handle
`hits`. There is no `h1`, no server-side identifier, and no translation step:
the name the model wrote is the name it addresses next turn.

Only three things become handles: a top-level binding, the value a cell
yields, and an object the model explicitly `keep(name, value)`s. An
intermediate a program never binds is garbage and is never previewed.

**Collision behaviour: replacement, announced.** A later cell may redeclare a
name (the persistent scope is a REPL scope, not a module scope, so this is not
a `SyntaxError`). The redeclaration wins, the previous object is freed
immediately, and the handle table's next rendering carries
`hits  (replaced at cell 4)` on the line. Silently keeping both under
generated suffixes was rejected: it produces names the model never wrote and
cannot predict, which is the one property this naming rule exists to avoid.

**Lifetime.** A handle lives until exactly one of three events, and no other:

1. the model redeclares its name;
2. the model calls `free("name")`;
3. the task ends.

**Nothing is evicted by an LRU, a heap watermark, or a timer.** A handle
vanishing under a program that still names it is the failure that would make
the whole channel untrustworthy, and no memory saving is worth it. When the
isolate's heap crosses its configured ceiling the **cell** fails with
`RuntimeOutOfMemory`, and the error preview lists the five largest live
handles by retained size so the model can choose what to free. The model
decides; the runtime never does.

## 3. A preview is type-directed first and size-capped second

Every live handle renders as one entry in the handle table. The shape is
chosen by the value's type; the caps then bound it.

| type | what the preview shows |
|---|---|
| `T[]` | `n=<len>`, then elements `[0] [1] [2]` and `[len-1]`, each rendered at depth 1 and cut at 120 characters |
| `File` | path, byte length, line count, mtime, then lines 1–2 verbatim. **Never the contents.** |
| `TestReport` | `passed/failed/skipped`, then up to 3 failing test names. **Never the log.** |
| `string` | `len=<chars>`, then the first 200 characters, then `…(+N chars)` |
| number, boolean, null, undefined | the value verbatim |
| unknown object / struct | up to 12 key names with each value's *type*, never its value, then `…(+N more keys)` |
| `Error` | class, message cut at 200 characters, and the top 3 stack frames that lie inside the model's own program |

Two hard ceilings, both measured with
`crate::firewall::estimate::estimate_tokens` — the `chars / 4` heuristic
Phase 57 already documents and ships
(`crates/glasshouse/src/firewall/estimate.rs:12`), reused so pane and the
firewall never report two different sizes for the same bytes:

- **256 tokens per preview.** A preview that would exceed it shrinks by its
  own type rule — element counts go 4 → 2 → 0, key counts 12 → 4 → 0 — before
  any string is cut. Bytes are never truncated mid-way to hit the number.
- **2,048 tokens per turn for the whole handle table.** Over it, entries are
  dropped from the *rendering* oldest-first — never freed — and one line says
  `…N older handles not shown; call handles() for the full list`.

A program's own `console.log` output is not conversation and is not a
preview: the last 512 tokens of it are appended to the turn under a
`[stdout]` heading, and the rest is dropped with a count. That number is
pane's own; Codex's comparable cap is 10,000 output tokens per exec call
(`DEFAULT_MAX_OUTPUT_TOKENS_PER_EXEC_CALL`), which is a reasonable ceiling for
a harness whose results still travel as text and far too large for one whose
whole claim is that they do not.

## 4. The rollout records programs and previews, never objects

One rollout line per cell, JSON, append-only:

```json
{"cell": 4, "source": "const hits = await grep({...});\n",
 "outcome": "yielded",
 "handles": [{"name": "hits", "type": "Grep.Match[]", "preview": "n=1195 …",
              "provenance": {"tool": "grep", "args": {"pattern": "IntegrationId",
              "glob": "crates/glasshouse/**/*.rs"}, "sha256": "9f2c…", "pure": true}}]}
```

**Resume rebuilds no object by re-running anything.** `pane resume` replays no
cell: a program that deleted a branch would delete it twice. Every handle
comes back **stale** — present in the table, marked `stale`, and any property
access throws `StaleHandle("hits")` with the provenance in the message so the
model can re-derive it in one line.

The single exception is a handle whose provenance names a tool the tool
registry declares **pure** (`grep`, `glob`, `read` — purity is the tool's own
declaration, never inferred): such a handle re-materialises lazily on first
access by re-running that exact recorded call, and the result's SHA-256 is
compared with the recorded one. Equal, the handle is live and the model is
never told anything happened. Different, it stays stale and the message says
the tree moved. This is the raw store's discipline
(`crates/glasshouse/src/firewall/store.rs`) applied to a live object: content
addressing is what makes "the same result" a checkable claim rather than an
assumption.

## 5. A throw is a result, and the turn is never retried

A cell that throws produces, in the same turn slot a yield would have used:

1. the `Error` preview from §3;
2. the source line and column inside the model's own program;
3. the handle table **as it stood after the last statement that completed** —
   bindings made before the throw persist, which is what lets a model recover
   in one cheap cell instead of redoing its work;
4. nothing else. No stack from inside the runtime, no host frames, no tool
   payload.

**The turn is not retried automatically, ever.** An automatic retry re-runs
side effects the runtime cannot know are idempotent, and the model is both the
cheapest and the best-informed thing in the loop at deciding whether the call
should happen again. A refused tool call (`PermissionDenied`, see
`sandbox-grants.md` §5) is an ordinary throw and follows this rule exactly:
it is catchable inside the program and never escalates to a prompt.

## 6. The worked turn

The same turn appears in `model-contract.md` §7 as the bytes the model
receives; the two must agree on every name and every preview.

Task: *"Every file that names `IntegrationId` — how many are tests, and which
production files would a new variant force me to touch?"*

**Cell 1**, as the model emitted it:

```pane
const hits = await grep({ pattern: "IntegrationId", glob: "crates/glasshouse/**/*.rs" });
const adapter = await read({ path: "crates/glasshouse/src/harness/mod.rs" });
```

It falls off the end, so the runtime yields. Handle table after cell 1
(measured on this repository at `4d97c8f`):

```
hits     Grep.Match[]   n=1195   inline cost ~30,565 tok · preview 139 tok
  [0]      crates/glasshouse/tests/gateway_translate_effort.rs:29  "use glasshouse::integrations::IntegrationId;"
  [1]      crates/glasshouse/tests/gateway_translate_effort.rs:512 "let mut profile = LaunchProfile::native(IntegrationId::ClaudeCode);"
  [2]      crates/glasshouse/tests/gateway_translate_responses.rs:35 "use glasshouse::integrations::IntegrationId;"
  [1194]   crates/glasshouse/src/session/store/record.rs:425  "/// [`crate::integrations::IntegrationId`] string."
adapter  File   crates/glasshouse/src/harness/mod.rs   63,979 B · 1,508 lines · 2026-09-05T14:18:26Z
  L1       "//! The contract every supported harness is reached through."
  L2       "//!"                                                          preview 66 tok
```

122,261 bytes of grep output cost the model 139 tokens to know about and zero
to compute over. That ratio is the capability; everything above is the
bookkeeping that makes it safe.

**Cell 2**, the model's next turn:

```pane
const isTest = (m) => m.path.startsWith("crates/glasshouse/tests/");
const inTests = hits.filter(isTest);
const prodFiles = new Set(hits.filter(m => !isTest(m)).map(m => m.path));
return { total: hits.length, in_tests: inTests.length, prod_files: prodFiles.size };
```

`return` executes, so the task ends with `{total: 1195, in_tests: 290,
prod_files: 62}`. Neither the 122 KB nor the 64 KB was ever a conversation
token.

## 7. What this contract does not decide

- **The tool registry's own schema** — which tools exist, their argument
  types, and the `pure` declaration §4 depends on. That is 61E's own package;
  this contract only requires that `pure` be *declared*, never inferred.
- **The isolate's heap ceiling and the yield timeout.** Both are `pane.toml`
  runtime limits, and their defaults are the supervisor's business (61F).
- **Whether a handle may be shared between two tasks.** It may not, today,
  because nothing needs it; the day something does, it is a new line.
- **The single-binary promise.** pane embeds a V8 isolate, so the README's
  *"no daemon, no Node, no Python"* scopes to the `glasshouse` binary alone.
  `DECISION PARKED` — the brief's default, taken.
- **TypeScript and nothing else.** No second generated language, no Python
  escape hatch. `DECISION PARKED` — the brief's default, taken.

CONTRACT
behaviour:  A tool result becomes a named live object in pane's V8 isolate and the model receives its name and a preview capped at 256 tokens, never the payload.
invariant:  A live handle is freed only by redeclaration, an explicit `free`, or the task ending — never by eviction — and a resumed handle is stale until its recorded pure call re-materialises it to an identical SHA-256.
path:       `crates/pane/src/runtime/`: the cell executor that lifts top-level bindings into the persistent scope, renders the handle table, and appends one rollout line per cell.
test:       `crates/pane/tests/handles.rs::a_grep_of_122kb_costs_under_300_tokens_and_survives_one_yield` — runs the §6 worked turn against a fixture tree, asserts the payload appears nowhere in the rendered turn and that `hits.length` is readable in cell 2.
