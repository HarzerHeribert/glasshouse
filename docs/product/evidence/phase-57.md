# Phase 57 — Context firewall: tool-output compaction between harness and model

Opened 2026-09-01 from the user's instruction of record (the full spec is
summarized in the map's phase rationale and the architecture is recorded in
`docs/product/design-decisions.md` §Phase 57). No line is closed yet.

**Standing constraints every package in this phase inherits:**

- **Never generate evidence.** A forwarded result is reconstructed from
  original bytes by candidate id; reducer text appears only as clearly
  marked metadata. A package that lets generated text stand in for tool
  output has failed regardless of its tests.
- **Fail open, never empty.** Any reducer failure forwards the original
  with a recorded bypass reason.
- **False negatives outrank false positives.** The fixture family for the
  semantic lines must include the one-relevant-line-in-thousands case, and
  a reducer package passes only if that evidence survives.
- **The hook-replacement premise is UNVERIFIED against the installed
  harness** — `docs/process/harness-hook-protocol.md` records "no hook
  return field carries a substitute tool result" from this project's own
  earlier experience; map line 1994 therefore requires session-start
  verification with fallback to shadow. Do not build on the replacement
  premise without the recon evidence.

# Lines 1980–1990 — COMPLETE 2026-09-01; the core and the deterministic ladder

Package `GH-FIREWALL-CORE` (Sonnet, high, Amber; batch 71). Wave gate: 113
targets, zero failures, rustdoc clean. `src/firewall/` (model, estimate,
eligibility, reduce, provenance, store, adapter) behind `glasshouse
context-firewall hook|show`, with one shipped-binary test per line and two
KILLED mutations (the blank-line conservatism decision; the hard-blocked
eligibility decision).

- **1980** normalized `ToolResult` — only `adapter.rs` sees Claude Code
  JSON; two shapes recognized (uniform text; a structured Bash shape),
  anything else bypasses.
- **1981** passthrough below `--passthrough-tokens` (default 4000,
  chars/4 estimator — no existing estimator fit; documented heuristic),
  byte-identical and header-free.
- **1982/1983** one mechanism at line granularity: exact-duplicate-line and
  blank-run collapsing plus unbroken-blob elision — covering duplicate
  hits, repeated logs, progress lines, and stack traces by generalization;
  exact-match only, so anything differing by one byte survives. The
  flagship needle test: one relevant line among thousands of duplicate
  hits survives.
- **1984/1985** per-session content-addressed write-once store under the
  state dir (`gh-tool://<session>-<content>`), `show <id>` round-trips
  byte-identically, and every forwarded byte is a verbatim slice
  (containment-tested).
