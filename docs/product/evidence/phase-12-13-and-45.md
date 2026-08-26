# Capability evidence — phase 12-13-and-45

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phases 12, 13 and 45 — the lifecycle event bus, the session API, and failure isolation (18 of 24)

Delivered by the `lead-events` team lead (Claude Code, Opus 5, effort high)
with two Sonnet subcontractors. 25 mutations, every one run by the lead.

Contract: Given any supported harness, when it does something worth knowing
about, Glasshouse records one normalized event; a quiet or exited process is
never mistaken for a finished turn; an orchestrator can list, inspect, message
and interrupt a live session without reaching into a harness; and one worker
dying takes nothing else with it.

State: COMPLETE for 18 lines. Six stay open, listed below with what each needs.

Production evidence:
- `events/mod.rs` — `LifecycleEvent`, harness-independent by construction and
  asserted so by a source scan of the module for six harness names.
- `events/bus.rs` — the publish path never blocks on a subscriber.
- `session/api.rs` — `SessionApi::{list, state, send_text, interrupt,
  recent_output}`. Every method resolves scope **before** liveness, so a
  session from another project is refused *as foreign*, not as dead — the
  weaker ordering leaks the existence of other projects' sessions.
- `session/recovery.rs` — recovery refuses an unknown task kind exactly as it
  refuses a destructive one, and refuses to accept an event history as task
  state.
- `session/runtime.rs`, `session/lifecycle.rs` — crash classification and the
  translator.

Regression evidence:
- 40 new lib tests and 5 integration tests; workspace 973 tests, 0 failures.
- 25 mutation proofs, each restored and the restore verified byte-identical,
  in a private `CARGO_TARGET_DIR` with a `touch` before each rebuild
  (practice §16). Three worth naming, because each writes a mistake that
  *compiles*:
  - `ProcessExited { exit } if !exit.is_crash() => Some(TurnOutcome::Completed)`
    — kills `a_quiet_process_that_exited_cleanly_reports_no_task_outcome` and,
    against a real child, `a_quiet_harness_that_exits_cleanly_is_never_reported_as_having_finished`.
  - a `TurnEnded { outcome: Completed }` added to `poll_exits` gated on
    `status.success()` — the exact inference the map forbids — kills
    `turn_completion_is_minted_in_exactly_one_place`.
  - `poll_exits` killing every remaining session when one ends — kills
    `one_worker_crashing_leaves_unrelated_sessions_running`, which asserts the
    survivors still *answer input*, not merely that they are listed.

Platform evidence:
- **No CI.** The Actions quota is exhausted (practice §27). All ten checks of
  `scripts/ci-local.sh` pass — macOS natively and ubuntu in a container —
  which is five of the seven CI jobs. **Windows is unexercised**; nothing here
  is evidence about Windows.

Map/design conflict, resolved:
- Phase 12 says to preserve raw adapter event payloads in debug logs. The
  standing decision "Codex lifecycle hooks — a payload not to read" says the
  opposite about the payload that exists: it carries the user's prompt and the
  model's last message, and a test proves no field of it reaches a log.
  `RawObservation { harness, event, detail }` preserves whatever an adapter
  hands it; the two shipped adapters hand it the event name and nothing else,
  because the payload rule is *adapter* policy. Mechanism satisfies the map,
  policy satisfies the decision — **and the box stays open anyway**, because
  `observe()` has no production caller yet.

Missing evidence — the six open lines and exactly what each needs:
- *Record every translated lifecycle event with session ID and timestamp* —
  needs one call in `main.rs::report_hook`, which was forbidden to the lead.
- *Deliver lifecycle events to the TUI without blocking the harness process* —
  the bus is production-live and proven non-blocking, but **nothing in the TUI
  subscribes**. A delivery path with no consumer does not deliver, by the same
  test that left the Memory settings box open.
- *Preserve raw adapter event payloads* — as above, no production caller.
- *Deliver lifecycle events to the orchestration layer* — not claimed.
- *Preserve the most recent checkpoint after a worker crashes* — Phase 19 does
  not exist, so there is no checkpoint to preserve.
- *Detect gateway failure separately from harness-process failure* — the lead
  rated this its weakest claim, noted it was not separately mutated, and said
  it would not object to it staying open. Taken at its word: an unmutated
  claim is not proof here.
