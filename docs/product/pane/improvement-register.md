# Pane robustness and efficiency register

This is the implementation register for Phase 61H. Correctness comes before
token or latency wins. It applies across providers and scripting styles; no
row may be closed using one model's behavior as the acceptance test.

## Prompt boundary

The stable system prompt contains only durable invariants: Pane's role, the
single native cell channel, the fact that an unreturned cell has not executed,
the correlated-result boundary, completion semantics, sandbox authority and
the declared tool types. It does not contain a history of model mistakes,
shell-escaping recipes, benchmark-specific paths, preferred variable names or
advice added because one provider once failed.

A rule belongs outside the prompt whenever Pane can detect the condition from
syntax, types, runtime state, tool arguments, filesystem state, process status
or the rollout. Pane corrects safe mechanical cases locally. Otherwise it
returns one short sentence in the correlated cell result and lets the next turn
repair the work. Raw invalid output never executes and silent repair never
changes meaning.

Before adding a sentence to the stable prompt, its author must show that the
rule is durable across models, cannot be enforced mechanically, is needed
before the first action, and costs less than the repeated failures it prevents.
If any condition is false, the rule belongs in a schema, validator, diagnostic,
tool result, project context object or supervisor policy.

The current verbatim preamble predates this ruling. Its migration is explicit:

| Current prompt material | Destination | Keep in stable prompt? |
|---|---|---:|
| One native cell call and correlated-result execution boundary | Protocol invariant | Yes |
| Completion, yield, throw and sandbox-authority semantics | Protocol invariant | Yes |
| Tool names, argument types, result fields and caller ownership | Generated declarations/schema | No |
| `context` before broad source reads and version-bound edits | Tool affordance plus stale/ambiguous edit diagnostics | No |
| The forbidden variable name `new` | Compiler diagnostic | No |
| Line arrays, quotes, dollars and heredoc advice | Literal validator and tool error | No |
| Bounded excerpts instead of broad printing | Output renderer and continuation cursor | No |
| `glob` returning directories | Return type and path-kind diagnostic | No |
| Bash success depending on `exit_code` | Typed process outcome and false-success validator | No |
| Fresh runtime for each user request | Request-boundary protocol invariant | Yes |
| Structured values continue while strings complete | Completion protocol invariant | Yes |
| Incident examples, provider quirks and benchmark paths | Regression corpus only | No |

## Improvement table

`P0` blocks a fair parity claim. `P1` is needed for dependable long sessions.
`P2` improves efficiency or diagnosis after correctness is stable.

