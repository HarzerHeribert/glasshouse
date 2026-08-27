# Capability evidence — phase 10A

See `docs/product/evidence/README.md` for the index and the entry template.

### Phase 10A — Session supervision, 13 of 13 (lines 664–676)

Contract: Glasshouse records, for every session it starts, an identity of the
process it was started in that a later process cannot inherit; on every start it
discovers the sessions this project recorded, verifies each against the machine,
adopts what it can verify, quarantines what is alive and unaccounted for,
refuses to start beside either, records a start that never became ready as a
failure with a stated reason, and applies every lifecycle change and every input
through one ordered path — while never ending a session the user did not ask it
to end.

State: VERIFIED — all thirteen lines carry production callers and
mutation-killed regression evidence. Lines 673 and 674 (bounded restart and its
reset condition), open in the first pass of this phase, are closed here through
an owned `HarnessLaunch` in `src/launch.rs` and a bounded restart in
`SessionRuntime::poll_exits`.

**One thing changed rather than was added, and it is the headline of this
entry.** Line 672's *in-start* readiness refusal was **removed**. It gave
opposite answers on macOS and Linux for the same input, and it turned the
already-closed line 1730 — *"preserve terminal output and event history after a
worker crashes"* — red on one of them. The measurement, the mechanism and the
argument are under "The platform decision" below. Line 672 is now carried
entirely by `supervision::reconcile`, which is deterministic and
platform-independent.

## The three architectural requirements, and where each is kept

- **Supervision covers only sessions this project recorded.** `supervision::
  discover` starts from `SessionStore::list` filtered to this project, and there
  is no process enumeration anywhere in the module.
  `supervision_never_enumerates_processes` is the guard: it fails on
  `proc_listpids`, `proc_listallpids`, `CreateToolhelp32Snapshot` and a
  `/proc` directory walk appearing in production code.
- **Alive-and-no-longer-owned is a distinct condition.** It is
  `Supervision::Quarantined`, it is a fourth stored word beside `owned`,
  `adopted` and `lost`, and `reconcile` deliberately **does not touch the
  lifecycle** when it quarantines: the record keeps saying whatever it said, so
  a quarantined session is neither reported as stopped nor treated as healthy.
  M6 is the mutation that collapses the distinction, and it dies.
- **Glasshouse reports and refuses; it never ends a session.** There is no
  `kill`, no signal, no `TerminateProcess` and no `wait` in `supervision.rs`,
  and `nothing_in_supervision_ends_a_process` fails if one appears. The same
  rule is now enforced at the start itself: `SessionRuntime::start` no longer
  discards a session because its process died inside a settle window. The one
  place in the crate where Glasshouse ends a process it started, other than
  `close`, is a restart whose reader thread could not be created — and there the
  alternative is a harness wedged on a terminal nobody is draining. It is
  commented as such.

## The platform decision

**The question.** Is a harness that prints one line and dies *"a crashed
worker"* (`docs/product/capability-map.md:1730`, closed) or *"a start that
never became ready"* (line 672, being closed)? Both lines are in the map and
they gave different answers for the same input, and the first pass of this
phase answered it by refusing the start — which broke 1730 on Linux.

**What was measured, and why it is not a race.** One tree, one
`scripts/ci-local.sh` run, `tests/events_lifecycle.rs`: **macOS 5 passed,
Linux 3 passed and 2 failed**, the two failures being the crashing-harness
tests. Instrumenting `await_ready` on macOS showed why, and it is not the
length of `READINESS_SETTLE`:

- The readiness check sat **before the session's output reader thread was
  started**. On macOS the fixture's `echo STARTED` then blocks — nothing is
  draining the pseudo-terminal — so the harness never reaches its `kill -9`,
  and the parent handle reports it running for **more than three seconds**.
  `await_ready` therefore never saw a failure at all; it fell out through
  `observe` returning `None` and kept the session.
- On Linux the write does not block, the harness dies at about **6ms**, and the
  parent handle reports the `SIGKILL` well inside the window, so the start was
  refused.
- Moving the same check to *after* the reader thread makes macOS report the
  death in **3ms** and refuse too. The answer depended on where in `start` the
  check sat and on which kernel — never on how long it waited.

