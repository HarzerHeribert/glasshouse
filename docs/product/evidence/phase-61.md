# Capability evidence — phase 61

Phase 61 — pane, the first-party harness (map lines 2419–2481), approved by the user on 2026-09-05. Design of record: the session artifact *The Glasshouse Native Harness*, recorded in `design-decisions.md` (*pane, the first-party harness*). Hand-off draft: `.agent-runtime/pane/phase-61-draft.md`.

Entries are bounded by the *Decompression* ruling: the contract, the tests by name, the mutation on each decision, the limits, and the worker's report by path.

**This phase is built in its own lane.** A team lead owns `crates/pane/**` on branch `pane/integration` and pays review out of its own context; the primary orchestrator owns `main`, this file, and everything outside `crates/pane/`. Reports live under `.agent-runtime/pane/`. Merges are per sub-phase, not per packet — six across the whole build.

**Its gate is not the glasshouse gate.** `pane` is excluded from every `--workspace` invocation in `ci-local.sh` and the GitHub matrix and has its own job; `cargo test -p pane` plus that job is this phase's acceptance. That exclusion is part of the user's decision, not housekeeping: without it all twelve cells and the local gate would compile V8 on every run.

**Two orderings are not negotiable, and a reviewer should refuse a package that breaks either.** 61A comes first, because every claim below it is unfalsifiable without a ruler and the interesting one — that code-mode beats tool-calls on real work — is exactly the kind that feels true and often isn't. **61D comes before any model-authored code executes**; the alternative is a window in which generated code runs beside a keyring with nothing between them.

---

## 61A — The ruler — lines 2430–2432

State: **NOT STARTED**.

## 61B — The crate and the adapter — lines 2436–2440

State: **NOT STARTED**.

Kickoff note: the four Glasshouse-side files this sub-phase touches — the workspace manifest, `harness/mod.rs`, `scripts/ci-local.sh` and `.github/workflows/ci-extended.yml` — change exactly once, in the primary's kickoff commit, and never again. `integrate.sh` refuses any file two trees both touched, and after the kickoff there is no such file.

## 61C — The loop and the three seams — lines 2444–2451

State: **NOT STARTED**.

Owed before this sub-phase, and it is a user question rather than a worker's: whether `gateway/translate/canonical.rs` round-trips reasoning blocks byte-identically. A native harness routed through our own gateway is the case where we own both ends and have no excuse. Test it before 61C, not after.

## 61D — The sandbox — lines 2455–2457

State: **NOT STARTED**. Red tier: Opus specialist plus an independent verifier, platform code on three operating systems.

## 61E — Code over live objects — lines 2461–2465

State: **NOT STARTED**. Red tier.

This is the sub-phase the whole phase exists for, and line 2464 is the one that can honestly fail: *a measured win on at least one workload tier by 61A's ruler, or record why not*. Recording why not is a complete outcome of this line, not a failure of it.

## 61F — The supervisor — lines 2469–2471

State: **NOT STARTED**.

## 61G — Events in batches, background work, messages — lines 2475–2481

State: **NOT STARTED**. Amber: the batch window and the interrupt class are decisions; the event bus and direct session messaging (Phases 12, 13) already exist and are what this rides on inside Glasshouse.

Recorded 2026-09-05 (evening) from the ended session `glasshouse-9c`'s hand-off (`.agent-runtime/subpacket-pane-phase-61.md`) on the user's instruction; the sixth spec, `docs/product/pane/events-contract.md`, is what makes these seven lines pass Phase −1 and is dispatched as `GH-PANE-EVENTS`. The reason this sub-phase exists is measured on the orchestrator itself: one Monitor per worker plus three standing watches delivers every event as its own turn, and a harness meant to hold the orchestrator role has to coalesce.

