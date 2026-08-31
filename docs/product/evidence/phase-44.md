# Capability evidence — phase 44

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 44 — user control and override, 9 of 9 (lines 1712–1720)

Package `GH-USER-CONTROL`, 2026-08-31, Opus specialist at high effort. Nine lines
addressed, **nine closed** — 1718 by evidence where the packet had allowed a
refusal. Nine mutations, all KILLED, two of them only after a SURVIVED sent the
worker back to the code.

Contract: Given a person using Glasshouse alongside an orchestrator, when they
state where work should go — a harness, an existing session, a fresh session,
routing off entirely, a checkpoint before any move, or "leave this session alone
for now" — Glasshouse does exactly that and says what automation would otherwise
have done, while preserving the rule that a person's own keystrokes always
outrank a machine's message into the same session, and that an interrupt is
never held back by either control.

State: **COMPLETE** for 1712, 1713, 1714, 1715, 1716, 1717, 1718, 1719, 1720.

Production evidence:
- **1712** — `RoutingConfig::automatic` / `EffectiveConfig::automatic_routing`
  (layered), `launch --no-routing`; `launch_session` takes the off switch
  **before** `routing_destinations`. **Ruling accepted: off does not compute
  the ranking to report what it would have done** — the ranking opens the
  session store, the quota cache and the health cache, and (once
  `launch-classifier` lands in the same region) can reach a routing model;
  computing all of that to render one sentence would make "off" mean the same
  work, silently, plus a message about it (§65). The launch prints that routing
  is off and points at `glasshouse route`.
- **1713, 1714, 1715** — existing production behaviour (`launch <harness>` →
  `session::select::select`; `--to`/`--fresh` → `RoutingOverride`), **evidenced
  through the shipped binary against a ranking that would have chosen
  otherwise**; no code added.
- **1716** — `--checkpoint-first` on `launch` and `resume`:
  `checkpoint_before_moving` through the same `Checkpoint::capture` the two
  existing checkpoint paths use, `CheckpointReason::Manual` (a third spelling
  would have been a migration — the schema `CHECK`). It **invents nothing** — no
  objective read off the terminal. Three of its four cases are no-ops and each
  says which (a silent no-op is §68's shape in a new costume).
- **1717** — `Request::MuteSession { session, seconds }` / `UnmuteSession`,
  `glasshouse api mute|unmute`; in-process state in `ServerContext` (a restart
  of `glasshouse api serve` clears every mute — documented on the protocol, the
  command, and printed by `api mute` itself); `Interrupt` is never muted.
- **1719** — `SessionApi::send_text` refuses machine text while a person has
  put something into the same session within `USER_INPUT_PRECEDENCE` (10 s, a
  constant with its reasoning — a knob here would be a knob for turning off the
  rule). **Refused, not queued** — the seam's existing rule
  (`SessionRuntime::deliver` already refuses a concurrent delivery because
  queuing would deliver it out of the order its sender believed). The stamp is
  taken on the keyboard path (`write_to_focused`) and the interrupt path too,
  so the rule is already correct if the TUI and the door ever share a process.
  `pump_watches` **defers rather than drops** on `UserHasTheKeyboard` (winds its
  cursor back and retries next tick), so a person typing never costs an
  orchestrator a worker completion.
- **1718** — evidenced: line 745's verbs already put a person *inside* a
  running worker; 1717 + 1719 are what get the orchestrator *out*.
  `a_person_takes_over_an_orchestrated_worker_and_the_orchestrator_is_locked_out`
  drives the whole of it and asserts **which** control produced each refusal.
- **1720** — the announcement block in `launch_session` covers continued vs
  fresh, override honoured and refused, routing off, checkpoint forced — every
  sentence on stderr, before the action.

Regression evidence (`tests/user_control.rs`, 13 tests, through the shipped
binary; `route_command` 36; `routing_api` 11; `api_event_log` 8; `worker_access`
19; `worker_wakeup` 9; `session::api` lib 12):
`automatic_routing_can_be_turned_off_and_the_launch_says_so`,
`the_no_routing_flag_turns_the_ranking_off_for_one_launch`,
`pinning_a_harness_opens_that_harness_and_not_the_other_one`,
`to_and_fresh_override_a_ranking_that_would_have_chosen_otherwise`,
`checkpoint_first_leaves_a_checkpoint_for_the_session_being_left` (+ the resume
twin), `a_muted_session_refuses_machine_messages_but_not_interrupts`,
`a_persons_keystroke_outranks_a_machine_message_to_the_same_session`,
`a_person_takes_over_an_orchestrated_worker_and_the_orchestrator_is_locked_out`,
`every_automated_move_is_announced_before_it_happens`.

Failure / isolation evidence — nine mutations, nine KILLED:
- ignore-the-off-switch (`routing_off = false`) — *"`--no-routing` starts a
  session rather than continuing the warm one"*.
- ignore-the-pinned-harness (`select(None, …)`) — codex's argv log stayed empty.
- drop-the-user-override (`if false`) — kills 1714 and 1715 independently.
- checkpoint-the-move-as-if-nothing-were-left (`Some(id)` → `None`), on the
  launch path and again on the resume path.
- a-mute-admits-everything — killed by the mute test.
- the-person-cannot-quiet-the-orchestrator (`api::mute` a no-op) — killed by
  the take-over test **only after its assertion was strengthened**: the first
  draft still passed because 1719 had refused the orchestrator anyway; the
  assertion now reads the *reason*.
- the-seam-admits-everything — **SURVIVED once**: a second copy of the check on
  the control door meant nothing in the suite ever reached the seam, and the
  seam's own doc ("the one seam every machine sender passes through") was
  false. The door's copy was removed; the same mutation is now KILLED by two
  tests. The cost — a memory store opened for a briefing about to be refused —
  is paid only where a person is already using the session, and written at the
  call site.
- stop-announcing-an-honoured-override — killed by the announcement test.

Two existing tests changed, unavoidably and without weakening: 1719 is a
behaviour change and `api_event_log::the_origin_a_request_states_is_the_origin_recorded`
and `worker_access::a_persons_intervention_and_the_orchestrators_own_are_different_rows`
sent a machine line right after a person's and asserted `ok`. Both are about
which origin each row carries, not their order; the order is reversed and every
property they claim is preserved.

Gates: fmt, clippy `-D warnings`, doc clean; targets above green;
`blast-radius.sh` exit 0 run alone. One flake attributed with five runs (§34):
`session_supervision::a_harness_that_never_came_up_is_not_restarted` failed
twice under full-target load and passed 5/5 alone; the diff to
`session/runtime.rs` is 122 lines of additions, none in the supervision path.

Limits: a mute does not stop `pump_watches` (1719 covers that path by deferring;
the gap is "muted but nobody is typing"); `--to` needs the full identifier
`glasshouse route` prints, not `sessions`' twelve characters (a prefix resolve is
a small follow-up); `glasshouse api send` always states `origin: user` —
attribution, not authentication, `api::client`'s documented position; 1720's
"fresh chosen over continuable sessions" sentence is unevidenced because the
fixture cannot reach it (a warm resumable session always outscores a fresh one
there) — named rather than hidden; macOS only.
