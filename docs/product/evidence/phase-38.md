# Capability evidence — phase 38

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 38 — quota-preserving routing: 1606 and 1612 CLOSED, 1610 REFUSED (from `GH-SUBSCRIPTION-PRESSURE`, 2026-08-31) — **1610 CLOSED 2026-09-03, see below**

- **1606** ☑ — the reserve band is admitted for Heavy/Frontier work and denied below when an adequate available alternative exists: *"reserve scarce premium-session capacity for difficult tasks"*, on 1571's evidence in `phase-35d.md`. Same thin spot as 1575: on the shipped binary the task tier is `None` until `launch-classifier` lands; the conservative branch is binary-proven.
- **1612** ☑ — `routing/pressure.rs` names no harness, provider template or model family (whole-word source scan), and every knob it reads is configuration (`routing.reserve.*`, `routing.capacity_band_thresholds`, `providers.<p>.quota.reserve_percent`, the last proven on the binary to move a destination between bands).
- **1610** ~~☐ **REFUSED**~~ → **☑ CLOSED 2026-09-03.** The refusal read: *line 1294's standing refusal: nothing in this build observes that a task is nearly complete; `reserve_verdict` passes `task_nearly_complete: false` with the refusal cited, and `the_policy_does_not_invent_task_completion` kills the fabrication.* **That was correct, and it is what the closure had to answer rather than sidestep.** Nothing in this build *observes* task progress still — the producer is a **declaration**, not an observation. See the entry at the end of this file.
- 1607, 1608, 1609, 1611 — open; see the refusal register and the next disposable-router package.

Full entry: `phase-35d.md`.

---

## Task progress is declared — lines 1294 and 1610 closed together, 2026-09-03

One package, `GH-TASK-PROGRESS` (Opus, **Red** — migration 28 and a persisted, session-scoped row), worktree `.worktrees/task-progress`, packet `.agent-runtime/packet-task-progress.md`, report **`.agent-runtime/report-task-progress.md`**. The design ruling it implements is `design-decisions.md`, *A task's progress is declared, never guessed*.

**The two lines are one mechanism seen from two phases**, which is why they closed together and why this entry is the same in both ledgers: 1294 is the reserve-threshold guard (`provider/quota/mod.rs :: evaluate_reserve_spend`, its *first* branch), 1610 the quota-conservation guard (`routing/pressure.rs :: reserve_verdict`).

**What the earlier refusal got right, and what changed.** The standing refusal was that *nothing in this build observes that a task is nearly complete*, and that a turn-count or elapsed-time proxy would report "almost complete" for work that had merely been running a while — inverting the protection at the one moment it matters. **That is still true and nothing here weakens it.** No signal Glasshouse already observes was touched; the event vocabulary is unchanged, and `reserve_inputs::the_event_vocabulary_cannot_express_almost_complete` still passes untouched — it is now the *reason* a declaration was the only honest source rather than the evidence for a refusal. The producer is a person or orchestrator saying so on purpose, through `glasshouse task-progress --session <id>`, and the statement expires.

**The three properties the design required, and how each is held:**

1. **Never infer.** The only thing that sets the field true is a declaration. A source scan forbidding the words `turn_count`/`elapsed` was written and **removed**, because it failed on the module's own doc comments explaining why such a proxy inverts the policy — a pin that punishes stating the invariant is worse than no pin.
2. **Scoped and expiring, never sticky.** The source is a store row, not a configuration value: a settings value is sticky by nature, and a sticky declaration re-creates the inversion by the slower route. `TASK_PROGRESS_EXPIRES_AFTER` is **30 minutes and deliberately shorter than `STALE_CLAIM_AFTER`**, with the asymmetry argued at the constant — expiring early falls back to today's behaviour, expiring late keeps a dead statement outranking every other signal the policy has. A `const _: () = assert!(…)` **fails the build** if the two are ever made equal.
3. **A default that changes nothing.** `DeclaredTaskProgress::default()` can never match — no constructor means "everywhere", and `deciding_for` is `None` for every caller predating these lines, exactly as `ReserveOverride` arrived as a no-op for line 1290.