**And there is no in-start refusal that would have been right.** `spawn`
returns a live process id before anyone knows whether the `exec` behind it
worked, so at start time *"the process is alive"* is always true and *"it
died"* is always a later observation — the same observation for a harness whose
configuration was unreadable and for one that ran and crashed. On Windows this
repository's two fixtures for those cases are **the same three lines**
(`@echo off` / `echo STARTED` / `exit /b 3`), so the exit status cannot
separate them either. Nor can *"did it print anything"*: §34 records the Linux
container flake where the crashed worker's output is not observed at all.

**The decision.** A harness that Glasshouse started and that died is **a
session that failed**, not a start that was refused. Glasshouse cannot tell the
two apart at start time and must not throw away what the process printed in
order to pretend it can — for a bad flag or an unreadable configuration, that
line *is* the diagnosis. So:

- the in-start refusal, `supervision::await_ready`, `Readiness`, `ExitCheck`
  and the three readiness constants are **deleted**; nothing in production
  called them once the refusal went, and §5 does not keep a mechanism for the
  shape of it;
- line 672 rests on `supervision::reconcile`, which decides the same question
  where the difference is real — **in the record**: a start that never became
  ready is one whose record never left `starting` and whose process is gone.
  That is durable, identical on every platform, and it survives the
  `glasshouse` that gave up;
- `tests/events_lifecycle.rs` is green on macOS and Linux, unchanged, and line
  1730 stays closed.

`a_harness_that_printed_and_died_is_a_failed_session_not_a_refused_start` is
the acceptance test at the record, and **M18b** — the refusal put back where it
*can* observe the failure — kills it. **M18, the refusal exactly as it was
written, survives on macOS**, which is the same finding from the other side: in
its original position it could not fire on this platform at all.

## Production evidence

- `crates/glasshouse/src/database.rs` migration 9 — five appended columns on
  `sessions`: `process_id`, `process_started_at`, `process_host`,
  `supervision`, `supervision_reason`. `ALTER TABLE ADD COLUMN` only, in
  migration 3's shape; no table is rebuilt, no `CHECK` is altered, and
  `lifecycle_events` is untouched (its `seq` is `AUTOINCREMENT` and `memories`
  references it, so a supervision conclusion is a column on `sessions` and
  never a new event kind).
- `crates/glasshouse/src/session/supervision.rs` (new) — the whole of
  discovery, verification, adoption, quarantine, readiness and the refusals.
  Declared in `session/mod.rs`.
- `crates/glasshouse/src/session/store.rs: SessionStore::create` — **the
  production writer of the identity.** It is the only door a session record
  comes through, so no future caller can start a session Glasshouse would later
  be unable to identify. Both shipped callers reach it: `main.rs::launch_session`
  for the command line and `shell/mod.rs::start_session` for the interactive
  shell.
- `crates/glasshouse/src/session/store.rs: ProjectSessions::open_with_clock` →
  `supervise` → `supervision::reconcile` — **the production caller of
  discovery.** Every `glasshouse` invocation that touches sessions comes through
  it: `launch`, `resume`, `sessions`, `hook` and the interactive shell. Putting
  it in the shell alone would have missed the processes this phase exists for,
  because nobody was in the shell when they ran away.
- `crates/glasshouse/src/session/store.rs: SessionStore::open_for_resume` →
  `supervision::guard_start` — **the production caller of both refusals.**
  `glasshouse resume` is the path a replacement is started through.
- `crates/glasshouse/src/session/store.rs: SessionStore::create` →
  `refuse_if_quarantined_holds` — the other half of line 670: a new session may
  not claim a harness conversation a quarantined process still holds.
- `crates/glasshouse/src/session/runtime.rs: SessionRuntime::start` — the
  duplicate refusal inside one Glasshouse, and the readiness bound. Reached from
  `main.rs::run_headless` (`glasshouse launch --headless`) and
  `shell/mod.rs::start_session` (`n` in the interactive shell).
- `crates/glasshouse/src/session/store.rs: SessionStore::write_lifecycle_locked`
  — **the only statement in the crate that moves a session's lifecycle**, and
  `in_a_write_transaction` is the only way to reach it. `set_lifecycle`,
  `record_supervision` and `close` all go through both.
- `crates/glasshouse/src/session/runtime.rs: LiveSession::deliver` — **the only
  path that writes to a session's process**. Keystrokes
  (`write_to_focused`), a line typed at the shell's prompt (`send_text`), a
  machine-originated message (`send_text_from`), an interrupt (`interrupt_from`)
  and the runtime's own answer to a terminal query
  (`answer_terminal_queries`) all arrive there.

