# Capability evidence — phase 59

Phase 59 — Decompression (map lines 2043–2054), recorded from the user's ruling of 2026-09-03 (`design-decisions.md`, *Decompression*; `CLAUDE.md`, *Decompression*). Entries here are bounded by that ruling: the contract, the tests by name, the limits, and the worker's report by path. A pure move owes no mutation.

### Split the routing evidence module into the ledger that writes rows, the readers that summarise them, and the joins across tables. (line 2051)

Contract: Given `routing/evidence.rs` at 5,728 production and 2,432 inline-test lines, when the package lands, Glasshouse behaves byte-for-byte as before — every `routing::evidence::…` path still resolves and every test passes with the same count — while no file under `routing/evidence/` exceeds 2,500 production lines and the tests live in their own file.

State: **COMPLETE** — ruled 2026-09-03 by the orchestrator. Package `GH-DECOMP-ROUTING-EVIDENCE` (Sonnet high, Green pure move); report `.agent-runtime/report-decomp-routing-evidence.md`. `evidence/` is `mod.rs` (1,640: shared row types, `EvidenceLedger` and `lock`, re-exports), `ledger.rs` (123: `open`, `record`), `readers.rs` (2,071: the SQL-querying summaries), `signals.rs` (798: classification over a fetched slice — the packet's own "split once more by subject" fallback, since `readers.rs` first came out at 2,894), `joins.rs` (1,169: cross-table reads), `tests.rs` (2,440: six inline test modules moved verbatim). Every non-move hunk is enumerated in the report (five `mod` lines, three `pub use` blocks, trimmed and per-file `use` blocks, three `impl` wrappers, five `//!` headers, two `super::` → `crate::routing::` path corrections, three doc-link qualifications). `--lib routing::evidence` 62/62 before and after; clippy, rustdoc and the ratchet clean; `blast-radius.sh --targeted` exit 0; `cargo check --all-targets` proves all 74 importers resolve. Re-run on the merged tree by the orchestrator: `--lib routing` and `cargo check --all-targets`.

Limits: a move proves nothing about behaviour beyond "unchanged"; the trailing sweep on the integration commit carries the 74 importers' own tests. The comment share of these files is untouched by design — `GH-TRIM-ROUTING-EVIDENCE` is the separate package.

---
