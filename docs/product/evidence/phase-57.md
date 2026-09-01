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