## Line by line

- **664 — record a durable process identity, including the start time.**
  `SessionStore::create` records `std::process::id()`, the kernel's start time
  for that process in milliseconds since the Unix epoch, and the host.
  Milliseconds since the epoch rather than each platform's own unit, because
  Linux reports a start time in ticks *since boot*, which repeats after every
  reboot and would leave exactly the collision the column exists to close —
  `supervision::observe` converts on the way in, adding `/proc/stat`'s `btime`
  on Linux, reading `proc_bsdinfo.pbi_start_tvsec` on macOS and converting
  `GetProcessTimes`' `FILETIME` on Windows. A partially recorded identity reads
  as **no** identity (`SessionStore::supervision_of`), so a pid without a start
  time is never trusted. M1, M2.
  *Which process this is, and what is not yet recorded, is under "Missing
  evidence".*
- **665 — discover, on start, the recorded sessions whose processes are still
  running.** `supervision::discover` — this project's own session table,
  filtered to the active project and to records that still claim a live
  lifecycle. Run from `ProjectSessions::open`. M3.
- **666 — verify a discovered process before treating it as the session it
  claims to be.** `supervision::verify` returns one of four answers, never a
  bool: the host is checked *first*, so a record from another machine is
  reported as unverifiable rather than as a pid comparison that happened to
  succeed. M4 (the start time is ignored) and M5 (the host is ignored) both
  die.
- **667 — adopt a verified live session rather than starting a second beside
  it.** `reconcile` writes `supervision = 'adopted'` and **leaves the lifecycle
  alone**: adoption changes who is watching, never what the session is doing.
  Proved by a second, separate `glasshouse` process opening a project whose
  session is genuinely running.
- **668 — refuse to start a session that would duplicate a live, verified
  session of the same record.** Two halves, one function each.
  `supervision::guard_start` refuses across processes, and the refusal names the
  process; `SessionRuntime::start` refuses inside one Glasshouse, where two
  `LiveSession`s under one identifier would give `get`, `focus` and `poll_exits`
  whichever the vector reached first and leave one real process steerable by
  nobody. M8, M14.
  The duplicate refusal is deliberately conditioned on the record being **live**
  and the verified process being **not this one** — a stopped record is not
  duplicated by starting again however alive the Glasshouse that recorded it
  still is, and that Glasshouse is very often the one asking.
- **669 — detect alive-but-mismatched and quarantine it.** `Verdict::Mismatched`
  (a reused process id) and `Verdict::ForeignHost` (a record from another
  machine) both become `Supervision::Quarantined`, with the lifecycle untouched.
  M4, M6.
- **670 — refuse to start a replacement while a quarantined process still holds
  the same resources.** `guard_start` refuses whatever the record's lifecycle
  says — "it reads as stopped, therefore it is free to replace" is precisely the
  reasoning that produces a second process over the top of a first — and
  `create` refuses a new session claiming a conversation a quarantined record
  holds. M7.
- **671 — surface a quarantined session with what is known and what it holds.**
  `SupervisionReport::describe`, printed by `ProjectSessions::open` on **standard
  error** so a script reading `glasshouse sessions` still gets the session list
  and nothing else. It names the session, the harness, the process, how far the
  observed start time is from the recorded one, the harness conversation and the
  session directory still held, and states that Glasshouse will not reuse,
  replace or end it. Adopted and lost sessions are deliberately *not* announced:
  one is working as intended and the other has already been recorded, and
  announcing either on every invocation would train people to ignore the line
  that matters. M9.
- **672 — require a started session to become verifiably ready within a bounded
  time, and record a start that never became ready as a failure with a stated
  reason rather than as a session.** `supervision::reconcile`, called from
  `ProjectSessions::open` and through it from five sites in `main.rs`. A record
  still reading `starting` whose process is gone, or which has been starting
  longer than `NEVER_READY_AFTER` with no identity ever recorded, becomes
  `failed` with the reason written to `supervision_reason`, and the report says
  *"never started"* on standard error. That is what *"with a stated reason
  rather than as a session"* means once the process that gave up is itself gone,
  and it is the same answer on every platform. M6, M10, and — for the refusal
  that used to sit inside `start` and no longer does — M18b. See "The platform
  decision".
