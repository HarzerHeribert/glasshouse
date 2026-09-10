# Pane Semantic Command Lifting

Status: **next implementation phase after the hybrid Tool ABI is green**

This document specifies how Pane should recognize familiar shell-shaped operations and, only when their semantics are mechanically provable, execute them through an existing canonical Pane capability instead of treating them as opaque shell text.

It is not an `rg` optimization. `rg` is merely the smallest useful example.

The product idea is:

> **Let the model reach for the tool shape it already knows. When Pane can prove what that operation means, run the stronger Pane capability underneath. When Pane cannot prove equivalence, run the shell request faithfully.**

This preserves the Tool ABI principle:

> **Familiar to the model; Pane-grade underneath.**

It also preserves the more important invariant from `tool-abi.md`: there is still one capability kernel and one execution path. Semantic command lifting is a decoder into that kernel, not a second executor.

---

## What a person gets

A Codex-like model may naturally write:

```text
shell("rg 'SessionManager' crates/")
```

Pane may recognize that exact command shape as repository search and execute the same canonical search capability that a direct `Grep` call or a search inside `execute_cell` reaches.

The model keeps its familiar interface. Pane gains structured semantics:

```text
model shell call
     │
     ▼
mechanical command recognition
     │
     ├── proven equivalent ──> canonical capability ──> one Pane kernel
     │
     └── not proven ─────────> ordinary sandboxed shell execution
```

A recognized search can therefore return:

- exact or bounded-exact matches rather than undifferentiated stdout;
- `count`, `complete`, continuation/addressability and `artifact?`;
- the same provenance contract as direct Pane search;
- the same ledger identity;
- the same optional internal Little Helper reduction when result volume or semantic noise makes that worthwhile.

The same pattern applies to file reads, listings, repository inspection, structured projections, tests and checks where equivalence is mechanically decidable.

The model should not need to know that lifting happened in order to benefit from it.

---

## Non-negotiable rule: prove, or do not translate

Semantic command lifting MUST NOT be heuristic.

A recognizer has only two successful states:

```text
ExactMatch(canonical intent)
NotRecognized
```

There is no `ProbablySearch`, `LikelyTest`, confidence score, LLM classifier or semantic guess in this layer.

Normative rule:

> **Lift only when Pane can mechanically prove that executing the canonical capability preserves the observable semantics promised by the provider-facing shell contract for the recognized subset. Otherwise fall back to the ordinary shell path unchanged.**

This rule includes more than parsing the executable name. A recognizer must account for every option and shell construct that can change the meaning Pane would otherwise preserve.

Examples that may be recognizable:

```bash
rg "Foo" src/
rg -n --glob '*.rs' "Foo" crates/
cat src/config.rs
head -n 80 src/config.rs
tail -n 100 build.log
git status --short
git diff -- src/runtime.rs
cargo test -p pane abi
cargo check -p pane
pytest tests/auth -q
```

Examples that MUST remain opaque shell unless Pane later proves the entire expression:

```bash
rg foo | awk '{print $2}' | sort -u | xargs sed -i ...
cargo test && ./scripts/fix-whatever.sh || echo broken
FOO=bar command ...
for f in ...; do ...; done
$(some-command)
rg foo > result.txt
```

Compound shell syntax is a program. Pane must not rewrite a program into a capability merely because one token inside it looks familiar.

---

## One recognition layer, not command-specific hacks

Do not scatter checks such as:

```rust
if command.starts_with("rg ") { ... }
```

across the shell implementation.

Semantic lifting should have one explicit recognition boundary:

```text
provider / cell shell-shaped request
               │
               ▼
          shell parser
               │
               ▼
     recognizer registry
      │    │    │    │
      │    │    │    └── verification/build recognizers
      │    │    └─────── repository-state recognizers
      │    └──────────── structured projection recognizers
      └───────────────── read/search/list recognizers
               │
          ┌────┴────┐
          │         │
      exact match   none
          │         │
          ▼         ▼
   canonical intent ExecuteCommand
          │         │
          └────┬────┘
               ▼
        one Pane kernel
```

A recognizer should be data-driven or registry-driven where practical. Each recognizer owns:

- the command family it understands;
- the exact accepted grammar/options;
- the translation into a canonical Pane intent;
- a proof-oriented test set for accepted forms;
- negative tests for nearby forms it must refuse to reinterpret.

Adding a new recognizable command should extend this boundary rather than add a new execution mechanism.

---

## Initial lifting families

The first complete phase should cover high-value, low-risk semantic families. It does not need to understand every spelling ever accepted by the underlying CLI. It does need to completely and correctly support the subset it claims to recognize.

### Repository search

Candidate familiar forms:

```text
rg
grep / grep -R where an exact supported subset can be proved
```

