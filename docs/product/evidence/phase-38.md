# Capability evidence — phase 38

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 38 — quota-preserving routing: 1606 and 1612 CLOSED, 1610 REFUSED (from `GH-SUBSCRIPTION-PRESSURE`, 2026-08-31)

- **1606** ☑ — the reserve band is admitted for Heavy/Frontier work and denied below when an adequate available alternative exists: *"reserve scarce premium-session capacity for difficult tasks"*, on 1571's evidence in `phase-35d.md`. Same thin spot as 1575: on the shipped binary the task tier is `None` until `launch-classifier` lands; the conservative branch is binary-proven.
- **1612** ☑ — `routing/pressure.rs` names no harness, provider template or model family (whole-word source scan), and every knob it reads is configuration (`routing.reserve.*`, `routing.capacity_band_thresholds`, `providers.<p>.quota.reserve_percent`, the last proven on the binary to move a destination between bands).
- **1610** ☐ **REFUSED** — line 1294's standing refusal: nothing in this build observes that a task is nearly complete; `reserve_verdict` passes `task_nearly_complete: false` with the refusal cited, and `the_policy_does_not_invent_task_completion` kills the fabrication.
- 1607, 1608, 1609, 1611 — open; see the refusal register and the next disposable-router package.

Full entry: `phase-35d.md`.