| Priority | Failure or missing capability | Pane-owned detection or correction | Model-visible feedback on the next turn | Evidence required to close |
|---|---|---|---|---|
| P0 | Generated code continues after the native cell boundary as though it ran | Execute only a complete decoded native call; record planned, executed, skipped and failed operations separately | `This cell had not run; only the correlated result below is evidence.` | Fragmented-stream and speculative-branch tests across at least three provider adapters |
| P0 | Markdown or prose is mistaken for executable code | Accept native `execute_cell`; keep fenced input as an explicit bounded repair path and reject ambiguous mixtures | `No code ran because the reply did not contain one valid cell call.` | Malformed, truncated, mixed-fence and ordinary-code-block corpus runs with zero accidental execution |
| P0 | A parse error causes the model to rewrite a large cell | Preserve the failed source and offer exact single-replacement `pane-edit` against its immutable cell ID | `Cell N did not run; repair the marked span with pane-edit.` | Large-script syntax mutation is repaired without retransmitting the full cell |
| P0 | A runtime error is confused with an entirely failed cell | Persist completed effects and bindings; report the exact failing statement and calls that never began | `Work before line N completed; the failing statement and later calls did not.` | Tests with effects before and after throws, rejections, cancellation and returns |
| P0 | Undefined names, reserved bindings or unavailable globals waste turns | Resolve against live handles and host declarations before execution; list only valid nearby names | `` `name` is unavailable; use one of: … `` | Mutation corpus for misspellings, stale names, `console`, `result` and reserved tool names |
| P0 | Tool arguments use the wrong shape or result field | Validate against the runtime schema and return expected fields plus closest matches | `` `read` returns `content`; `text` is not a declared field. `` | Cross-model invalid-call corpus repairs on the immediately following turn |
| P0 | Shell and source literals are damaged by JavaScript interpolation or quoting | Support string, line-array and uploaded-handle forms; detect template interpolation in literal script payloads before execution | `The literal contains JavaScript interpolation; send it as lines or a file handle.` | Shell, Python, JSON, YAML and source fixtures containing quotes, dollars, backticks, heredocs and Unicode |
| P0 | Pane's tests confirm a convenient fixture but miss the real platform boundary | Derive boundary probes from touched behavior: logical/physical paths, symlinks, linked worktrees, permissions, case and line endings | `Local checks passed, but the platform-boundary probe for … has not run.` | The benchmark's `/var` versus `/private/var` defect is caught before completion; Linux and Windows counterparts exist |
| P0 | The model declares success without observing the requested validation | Track requested acceptance evidence and executed checks; label unobserved claims in the completion candidate | `Completion is unverified: the requested check … has not succeeded.` | Action tasks cannot finish as verified when their named test never ran or failed |
| P0 | Repeated failures consume many requests without progress | Detect repeated error fingerprints, identical calls, unchanged diffs and recurring no-op edits; interrupt after the configured recurrence threshold, never because of token spend | `This failure repeated without progress; change approach before running another cell.` | Planted loops are caught within two recurrences, with no false stop on productive retries |
| P0 | The supervisor is implemented but silently inactive or uses an unproven model | Resolve an eligible lower-cost model from the active Glasshouse entitlement or an explicit configuration, show every look and its model, and stay visibly off when none qualifies | `Supervisor noticed repeated work; verify the last assumption before continuing.` | A real subscription session shows metered supervisor looks and catches a planted loop; the chosen model is distinct and cheaper than the task model |
| P0 | Pane completes on an inspection object or intermediate structured value | Classify structured values as notebook output and continue; only explicit completion forms end the request | `The value was recorded as output; the user request still needs an answer.` | Foreground and subagent tests for arrays, objects, handles and scalar/string completion |
| P0 | A model misstates whether a tool ran | Compare assistant claims with the cell event ledger before accepting completion | `The claimed tool call did not run in the recorded cell.` | Planted false-success responses are rejected without hiding the original transcript |
| P0 | Filesystem identity is ambiguous | Expose logical and canonical project, Git common-dir and temporary roots as typed environment fields | `The logical and physical paths differ; compare canonical identities.` | macOS alias, symlinked checkout, linked worktree and unrelated-repository cases |
| P0 | Pane lacks enough repository orientation to choose the right first action | Build a deterministic project object from OS, architecture, time, shell, executables, Git state, key files and scoped instructions | `Project context changed; refresh the project object before continuing.` | Cold-start tasks navigate representative Rust, Python and mixed repositories without broad directory dumps |
| P0 | Edits are made from incomplete or stale source understanding | Serve the complete target definition plus imports, callers, tests, omissions and a version-bound exact edit | `The source version changed; refresh context before editing.` | Stale, ambiguous, large-function and cross-file edits fail closed and recover in one turn |
| P1 | Broad reads and prints flood context | Promote edit-oriented reads to symbol context, paginate oversized data and retain the full value behind a handle | `The output was bounded; inspect the named handle or request the next page.` | Long-ledger tasks show bounded context without hiding required evidence |
| P1 | Persistent handles outlive the evidence they name | Track provenance and version; mark stale handles without replaying their payload | `Handle X is stale; refresh its source before use.` | Resume, external edit and task-reset tests preserve or invalidate handles correctly |
| P1 | A shell command exits nonzero but the model treats stdout as success | Make exit status and signal first-class and classify the tool outcome independently from printed text | `The command failed with exit N; its stdout is not success evidence.` | Commands with misleading stdout, stderr-only failures and signals |
| P1 | Permission failures cause repeated attempts to widen access | Return the exact denied operation and governing grant; suppress equivalent retries in the same task | `Permission denied by rule R; choose an action inside the existing grant.` | Equivalent command spellings cannot create a retry loop or widen authority |
| P1 | A tool plan is shown as though every speculative branch executed | Correlate predicted call trees with actual events and mark each branch ran, skipped, failed or cancelled | `The planned branch was skipped because its guard was false.` | Conditional, parallel, early-return and rejected-call visualization tests |
| P1 | A completed edit is invisible or hard to assess | Snapshot touched files around the turn and render bounded programmatic diffs outside model context | `Pane observed N changed files; inspect the diff in the cell view.` | Create, modify, delete, rename, binary and capture-limit cases; zero model tokens added |
| P1 | Long sessions lose the current user's task among prior turns | Give each request a fresh runtime marker, compact completed history and preserve unresolved evidence explicitly | `This is a new request; earlier cells are history unless referenced here.` | Multi-request spam tests maintain the latest request and retain named unresolved constraints |
| P1 | Context exhaustion is confused with cumulative spend | Meter current/peak prompt footprint separately from uncapped cumulative spend; compact before the provider limit | `Context is near its limit; Pane compacted completed history and kept active evidence.` | Long-run tests continue past large cumulative spend without a token-based stop |
| P1 | Transport truncation produces a plausible partial answer or cell | Reject `max_tokens` and incomplete JSON as incomplete; preserve a bounded diagnostic and execute nothing | `The provider response was truncated; no partial cell ran.` | Truncation at every JSON/token boundary across supported transports |
| P1 | The user cannot inspect why Pane continued or stopped | Preserve scrollback, per-cell code/results, actual tool tree, diffs, errors and completion reason | `Open cell N to inspect its code, calls, results and changes.` | PTY tests plus a live session at narrow and wide terminal sizes |
| P1 | The supervisor gives vague model advice for a mechanical error | Route syntax, schema, process and filesystem facts directly from host validators; reserve supervisor calls for semantic drift | `Pane caught a mechanical failure locally; no supervisor request was needed.` | Ledger attribution proves zero supervisor calls for mechanically decidable failures |
| P1 | Runtime diagnostics grow into another prompt | Keep diagnostics event-local, deduplicate repeated wording and compact resolved failures out of future context | `The earlier failure is resolved and was removed from active context.` | A repaired ten-turn session does not carry obsolete diagnostics in every request |
| P2 | One scripting representation dominates the workflow | Offer typed process execution, line arrays, exact edits, file handles and ordinary strings under one result model | `Choose the representation that preserves the payload without re-encoding it.` | Equivalent tasks completed through several scripting styles and at least three models |
| P2 | Tool output formatting is unreadable | Render tables, diffs, trees, code and structured values in the TUI without serializing them back to the model | `A richer user view is available; model context remains unchanged.` | Snapshot tests and live review show no context-token increase |
| P2 | Cache behavior and compaction obscure efficiency | Record uncached, cached, cache-creation and output tokens where reported, with coverage labels | `Usage is partial because the provider omitted …` | Accounting reconciles with gateway rows and never invents missing classes |
| P2 | A benchmark rewards a fast wrong answer | Score external correctness first, then requests, wall time, context, spend, retries and intervention | `This attempt is incorrect; efficiency is reported only as diagnostic data.` | Several tasks, users, scripting styles, models and repeated trials; assisted runs labeled separately |
| P2 | A benchmark runner wastes requests while nobody watches | Stream progress to a visible surface; alert on repeated fingerprints and stop a diagnostic run when it cannot add evidence | `The diagnostic run stopped after repeated identical failure evidence.` | Watch records show the first repeated fault was acted on within two recurrences |
| P2 | Model-specific quirks leak into permanent behavior | Maintain a provider/model failure corpus outside the prompt and replay it against validators | `Pane recognized a known output shape and applied the protocol rule.` | At least 100 varied recorded outputs replay with the same safety and completion outcomes |

## Completion rule

Phase 61H is complete only after every P0 row has independent evidence, the
matched ruler contains several tasks and repeated trials, and no benchmark
requires incident-specific system-prompt text. A fast assisted recovery is
useful evidence but never counts as an autonomous win.