Canonical target:

```text
SearchRepository
```

Useful structured result:

```ts
interface SearchResult {
  matches: Match[];
  count: number;
  complete: boolean;
  source: "exact" | "bounded_exact" | "derived";
  artifact?: ArtifactRef;
}
```

Pane should preserve exact match identity, paths and line information. A large result may be bounded for presentation while the full observation remains addressable.

An internal Little Helper may cluster or rank a large/noisy exact result, but that additional view is `derived`; it never replaces the exact evidence invisibly.

### File and artifact reads

Candidate familiar forms:

```text
cat
head
tail
```

Canonical target:

```text
ReadArtifact
```

Recognized forms should preserve the requested range semantics exactly. A `head -n 100` lift is a bounded exact read, not a summary. A `tail` lift must retain suffix semantics rather than silently convert into an arbitrary preview.

Large content follows the existing handle/provenance rules.

### File/path listing

Candidate familiar forms:

```text
fd
find only for a deliberately small, exactly supported grammar
```

Canonical target:

```text
ListArtifacts
```

Do not attempt to emulate arbitrary `find` expressions. `find` has a programming language hidden in its arguments; unsupported predicates, actions, boolean composition or execution forms must fall back to shell.

### Structured projection

Candidate familiar form:

```text
jq
```

Canonical target:

```text
structured deterministic projection over an exact artifact
```

This is worthwhile only for a supported jq subset whose result can be proven equivalent. Do not build a half-compatible jq interpreter merely to claim lifting. If Pane cannot prove the expression it should run `jq` normally.

The value of recognizing a projection is that Pane can preserve the source artifact and provenance instead of treating the projected bytes as unrelated stdout.

### Repository state and diff inspection

Candidate familiar forms:

```text
git status
git diff
git diff -- <paths>
git show only if an exact read-only subset is intentionally supported
```

Canonical targets should use existing repository/diff evidence rather than inventing a new Git subsystem.

A recognized diff should remain a diff as typed evidence, making it directly usable by CHECKER and verification without requiring the parent to reinterpret arbitrary stdout.

Read-only Git inspection is an especially useful lifting target because its semantic category is stronger than "some command printed text".

### Tests, builds and checks

Candidate familiar forms include deliberately supported subsets of:

```text
cargo test
cargo check
cargo build
pytest
python -m pytest
npm test
pnpm test
yarn test
npx tsc --noEmit / tsc --noEmit
```

Canonical targets:

```text
RunTests
RunCheck
```

This family is likely more valuable than search lifting because command output can be huge while the facts the parent needs are small.

A recognized test/check still executes the requested underlying work. Pane may then expose:

```text
exit status
success/failure
fresh/reused status where existing check semantics apply
exact stdout/stderr artifact
bounded exact failure windows
optional derived Little Helper diagnosis/reduction
```

`ok` is decided by the actual execution status, never by whether stdout contains a reassuring string.

Recognition must not silently broaden the invocation. For example, `cargo test -p pane abi` must not become an unscoped project-wide test run.

---

## What should not be lifted first

Mutation deserves a much higher proof bar than observation.

The first phase SHOULD NOT reinterpret arbitrary mutating shell into Pane mutations merely because a familiar executable appears.

Examples to leave as ordinary shell initially:

```text
rm
mv
cp with overwrite semantics
sed -i
perl -pi
package installation
shell redirection
xargs with mutation
Git mutation commands
```

Pane already has explicit `Edit`, `Write` and `apply_patch` capability forms for mutations. A false equivalence on a read wastes time; a false equivalence on a mutation can change the project incorrectly.

A future mutation recognizer is allowed only if its complete supported semantics, stale-target behavior, atomicity and resulting diff can be proven equivalent to the canonical mutation capability.

---

## Environment and executable-availability semantics

Recognition must define what the provider-facing `shell` contract promises about executable identity and availability.

This cannot be hand-waved.

If the contract means "execute this literal program from PATH", then converting a missing `rg` binary into a successful internal search would be observably different and therefore is not equivalent.

If the provider-facing shape is explicitly defined as a familiar command-shaped request whose recognized subset may be serviced by an equivalent Pane capability, then Pane may lift independently of the external binary — but that semantic promise must live at the ABI boundary and be tested.

The implementation must inspect the current Tool ABI contract and choose the interpretation already implied there; it must not silently change shell semantics during this phase.

Whatever rule is selected by the existing contract must be consistent across:

- direct provider shell calls;
- the same familiar binding inside `execute_cell`;
- tools-only, cells-only and hybrid visibility modes;
- supported platforms.

Do not make `rg` happen to work differently merely because GitHub's Ubuntu image has or lacks the binary.

---

## Direct calls and Cells remain semantically identical