- **673 — restart a session that exits unexpectedly up to a bounded number of
  consecutive attempts, and stop with a stated reason when that bound is
  reached.** `SessionRuntime::consider_restart`, called from `poll_exits`, which
  is what `main.rs::run_headless` and the shell's draw loop already call. The
  recipe it re-runs is `launch::OwnedHarnessLaunch`, kept on the `LiveSession`
  because the exit that needs it is noticed long after `start` returned and with
  no project in scope. `MAX_CONSECUTIVE_RESTARTS` is three; on the fourth the
  session is left exited with the reason written into its **own terminal**, so
  the user finds it where they are already looking. M19, M23.

  *"Exits unexpectedly"* excludes four things, and the third is the one that
  keeps this line clear of an already-closed capability: a clean exit; an
  ending the user asked for (`interrupt` marks the session, and the mark lasts
  only until it is next seen alive); **a session that was never healthy** — a
  harness that has not once come up is a start that did not work, not a session
  that exited, and restarting it would turn a mistyped executable into four
  processes; and a bound already reached. M22 is the third of those, and
  `a_harness_that_never_came_up_is_not_restarted` is why
  `tests/events_lifecycle.rs`'s crashing harness is untouched by this line.
- **674 — reset the consecutive-restart count only when a restarted session has
  been verified healthy, never when it has merely been started.** The only
  assignment that clears `restarts` is in `poll_exits`'s *still-running* arm,
  and it is guarded by both halves of *verified healthy*: the process has been
  alive for `HEALTHY_AFTER`, and `supervision::verify` still says the thing
  under that pid is the process whose identity was recorded for it. M20 (the
  count is cleared by having been restarted) and M21 (health is granted on
  sight) both die.

  `HEALTHY_AFTER` is a timing constant in a phase that just deleted one, and the
  difference is that this one is **one-sided**: too long merely delays a reset,
  where the deleted one decided whether a session existed at all. Two seconds is
  more than two orders of magnitude above the crash loop it exists to refuse to
  reset for.
- **675 — apply lifecycle changes through a single ordered path.**
  `write_lifecycle_locked` is the only `UPDATE sessions SET lifecycle` in the
  crate, and `one_statement_moves_a_sessions_lifecycle` fails if a second
  appears — which it did: `close` had its own statement, and its own
  read-outside-a-transaction check, so a hook process moving the session back to
  `running` in between would have left a `closed` row that a live harness kept
  updating. That defect was found by writing this guard. The transaction is
  `BEGIN IMMEDIATE`, not deferred: a deferred one reads without the write lock
  and then has to upgrade, which SQLite refuses outright once another connection
  has committed, and `busy_timeout` cannot help because there is nothing left to
  wait for. M11, M12, M16, M17.
- **676 — never deliver two inputs to the same session concurrently.**
  `LiveSession::deliver` is the one path, guarded structurally by
  `only_one_path_writes_to_a_session`, which counts the call sites that touch a
  session's process and fails at two. The terminal-query reply was the second
  one and now goes through the funnel: it is bytes on the same terminal, and a
  `\x1b[24;80R` landing in the middle of a line somebody typed corrupts both.
  M15. See "Missing evidence" for the part of this line that has no killing
  mutation and why.

## Regression evidence

macOS 15 (Darwin 25.5.0), `cargo test -p glasshouse`: **0 failed**, every
target. `tests/session_supervision.rs` and `tests/events_lifecycle.rs` were then
run together **ten more times: 10/10**, and **five more times under
deliberately added load** — twelve busy loops on a twelve-core machine, load
average 38 — **5/5** (§26, §60). That matters more here than usual: three of
these tests kill real processes, two turn on how several real writers
interleave, and the two new restart tests turn on a process outliving a
two-second window.