- **1986** one provenance header on reduced results only.
- **1987 — ORCHESTRATOR RULING RECORDED.** The map line asks token counts
  through the evidence ledger; the ledger's own contract
  (`with_tokens`: "counts a provider reported"; confidence: "never a
  derived estimate") forbids chars/4 figures in the token columns. The
  worker stopped exactly as instructed and the ruling is: rows carry
  purpose/tool/bypass with token columns honestly NULL; the counts are
  recorded in the provenance header and `Outcome::Reduced`; the semantic
  reducer's PROVIDER-REPORTED tokens (57B) will populate the ledger's
  token columns for reducer calls, which is the integration the line's
  spirit wants. Closed with this limit standing.
- **1988** `CONTEXT_FIREWALL_EXPANSION_PURPOSE` rows on every `show` (a
  third purpose beyond the packet's stated two — the honest deviation,
  flagged, accepted: folding expansions into reduction/bypass would erase
  the recall signal the line names as primary).
- **1989** eligibility flags with a hard block no flag lifts (Edit, Write,
  MultiEdit, NotebookEdit, permission/security-named tools).
- **1990** Bash reduced ONLY when a structured stdout/stderr/interrupted
  shape is present; the uniform text shape bypasses for Bash specifically —
  the conservative consequence of the recon's "uniformly text" finding.
  The real Bash payload capture is GH-FIREWALL-BRIDGE's named task for
  1995; non-zero-exit output is never reduced.

Also in batch 71, infra: GH-FIXTURE-REUSE shared 20 fixture writers across
the three worst suites (pty_smoke 76 tests, launch_preflight 12,
session_supervision 23 — median pty_smoke wall-clock 7.77s→5.13s), found
and fixed a path-coupling bug its own conversion surfaced, and named the
argv-log hoist follow-up for the nine no-win files. New known-flaky note:
`v1_criteria_setup::v1_1907` fails 2-of-3 ALONE on a quiet machine — a TCP
listener race, not load; it predates batch 71 and needs its own fix worker.

# Lines 1991–1996 — COMPLETE 2026-09-01; the Claude Code bridge, on verified facts

Package `GH-FIREWALL-BRIDGE` (Sonnet, high, Amber; batch 72). Integration
gate: 101 green targets, rustdoc clean; the one red (`events_lifecycle`,
flake family, untouched) 8/8 twice alone. Two mutations KILLED
(shadow-emits-anyway; version-floor inverted). 11 shipped-binary bridge
tests stable ×4, 144 unit tests across firewall/config/harness modules.

- **1991** four modes on `[context_firewall]` (project→user→default-off
  layering mirroring guardrails); off is byte-identical by construction —
  the registration function returns before touching anything, and `args`
  is never touched in any mode; shadow runs the full pipeline and provably
  never emits `updatedToolOutput`.
- **1992** no mode names a reducer; the config has no reducer field to
  misuse, plus a tripwire test on the registered command line.
- **1993** registration merges ONE `PostToolUse` key into the session
  settings document Glasshouse already owns — never the user's files,
  never other hooks. The packet's "inject a second `--settings`" reading
  was refused on a pre-verified fact: `claude --settings A --settings B`
  silently discards A (design-decisions §57 addendum 2).
- **1994** version probe (`MIN_UPDATED_OUTPUT_VERSION = 2.1.252`, dated)
  with shadow fallback, one stated line, one bypass telemetry row; probed
  per launch, not per tool call.
- **1995** the REAL Bash payload captured from the installed harness:
  `{stdout, stderr, interrupted, isImage, noOutputExpected}` — no
  `exit_code` key, and failing Bash never reaches PostToolUse at all
  (PostToolUseFailure carries it). `confirmed_clean_exit` corrected:
  explicit non-zero refuses, absent does not — the previous `Some(0)`
  requirement made Bash permanently un-reducible against the real
  harness. Captured payloads are fixtures at unit and shipped-binary
  level. Grep/Glob/Read stay the recon-verified uniform text shape.
- **1996** two concurrent sessions through the REGISTERED path: separate
  stores, separate telemetry, no mixing.

Phase 57 stands at 17/27: the remaining ten are 57B (the semantic reducer
as a disposable job, 1997–2003) and 57C (expansion granularity, shadow
evaluation, status surfaces, 2004–2006).

# Lines 1997–2003 — COMPLETE 2026-09-01; Phase 57 at 24/27

Package `GH-FIREWALL-REDUCER` (Sonnet, high, Amber; batch 76). The semantic
rung of the ladder. 1,559 insertions across eleven files; the deterministic
core (batch 71) and the harness bridge (batch 72) are untouched beneath it.

**1997 — a disposable support job, never a firewall-private client.**
`JobKind::ContextReduction` routes through `DisposableRouting` unchanged, so
Phase 39's job-kind roster, Phase 9I's free-pool routing and Phase 56A's
per-entitlement `deny_job_kinds` all apply to the reducer without a line of
new policy. **The `disposable_interface.rs` variant-roster tripwire fired by
design** and was updated to the new set; `support_work_economy.rs` carried a
second exhaustive `JobKind` match and was updated the same way (declared as
scope overflow, and correct — it could not compile otherwise).

**1998 — the reducer cannot be handed a transcript, because there is no
field for one.** `ReductionRequest` has exactly four: `task`, `tool_name`,
`tool_query`, `candidates`. Its own doc comment states the guarantee —
*"there is no field here able to carry one"* — which is the structural form
of the rule rather than a convention the code has to remember.

**1999 — rebuilt from trusted originals by id.** `Verdict` carries ids,
never text; `rebuild()` copies candidate text verbatim. Mutation
*rebuild-generates-text-instead-of-copying-by-id* — KILLED by
`firewall::tests::semantic_reduction_never_introduces_text_absent_from_the_original`,
*"forwarded line `[GENERATED-0][GENERATED-1]` is not a verbatim slice of the
original"*. **Re-run by the orchestrator on the merged tree and KILLED
again by the same named test** — this is the subsystem's whole safety claim
(*it reduces and ranks; it never generates*) and it is the one worth paying
twice for.

**2000 — thresholds biased toward inclusion.** Safe mode keeps an uncertain
candidate; aggressive mode drops it only when configured to, and says so.
Mutation *uncertain-always-dropped-in-safe-mode* — KILLED by
`firewall::tests::safe_mode_forwards_a_needle_the_reducer_marked_only_uncertain`.

**2001 — fail open on every reducer failure.** Timeout, transport, rate
limit, schema, validation and outage each fall back to the deterministic
result with a recorded bypass reason, never to an empty one. Mutation
*fail-open-replaced-by-empty-result* — KILLED by
`firewall::tests::a_timed_out_reducer_fails_open_to_the_deterministic_result`,
*"a failed reducer must never lose the deterministic result"*.

**2002 — pinned models and free-router aliases** through the existing
provider and free-model configuration, with reducer output validated
regardless of which model a router answered with.

**2003 — privacy before transmission, and the ORDER is the capability.**
`privacy_blocks_reduction(file_paths)` is evaluated **before** the reducer
runs (`firewall/mod.rs`: *"Order matters: the privacy gate (2003) is checked
before the …"*), so a secret-shaped path suppresses semantic reduction
rather than being noticed after the bytes have left. Pinned by
`a_secret_shaped_path_suppresses_semantic_reduction`. Tool-input path
parsing lives in `firewall/adapter.rs` — declared scope overflow, and
correct: `design-decisions.md` confines all Claude Code JSON parsing to that
file, so no other file could have added it.

**1625 is NOT unlocked by this, and the worker said so first.** Phase 39's
refusal was blocked on *"no reduction-shaped JobKind"*; one now exists, and
1625 still names **reranking** — a job that reorders candidates — which this
does not provide. The refusal stands on its own terms. The worker raised
this itself rather than reaching for a nearby box.

**Gates.** Worker: `--lib firewall::` 74 passed, `--lib config::` 95 passed,
`context_firewall` 13, `firewall_bridge` 11, `disposable_interface` 7,
`support_work_economy` 13, `firewall_reducer` 9 — all 0 failed; clippy
`-D warnings`, fmt and rustdoc clean; its own full sweep green except
`tests/handoff_lines.rs`, attributed as a flake that passed alone twice with
the file untouched — **an attribution the orchestrator reached independently
on a different run** (a fresh-binary spawn timing out under load). Merged
tree: `blast-radius.sh --targeted` — every traced target passed, twelve
targets quoted with counts.

**Phase 57 stands at 24/27.** The remaining three (2004 expansion
granularity, 2005 shadow-vs-original comparison, 2006 mode and per-session
savings) have their Phase −1 established and are the next package.

# Lines 2004, 2005, 2006 — COMPLETE 2026-09-01; **PHASE 57 CLOSED 27/27**

Package `GH-FIREWALL-OBSERVABILITY` (Sonnet, high, Amber; batch 77). The
firewall's observability half, and the last three boxes of the phase: getting
a suppressed result back, proving the reduction was safe, and showing what it
saved.

**2004 — expansion at four granularities.** `context-firewall show` already
returned a whole result by its `gh-tool://` reference; it now also expands by
candidate id, by file, and by line range, through the same subcommand rather
than a second door — the line's own *"supported Glasshouse surface rather
than an invented side channel"*. Every bad input refuses clearly: an unknown
reference, an out-of-range candidate id, a file not in the result, a reversed
range. Mutation *skip-refusal-fallback-to-whole* — returning the whole entry
instead of refusing an out-of-range id — KILLED by
`line_2004_an_out_of_range_candidate_id_refuses_clearly`. That refusal is the
security-relevant half: the raw store holds unredacted output, and a
too-generous expansion is how it would leak.

**2005 — shadow comparison against the forwarded original.** A shadow run
records both sides — `original_tokens`/`forwarded_tokens` and
`retained_candidates`/`total_candidates` beside the `raw_ref` that lets a
reader check for themselves — so recall and savings rest on recorded evidence
rather than on a compression ratio. **And shadow stays shadow**: the harness
receives the original, byte-identically. Mutation *shadow-emits-reduced-anyway*
(dropping `&& mode != FirewallMode::Shadow` from the emit guard) — KILLED by
`line_2005_a_shadow_mode_run_records_both_sides_and_forwards_the_original`,
*"shadow mode must never substitute anything, whatever the flag says"*.
**Re-run by the orchestrator on the merged tree and KILLED again by the same
named test.**

**2006 — mode and savings in `status`, from the RIGHT source.** The packet
constrained this one hard and the constraint held: the ledger's
`input_tokens`/`output_tokens` columns are documented as *a provider's own
reported count* and are NULL for firewall rows by ruling, so savings could
not come from there. They come from **`RawStore::all_entries`** — the
per-session originals with their `original_token_estimate`, already durable
— rendered as a session count, a result count and estimated tokens kept
local. `git diff` confirms the package added **no** write to a provider token
column anywhere. The section is absent entirely when the firewall is off
(*"without cluttering the primary workflow"*), and says "no activity yet" when
on but unused.

**Gates.** Merged tree: `--lib firewall` 30 passed, `--lib config` 81 passed,
`--lib` (cli) 8 passed, `firewall_observability` 16 passed — all 0 failed;
`blast-radius.sh --targeted` every traced target passed. Scope was clean: the
package touched only `cli.rs`, `firewall/mod.rs`, `firewall/store.rs`,
`main.rs` and its own new test file, and never opened
`routing/evidence.rs`, which was forbidden to it because a peer held it.

**One report inaccuracy, minor and worth recording.** The mutation entry
described the shadow guard's `change:` without naming its file, and the
orchestrator's first re-run attempt aimed at `firewall/mod.rs`; the guard is
at `main.rs:1872`. `mutate.sh` refused loudly (*"find string occurs 0
times"*) rather than silently mutating nothing — the tool behaving exactly as
designed, and the reason a re-run is cheap to attempt.

## Phase 57, end to end

Twenty-seven boxes across four batches in three days: the deterministic core
and raw store (71), the harness bridge and shadow mode (72), the semantic
reducer (76), and observability (77). The subsystem is off by default,
provider-agnostic, harness-abstracted, fail-open, and measured.

**TWO properties are enforced by SHAPE — corrected 2026-09-01 after an audit,
because the first version of this paragraph said three.**

- The reducer **cannot be handed a transcript**: `ReductionRequest` has four
  fields and none can carry one. A violation does not compile.
- It **cannot generate**: `Verdict` carries ids, `rebuild()` copies candidate
  text verbatim, and the containment test compares against the original.
- **2002's "regardless of which model a router answered with" is also
  structural**, and was not claimed before: `reducer::validate(verdicts,
  candidates)` (`reducer.rs:237`) takes **no model parameter**, so validation
  cannot vary by model even in principle.

**2003's privacy gate is NOT structural, and saying it was is an overclaim
this ledger made.** `GH-AUDIT-BATCH-76-77` read the gate and was right:
`firewall/mod.rs:337-348` is an ordinary `bool` from an `&&` chain, and a
refactor that dropped `!reducer::privacy_blocks_reduction(...)` — or turned
the chain into a match missing that arm — **would compile cleanly** and route
secret-shaped paths into the reducer.

What is true, and what the audit could not know because the check did not
exist when it looked: the orchestrator then ran that mutation. **`&& !reducer::privacy_blocks_reduction(semantic.file_paths)` → `&& true`:
KILLED** by `firewall::tests::a_secret_shaped_path_suppresses_semantic_reduction`
(panic at `firewall/mod.rs:941`). `GH-FIREWALL-REDUCER`'s three mutations
were never-generate, safe-mode-uncertain and fail-open — **none touched the
privacy gate**, so this is the fourth and it closes the gap the audit named.

So 2003 is a **runtime check with a killed mutation behind it**, which is a
real standard and the one most of this codebase meets — it is simply not the
type-level guarantee 1998 and 1999 have. The distinction matters because a
reader who believes it is type-enforced will not write the test that catches
the refactor.

**The audit's own gap, recorded for symmetry.** Its verdict table covered
fifteen of the sixteen boxes ticked that day and **omitted 2002 entirely**,
mentioning it only in passing while arguing about 2003. The orchestrator
checked 2002 separately — `a_pinned_reducer_model_reaches_the_wire`
(`tests/firewall_reducer.rs:613`), `set_reducer(provider, model)` covering
the pinned and unpinned cases, aliases arriving free through 1997's
disposable routing, and the model-independent `validate` above. **2002
HOLDS.** An auditor that silently drops a row is the same failure mode as a
report that silently drops a clause; the fix is to check the count, which is
why it is written down here.
