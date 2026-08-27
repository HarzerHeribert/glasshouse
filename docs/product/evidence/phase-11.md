# Capability evidence — Phase 11 (Session overview)

Written by the PHASE-11-OVERVIEW Sonnet-implementer package. Per
`docs/process/worker-capabilities.md`, a worker does not tick capability-map
boxes, edit the map, or edit this file's status into the map — this entry is
handed to the orchestrator, who decides each box against the evidence below.

Verified against the binary built from this worktree (`glasshouse-phase-11`),
`cargo test -p glasshouse --lib` (1109 passed) and
`cargo test -p glasshouse --test pty_smoke` (71 passed), both clean,
`cargo clippy -p glasshouse --all-targets` with `RUSTFLAGS="-D warnings"`
clean, and `cargo doc -p glasshouse --no-deps` with the same flag clean. macOS
(darwin/arm64) only — no Linux or Windows execution was performed for this
entry, and every claim below that depends on real process/pty behavior should
be treated as unverified on those platforms until CI runs it.

---

### Phase 11 line 680 — "Add a session overview that lists all current project sessions in one screen."

Contract: a single key opens a popup listing every session `ShellState`
holds, over the live shell rather than replacing it.

State: **was already CLOSED before this package; unchanged.**

Production evidence:
- `o` → `ShellState::open_overview` (`shell/state.rs`) → `Overlay::Overview` →
  `view::render_overview` (`shell/view.rs`), which iterates
  `state.sessions()` — every session the shell holds for this project — and
  draws one row per session inside a popup over the live shell.
