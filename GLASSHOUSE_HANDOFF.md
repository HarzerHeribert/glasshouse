# Glasshouse implementation handoff

Last updated: 2026-08-26 (Europe/Berlin)

## Current capability / phase

**Phase 9G, 2C, 9B, 9C and 9D are COMPLETE.** 9E eleven of thirteen; 2D six of
nineteen; Phase 9 five of seven; 9F eleven of thirteen; 9A nineteen of
twenty-six; **388 checked boxes (30%).** Local suite **1254 passing**.

**Phase 9H, 9I and 21B landed in one round.** Sticky gateway routing and
free-pool routing were both untouched before it; 21B is complete at 11 of 11.

## Next action

Three pieces, in this order. All three are small and fully specified — the
expensive part of each was done by the batch that found it.

1. **The disposable wiring — one line, four boxes.** `main.rs::report_hook_with`
   takes `impl Fn() -> Box<dyn ExtractionModel>` and production passes
   `NoExtractionModel`. Swapping in a `DisposableRouting`-backed model closes
   Phase 9I's 530, 531, 532 and 540, all four of which are stranded on that one
   absent caller. `ExtractionModel` is `Send + Sync` and synchronous by design.
2. **Migration 7.** Own `database.rs`, `events/mod.rs` and `events/log.rs`
   **together** — they cannot be split, because
   `every_lifecycle_event_kind_is_one_the_schema_accepts` asserts the enum and
   `LIFECYCLE_EVENT_KINDS` are equal in both directions. Rebuild
   `lifecycle_events` to admit `gateway_backend_changed` (SQLite cannot ALTER a
   CHECK, so it is rename/recreate/copy/drop plus the index and three triggers),
   add the `LifecycleEvent::GatewayBackendChanged` variant, and **prove `seq`
   survives the rebuild** with a test that stores a memory's event range across
   it — `memories.source_event_first/_last` now reference that column, so a
   renumbering silently re-points every extracted memory's provenance. This
   makes Phase 9H's line 515 durable across a restart.
3. **The Linux pty flake.** A standing debt on the only gate this project has —
   practice §34 and §40. The fix is a bounded wait on the observation, the same
   treatment `integrations/version.rs`'s ETXTBSY race got. Two different pty
   tests have now failed nondeterministically, and the same tree has passed and
   failed the same leg on consecutive runs.

**What is blocked and by what.** Phase 21's `809` (a configurable cheap or
local model) and `817` (extraction after task completion) both wait on Phase
39, and will close together: the trigger is built, proven and reachable, and
dead-ends every time because nothing can supply a model at a turn boundary.
`818` (extraction around compaction) is blocked two phases deep — Phase 7 line
307 and Phase 8 line 324 — because Glasshouse cannot observe a compaction from
either harness today.

**Before sizing any packet, read practice §32 and §36 together.** §32: put the
caller's file in the partition. §36: name the function that will *ask* the
policy, and check it is being built *for that purpose*. Batch 22–23 failed at
both ends — a policy package whose every miss was an absent consumer, and a
wiring package whose misses are an absent callee. A package fails at whichever
end of the chain the partition did not reach.