**Both production construction sites are fed, and that was proven by mutation rather than by reading.** `routing/disposable/mod.rs`'s per-candidate loop and `routing/pressure.rs :: reserve_verdict` both read the declaration; `commands/routing_destinations.rs :: session_router` is the one constructor every real ranking goes through, without which the field would be wired structurally and always false in production — `cluster-b.py`'s shape.

Six mutations, **all KILLED**: `guard-does-not-fire`, `declaration-never-expires`, `drop-scope-predicate`, `drop-liveness-check`, `disposable-site-unfed`, `pressure-site-unfed`. The last two are the ones that matter most — they prove each site independently, and either surviving would have meant a site with no test.

Gates: fmt, `cargo check --all-targets`, clippy `-D warnings`, rustdoc `-D warnings`, `check-doc-boundary.sh` and the size ratchet all clean; `--test task_progress` 20/20, `--test subscription_pressure` 18/18, `--test reserve_inputs` 18/18, `--test capacity_score` 31/31, `--test support_work_economy` 13/13, `--test v1_criteria_routing` 8/8, `--test session_context` 18/18; `blast-radius.sh --targeted` over 27 changed files exit 0.

**Limits, and the first is the one to read:**

- **The declaration scopes to a *session*, not a task.** The lines say "task"; a disposable job carries a `JobKind`, not a task identity, so a session is the narrowest real scope this build has. `ReserveOverride` records the identical limit for line 1290, so this is consistent with the existing precedent rather than a new compromise — but a session running several tasks is protected as a whole for the horizon.
- The 30-minute horizon is a judgement argued from the asymmetry of the two failure directions, **not a measurement of real task lengths**.
- The declaration is honoured only inside `evaluate_reserve_spend` and `reserve_verdict`; no other Glasshouse decision consults it.
- `declared_task_progress_sessions` is best-effort: an unopenable database yields an empty set, so a broken database silently loses a declaration rather than failing a routing decision.
- Migration 28's rollback is proven on **macOS only** in this worktree; the trailing sweep owns the other two platforms.

**Four errors in the orchestrator's own packet, found by the worker and recorded here rather than in five places:**

1. **The packet named one source-scanning pin; there are two.** `tests/reserve_inputs.rs::nothing_in_this_build_produces_task_nearly_complete` asserts the identical refusal over `disposable/**`, `provider/quota/mod.rs` and `main.rs`+`commands/*`, is not reachable from the packet's traced targets, and had to be re-stated by the same argument (renamed `::nothing_in_this_build_infers_task_nearly_complete`).
2. **The packet said to extend the scan using practice §81's `#[cfg(test)]` boundary; applied to `routing/disposable/mod.rs` that is wrong.** Its only `#[cfg(test)]` is `mod tests;` at line 55 — the unit tests live in a sibling file — so slicing there discards ~1,470 lines of production code **including the construction site the scan exists to watch**. That is §68's shape (a filter matching nothing reads as a pass) hiding inside the fix for §81's. `disposable_production_source()` treats the whole file as production **and asserts that assumption**, so it cannot be silently kept if an inline test block ever appears. *(General lesson: after Phase 59 moved inline tests into sibling files, `mod tests;` is a declaration, not a boundary. Check where a file's tests live before writing any scan over it, and assert what you scanned.)*
3. **The migration ripple is 13 version pins and 25 rollback fixtures, not nine pins** — 4 in `src/database/tests.rs`, 4 in `src/session/store/tests.rs`, 5 under `crates/glasshouse/tests/`.
4. **A 14th pin is invisible to any comma-anchored grep**: `tests/session_context.rs:242` is a bare `27,` on its own line after `schema_version(&conn),`. The targeted blast radius caught it (left: 28, right: 27). A successor packet should say *"any bare version literal on its own line"*.