- Exercised end to end by every pty_smoke test that presses `o` and reads the
  popup back (e.g. `an_interrupt_sent_from_the_overview_reaches_a_real_child`,
  and this package's own two new tests below).
- Unit-tested: `shell::view::tests::the_session_bar_lists_every_known_session`
  and the whole `shell::state::overview_tests` module.

This package did not touch this box's mechanism, only verified it still
holds under the new columns (see 682/683/684 below).

---

### Phase 11 line 681 — "Show the harness name for every session."

State: **was already CLOSED before this package; unchanged.**

Production evidence: `render_overview`'s `HARNESS` column renders
`session.harness` for every row, unconditionally — `shell/view.rs`, the
format string inside the `for (index, session) in state.sessions().iter()`
loop. `session.harness` is `SessionRecord::harness`, an `IntegrationId` slug
recorded at session creation (`session/store.rs`), never derived or guessed.

---

### Phase 11 line 682 — "Show the user-assigned session name or purpose for every session."

Contract, as decided by this package (a Sonnet implementer's decision within
its authority — a view-layer rendering choice, not a capability-map
interpretation): **one column, not two.** `SessionRecord::display_name` and
`SessionRecord::purpose` are two fields for one fact the box states as one
sentence ("the … name or purpose"); a session with both shows the name with
the purpose alongside it in parentheses, a session with one shows that one,
and a session with neither renders the literal text `(unnamed)` — not a blank
cell. A blank cell is indistinguishable from a column truncated off-screen at
a narrow width (see 684 below and practice §17); `(unnamed)` cannot be
confused with either.

State: **CLOSED.**

Production evidence:
- `shell/view.rs`, `name_or_purpose(session: &SessionRecord) -> String`, a new
  function, called from `render_overview`'s row-building `format!` for the
  new `NAME` column, drawn for every row of `state.sessions()`.
- No schema change: `display_name` and `purpose` were already on
  `SessionRecord` (Phase 10). This is a view gap, not a schema gap, exactly as
  the packet predicted — `session/store.rs` was not touched, per its
  `FORBIDDEN FILES`.

Mutation: `name_or_purpose` replaced with a body that always returns
`"(unnamed)"` regardless of the record. `SURVIVED` before the fix (there was
no fix to survive against — this was the first version written), `FAILED`
(caught) against `shell::view::tests::the_new_overview_columns_survive_a_realistic_and_a_wide_width`,
restored, `ok`. See that test for both the named/purposed/neither cases and
the width proof below.

---

### Phase 11 line 683 — "Show the current lifecycle state for every session."

Contract, as decided by this package: **the STATE column now answers both
questions rather than replacing one with the other**, per the packet's
explicit instruction ("do not delete the disposition… Either answer is
acceptable; an unstated one is not").

`SessionRecord::disposition()` — what the shipped `STATE` column rendered
before this package, via `disposition_label` — collapses `SessionLifecycle`'s
seven values into four categories (`Active`, `Resumable`, `Closed`,
`Failed`). That is the right question for "is this resumable" (line 685) and
"can this be interrupted" (line 689), and both of those capabilities depend on
it, so it was kept rather than replaced.

But it is a different, coarser question than "the current lifecycle state" —
a `Running` session and an `Idle` one both read `active`, and a session
`Stopped` with no native identifier and one explicitly `Closed` both read
`closed`. Those are exactly the two dispositions (`Active`, `Closed`) whose
underlying lifecycle is genuinely ambiguous from the disposition alone —
`Resumable` only ever means `Stopped`-with-a-native-id and `Failed` only ever
means the `Failed` lifecycle, per `SessionRecord::disposition`'s own
exhaustive match, so appending the lifecycle word there would repeat the
disposition rather than add to it.

State: **CLOSED**, against this reading — the fine lifecycle state is shown
for every session for which it is not already implied by the disposition
alone, and the disposition itself, which line 685/689 depend on, is preserved
unchanged. **This is a judgement call inside a Sonnet implementer's stated
authority (a rendering decision, not a capability-map interpretation), and it
is flagged here rather than silently ticked** in case the orchestrator reads
"the current lifecycle state" more strictly — as meaning the raw
`SessionLifecycle` value with no exception, on every row, unconditionally.
Under that stricter reading this box is **OPEN**, and the fix is one line:
drop the `Resumable | Failed` short-circuit in `state_label` and always
append `lifecycle_word`. Both readings were considered; this package believes
the stated one is right, because a user who already knows a session is
`resumable` gains nothing from being told, redundantly, that it is `stopped`
— but the choice is recorded here rather than assumed away.

Production evidence:
- `shell/view.rs`: `lifecycle_word(SessionLifecycle) -> &'static str`, a new,
  exhaustive (no `_` arm) function giving each of the seven lifecycle values
  a column-width word.
- `state_label(session: &SessionRecord) -> String`, a new function,
  `disposition_label(session)` alone for `Resumable`/`Failed`,
  `"{disposition}/{lifecycle}"` for `Active`/`Closed`. Called from
  `render_overview`'s `STATE` column in place of the bare
  `disposition_label` call it replaced — `disposition_label` itself is
  unchanged and still the sole source of the disposition word.

Mutation: `state_label` replaced with `disposition_label(session).to_owned()`
(i.e., the pre-existing behavior, dropping the lifecycle half entirely).
`FAILED` against `the_new_overview_columns_survive_a_realistic_and_a_wide_width`'s
`"active/running"`/`"closed/stopped"` assertions, restored, `ok`.

---

### Phase 11 line 684 — "Show the last activity time for every session."

Contract: `SessionRecord::last_activity_at` (Phase 10, unchanged by this
package) is shown for every session, not for some.

The packet's own finding, confirmed: the overview's pre-existing `ACTIVITY`
block is a **global lifecycle-event feed**, bounded at `ACTIVITY_ROWS` (8)
and keyed by whichever sessions happened to produce a recorded event
recently — it renders nothing at all when no event has been observed, so a
session with a real, non-null `last_activity_at` can appear on a row with no
activity information shown anywhere near it. That block is unchanged by this
package (still useful — "what has this project been doing" is a different
question from "when did this session last do anything") but it does not
answer line 684, and nothing else did before this package.

State: **CLOSED.**

Production evidence:
- `shell/view.rs`: `render_overview`'s row loop now calls
  `describe_age(now, session.last_activity_at)` — the same function the
  `ACTIVITY` feed already used for its own per-event ages — for the new
  `ACTIVE` column, for every row of `state.sessions()`, unconditionally.
  `now` is read once, via `crate::provider::cache::now_unix_seconds()`,
  outside the loop.

Mutation: the call site changed from `describe_age(now, session.last_activity_at)`
to `describe_age(now, now)` (i.e., every row reads "just now" regardless of
its actual `last_activity_at`). `FAILED` against
`the_new_overview_columns_survive_a_realistic_and_a_wide_width`'s
`"1 minute ago"`/`"3 days ago"` assertions (the fixture sets distinct,
non-"just now" ages precisely so this mutation cannot hide), restored, `ok`.

---

### Phase 11 line 685 — "Show whether the native session can be resumed."

State: **REFERRED UP, not ticked — the orchestrator's call, per practice §33
and the packet's own instruction.**

`SessionRecord::disposition()` answers `Resumable` exactly when the session
is `Stopped` and `native_session_id.is_some()`, and that is what the `STATE`
column already showed before this package and still shows (see 683). The
open question, stated in the packet and not resolved by anything this package
built, is whether *"show whether the native session can be resumed"* is
answered for a **live** session too — one that reads `active`/`active/running`
and says nothing about resumability, because resumability is not yet a
meaningful question for a process that has not stopped.

**The argument for CLOSED as it stands:** the box is about a fact the user
needs *when it becomes actionable* — once a session has actually stopped, its
resumability is shown, unambiguously, on every row. A live session's future
resumability is not a fact yet; it is a prediction about how the session will
end, and Glasshouse does not currently predict that (nor does the map ask it
to elsewhere).

**The argument for OPEN:** a user scanning the table for "which of my stopped
things can I bring back" gets a complete, honest answer; a user asking "if I
stop *this* one, will I be able to resume it" — relevant before killing a
long-running session — gets nothing, because nothing here says whether *this*
harness, running or not, has a verified resume mechanism at all (some
harnesses' adapters have none; see line 688 below, `resume_args` returning
`None`). That is a fact about the *harness*, not the *session's* current
state, and it is knowable before the session stops.

This package did not build a live-session resumability indicator either way,
because doing so is a genuine design decision (a new fact source — the
adapter's `resume` capability, not the session record) that the packet
correctly flagged as the orchestrator's, not this tier's, to make.

---

### Phase 11 line 686 — "Show whether the session is embedded, headless, or external."

State: **was already CLOSED before this package; unchanged.**

Production evidence: `render_overview`'s `PRESENTED` column renders
`session.presentation` (`SessionPresentation: Embedded | Headless |
External`), which has its own `Display` (via the `sql_enum!` macro in
`session/store.rs`) rendering the three variants as `"embedded"`,
`"headless"`, `"external"` — three visually distinct strings, confirmed by
reading the `sql_enum!` invocation and by every existing test that starts a
headless session and checks the column (`a_session_started_headless_runs_and_is_listed_but_never_reaches_the_viewport`,
pty_smoke; `the_overview_shows_detail_the_session_bar_has_no_room_for`,
unit). No `External` session is producible from inside this codebase today
(nothing constructs one outside a test fixture — cmux integration is a later
phase), so the third variant's distinctness is verified by direct
construction (`shell/view.rs` and `shell/state.rs` test fixtures) rather than
by a live external session, which is the same limitation the map's own later
phases exist to remove.

---

### Phase 11 line 687 — "Allow the user to focus any live embedded session from the overview."

Contract: from the overview, some key brings the cursor's session — not
necessarily the one the bar is presenting — into the viewport and hands it
the keyboard, refusing a session that is not live or not embedded, by name.

State: **CLOSED.**

Production evidence:
- New key: `Enter`, claimed inside `handle_overview_key`
  (`shell/state.rs`), which was previously unclaimed there and fell through
  to `handle_control_key`'s own `Enter`/`i` binding
  (`enter_session_mode`, which acts on `active_session()` — the *presented*
  session, not the cursor's).
- New method `ShellState::focus_overview_target`, gated on
  `actionable_overview_target("focus")` (the shared liveness refusal
  `c`/`m` already use) **and** a second, `focus`-specific refusal —
  `session.presentation != SessionPresentation::Embedded` — spoken by name,
  matching the box's second adjective ("embedded"), which
  `actionable_overview_target` alone does not know about. On success: moves
  `ShellState::selected` to the cursor's session, closes the overview, and
  sets `Mode::Session`.
- The run loop's existing `sync_focus` (`shell/mod.rs`, called after every
  key) brings `SessionRuntime`'s own focus in line with `ShellState::selected`
  on the very next tick — no change needed there; this was already the
  mechanism `Tab`/`BackTab` use to move the viewport.

Unit tests (`shell::state::overview_tests`):
`enter_focuses_the_cursors_session_not_the_presented_one`,
`enter_refuses_a_live_headless_session_by_name`,
`enter_refuses_a_stopped_session`.

pty_smoke test, through the shipped binary:
`enter_from_the_overview_focuses_the_cursors_session_not_the_presented_one` —
two live sessions running a harness that tags its reply with its own
`--session-id`, cursor moved off the presented row with `Down`, `Enter`
pressed, and the *typed keystroke's reply* is asserted to come from the
cursor's session's own identifier, not the presented one's. (A single-session
version of this test was written first and passed unchanged with the new
`Enter` binding deleted — §35's trap, caught before it shipped; see the
mutation table.)

---

### Phase 11 line 688 — "Allow the user to resume any compatible stopped session from the overview."

Contract: from the overview, some key resumes the cursor's session if it is
`Resumable`, refusing every other disposition by name — the "compatible"
adjective being `session::select::HarnessSelection::resume_args` returning
`None` for a harness with no verified resume mechanism.

State: **CLOSED**, with one gap and one production fix, both recorded below.

Production evidence:
- New key: `r` (unclaimed inside `handle_overview_key`, unclaimed by
  Settings' own `r`, a different overlay).
- New method `ShellState::resumable_overview_target`, gated on
  `SessionRecord::disposition() == Resumable` — **its own gate, not
  `actionable_overview_target`**, per the packet's explicit warning: that
  helper refuses every session whose lifecycle is not live, which is exactly
  backwards for resume (whose entire subject is a session that is *not*
  running). Refuses `Active`/`Failed`/`Closed` by name with a distinct reason
  for each.
- New `Action::ResumeSession(SessionId)` and a new function
  `shell::resume_session` (`shell/mod.rs`), the embedded counterpart of
  `main.rs::resume_session`: `store.open_for_resume(id)` (the same store-side
  gate the CLI path uses — belongs to this project, not live, has a native
  id), `session::select::select(Some(harness), …)` (the record's own
  harness, never whatever is configured now), `selection.resume_args(…)`
  (`None` → a spoken refusal naming the harness), hooks/response-profile
  installed the same way `start_session` does, `live.start` with the
  session's own recorded id, and `store.set_lifecycle(…, Running)` on
  success.

**Production fix found and made while proving this end to end**:
`SessionRuntime` (`session/runtime.rs`, outside this package's writable
files but freely callable) never drops an exited `LiveSession` on its own,
and `SessionRuntime::get`/`focus` resolve the *first* entry matching a given
id. Calling `live.start` again with a resumed session's own (pre-existing)
id, without first removing the exited entry, would have left every viewport
read, focus, interrupt and send silently resolving to the dead process's
frozen final screen instead of the one just started — the resumed process
would run for real, but nothing in the shipped binary would ever show or
reach it. Fixed with one call, `live.close(&resumable.id)`
(`SessionRuntime::close`, an existing public method — no `session/**` edit),
immediately before `live.start`, best-effort (a `NotLive` error there would
only mean the entry was already gone).

**Second production fix, load-bearing for this box specifically**: nothing in
the shipped binary called `ShellState::refresh` when `SessionRuntime::poll_exits`
noticed a process exit — the session bar and overview only re-read the store
on `n`/`N` succeeding or on `AppEvent::Redraw`, and an exit noticed on a plain
tick is neither. Without a fix, a session's disposition would turn
`Resumable` in the database the instant it exits, but `ShellState::sessions()`
— what the overview renders and what `resumable_overview_target` reads —
would keep showing it as `active` until some unrelated event happened to
trigger a refresh, so `r` pressed against a session that had just stopped
would be refused as "still running" against a record the store had already,
correctly, marked `Stopped`. Fixed in `shell/mod.rs`'s `Event::Tick` handler:
when `poll_exits` reports at least one exit, `state.refresh(sessions.store().list()?)`
is called in the same tick, mirroring `Event::App(AppEvent::Redraw)`'s
existing pattern.

Unit tests (`shell::state::overview_tests`):
`r_resumes_a_stopped_session_with_a_native_identifier`,
`r_refuses_a_stopped_session_with_no_native_identifier`,
`r_refuses_a_live_session` — the last one specifically proving
`resumable_overview_target` is not `actionable_overview_target`, by
reproducing the trap the packet warned about (mutated to call
`actionable_overview_target("resume")` directly; all three tests above
failed, confirming the trap is real and this gate catches it).

pty_smoke test, through the shipped binary:
`a_session_started_from_the_shell_is_resumed_from_the_overview` — `n` starts
an embedded session under a fake harness that exits immediately and logs its
own arguments to a file (not its stdout — see the test's own doc comment on
why a value ratatui's diff-based renderer redraws is not reliably visible in
the raw pty byte stream a test captures), waits for the store to report
`Resumable` (proving the second fix above), opens the overview (`"resumable"`
now visible on screen, proving 683's STATE column), presses `r`, closes the
overview, and asserts the *second* invocation's own marker text reached the
viewport and its log line carries `--resume` and the original session's
identifier, not a fresh `--session-id` (proving the first fix above and the
whole resume path end to end).

**What this package did not build, and why, named per the packet's stop
condition:** `resume_session` does not re-resolve the session's launch
profile overlay the way `main.rs::resume_session` does
(`resolve_resume_overlay`) — that function is private to `main.rs`, which is
outside this package's writable files. A session resumed from the overview
therefore runs on a plain resume invocation with no regenerated provider
configuration; a session resumed via `glasshouse resume` on the CLI still
gets the full overlay treatment. This is a real, scoped gap for the next
package to close (making `resolve_resume_overlay` `pub(crate)` and sharing
it, or an equivalent), not a silent approximation — it does not affect
whether the box's stated capability ("resume … from the overview") works,
only whether a resumed embedded session's provider configuration is as
complete as a resumed CLI one's.

---

### Phase 11 line 689 — "Allow the user to interrupt a running session from the overview."

State: **was already CLOSED before this package (Phase 4); unchanged, not
rebuilt.**

Production evidence: `c` → `ShellState::interrupt_overview_target` →
`Action::InterruptSession(id)` → `shell::interrupt_session` (`shell/mod.rs`)
→ `SessionRuntime::interrupt`. Confirmed unbroken by this package's own new
`state.refresh` call in the `Tick` handler (full `pty_smoke` suite, 71/71,
includes `an_interrupt_sent_from_the_overview_reaches_a_real_child` and
`an_interrupt_reaches_an_unfocused_session_and_leaves_it_running`) and by the
full `shell::state`/`shell::view` unit suite (1109/1109), run after every
change in this package, including after the `state.refresh` addition.

---

## Widths proved, and at what size

Per practice §17: every new column asserted at a realistic width **and** a
wide one, both directions (hardened test + mutation FAILS, hardened test +
clean code passes) — see the mutation table below.

- `HARNESS`/`STATE`/`ROLE`/`PRESENTED`/`SESSION`, the five pre-existing
  columns, are unchanged in position and (mostly) width, and remain proved at
  the pre-existing tests' width of 120 columns
  (`the_overview_distinguishes_the_row_it_acts_on_from_the_one_on_screen`,
  unchanged and still passing) — `STATE`'s content can now be a few
  characters longer for an `Active`/`Closed` session (e.g.
  `"active/running"` vs. the old `"active"`), which was checked against that
  test and does not push anything else out of view at that width.
- `ACTIVE` and `NAME`, the two new columns, are written **last** in the row —
  after `SESSION` and its `(viewport)` marker — specifically so a narrow
  terminal clips them, not the identifier the interrupt/send/resume keys act
  on and name in their own refusals. Proved present and correct at **160
  columns** (the first width at which this package's fixture — a 15-character
  name and a 5-character purpose — fits) and at **400**. At the
  pre-existing suite's 100-column width, `ACTIVE`/`NAME` do not fit and are
  clipped — a design finding, recorded rather than hidden, matching the
  packet's own permission to report this outcome instead of forcing a fit at
  an unrealistic width.

## Mutation table

| # | file:function | mutation | test | before fix | after fix |
|---|---|---|---|---|---|
| 1 | `view.rs::name_or_purpose` | always return `"(unnamed)"` | `the_new_overview_columns_survive_a_realistic_and_a_wide_width` | FAILED (caught) | ok |
| 2 | `view.rs::state_label` | drop lifecycle, return disposition only | same | FAILED (caught) | ok |
| 3 | `view.rs` row loop | `describe_age(now, now)` instead of `describe_age(now, session.last_activity_at)` | same | FAILED (caught) | ok |
| 4 | `state.rs::handle_overview_key` | `KeyCode::Enter if false => …` (unclaims Enter) | unit: `enter_focuses_the_cursors_session_not_the_presented_one`; pty: `enter_from_the_overview_focuses_the_cursors_session_not_the_presented_one` | FAILED both (caught) | ok |
| 5 | `state.rs::focus_overview_target` | `if false && … != Embedded` (drops the embedded refusal) | `enter_refuses_a_live_headless_session_by_name` | FAILED (caught) | ok |
| 6 | `state.rs::handle_overview_key` | `KeyCode::Char('r') if false && !ctrl => …` (unclaims `r`) | pty: `a_session_started_from_the_shell_is_resumed_from_the_overview` | FAILED (timed out — never became actionable) | ok |
| 7 | `state.rs::resumable_overview_target` | delegate to `actionable_overview_target("resume")` (the trap the packet named) | `r_resumes_a_stopped_session_with_a_native_identifier`, `r_refuses_a_stopped_session_with_no_native_identifier`, `r_refuses_a_live_session` | FAILED all three (caught) | ok |
| 8 | `mod.rs::resume_session` | comment out `live.close(&resumable.id)` | pty: `a_session_started_from_the_shell_is_resumed_from_the_overview` | FAILED (timed out — resumed process's own text never reached the viewport) | ok |
| 9 | `mod.rs` `Event::Tick` handler | `if false && any_exited && state.refresh(…)` (drops the post-exit refresh) | pty: same | FAILED (timed out — STATE column stayed `active/starting`, overview never showed `"resumable"`) | ok |

Every mutation above was restored immediately after its `FAILED` was
observed; `cargo test -p glasshouse --lib` (1109 passed) and
`cargo test -p glasshouse --test pty_smoke` (71 passed) both ran clean on the
restored tree, after mutation #9 (the last one applied).

## Gate

`cargo fmt --edition 2024` on the four files this package touched (narrow
rustfmt invocations only, per practice §37 — `cargo fmt --all` was not run).
`RUSTFLAGS="-D warnings" cargo clippy -p glasshouse --all-targets`: clean.
`RUSTFLAGS="-D warnings" cargo doc -p glasshouse --no-deps`: clean, run alone
per the packet's instruction. The full local `ci-local.sh` gate was **not**
run by this package — only `cargo test --lib`, `cargo test --test
pty_smoke`, `cargo clippy`, and `cargo doc`, each run alone (never beside
another `cargo` invocation, per practice §40).
