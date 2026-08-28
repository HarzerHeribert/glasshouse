# Capability evidence — phase 1

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 1 — Display the active canonical project root prominently in the TUI

Contract: Given the interactive shell, when any frame is drawn, the active
canonical project root is on its own labelled row, on every screen including
behind an overlay, while a narrow terminal drops the head of the path and keeps
the tail that identifies the project.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/shell/view.rs: render_root` — a dedicated row, not a
  corner. The value comes from `Project::display_root`, the same canonical root
  every access-control decision uses.

Regression evidence:
- `the_project_root_is_displayed_on_every_frame`
- `the_project_root_stays_visible_while_an_overlay_is_open` — "prominently"
  cannot mean "until you open something".
- `a_narrow_terminal_keeps_the_end_of_the_project_root`
- `a_wide_terminal_shows_the_whole_project_root`
- `the_shell_opens_in_a_real_terminal_and_answers_the_keyboard` — proved in a
  real terminal, anchored to the `root ` field rather than to any word that
  also appears in the title bar.

Failure/isolation evidence:
- Mutation: blanking the root fails the unit test and the real-terminal test.
- Mutation: truncating from the wrong end fails the narrow-terminal test.
- Both assertions were vacuous in their first form and were tightened after the
  mutations exposed them.

Platform/external evidence:
- CI `32816717226` on `5da067a` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 257 lib and 23 PTY tests,
  including the real-terminal shell test, rather than merely reporting green.

Missing evidence:
- none.

### Phase 1 — Reject any attempt to resume a Glasshouse-managed session whose project identifier differs from the current project identifier

Contract: Given a session record whose project identifier differs from the
active project's, when anything attempts to resume it, Glasshouse refuses and
names both projects, while leaving the record untouched and while the database
itself refuses to store such a record in the first place.

State: COMPLETE.

Production evidence:
- `session/store.rs: SessionStore::open_for_resume` — compares the stored
  project identifier against the active one before returning anything
  actionable.
- `main.rs: resume_session` — **the production caller this entry waited three
  sessions for.** `glasshouse resume` resolves an identifier and then goes
  through `open_for_resume` *before* a harness is selected and long before any
  process exists, so a refusal costs nothing and cannot half-start a session.
- `database.rs: MIGRATIONS[1]` — `BEFORE INSERT` and `BEFORE UPDATE OF
  project_id` triggers abort any row whose `project_id` is not the identifier
  bound in `project_metadata`. Structural, so no present or future query has to
  remember to filter by project.

Regression evidence:
- `resuming_a_session_belonging_to_another_project_is_refused` — the error names
  both projects and the planted record is left byte-for-byte intact.
- `the_database_refuses_to_store_a_session_from_another_project`
- `a_stored_session_cannot_be_reassigned_to_another_project`
- `a_stopped_session_of_this_project_can_be_resumed` — the permitted case, so
  the refusals above are not merely "resume never works".
- `two_projects_have_independent_session_lists`
- `resuming_a_session_with_no_conversation_is_refused` (PTY smoke, Unix) — the
  shipped binary refuses a session that has nothing to resume to, and the
  harness is never started. This is the test that reaches `open_for_resume` on
  the production path; `resuming_an_unknown_session_is_refused` does not,
  because the identifier resolver turns it away first.
- `a_recorded_session_is_resumed_under_the_identifier_it_was_given` (PTY smoke,
  Unix) — the permitted case end to end through the shipped binary.

Failure/isolation evidence:
- Mutation: removing the project comparison in `open_for_resume` fails the
  cross-project test.
- Mutation: dropping the `BEFORE INSERT` trigger fails the structural test.
- Mutation: weakening the trigger's `IS NOT` to `<>` fails
  `a_session_write_is_refused_when_the_project_binding_is_missing`, which is
  what proves the guard fails closed rather than silently passing a NULL
  comparison.
- Mutation: making `resume_session` read the record directly instead of through
  `open_for_resume` fails `resuming_a_session_with_no_conversation_is_refused`.
  **This mutation initially passed**, which is how it was discovered that the
  unknown-identifier test proved nothing about the guard — the resolver refuses
  first. The test above was written specifically to reach it.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- None. Note that the cross-project case cannot be reached end to end through
  the binary, because the migration triggers refuse to store such a row in the
  first place — reaching it at all requires the test to plant one by tampering,
  which `resuming_a_session_belonging_to_another_project_is_refused` does. That
  is the guard being defence in depth, not a gap: the structural refusal is the
  first line and the comparison is the second.

### Phase 1 — Ensure every spawned harness process starts with its working directory set to the current project root

Contract: Whenever Glasshouse invokes an installed harness—including discovery
probes and interactive sessions—the child starts in the active canonical
project root and never inherits an unrelated caller directory.

State: COMPLETE

Production evidence:

- `main::launch_session` is the production consumer: `glasshouse launch
  [harness]` resolves a harness through `session::select::select` and starts it
  through `launch::HarnessLaunch`, which is the only route that reaches PTY
  spawn for a harness. This closes the gap recorded in the previous revision of
  this entry, where `HarnessLaunch` had no production caller at all.
- `session::attach::attach` runs the resulting session against the real
  terminal: raw mode, input/output pumps, resize forwarding, exit propagation,
  and restoration on every exit path.
- `launch::HarnessLaunch::spawn` reaches PTY spawn only through
  `TerminalCommand::for_harness`, which derives the directory from
  `Project::display_root` and is `pub(crate)`.
- `integrations::Discovery::run(&Project)` threads the active project into
  `version::probe_version`, which sets `Command::current_dir` from
  `Project::display_root`.

Regression evidence:

- `the_launch_command_opens_the_configured_harness_inside_the_project_root`
  (pty_smoke, macOS) runs the *shipped binary* in a real pseudo-terminal and
  matches the harness's own report of its working directory against the project
  root by filesystem identity. Glasshouse itself is deliberately run from a
  different directory, so an inherited cwd cannot pass.
- `a_fake_installed_harness_launches_inside_the_discovered_project_root`
  proves the same for the `HarnessLaunch` mechanism directly.
- `version_probe_child_starts_in_the_active_project_root` uses a resolved fake
  probe that prints a version only in the correct child directory.
- `project_configured_executable_wins_over_user_level` and the rest of
  `session::select::tests` pin executable precedence and every refusal path.
- Windows-only tests pin verbatim drive and UNC prefix conversion.

Failure/isolation evidence:

- The end-to-end test installs a *decoy* executable at the user level and the
  real one at the project level; a precedence failure runs the decoy and fails
  the test loudly rather than silently passing.
- `a_failing_configured_executable_never_falls_back_to_path` proves a broken
  configured path is an error, not a silent substitution of another binary.
- `attaching_without_a_terminal_is_refused` fails closed rather than hanging on
  a pty query nothing can answer.
- `PtyProcess::spawn` refuses a working directory that does not exist instead
  of starting the child somewhere else.
- Unsafe Windows-script arguments are rejected before `HarnessLaunch` spawns.

Non-vacuity (mutations actually run, each observed to fail the test):

- Removing the project layer from `EffectiveConfig::executable` → the decoy
  runs; the end-to-end test fails on precedence.
- Making `TerminalCommand::for_harness` use the process cwd instead of the
  project → no harness reports the project root; the test fails.
- Making `exit_code_for` always return success → the test fails on exit-code
  propagation.
- Setting `PTY_ALLOCATION_ATTEMPTS` to 1 → the pty retry test fails.

Platform/external evidence:

- **CI run `32788123095` on commit `f3effe6` is green on `ubuntu-latest`,
  `macos-latest`, and `windows-latest`, plus lint.** The Windows job was
  confirmed to have *actually executed* the tests cited here — 186 lib tests
  and 20 PTY smoke tests — rather than only reporting a green tick;
  `the_launch_command_opens_the_configured_harness_inside_the_project_root`
  and `a_fake_installed_harness_launches_inside_the_discovered_project_root`
  both appear as `ok` in the Windows log.
- Local macOS: `cargo fmt --check`, `cargo clippy -D warnings`, unit and PTY
  smoke tests, MSRV 1.85.0 `cargo check --locked`, `git diff --check`, and
  live CLI probes of `glasshouse launch` (help, no-terminal refusal, unknown
  harness, non-harness integration).
- An independent spawn-site inventory, re-run after the merge, found three
  production spawn sites, all project-bound, and zero production callers of
  the generic `TerminalCommand::new`.

What CI caught that local evidence had not:

- `cmd.exe` cannot open the verbatim `\\?\` path that resolving an executable
  produces on Windows, so **no `.cmd`-shimmed harness could start there at
  all** — and npm installs most of them that way. Fixed in `4aa31ad`.
- A prior revision of this entry cited a Windows job that had never run
  `tests/pty_smoke.rs`: when the lib target fails, cargo never reaches the
  integration tests, so the `.cmd` and verbatim-path claims were unproven
  while the entry implied otherwise. Confirming *execution*, not just the
  job's conclusion, is part of this evidence and not a formality.

Remaining caveats (recorded, not blocking):

- Native Windows UNC project roots are still refused rather than supported;
  `cmd.exe` cannot reliably hold a UNC working directory. This is a
  documented limitation of the contract, not a gap in it.

### Phase 1 — Keep cross-project memory retrieval disabled by design rather than relying only on query filters

Contract: Given two Glasshouse projects on one machine, when either retrieves
durable memory, it can reach only its own project's memories — and it can do so
because the storage is physically and structurally separated, not because a
`WHERE` clause remembered to filter.

State: **COMPLETE**

Production evidence:
- `memory/store.rs::ProjectMemory::open(runtime)` — **takes no path and no
  project identifier.** Its own doc states the constraint the line asks for:
  *"The path comes from `runtime` and nowhere else, so there is no argument a
  caller could pass to reach another project's file."*
- `memory/store.rs::MemoryStore::new` / `with_clock` — the project identifier is
  **read out of the database** rather than accepted as an argument, so a store
  is bound to whichever project's file it was handed, and a caller that is
  confused about which project it is in cannot express that confusion.
- One database per project, in its own directory keyed by the project id —
  Phase 1's *"store each project's memory in its own SQLite database"*, already
  COMPLETE. Verified on this machine: fourteen projects, fourteen
  `projects/<project-id>/glasshouse.db` files.
- `database.rs::memories_reject_foreign_project_insert` — a `BEFORE INSERT`
  trigger comparing `NEW.project_id` against the database's own recorded
  binding. **This is the mechanism the line means by "by design".**

Regression evidence:
- `tests/project_isolation.rs::one_project_database_cannot_be_queried_through_another_projects_glasshouse_instance`
- `tests/project_isolation.rs::memory_extraction_only_ever_writes_into_its_own_projects_database`
- `tests/project_isolation.rs::deleting_one_projects_state_leaves_a_sibling_projects_state_intact`
  — proves the physical separation the other two rest on.

Failure/isolation evidence:

**The decisive observation is what the isolation test has to do to test
anything at all.** `plant_foreign_memory` must
`DROP TRIGGER memories_reject_foreign_project_insert`, insert, and then
re-create the trigger. A foreign memory row **cannot be created through any
ordinary path** — the test has to dismantle the guard to manufacture the
condition it then checks the filter against.

Confirmed by the integrator directly against a **real database written by the
shipped binary** (copied to a scratch path first; no live project database was
written). A plain `INSERT` naming another project is refused by SQLite itself:

    IntegrityError: memory belongs to a different project
    foreign rows present: 0

That is the whole of this line's claim, demonstrated rather than argued: the
`AND memories.project_id = ?2` in `MemoryStore::search` is **defence in depth
over a state the schema refuses to reach**, not the thing standing between two
projects. Both layers are load-bearing and neither is alone: the trigger
prevents the row existing, and the filter still excludes one planted by
tampering — which is exactly *"disabled by design rather than relying **only**
on query filters."*

Missing evidence:
- None. Note the one thing this does **not** claim: it is about *retrieval*.
  The memory store's six `UPDATE memories … WHERE id = ?1` statements carry no
  `project_id` in their own `WHERE` clause and are scoped by a leading
  `self.get(id)` guard in each function instead. No live defect — every one of
  those functions currently has the guard — but that is the *write* path, it is
  a check-then-act pair rather than a structural refusal, and
  `memories_reject_foreign_project_update` does not cover it because it is
  `BEFORE UPDATE OF project_id`. See `docs/product/evidence/phase-21f.md` for
  the mutation that demonstrates it and the red-tier package it earns.
