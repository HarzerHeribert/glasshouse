# Capability evidence — phase 46

Six of the map's eight lines are provable and closed by this batch. The other
two — cmux session metadata and MCP — name surfaces that do not exist in this
crate; see their own entries below rather than the closed six.

All evidence here is local (no CI run yet): `crates/glasshouse/tests/project_isolation.rs`,
uncommitted on top of `60f8c9f`, gate not run by this worker (§40 — never
beside another `cargo`; local `cargo doc -p glasshouse --no-deps`, targeted
`cargo test`, and `cargo clippy -p glasshouse --test project_isolation --
-D warnings` all pass). The orchestrator's own gate run is the platform/external
evidence this entry is missing.

### Phase 46 — Add automated tests proving one project database cannot be queried through another project's Glasshouse instance.

Contract: Given two real, canonicalised project roots each with their own
`glasshouse.db`, when either project's instance is asked for a record, it
never returns another project's row — neither silently (physical separation)
nor when a foreign row has reached its file by some route the insert trigger
never saw.

State: COMPLETE

Production evidence:
- `memory/store.rs: MemoryStore::get` — compares the stored `project_id`
  against the active project before handing a record back, the same read
  boundary `session/store.rs::open_for_resume` uses for sessions (see the next
  entry) and that `memory/store.rs`'s own module doc names as the third of
  three independent enforcement points (file, row-trigger, read-boundary).

Regression evidence:
- `one_project_database_cannot_be_queried_through_another_projects_glasshouse_instance`
  — honest case first (beta's `get` on alpha's real id returns `None`, and
  vice versa; each instance still answers for its own record), then the
  defence-in-depth case: a row planted directly into beta's file with
  `project_id` set to alpha's identifier (bypassing and restoring
  `memories_reject_foreign_project_insert`, the way a restored backup or an
  older build might arrive) is refused with `MemoryStoreError::ForeignProject`
  naming both projects, not silently returned and not silently absent.

Failure/isolation evidence:
- Mutation: in `memory/store.rs::MemoryStore::get`, changed the guard's
  condition from `record.project_id != self.project_id` to
  `false && record.project_id != self.project_id` (never holds). The test
  failed: the planted foreign row came back as `Ok(Some(record))`, i.e. beta's
  instance handed back alpha's row as its own. Restored with `cp` from a
  backup + `touch`; test green again.

Platform/external evidence: none yet (see the file header). No `#[cfg(unix)]`
gate on this test — it runs everywhere.

Missing evidence:
- CI run confirming the same on Linux, macOS and Windows.

### Phase 46 — Add automated tests proving a session from project A cannot be resumed from project B.

Contract: Given a session recorded in project A, when project B's instance is
asked to resume it, Glasshouse refuses — whether B simply has no record of it
(the honest case) or a foreign row for it has reached B's file by some route
the insert trigger never saw (the defence-in-depth case) — and the refusal
names both projects while leaving any planted record untouched.

State: COMPLETE

Production evidence:
- `session/store.rs: SessionStore::open_for_resume` — already the production
  caller `docs/product/evidence/phase-1.md` establishes reaches through
  `main.rs::resume_session` before any harness is selected. This entry is new
  regression coverage of the same guard from the *other* project's instance,
  which phase 1's entry does not exercise.

Regression evidence:
- `a_session_from_project_a_cannot_be_resumed_from_project_b` — asserts the
  premise first (alpha resumes its own stopped, native-id-bearing session
  successfully — practice §17), then the honest case (beta's `open_for_resume`
  on alpha's real session id is `SessionStoreError::NotFound`, because beta's
  database was never handed it at all), then the defence-in-depth case (a row
  planted directly into beta's file with `project_id` set to alpha's
  identifier is `SessionStoreError::ForeignProject`, naming both projects, and
  the planted row is still there afterwards, byte-for-byte).

Failure/isolation evidence:
- Mutation: in `session/store.rs::open_for_resume`, changed
  `record.project_id != self.project_id` to
  `false && record.project_id != self.project_id`. The test failed: beta's
  instance returned `Ok(ResumableSession { id: "planted-session", .. })` for
  alpha's session. Restored with `cp` + `touch`; test green again.

Platform/external evidence: none yet. Not `#[cfg(unix)]` — no PTY or symlink
involved, runs everywhere including Windows.

Missing evidence:
- CI run on all three platforms.

### Phase 46 — Add automated tests proving canonicalized paths cannot escape the project root through parent-directory traversal ...

