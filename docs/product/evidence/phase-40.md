# Phase 40 — Fresh-session handoff, 7 of 9 closed

Capability map lines 1638–1646. Sonnet implementer packet `GH-HANDOFF-CHECKPOINT`,
worktree `.worktrees/handoff-checkpoint`; full report in
`.agent-runtime/report-handoff-checkpoint.md`. Integrated 2026-08-29.

## The evidence class this phase rests on, and why it is the strong one

Six of these seven lines are closed on `crates/glasshouse/tests/handoff_lines.rs`,
a new integration test that runs the **real `glasshouse` binary under a real pty**
against three fake harness executables that dump their own argv to a file. Its
assertions therefore read *the bytes the shipped binary actually handed a
harness*, not a value a test constructed.

That file is also the first coverage `--from-checkpoint` has ever had:
`grep -rn 'from.checkpoint' crates/glasshouse/tests/` matched **nothing** before
this package. Every existing test in `checkpoint_portability.rs` reaches the
format through the library (`Checkpoint::capture`, `store.save`,
`bootstrap_prompt()` called directly) and never through `main.rs::launch_session`.

**The mutations below were run by the integrator, not the worker.** The worker's
packet forbade `main.rs` and `session/**` — the very files these six lines'
behaviour lives in — so it could not mutate the calls it was asked to prove and
said so rather than lowering the bar (§78: a packet must not forbid the file a
line's evidence lives in). The integrator is not bound by a worker's packet and
ran them on integration. All four killed.

| mutation | vocabulary | killed at | observed |
|---|---|---|---|
| `NewSession::embedded(selection.id().slug())` → `NewSession::embedded("antigravity")` | `skip-state-update` | `handoff_lines.rs:314` | `left: "antigravity"` / `right: "claude-code"` on pair 1643 |
| `Ok(Some(stored.checkpoint.bootstrap_prompt()))` → `Ok(Some(format!("{:?}", stored.checkpoint)))` | `accept-stale-state` | `handoff_lines.rs:326` | the `Debug` dump contains no `OBJECTIVE` section and names its source harness |
| close every other session after `store.create` | `skip-state-update`, inverted | `handoff_lines.rs:362` | `the source session's lifecycle changed — left: Closed, right: Stopped` |
| a second `store.create(...)` per launch | `skip-state-update` | `handoff_lines.rs:268` | `must record exactly one session — left: 2, right: 1` |

`main.rs` was restored from a byte-for-byte backup after each and `diff`-verified
identical before the next; the suite is green on the restored tree.

---

### Phase 40 — Allow the router or user to create a fresh session from an existing portable checkpoint (line 1638)

Contract: Given a saved portable checkpoint, when a user launches with
`--from-checkpoint`, Glasshouse records exactly one fresh session whose identity
is its own, while leaving the checkpoint and the session it came from untouched.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/main.rs` — `launch_session` resolves the checkpoint
  through `resolve_bootstrap_prompt` *before* any session record or process
  exists (a bad identifier must cost nothing), then calls
  `store.create(NewSession::embedded(...))` unconditionally.
- `crates/glasshouse/src/main.rs` — `resolve_bootstrap_prompt` treats a named
  checkpoint that does not exist as an error rather than an empty prompt,
  because a fresh session that silently lost its handoff looks exactly like one
  that worked.

Regression evidence:
- `handoff_lines.rs::a_checkpoint_bootstraps_a_fresh_session_under_a_different_harness_through_the_shipped_binary`
  — for each of three harness pairs, the session count grows by exactly one, the
  new record is found by set difference against the pre-launch records, and its
  id differs from the source's.

Mutation: a second `store.create` per launch — **killed** at
`handoff_lines.rs:268` (`left: 2, right: 1`).

---

### Phase 40 — Include the checkpoint as explicit handoff context rather than replaying the complete old conversation (line 1639)

Contract: Given a checkpoint being handed to a fresh session, when Glasshouse
builds the opening prompt, it passes a bounded plain-text handoff naming the
objective and current state, while never replaying the old transcript and never
naming the harness that produced it.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/checkpoint/mod.rs` — `Checkpoint::bootstrap_prompt`
  renders `OBJECTIVE` / `CURRENT STATE` sections from the `Handoff` struct's
  named fields. The format holds no native session state by construction.
- `crates/glasshouse/src/main.rs` — `resolve_bootstrap_prompt` returns
  `stored.checkpoint.bootstrap_prompt()` and nothing else; the source harness is
  logged at `info` and deliberately not put in the prompt.

Regression evidence:
- `handoff_lines.rs` — asserts the bytes handed to the fake harness are plain
  text (not JSON), contain `OBJECTIVE` and `CURRENT STATE`, carry the exact
  objective and state markers the test wrote, and **name no harness**.
- `checkpoint::mod::tests::the_format_holds_no_native_session_state` and
  `the_bootstrap_prompt_is_plain_text_that_names_no_harness` — pin the field list
  and prose shape at library level.

Mutation: `bootstrap_prompt()` replaced with a full `Debug` dump of the
checkpoint — **killed** at `handoff_lines.rs:326`.

---

### Phase 40 — Include current Git status and relevant diff references in the handoff when useful (line 1640)

Contract: Given a checkpoint captured inside a Git repository, when Glasshouse
records it, the handoff carries whether the working tree is dirty and how many
tracked files changed, while never claiming "clean" about a state it did not
actually check.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/checkpoint/git.rs` — `WorkingTreeStatus::detect` reads
  `.git/index` directly (no subprocess, matching the module's stated
  architecture), parses the version-2/3 fixed-entry format for each tracked
  path's size and mtime, and compares against `fs::symlink_metadata` — the same
  size+mtime "racy git" fast path Git itself uses.
- `crates/glasshouse/src/checkpoint/mod.rs` — `Checkpoint::capture` populates the
  new `working_tree` field. It already receives `project_root`, so **no signature
  changed and no caller needed editing**: all three capture sites
  (`main.rs::checkpoint_command`, `api/unix.rs::request_checkpoint`,
  `shell/mod.rs`'s task-boundary checkpoint) pick it up for free.
- `crates/glasshouse/src/checkpoint/mod.rs` — `git_dirty` / `git_changed_files`
  round-trip through `Document`/`render`/`parse`, with `git_dirty: Option<bool>`
  as the presence discriminator so a real "clean" is distinguishable from
  "unknown"; `fit()` sheds changed-file entries after `failed_approaches`.

**Recorded scope limit, not a gap discovered later.** It compares the working
tree against the **index** only — unstaged modifications and deletions of
tracked files. It does not detect untracked files and does not compare the index
against `HEAD`. Both need Git's object store, and the module's own doc refuses
to ship a checkpoint that could claim "clean" incorrectly. The line says "when
useful"; this is real, bounded, never-wrong information.

Regression evidence:
- `checkpoint_portability.rs::capturing_a_checkpoint_reads_the_working_tree_status_of_the_repository_it_is_standing_in`
- `checkpoint::git::tests::working_tree_status_reads_the_real_checkout_this_test_runs_in`
  — runs `detect` against this repository's own 251-entry index, in a linked
  worktree, which is the module's stated worst case.
- Seven unit tests in `git.rs` each isolate one behaviour by construction:
  matching index, size mismatch, deletion, gitlink skip, entry cap, unsupported
  version, absent index.

Mutation: `working_tree: WorkingTreeStatus::detect(project_root)` → `None` —
**killed** (worker-run).

**A real bug this caught, worth recording:** the first draft read
`mtime_secs`/`mtime_nanos` at the `ctime` byte offsets. The real-index test did
*not* catch it — it still returned `Some(_)`, just with comparisons that happened
not to matter for that assertion. The hand-built fixtures caught it. Neither
class alone was sufficient.

---

### Phase 40 — Include relevant project-memory records in the handoff when useful (line 1641)

State: **NOT STARTED — blocked, with a written patch and a verified breakage.**

`Handoff` has no `memory` field. Adding one is not additive: the struct is
constructed as a literal in two forbidden files, and the worker **verified the
breakage by doing it** rather than assuming — `cargo check --workspace
--all-targets` failed with `E0063: missing field` at both
`main.rs::checkpoint_command` and `api/unix.rs::request_checkpoint`.

What a package for this owes, beyond the field: `Document` mapping, `fit()`'s
per-item clamp, `shed()`'s pop order (binding memory records are constraints, so
they should shed after `decisions`, not before), a `RELEVANT MEMORY` section in
`bootstrap_prompt`, and two judgment calls the worker correctly refused to make
alone — `MemoryRecord`'s actual text accessor, and whether a failure to open
`ProjectMemory` should fail the whole checkpoint (it should not).

This is an implementation task, not a patch to apply. See
`.agent-runtime/report-handoff-checkpoint.md` for the exact diff sketch.

---

### Phase 40 — Allow a Claude session to hand off to Codex (line 1642)<br>Phase 40 — Allow a Codex session to hand off to Claude Code (line 1643)<br>Phase 40 — Allow either session type to hand off to Antigravity when supported (line 1644)

Contract: Given a checkpoint recorded under one harness, when a user launches
`--from-checkpoint` naming a different harness, Glasshouse starts the fresh
session under the harness the user asked for, while never letting the
checkpoint's own provenance decide which harness runs.

State: COMPLETE (all three)

Production evidence:
- `crates/glasshouse/src/session/select.rs` — `select` / `select_with` resolve
  the harness from the CLI `--harness` argument and `EffectiveConfig` alone.
  `grep -rn checkpoint crates/glasshouse/src/session/select.rs
  crates/glasshouse/src/harness/mod.rs` returns **nothing**: no code path reads a
  checkpoint's `harness` field when resolving a launch. The independence is
  structural, not conditional.
- `crates/glasshouse/src/harness/mod.rs` — `IntegrationId::Antigravity` is a
  registered harness with its own adapter, reached through that identical
  checkpoint-blind resolver.

Regression evidence:
- `handoff_lines.rs`, three pairs driven through the shipped binary:
  `(claude-code → codex)` for 1642, `(codex → claude-code)` for 1643, and
  `(claude-code → antigravity)` for 1644. Each asserts the checkpoint was
  recorded under the source harness and the new session record's `harness` is the
  **target** slug.

Mutation: the created session's harness hardcoded to `"antigravity"` — **killed**
at `handoff_lines.rs:314`, `left: "antigravity"` / `right: "claude-code"`. The
assertion is one shared line of code exercised by all three pairs, so proving it
live proves it for each.

---

### Phase 40 — Preserve the old session as resumable unless the user explicitly closes it (line 1645)

Contract: Given a session that has been checkpointed and handed off, when the
fresh session is launched, the original session's record is left exactly as it
was, while remaining resumable until the user closes it themselves.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/main.rs` — `checkpoint_command`'s `Save` arm reads the
  active session through `store.get` and writes nothing back.
- `crates/glasshouse/src/main.rs` — `resolve_bootstrap_prompt` opens only the
  *checkpoint* store; it never touches the session store.

This is a negative claim, so the regression evidence is a before/after
comparison rather than an assertion about a value.

Regression evidence:
- `handoff_lines.rs` — for every pair, the source session's record is read before
  the checkpoint save and again after the target launch, then compared field by
  field: `harness`, `lifecycle`, `presentation`, `native_session_id`. All
  unchanged in all three cases.

Mutation: a write that closes every other session on launch — **killed** at
`handoff_lines.rs:362`, `the source session's lifecycle changed — left: Closed,
right: Stopped`. This is the non-vacuity check a negative claim needs: it proves
the test would notice if some future code path did start closing the source.

---

### Phase 40 — Record the handoff relationship between source and destination sessions (line 1646)

State: NOT STARTED. Not claimed by this package and not investigated by it.
Nothing durably links the fresh session's record back to the checkpoint or the
session it came from; `SessionRecord` has no such field today.

---

## Platform evidence

`handoff_lines.rs` is `#[cfg(unix)]`: its three fake harnesses are shell scripts.
The semantics under test — which harness a launch resolves to, what bytes the
prompt carries, whether the source record is written — are platform-independent
and none of these contracts makes an OS-specific claim. Linux and macOS both run
this file in `scripts/ci-local.sh`. Windows runs the rest of the suite but not
this file; a Windows-native rewrite would need `.bat` argv-dump harnesses and is
recorded here as the one uncovered platform, not silently omitted.