- `crates/glasshouse/tests/session_supervision.rs` (new) — fifteen tests, and
  **every one of them either runs the shipped `glasshouse` binary, kills a real
  process, or drives a real pseudo-terminal.** No test in this file concludes
  anything from a constructed value, because none of this phase's claims is
  about a value.
  - `a_launched_session_records_the_process_it_was_started_in` — two launches,
    two identities, and they are not interchangeable.
  - `an_orphaned_session_is_discovered_and_recorded_as_lost` — the 2026-08-26
    incident on purpose: a `glasshouse` holding a running session is `SIGKILL`ed
    so it never records the exit, and the next one finds it. It asserts the
    recorded identity **verifies while the process is genuinely alive** first,
    which is the control: without it, "gone" afterwards would pass against a
    build that could never verify anything.
  - `a_verified_live_session_is_adopted_and_a_second_is_refused`.
  - `a_runtime_refuses_to_start_a_second_session_under_one_identifier`.
  - `a_process_that_is_alive_and_unaccounted_for_is_quarantined_never_replaced`
    — a live process id carrying a start time that is not its own, which is what
    a recycled id looks like. Waiting for a real recycle would take hours and
    would still be a coincidence rather than a test; what is under test is the
    comparison, and the comparison gets exactly the input a recycle produces.
  - `a_start_that_never_became_ready_is_recorded_as_a_failure_with_a_reason`,
    which also carries the **control for the first architectural requirement**: a
    start with no recorded identity that has only just begun is left completely
    alone — no conclusion, in either direction.
  - `a_start_whose_process_died_before_it_ran_is_a_failure_not_a_stopped_session`.
  - `a_harness_that_finishes_at_once_still_ran` — the readiness bound must not
    fabricate a failure for a session that simply finished quickly.
  - `a_stopped_session_is_not_revived_by_a_writer_that_read_before_the_stop`.
  - `many_writers_on_one_session_all_succeed`.
  - `no_two_inputs_are_ever_delivered_to_one_session_at_once`.
  - `a_harness_that_printed_and_died_is_a_failed_session_not_a_refused_start` —
    the platform decision at the record, through the shipped binary. It asserts
    what a refusal cannot satisfy: the user is told how the **harness** ended,
    not that Glasshouse decided it never came up. The lifecycle alone would not
    do — a refused start is written down as a failed session too, so a test that
    read only `lifecycle` would pass against a build that refuses.
  - `a_session_that_keeps_crashing_is_restarted_a_bounded_number_of_times` —
    line 673's bound, **three trials inside the test** because a bound proven
    once is proven for one trial (§60). The harness comes up, is verified
    healthy, dies, and every later run of it dies after 400ms. The 400ms is
    deliberate and was bought by a surviving mutation: a harness that died
    instantly was already dead by the next poll, so a build whose health rule
    ignored `HEALTHY_AFTER` reached the same bound and M21 survived. §41 — the
    test and the mutation shared the assumption that a crash loop is never
    *seen* alive.
  - `a_harness_that_never_came_up_is_not_restarted` — the exclusion that keeps
    line 673 clear of line 1730, with Phase 45's own crashing harness as the
    input.
  - `a_restarted_session_that_becomes_healthy_again_clears_the_bound` — line
    674. The count is watched as a *transition* rather than sampled, because a
    build that reset on "started" would leave it at zero throughout and a test
    that read it at the end could not tell the two apart.
- `crates/glasshouse/src/session/supervision.rs` — nine unit tests: the two
  source-scan guards for the architectural requirements, identity stability, the
  reused-id and foreign-host verdicts, the stored vocabulary, and what a report
  does and does not announce. The four that covered `await_ready` went with it.
- `crates/glasshouse/src/session/store.rs` —
  `one_statement_moves_a_sessions_lifecycle`, and migration 9 added to the
  credential-inventory list and to all four forward-compatibility rollbacks
  (`upgrading_a_version_2/7_database…`, `a_version_one_database…`, and the
  version-3, -5 and -6 rollbacks in `tests/memory_store.rs` and
  `tests/memory_provenance.rs`). The runner resumes from `MAX(version)`, so a
  rollback that leaves migration 9's columns behind re-applies it against a
  table that already has them.

## Failure/isolation evidence

Mutation evidence — **twenty-one run, twenty-one killed** on *this* tree, each
named test `ok` before, `FAILED` mutated, `ok` restored, with the mutated file
touched every time so no verdict came from a cached test binary (§16). A
verdict from the tree this work was first written on is not a verdict about
this one, so all of them were re-run here rather than carried over.

M13 is gone with `await_ready`. M18, M18b and M19–M23 are new.

