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
