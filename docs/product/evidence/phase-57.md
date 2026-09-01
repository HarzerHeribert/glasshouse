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