| id | mutation | test | result |
|---|---|---|---|
| M1 | `create` records no process identity | `a_launched_session_records_the_process_it_was_started_in` | FAILED |
| M2 | `create` records the pid but not the start time | `a_launched_session_records_the_process_it_was_started_in` | FAILED |
| M3 | opening a project never supervises | `an_orphaned_session_is_discovered_and_recorded_as_lost` | FAILED |
| M4 | verification ignores the recorded start time | `a_process_that_is_alive_and_unaccounted_for_is_quarantined_never_replaced` | FAILED |
| M5 | verification ignores the recorded host | `a_record_from_another_machine_is_never_verified_and_never_assumed_dead` | FAILED |
| M6 | a start that never came up is filed as a stopped session | `a_start_whose_process_died_before_it_ran_is_a_failure_not_a_stopped_session` | FAILED |
| M7 | a quarantined session does not block a replacement | `a_process_that_is_alive_and_unaccounted_for_is_quarantined_never_replaced` | FAILED |
| M8 | a verified live session does not block a second beside it | `a_verified_live_session_is_adopted_and_a_second_is_refused` | FAILED |
| M9 | quarantine is surfaced without what it still holds | `a_process_that_is_alive_and_unaccounted_for_is_quarantined_never_replaced` | FAILED |
| M10 | a session with no recorded identity is concluded about anyway | `a_start_that_never_became_ready_is_recorded_as_a_failure_with_a_reason` | FAILED |
| M11 | a finished session may be moved back to a live state | `a_stopped_session_is_not_revived_by_a_writer_that_read_before_the_stop` | FAILED |
| M12 | the ordered path takes a deferred transaction | `one_statement_moves_a_sessions_lifecycle` | FAILED |
| M14 | a runtime starts a second session under one identifier | `a_runtime_refuses_to_start_a_second_session_under_one_identifier` | FAILED |
| M15 | a second path writes straight to a session's process | `only_one_path_writes_to_a_session` | FAILED |
| M16 | closing a record writes its lifecycle outside the ordered path | `one_statement_moves_a_sessions_lifecycle` | FAILED |
| M17 | a supervision conclusion moves the lifecycle by its own statement | `one_statement_moves_a_sessions_lifecycle` | FAILED |
| M18 | the in-start readiness refusal, exactly where it used to sit | `a_harness_that_printed_and_died_is_a_failed_session_not_a_refused_start` | **SURVIVED** |
| M18b | the same refusal, moved to where it can observe the failure | `a_harness_that_printed_and_died_is_a_failed_session_not_a_refused_start` | FAILED |
| M19 | a session that exits unexpectedly is never restarted | `a_session_that_keeps_crashing_is_restarted_a_bounded_number_of_times` | FAILED |
| M20 | the restart count is cleared by having been restarted | `a_session_that_keeps_crashing_is_restarted_a_bounded_number_of_times` | FAILED |
| M21 | health is granted to a process that has only just started | `a_session_that_keeps_crashing_is_restarted_a_bounded_number_of_times` | FAILED |
| M22 | a harness that never came up is restarted anyway | `a_harness_that_never_came_up_is_not_restarted` | FAILED |
| M23 | the bound is reached without a stated reason reaching the user | `a_session_that_keeps_crashing_is_restarted_a_bounded_number_of_times` | FAILED |

**M7 survived its first run, and the test was wrong rather than the mutation
(§41).** The quarantine test asserted that the *resume refusal* contained the
word "quarantined" — but opening the project also surfaces the quarantine on
standard error, so the assertion was reading the report beside the refusal and
would have passed against a build whose refusal had been deleted outright. It
now asserts the refusal's own sentence, "refusing to start a replacement", and
the same tightening was applied to the duplicate refusal. Both mutations die.

**M18 survives on macOS, and that is the evidence rather than a gap.** The
refusal put back *exactly where it used to sit* — before the session's output
reader thread — changes nothing on this platform, because the harness is
blocked writing to a pseudo-terminal nobody is draining and the parent handle
reports it running for more than three seconds. It is the same fact as the
Linux failure seen from the other side: the check could not fire here and fired
every time there. M18b moves it to after the reader thread, where both
platforms report the death in single-digit milliseconds, and it dies at once —
which is what proves the acceptance test is testing the refusal and not its
placement.

**M21 survived its first run, and the fixture was wrong rather than the
mutation (§41).** Granting health to a process that had only just started
should have turned the crash loop infinite and timed the bound test out. It did
not, because the looping harness exited so fast that it was *already dead* at
the next poll, so the still-running arm — where health is decided — never ran
for it. The test and the mutation shared the assumption that a crash loop is
never observed alive. The loop now sleeps 400ms: long enough to be seen
running, far short of `HEALTHY_AFTER`. M21 then dies, and so does a whole class
of mutations the original fixture could never have reached.