(Map line 1743's own wording ends in a literal `...`; quoted as it stands.)

Contract: Given a project's `ProjectScope`, when a caller-supplied relative
path is resolved, any parent-directory (`..`) traversal that would leave the
canonical project root is refused, even when the escape target is a real,
existing directory (a sibling project, not merely an arbitrary outside path).

State: COMPLETE, with a caveat on caller breadth (see Missing evidence).

Production evidence:
- `project/scope.rs: ProjectScope::resolve` — the guard. It has exactly one
  production caller today, `config::project_config_path` (used by
  `load_project_config` / `write_project_config_with_consent`), and that
  caller only ever feeds it the fixed constant `.glasshouse/config.toml` —
  never caller-controlled input. There is currently no production route that
  feeds `resolve` an attacker-influenced path to traverse with. This test
  exercises the guard directly through `Project::scope()`, the same public
  accessor and identical code that caller runs.

Regression evidence:
- `canonicalized_paths_cannot_escape_the_project_root_through_parent_directory_traversal`
  — asserts the premise first (`inside.txt`, a real file in project alpha,
  resolves), then four traversal shapes (`../beta/secret.txt`, `../beta`,
  `sub/../../beta/secret.txt`, `..`) against a **real sibling project beta**
  sharing the same tempdir workspace, every one refused
  (`ScopeError::Traversal` or `ScopeError::OutsideProject`).

Failure/isolation evidence:
- Mutation: in `project/scope.rs::ProjectScope::resolve`, changed the final
  `if platform::is_within(&self.root, &resolved)` to
  `if true || platform::is_within(&self.root, &resolved)`. The test failed:
  `../beta/secret.txt` resolved `Ok` to beta's real file. Restored with `cp` +
  `touch`; test green again. (This single mutation also kills the symlink
  test below — same final check, two different ways of reaching it.)

Platform/external evidence: none yet. Not `#[cfg(unix)]` — no symlink or PTY
here, pure relative-path arithmetic, runs identically on Windows.

Missing evidence:
- **The guard is proven directly; a caller feeding it attacker-controlled
  input is not proven, because none exists yet.** If a future caller resolves
  a caller-supplied (not fixed-constant) relative path through
  `ProjectScope::resolve`, it should get its own regression test reaching the
  guard through that caller specifically — practice §35's lesson, that a
  caller every test bypasses is not a caller, applies in reverse here: there
  is no caller to bypass yet, only the guard everything must eventually go
  through.
- CI run on all three platforms.

### Phase 46 — Add automated tests proving symlink targets outside the project root are rejected by Glasshouse-controlled file operations.

Contract: Given a project whose `.glasshouse` directory is a symlink resolving
outside the project root, when Glasshouse's own project-config file
operations run, they refuse rather than following the symlink, and nothing is
written through it.

State: COMPLETE (Unix only — see Missing evidence).

**Orchestrator check on that Unix-only caveat, because this project has refused a
box for exactly this before.** Phase 4's interrupt line stayed open for months on
the rule that *a green `test (windows-latest)` is the absence of evidence wearing
the same colour* — every interrupt test being `#[cfg(unix)]`. The question is
whether that precedent applies here, and it does not:

    grep -nE 'cfg\(|target_os' crates/glasshouse/src/project/scope.rs
      188  #[cfg(unix)]        \ inside contains_nul() — NUL-byte encoding,
      193  #[cfg(not(unix))]   / OsStrExt::as_bytes vs encode_wide
      (nothing else before the #[cfg(test)] module at 223)

The **only** platform branch in production is `contains_nul`, which is about how a
NUL byte is spelled in an `OsStr` and is not on the symlink path. `resolve`,
canonicalisation and the `is_within` containment check are **one code path on every
platform**. So the Unix-only test exercises byte-for-byte the code Windows runs.

That is the opposite of Phase 4, where Windows ran a *different* implementation
(ConPTY) that had never executed at all. Here the limitation is the **fixture** —
creating a symlink needs `std::os::windows::fs::symlink_dir` and, on Windows,
privileges a CI account may not hold — not the code under test. **Box ticked, and
the fixture gap is recorded rather than hidden:** a Windows symlink fixture would
add coverage of the OS's own symlink semantics, and nobody should read this entry
as claiming it exists.

Production evidence:
- `config/mod.rs: load_project_config`, `write_project_config_with_consent` —
  both resolve `.glasshouse/config.toml` through `project_config_path`, which
  goes through `ProjectScope::resolve` rather than a raw `root().join(...)`
  specifically so a symlink planted at `.glasshouse` cannot be used to escape
  (see that function's own doc comment). This is a real, existing dynamic
  production caller, unlike the traversal box above.

Regression evidence:
- `symlink_targets_outside_the_project_root_are_rejected_by_project_config_io`
  (`#[cfg(unix)]`) — plants `.glasshouse` in a real project alpha as a symlink
  to a real sibling project beta's directory. Both `write_project_config_with_consent`
  and `load_project_config` are refused with `ConfigError::Scope`, and
  critically beta's directory gains no `config.toml`. Then asserts the premise
  in reverse order (practice §17: a negative test must first establish the
  positive case works) — with the symlink removed, the identical call
  succeeds and really does write inside alpha.
- This exercises the same production path the existing unit test
  `config::mod::tests::project_config_path_is_resolved_through_the_project_scope`
  covers, against a **real sibling project** as the escape target rather than
  an arbitrary outside directory, per this phase's contamination framing.

Failure/isolation evidence:
- Shares the mutation from the traversal test above (same guard,
  `ProjectScope::resolve`'s final containment check): under
  `if true || platform::is_within(...)`, this test also failed —
  `write_project_config_with_consent` returned `Ok(())` instead of refusing,
  meaning it would have written through the symlink. Restored, green again.

Platform/external evidence: none yet.

Missing evidence:
- **Windows.** `std::os::windows::fs::symlink_dir` needs a privilege this
  sandbox does not reliably have, so this test is `#[cfg(unix)]` only and
  proves nothing there — one of two `#[cfg(unix)]` tests in this batch (the
  other being none; this is the only one). `project::scope`'s own existing
  unit tests already establish `resolve` runs identical code cross-platform,
  which is why one platform is treated as sufficient for the *logic*, but the
  claim "Glasshouse-controlled file operations reject it" is unverified on
  Windows specifically.
- CI run on macOS and Linux at minimum.

### Phase 46 — Add automated tests proving memory extraction cannot write into another project's database.

Contract: Given two real projects, when memory extraction runs against one of
them, the recorded memory lands only in that project's database, and even a
mis-tagged write attempt is refused by the database itself.

State: COMPLETE

Production evidence:
- `memory/extract/mod.rs: Extractor::run` (via `store_one` →
  `MemoryStore::record`) — the same pipeline `glasshouse memory extract`
  drives (`main.rs`'s handler opens its store via `ProjectMemory::open(runtime)`,
  identically to every other `ProjectMemory::open` call site in that file).
  There is no path argument and no project argument anywhere between
  `Extractor::new` and the insert — structurally, extraction cannot be handed
  another project's database.

Regression evidence:
- `memory_extraction_only_ever_writes_into_its_own_projects_database` — runs a
  real `Extractor` (with a canned model, the same fake `tests/memory_extract.rs`
  uses) against project alpha's store and confirms `outcome.recorded.len() ==
  1`, then confirms alpha's active-memory count is 1 and **beta's is 0** —
  beta never gained a row from alpha's extraction run.

Failure/isolation evidence:
- Mutation: in `memory/store.rs::MemoryStore::record`, changed
  `project_id: self.project_id.clone()` to a fixed wrong string
  (`"deliberately-wrong-project-id"`), simulating extraction (or any writer)
  mis-tagging a row. The test failed: `outcome.recorded` went from length 1
  to empty, because `memories_reject_foreign_project_insert` aborted the
  insert and `record()` returned `Err`, which `store_one` folds into
  `outcome.rejected` rather than `outcome.recorded`. This is the database
  layer catching a bug the structural guarantee is not supposed to let happen
  in the first place — defence in depth, proven. Restored with `cp` + `touch`;
  test green again.

Platform/external evidence: none yet. Not `#[cfg(unix)]` — pure SQLite,
identical on every platform.

Missing evidence:
- CI run on all three platforms.

### Phase 46 — Add automated tests proving a project-state deletion removes only that project's Glasshouse state.

Contract: Deleting one project's entire Glasshouse state directory leaves a
sibling project's state directory, database, and recorded data completely
untouched.

State: **NOT STARTED — no project-state deletion exists to test.** Downgraded
from this package's COMPLETE by the orchestrator; the package's own note below
is the argument for the downgrade, and it made that argument honestly.

Practice §33: ask the capability as a question a user would ask. *"Does a
project-state deletion remove only that project's state?"* Glasshouse cannot
delete project state at all — verified independently: `remove_dir_all` appears
nowhere under `crates/glasshouse/src`, the only `delete_*` function in the crate
is `shell/mod.rs::delete_provider_credential`, and `cli.rs` exposes no delete,
remove, purge or forget subcommand. The line's verb is *deletion*; there is no
deletion, so there is nothing for the test to be about.

**The test is kept and is not wasted.** It proves the physical-separation
invariant any future deletion will depend on, and its mutation is the strongest
in the batch: collapsing `RuntimePaths::project_state_dir` to a shared directory
failed *earlier than the test's own assertions*, inside `bootstrap`, with
`DatabaseError::ProjectMismatch` — the separation is enforced two layers below
where the test looks. When a deletion command is added it needs its own
regression test reaching this invariant through that specific caller.

**Map note for whoever schedules next:** no phase in the capability map appears
to own a project-state deletion command, so this line is blocked on a capability
nothing is scheduled to build. It needs a home or a rewording — the user's call.

**There is no `glasshouse` subcommand, and no code path anywhere in
`crates/glasshouse/src`, that deletes a project's state** (confirmed:
`remove_dir_all` does not appear anywhere under `src/`). So "a project-state
deletion" is not yet a Glasshouse *feature* to test — it is the physical-
separation invariant `project/mod.rs`'s own module doc states ("everything
downstream ... is keyed by a `ProjectId` ... so cross-project access is
prevented by physical separation") applied to deletion specifically, by
whatever eventually performs it (an operator, `rm -rf`, a future housekeeping
command). This is not the "guard is missing" case the packet describes —
the guard (physical separation via `RuntimePaths::project_state_dir`) fully
exists and is exercised; there is simply no deletion *feature* yet for the
guard to be a precondition of.

Production evidence:
- `paths.rs: RuntimePaths::project_state_dir` — every project's state lives
  under `<data_dir>/projects/<project_id>`, a directory keyed structurally by
  the project identifier, which is what makes an operation scoped to one
  project's directory incapable of reaching another's regardless of who
  performs it.
- `lib.rs: bootstrap` — the only place a `Runtime`'s `state_dir` is derived,
  always through `project_state_dir`.

Regression evidence:
- `deleting_one_projects_state_leaves_a_sibling_projects_state_intact` — two
  real projects, each with a recorded memory; asserts the premise (distinct,
  non-nested state directories, both present); deletes alpha's entire
  `state_dir` with `std::fs::remove_dir_all` (the deletion under test);
  confirms alpha's directory is gone and alpha's own workspace (its Git
  checkout, a different tree entirely) is untouched; confirms beta's
  directory, database, and recorded memory all survive intact.

Failure/isolation evidence:
- Mutation: in `paths.rs::RuntimePaths::project_state_dir`, changed
  `self.projects_dir().join(project_id)` to ignore `project_id` and always
  join a fixed `"shared"` name. The test failed — but earlier than its own
  assertions: `Fixture::new` for project beta panicked during `bootstrap`
  with `DatabaseError::ProjectMismatch`, because the collapsed shared
  directory was already bound to alpha's identifier by the time beta tried to
  open it. This is the isolation guard failing exactly where it must: the two
  projects could no longer even coexist, which is a stronger and earlier
  failure than "beta's data got deleted too" would have been. Restored with
  `cp` + `touch`; test green again.

Platform/external evidence: none yet. Not `#[cfg(unix)]` — plain
`remove_dir_all`, identical everywhere.

Missing evidence:
- No product feature exists for "project-state deletion" yet; this entry
  proves the invariant such a feature would depend on, not the feature.
  Report this gap to the orchestrator if a deletion command is ever added —
  it will want its own regression test through whatever caller performs it.
- CI run on all three platforms.

### Phase 46 — Add automated tests proving cmux session metadata cannot bypass project-scope validation.

State: NOT ATTEMPTED — no test written, deliberately.

There is no cmux session-metadata path anywhere in `crates/glasshouse`.
Phase 17 (cmux integration) is recorded at 0/10 in `docs/process/handoff.md`.
A test claiming to prove a nonexistent surface is bound to the project would
prove nothing and would read as coverage forever (packet's own words). Left
open. If this surface is added, it needs project-isolation tests of its own
at that point — this phase does not anticipate its shape.

### Phase 46 — Add automated tests proving MCP operations remain bound to the active project.

State: NOT ATTEMPTED — no test written, deliberately.

There is no MCP surface anywhere in `crates/glasshouse`. Phase 43 is recorded
at 0/10 in `docs/process/handoff.md`. Same reasoning as the cmux entry above.
Left open.
