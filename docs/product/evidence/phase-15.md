
---

# Line 740 — refused 2026-08-30, and the refusal found a defect

Package `GH-INTERVENE-BEFORE-ACT`; report in
`.agent-runtime/report-intervene-before-act.md`. **Tests only**, +484 −1 in
`tests/worker_access.rs`, pinning the findings so the refusal is falsifiable.

The register recorded 740 as *"an ordering claim over 745; unreachable while
745 is [open]"*. 745 closed the same day, so the packet went out to find whether
the ordering could now be proven. It cannot, and the reason is sharper than the
old blocker.

## Why it is refused: neither event is observable as the thing the line names

> *"Preserve the user's ability to enter and modify a worker session **before
> the orchestrator acts on its result**."*

- **"the user … modify"** — recorded, but **never as the user's**.
  `SessionApi::send_text` (`session/api.rs:129`) and `SessionApi::interrupt`
  (`:139`) hard-wire `MessageOrigin::Machine`, and they are the **only** write
  path this door has. A person running `glasshouse api send` and an
  orchestrator issuing `Request::SendMessage` produce log rows equal field for
  field. The *enter* half writes nothing at all, which
  `reading_a_worker_changes_nothing_about_it` asserts as a feature.
- **"the orchestrator acts on its result"** — **not represented.** The only
  moment recorded is its converse: Glasshouse handing a result *to* an
  orchestrator (`pump_watches` → `SessionApi::send_text`, `api/unix.rs:2041`).
  What the orchestrator then does leaves no row, and one that polls
  `Request::Events` instead of registering a watch leaves not even the handoff.

An ordering guarantee needs both events to exist. One is unattributable and one
is absent.

## And nothing *preserves* the window

The packet asked the question in the form that decides it — **preserve, or
merely fail to prevent?** The answer is *merely fail to prevent*. There is no
interlock, hold, priority or reservation on this path. `pump_watches` runs on
the door's own 50 ms tick and takes the same plain `Mutex<SessionRuntime>` a
user's `api send` takes. **A mutex is mutual exclusion, not an ordering
guarantee.**

`RoutingOverride::hold()` — the candidate mechanism the packet named — has no
production caller, which `validate_round.py` flagged before dispatch.

## The defect this exposed, and it is partly mine

`MessageOrigin::UserKeystroke` **is** used in production — the TUI attributes
correctly (`session/runtime.rs:980`, `shell/state.rs:876`). The API door does
not.

That was harmless while nothing human reached the door. **`glasshouse api send`
shipped earlier the same day** (`f92deaa`, `18fb7a6`), so a person's keystrokes
now travel a path that logs them as machine-originated. The event log cannot
distinguish a human intervention from an orchestrator's, which is exactly the
distinction 740 needs — and which `memory/extract/lifecycle.rs:187` also reads.

**Next package: give the door an origin.** Small and well-specified: the
request carries it, the client sets it, `SessionApi` stops hard-wiring it. Note
it is an *attribution* boundary, not a security one — same user, peer-credential
checked — so a caller that lies is out of scope.


---

### Line 740 — closed 2026-09-06: the wake-up does not close, lock or mark read-only the worker it reports on

State: **COMPLETE** — `GH-PROVE-IT-BATCH-2` (Sonnet, Green, tests only; report `.agent-runtime/report-prove-it-batch-2.md`). The refusal above rested on 745; 745 closed with `GH-WORKER-READ`, and the register's disputed row was re-derived on 2026-09-06 (`refusal-register.md`, *Rulings on the census's sixteen disputed rows*).

Contract: given a worker session whose turn has ended and whose completion has reached the orchestrator through the wake-up flow, when a message is sent to that worker, Glasshouse delivers it and the session is still not closed, while preserving that nothing in the wake-up path mutates the worker's lifecycle.

Production: the `Stop` hook → `watch_worker`'s notification (the fixture of `a_workers_completion_reported_by_a_lifecycle_hook_wakes_the_orchestrator`) → `api/unix/sessions.rs :: send_through_pane` through the JSON door. Test: `worker_wakeup::send_message_still_delivers_to_a_worker_after_its_wake_up_completion` (`--test worker_wakeup`: 10 passed). Green: no mutation owed; the worker's hand-made removal (a stopped worker reported as `closed`) was reverted without a gate run and is not counted. Limit: proves the wake-up path; an explicit `close_session` elsewhere is out of this line's scope.