Semantic lifting is below the invocation surface.

These two forms must reach the same recognizer and same capability semantics when they represent the same request:

```text
shell({command: "cargo test -p pane"})
```

and, inside a Cell:

```ts
const result = await shell({ command: "cargo test -p pane" });
```

Likewise, a provider-native `Bash` or `shell` alias must not receive a private recognition path unavailable to authored Cells.

The invocation origin may differ in the ledger/TUI. The semantic capability and checked arguments may not.

---

## Interaction with Little Helpers

The parent model should not have to micromanage internal lifting or automatic reduction.

Example:

```text
parent asks:
  shell("cargo test -p pane")

Pane:
  recognizes test invocation
  executes exact command/check semantics
  captures full output outside parent context
  sees oversized/noisy failure output
  optionally invokes REDUCER within pushed-helper budget
  returns exact execution facts + small derived view
```

The parent sees enough provenance to know what is exact and what is derived, but it does not need to call `helper.reduce()` merely because a test emitted a large log.

Explicit/pulled Helpers remain available to the model for questions it intentionally wants answered. Pane's own pushed reductions MUST NOT consume the reserved budget needed for model-requested Helpers. This phase must reuse the existing helper budget and provenance rules rather than create a second reduction mechanism.

---

## Interaction with subagents

Semantic command lifting does not change subagent scheduling or lifetime.

A subagent gets the same familiar tool surface appropriate to its provider and therefore benefits from the same lifting rules. A subagent running `rg`, `git diff` or `cargo test` should not receive a worse execution substrate than the parent.

The existing lifetime distinction remains:

```text
Little Helper  = scoped semantic question; never outlives its Cell/caller
Subagent       = bounded goal; may continue as session-background work
```

No recognizer may spawn a subagent. Lifting is deterministic dispatch, not delegation.

---

## Result semantics and context protection

The point of lifting is not merely to substitute a faster executable. It is to retain semantic structure and keep transient bulk out of the expensive parent context.

Opaque shell:

```text
command
  ↓
large stdout/stderr
  ↓
generic command result
```

Lifted capability:

```text
command-shaped request
       ↓
canonical semantic capability
       ↓
exact observation retained
       ├────────────> artifact / evidence
       │
       └────────────> small structured parent result
                         + complete/count
                         + provenance
                         + artifact?
                         + optional derived view
```

The exact underlying execution remains authoritative.

Trust order remains:

```text
observed evidence
    > deterministic checks/projections
    > derived Helper analysis
    > model claims
```

Semantic lifting must never turn derived interpretation into exact evidence.

---

## Parsing and recognition safety

Do not recognize shell by naïve whitespace splitting.

The recognition boundary must parse enough shell syntax to prove that the request is exactly one supported simple command with supported argument semantics.

At minimum it must distinguish or reject where relevant:

- quoting and escaped characters;
- argument boundaries;
- pipelines;
- `&&` / `||` / `;`;
- redirections;
- command substitution;
- subshells;
- variable assignment/environment prefixes;
- glob expansion where expansion timing affects semantics;
- backgrounding;
- shell functions/builtins where relevant.

A parser may conservatively say `NotRecognized` for syntax Pane does not need. False negatives merely use the normal shell path. False positives can change semantics and are unacceptable.

This asymmetry is intentional:

> **Prefer missing an optimization over inventing equivalence.**

---

## Cross-platform behavior

Semantic lifting should make behavior more portable only where the ABI contract permits it; it must not hide meaningful platform differences.

Tests must cover at least Linux, macOS and Windows for the recognizer itself and for every lifted capability whose underlying execution is platform-dependent.

Important cases include:

- path separators and quoting;
- executable lookup rules;
- case behavior where relevant;
- command aliases unavailable on one platform;
- `git`, test runner and shell spelling differences;
- absence of optional tools such as `rg` or `fd`.

A missing optional binary must result according to the shell semantic rule already established at the ABI boundary, not according to which runner happened to execute the test.

---

## TUI, ledger and telemetry

A lifted call should remain understandable to a person.

The TUI should show what the model actually requested and may additionally state the canonical capability used, without claiming the model authored code it did not send.

Conceptually:

```text
shell  cargo test -p pane
       ↳ RunTests
```

or another compact presentation consistent with the current frame-origin work.

The ledger should record both facts needed for auditability:

```text
origin request: provider/cell shell-shaped invocation
canonical capability: RunTests
recognition: exact
```

Do not store the entire payload in the rollout merely to explain lifting.

Telemetry should make the mechanism benchmarkable:

```text
shell-shaped calls
recognized calls
fallback calls
recognition family
canonical capability
parent-visible bytes/tokens
artifact bytes retained out of context
pushed Helper calls/tokens caused by lifted results
```