**Two mutations that survived because the *test* could not produce the
interleaving they attacked, and what replaced it (§41 again).** The
single-ordered-path test began as six real `glasshouse hook` processes racing
one session, and then as four threads on separate connections with a millisecond
between each read and its write. **Neither ever reproduced the race**, and both
passed against a build with the ordering deliberately removed — twenty-five
rounds of a race that cannot happen is twenty-five rounds of proving nothing.
The interleaving is now **staged** rather than waited for: the hook writer reads,
the exit writer commits `stopped`, the hook writer then writes the `idle` it
decided. That costs the "rate" §60 asks for and buys certainty — it happens on
every run, and a build that allows it fails on every run. The rate is kept
separately by `many_writers_on_one_session_all_succeed`, which is what would
catch an ordering that turned contention into errors.

## Platform/external evidence

macOS 15, the shipped debug binary, in a **real pseudo-terminal**, against a
fake installed harness that stays up:

- `glasshouse launch claude-code --headless` in the background, then
  `glasshouse sessions` while it is genuinely alive → the session reads
  `active`. The owning `glasshouse` is then `SIGKILL`ed, and the next
  `glasshouse sessions` reads `resumable` — supervision found the orphan,
  verified its process gone, and recorded it. **Before this change the record
  would have read `active` forever and no command in the binary would have said
  otherwise.**
- With the recorded start time rewritten so a live process id no longer matches
  it, `glasshouse sessions` printed, on standard error:

  ```
  glasshouse: session 0a32813b (claude-code) is quarantined: process 2247 is
    running, but it started 1h 0m after the process this session recorded, so
    the id was reused and what is running under it now cannot be accounted for
    it was recorded as process 2247 on Friedolin.fritz.box, started 1h 0m ago
    it still holds the claude-code conversation `c72018b3-…`
    it still holds its session directory `…/sessions/0a32813b…`
    Glasshouse will not reuse it, replace it, or end it. Decide what to do with
    that process yourself.
  ```

  and `glasshouse resume 0a32813b` refused with *"…is quarantined and a process
  Glasshouse cannot account for still holds … ; refusing to start a
  replacement."*
- **A defect this found, which nothing else would have.** The first version of
  that message printed raw epoch milliseconds — *"it started at 1787830490528ms
  and this session's process started at 1787830430528ms"*. Correct, and useless
  to somebody deciding whether to end a runaway process. `ProcessIdentity`'s
  `Display` and the mismatch reason now say how long ago and how far apart, in
  words. The whole point of the incident behind this phase was that three
  processes had been running for nineteen hours without anyone noticing, which
  is a sentence, not a timestamp.
- **What `READINESS_SETTLE` was, and why it is gone.** It was a *measured*
  constant — a `fork` + `exec` + immediate `exit` is observable after about
  **4ms** idle and after **more than 75ms** on a busy machine, so a first
  attempt at 75ms lost the race the first time the suite ran while the machine
  was compiling, and it was raised to 250ms. It was still the wrong shape: what
  it was measuring was not the harness but **whether anything was yet reading
  the harness's terminal**, which is why 250ms was enough on Linux and three
  seconds would not have been enough on macOS. A constant cannot fix that, and
  the refusal it guarded is deleted. See "The platform decision".

`scripts/ci-local.sh` — see the report for the run and its two legs. This
phase's acceptance pair is `tests/events_lifecycle.rs` on macOS **and** Linux,
because that is the pair the first pass of this work split.

## Missing evidence

- **A restart is not configurable, and cannot be until `main.rs` and the shell
  are free.** `MAX_CONSECUTIVE_RESTARTS` and `HEALTHY_AFTER` are constants; there
  is no per-session policy and no way to turn restarting off, because every
  caller that could express one is in a file this package may not touch. That is
  survivable precisely because of the *never-healthy* exclusion — the only
  sessions that are ever restarted are ones that were genuinely working — but a
  user who wants a crash to stay a crash has no switch today.
