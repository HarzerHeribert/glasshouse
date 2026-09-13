# Capability evidence — phase 65

Phase 65 — Pane: smarter-and-cheaper execution (map lines 2580–2594), recorded 2026-09-13 from the user's ruling (`design-decisions.md`, *Pane wins by spending less expensive attention, not by forcing an interface*). The register is `docs/product/pane/smarter-cheaper-roadmap.md`; this file is its evidence ledger. Entries are bounded by the *Decompression* ruling: the contract, the tests by name, the decisive mutation where a decision was added, the limits, and the report by path.

**Non-negotiables every entry below is checked against.** One canonical execution kernel — a direct provider tool and an authored cell lower into the same isolate; no second executor, helper runtime, ledger or evidence class. Exact evidence stays retrievable behind every bounded or derived view. No direct-tool quota; interface choice is measured, never forced. Ordinary host security is unchanged; outer-container behaviour is separately named and fails closed elsewhere.

**Gate.** `cargo test -p pane --no-fail-fast`, `cargo clippy -p pane --all-targets -- -D warnings`, `cargo fmt -p pane --check`, `git diff --check`; the pane cells of the GitHub sweep are the platform verdict.

---

(Entries are written at integration; see the roadmap's status column for the row each line maps to.)