This lets Pane later answer whether lifting actually reduced parent context and retries instead of merely adding machinery.

---

# Next implementation phase: Semantic Command Lifting

Begin this phase only after the current hybrid Tool ABI head is green across its required CI matrix. This phase is one coherent implementation, not a succession of temporary executors.

### Build the recognition boundary

Create one shell-to-capability recognition API and registry. It receives a checked shell-shaped request and returns either an exact canonical intent or `NotRecognized`.

It must not execute anything itself.

The ordinary `ExecuteCommand` fallback and all recognized intents continue into the existing kernel.

### Ship the high-value observational families

Implement exact supported subsets for:

```text
repository search
file reads/ranges
path listing
repository status/diff
```

Start with syntax that is easy to prove. Add positive-equivalence and nearby-negative tests for every supported form.

### Ship verification lifting

Recognize deliberately supported test/build/check forms and route them to existing `RunTests` / `RunCheck` semantics while preserving exact requested scope and exit status.

This is the most important context-saving slice because test/build logs are frequently much larger than their useful semantic result.

### Reuse adaptive result handling

Ensure lifted capabilities use the same:

```text
Bounded result contract
artifact handles
exact / bounded_exact / derived provenance
pushed Little Helper reduction
reserved pulled-Helper budget
ledger
```

There must be no lifting-specific reducer, payload store or evidence class.

### Make direct and Cell paths equivalent

Prove that the same shell-shaped call made directly and inside `execute_cell` reaches the same recognizer, checked canonical intent and result semantics, differing only by invocation origin.

### Make fallback boring and exact

For unsupported options, compound shell, ambiguous syntax, mutation-heavy forms and unknown commands, prove that recognition declines and the existing sandboxed shell path receives the original request without reinterpretation.

A failed recognition is not an error and should not consume a model turn beyond the shell call the model already made.

---

## Required verification

The phase is complete only when the implementation can demonstrate all of the following in behavior rather than checklist arithmetic:

- A plain supported `rg`/search-shaped command can become the canonical repository search capability and returns structured bounded/exact evidence.
- At least one alternative familiar search spelling reaches that same canonical capability where exact equivalence is supported.
- Simple `cat`/`head`/`tail` subsets preserve exact content/range semantics.
- A supported read-only Git status/diff request becomes typed repository evidence.
- Supported test/check commands preserve their exact requested scope and real exit status while large output remains addressable outside parent context.
- Large lifted test/search output can trigger the existing pushed Helper path without hiding exact evidence or consuming the model-reserved Helper slots.
- Direct and Cell invocation of the same shell-shaped request are semantically equivalent.
- Unsupported flags cause fallback, not approximate translation.
- Pipelines, redirection, conditionals, substitutions and other unsupported compound syntax fall back unchanged.
- Mutating commands in the initial scope are not reinterpreted.
- Recognition never calls an LLM and never uses probabilistic classification.
- The recognizer does not execute; both recognized and fallback paths converge on the existing execution kernel.
- Linux, macOS and Windows tests cover command parsing and optional-tool availability semantics.
- The TUI never labels Pane-generated/lowered code as model-authored source.
- The ledger can explain that a shell-shaped request was lifted without persisting the command's bulk payload.
- Telemetry distinguishes recognized from fallback shell requests so the later cells/tools/hybrid benchmark can measure whether lifting helps.

---

## Benchmark consequence

The three interface modes remain an ablation of what the parent sees, not separate implementations:

```text
cells-only
tools-only
hybrid
```

Semantic lifting must be active wherever the same shell-shaped capability is available. Otherwise the benchmark would compare execution backends rather than interaction surfaces.

Later measurement should separate:

```text
parent inference tokens
Little Helper tokens
subagent tokens
persistent parent-context growth
recognized shell calls
fallback shell calls
artifact bytes retained out of context
correct task completion
```

The product goal is not the fewest total tokens at any cost. It is lower cost-to-correct-completion while preserving more useful parent context for a longer warm session.

---

## Guidance to the implementing agent

Read `tool-abi.md`, `little-helpers.md`, `helpers-and-subagents.md`, the current capability/registry code and the shell/provider dialect implementation before editing.

Do not build an `rg` special case and call the phase complete. Build the generic exact-recognition boundary first, then make `rg` one consumer of it.

Do not add a second shell executor, search executor, test executor, Helper path, payload store or provider-specific semantic backend.

Do not ask the user to choose ordinary parser structures or internal representations when the existing product invariants already determine the behavior. Ask only if a remaining choice changes observable product semantics; if so, explain the behavior of each option and recommend one in plain language.

Most importantly:

> **If Pane can prove the meaning, upgrade the implementation under the familiar call. If it cannot prove the meaning, execute what the model actually asked for.**