- **`consider_restart` is the one place, other than `close`, where Glasshouse
  ends a process it started.** If the restarted harness spawns but its reader
  thread cannot be created, the new process is signalled and the session is
  halted with a reason. The alternative is a harness wedged on a terminal
  nobody is draining, which nobody can see or steer; it is the lesser harm and
  it is commented as such, but it is a real exception to the third
  architectural requirement and it should be read as one.
- **Which process the identity names.** It is the **Glasshouse process that
  started the session and is responsible for it**, not the harness child.
  For `glasshouse launch` that is the same thing — Glasshouse blocks in
  `session::attach` or `run_headless` for the session's whole life and its death
  ends the session — and those are the processes the 2026-08-26 incident was
  about. For a session started from the interactive shell, the harness is a
  child of the recorded process and dies with it in the normal case, but its own
  pid is **not recorded**, so a harness that survives its Glasshouse is not
  identified. Recording it needs one line in `shell/mod.rs::start_session` and
  one in `main.rs::launch_session`: `SessionRuntime::start` is the only place
  that holds the child's pid and it has no `SessionStore`, and neither file is
  in this package.
- **There is no in-start readiness bound any more, and the cost of that is
  named.** Line 672 is enforced by `reconcile`, which runs at
  `ProjectSessions::open`. In the process that *did* the starting, a harness
  that dies is still noticed promptly — `poll_exits` reports the exit and the
  record becomes `failed` — but it is recorded as a session that failed, with
  the harness's own exit as the reason, rather than as a start that never came
  up. Those are the same sentence to a user and this phase argues they must be;
  a reader who wants them distinguished should read "The platform decision"
  before adding a bound back.
- **A harness that starts and then hangs is caught by nothing here.** Nothing
  observable at start time separates it from one that is thinking, and
  `NEVER_READY_AFTER` only judges a record still reading `starting`, which a
  hung harness's would not be. If that is wanted it is a new line in the map,
  not this one.
- **`RuntimeError::DeliveryInFlight` is an `Io` error with
  `ErrorKind::WouldBlock` instead of a variant of its own.** `RuntimeError` is
  matched exhaustively in `shell/mod.rs::refusal_reason`, which is not in this
  package, so a new variant would not compile. `WouldBlock` is honest — a write
  refused because one is already in flight is exactly that — and the shell
  renders the source's own sentence, so nothing is lost to the user. It should
  become `DeliveryInFlight` when `shell/mod.rs` is free.
- **The per-session delivery lock has no killing mutation, and this is a real
  §5 gap rather than an oversight.** Line 676 is enforced twice over: by the one
  funnel (M15 kills its removal) and by `&mut self` on every delivery method
  plus the `Mutex<SessionRuntime>` the shipped binary owns it behind. Because of
  the second, **nothing in today's build can contend the lock**, so removing it
  fails no test. It is kept for the shape of the change that would break the
  invariant — a delivery path taking `&self`, a session handed to a thread, a
  queue drained without the outer lock — all of which compile. The behavioural
  test proves messages arrive whole; it cannot prove the lock is what makes them
  so, because today the type system is.
- **`supervision::observe`'s Windows arm.** The Linux arm — which parses
  `/proc/<pid>/stat` from after the *last* `)`, the classic misparse being to
  split from the front because a command name may contain spaces and
  parentheses, and adds `/proc/stat`'s `btime` — is exercised by the Linux leg
  of the gate; see the report for that run. The **Windows** arm, which converts
  `GetProcessTimes`' creation `FILETIME` and reads `STILL_ACTIVE`, compiles only
  on Windows and runs only under `ci-local.sh --windows-vm`. See the report for
  whether that leg was run.
- **A `Verdict::ForeignHost` record is quarantined rather than left alone.**
  That is the deliberate reading of the second architectural requirement — it is
  neither alive nor stopped as far as this machine can tell, and reporting it is
  better than silence — but it means a project directory synchronised between
  two machines will quarantine sessions that are perfectly healthy on the other
  one. No user has hit it, and the alternative (silence) is worse, but it is a
  decision worth revisiting when Phase 19's portable checkpoints make moving a
  project between machines ordinary.
- **The interactive shell's own session list does not show supervision.**
  `glasshouse sessions` and the shell's overview print the lifecycle, so a
  quarantined session shows as whatever it was; the quarantine reaches the user
  through the standard-error report at open, which is on every invocation but is
  not a column. Adding one means `main.rs` and the shell's view, neither of
  which is in this package.
