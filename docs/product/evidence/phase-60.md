# Capability evidence — phase 60

Phase 60 — Parallel-session file coordination (map lines 2374–2408), promoted from Maybe A, B, C, F and H by the user's steering decision of 2026-09-03 (`design-decisions.md`, *Steering decisions of record*). One vertical capability, not five platforms. Implementation order is the user's: **A+F → B → C → H**.

Entries are bounded by the *Decompression* ruling: the contract, the tests by name, the mutation on each decision, the limits, and the worker's report by path.

**Gating note for every package in this phase.** `blast-radius.sh --targeted` resolves a change in `src/commands/*.rs` to `--lib commands::<name>`, which selects **zero** tests, because `commands/` lives in the *binary* crate — practice §68's shape inside the gate itself, created for all 21 `commands/*.rs` files by `GH-DECOMP-MAIN`. Until a successor fixes the mapping, **any package touching `commands/**` runs `cargo test -p glasshouse --bin glasshouse` explicitly**; both this phase's gates did.

---

## Claims, turn-scoped (Maybe A + F) — lines 2392–2398

All seven lines closed by one package: `GH-CLAIMS-AF` (Opus, Amber — a persisted field, a new table, and an ordering decision), worktree `.worktrees/claims-af`, packet `.agent-runtime/packet-claims-af.md`, report **`.agent-runtime/report-claims-af.md`**. New production: `src/session/store/claims.rs` (357 lines), migration 27 in `src/database/migrations/v14_on.rs`, `glasshouse claim` in `src/cli.rs`, the `TurnEnded` arm in `src/commands/hook.rs`, `claims_block` in `src/commands/sessions.rs`. New regression: `crates/glasshouse/tests/file_claims.rs`, 20 tests.

Gates on the merged tree, re-run by the orchestrator before the commit: fmt, `cargo check --all-targets`, clippy `-D warnings`, rustdoc `-D warnings` — all clean; `--bin glasshouse` **85/85**; `--test file_claims` **20/20**; `--lib session::store` 70/70; `--lib database` 55/55; `--test session_model` 21/21; the seven ripple-touched integration targets green; `blast-radius.sh --targeted` **exit 0** (22 traced targets, 58 full-trace targets skipped by design); size ratchet `ok (0 file(s) over 2500, none grown)`.

### Claim a file when a session begins an edit-oriented operation on that file. (line 2392)

Contract: Given a live Glasshouse session and a path inside the project, when the session declares an edit-oriented operation on it, Glasshouse records one soft claim naming that session and that path, while preserving that nothing locks, blocks, or fails another session's write.

State: **COMPLETE** — ruled 2026-09-03 by the orchestrator. Production: `claims.rs :: SessionStore::claim_file`, `commands/sessions.rs :: claim_command`, `cli.rs :: Command::Claim`, migration 27. Regression: `file_claims::claiming_a_file_records_it_against_the_session_that_asked`, `::a_claimed_path_is_stored_repo_relative_however_it_was_typed`, `::two_sessions_may_claim_one_file_and_neither_is_refused`, `::a_path_outside_this_project_cannot_be_claimed`.

Mutation, **run by the orchestrator at integration and the reason this line is ticked**: the worker's own mutation (dropping the sixth `VALUES` placeholder) killed by `InvalidParameterCount` rather than by behaviour, and the worker said so in its report rather than glossing it. The behavioural form it named but was told to stop before running — `claims.rs:144 now + STALE_CLAIM_AFTER,` → `now,`, parameter count preserved — was run here: **KILLED**, eleven tests FAILED, first panic `crates/glasshouse/tests/file_claims.rs:445:5` in `a_claim_inside_the_stale_timeout_stands` (`assertion left == right failed`). A claim written with a zero-length life is invisible the instant it exists, and the suite says so.

Limits: the claim is declared by a CLI verb — edit-intent detection is line 2402. No code path in this build reads a claim before deciding anything; that is line 2403 onward.

### Release a session's file claim automatically when the relevant turn completes. (line 2393)

Contract: Given a session holding claims, when the harness reports that its turn ended, Glasshouse releases every claim that session holds, while preserving that a failed turn releases exactly as a completed one does and that the hook never fails the user's turn.

State: **COMPLETE** — ruled 2026-09-03 by the orchestrator. Production: `commands/hook.rs :: report_hook_with` (the `TurnEnded` arm), `claims.rs :: SessionStore::release_claims_of`. Regression: `file_claims::a_turn_ending_releases_every_claim_that_session_held`, `::a_turn_ending_releases_only_the_session_whose_turn_it_was`, `::an_event_that_is_not_a_turn_ending_releases_nothing`.

Mutation `skip-state-update`: `hook.rs` `match store.release_claims_of(&id) {` → `match Ok::<usize, …>(0) {` — **KILLED** by `a_turn_ending_releases_every_claim_that_session_held`; panicked at `file_claims.rs:280:9`, the claims still present after `Stop`.

Limits: the release is best-effort — a failed `DELETE` is one debug line, and `STALE_CLAIM_AFTER` is what bounds a claim this line misses.

### Release abandoned file claims when the owning session exits, fails, or exceeds a safe stale-claim timeout. (line 2394)

Contract: Given a claim whose session has exited or failed, or whose stale timeout has passed, when anything reads this project's claims, Glasshouse reports none of them and removes the rows on the next claim written, while preserving that a live session inside its timeout keeps its claim.

State: **COMPLETE** — ruled 2026-09-03 by the orchestrator. Production: `claims.rs :: SessionStore::active_claims` (the expiry and liveness filters), `:: release_abandoned_locked`, `:: STALE_CLAIM_AFTER`. Regression: `file_claims::a_claim_whose_session_is_no_longer_live_is_released`, `::a_claim_older_than_the_stale_timeout_is_released`, `::a_claim_inside_the_stale_timeout_stands`, `session::store::claims::tests::live_lifecycles_are_exactly_the_live_ones`.

Mutation `drop-liveness-filter`: `AND session_id IN ({LIVE_SESSIONS})` → `AND (session_id IN ({LIVE_SESSIONS}) OR 1 = 1)` — **KILLED** by `a_claim_whose_session_is_no_longer_live_is_released`; panicked at `file_claims.rs:362:9`, a stopped session's claim still reported.

Limits: the timeout is 2 hours **by judgement, not by a measured turn-length distribution**. A row survives, unreported, in a project where no further claim is ever written.

### Allow a session to renew a claim when its next turn continues work on the same file. (line 2395)

Contract: Given a session that already holds a claim on a path, when it claims that path again, Glasshouse extends the existing claim, while preserving when the work started.

State: **COMPLETE** — ruled 2026-09-03 by the orchestrator. Production: migration 27's `PRIMARY KEY (session_id, path)`, `claims.rs :: SessionStore::claim_file` (`ON CONFLICT DO UPDATE`). Regression: `file_claims::claiming_a_file_the_session_already_holds_renews_it`.

Mutation `renew-does-not-extend`: `renewed_at = excluded.renewed_at, expires_at = excluded.expires_at` → `renewed_at = renewed_at, expires_at = expires_at` — **KILLED**; panicked at `file_claims.rs:480:5`, the second claim left both columns where the first put them.

Limits: renewal is driven by a claim being taken again; nothing renews on a timer.

### Associate every file claim with the owning Glasshouse session ID rather than only a process ID. (line 2396)

Contract: Given any file claim, when it is stored, Glasshouse records the owning Glasshouse session identifier and no process identity, while preserving that a session this project does not have cannot hold one.

State: **COMPLETE** — ruled 2026-09-03 by the orchestrator. Production: migration 27's `session_id TEXT NOT NULL`, `claims.rs :: SessionStore::claim_file` (the `get` + `is_live` check). Regression: `file_claims::a_claim_is_owned_by_a_glasshouse_session_and_no_process_identifier`, `::a_session_this_project_does_not_have_cannot_claim`, `::a_finished_session_cannot_take_a_claim`, `session::store::tests::the_project_database_schema_has_nowhere_to_put_a_credential`.

Mutation `drop-owner-check`: `if !record.lifecycle.is_live() {` → `if false {` — **KILLED** by `a_finished_session_cannot_take_a_claim`; panicked at `file_claims.rs:779:5`, a `Stopped` session handed a claim instead of a refusal.

Limits: the column pin proves no process column exists **today**, not that a future migration cannot add one.

### Keep file claims project-scoped so a claim can never affect another project. (line 2397)

Contract: Given a claim recorded in one project, when any query, listing or command runs in another project, Glasshouse can neither return nor name it, while preserving that a row carrying a foreign project identifier is refused before it is written.

State: **COMPLETE** — ruled 2026-09-03 by the orchestrator. Production: migration 27's two project triggers, `claims.rs :: SessionStore::active_claims` (`project_id = ?1`), `commands/sessions.rs :: claimed_path` via `context_firewall::project_relative_path` — the existing definition of "inside this project, spelled this way", widened to `pub(crate)` rather than duplicated, because the phase's CROSS-PLATFORM requirement forbids a second canonicalisation. Regression: `file_claims::a_claim_in_one_project_is_invisible_to_another`, `::the_database_refuses_a_claim_belonging_to_another_project`, `::a_read_never_reports_a_row_belonging_to_another_project`, `::a_path_outside_this_project_cannot_be_claimed`.

Mutation `drop-scope-predicate`: `WHERE project_id = ?1 AND expires_at > ?2` → `WHERE ?1 = ?1 AND expires_at > ?2` — **KILLED** by `a_read_never_reports_a_row_belonging_to_another_project`; panicked at `file_claims.rs:598:5`, the smuggled foreign-project row reported.

Limits: the killing test has to **drop the triggers** to reach the predicate, because the per-project database file already hides it. Defence in depth, proven one layer at a time.

### Surface active file claims in the session overview when they are relevant to parallel work. (line 2398)

Contract: Given active claims in this project, when a person runs `glasshouse sessions`, Glasshouse prints who claims which path and since when, ordered so claims on one file are adjacent, while preserving that nothing at all is printed when nothing is claimed.

State: **COMPLETE** — ruled 2026-09-03 by the orchestrator. Production: `commands/sessions.rs :: claims_block`, `:: session_report`, `claims.rs :: SessionStore::active_claims` (`ORDER BY path`). Regression: `file_claims::the_session_overview_shows_active_claims`, `::the_session_overview_says_nothing_when_nothing_is_claimed`.

Mutation `skip-surface`: `if let Some(claims) = claims_block(&sessions.store())? {` → `if let Some(claims) = None::<String> {` — **KILLED** by `the_session_overview_shows_active_claims`; panicked at `file_claims.rs:666:5`, the overview printed no `CLAIMED BY` block.

Limits: adjacency is the only signal — no warning, ranking or conflict verdict. Those are lines 2402–2408.

---

## Packet errors recorded against `GH-CLAIMS-AF`

Written down once, here, rather than retold in five places.

1. **The packet quoted none of its seven box lines** while claiming they were quoted "above"; its body named only 2394–2398. `new-packet.sh --lines` exists precisely to avoid this.
2. **The migration-ripple grep was scoped to `crates/glasshouse/tests` and should have been `crates/`.** Eight of the nine `SUPPORTED_SCHEMA_VERSION` pins that had to move live under `src/` (`database/tests.rs` ×4, `session/store/tests.rs` ×4), and 24 rollback fixtures across eight files additionally needed `DROP TABLE IF EXISTS file_claims`. The worker found and fixed all of it; the packet was simply wrong about where the ripple lands.
3. **`--lib session::store` and `--lib database` are name-substring filters, not module selectors** (practice §68's family). Both selected non-zero tests so their results stand, but `--lib database`'s 55 include `session::store::tests::*` and are not "the database module's tests".
4. **The `commands/` binary-crate blind spot in `blast-radius.sh --targeted`** — see the gating note at the top of this file. Found by this package; a filter that matches nothing is indistinguishable from a pass, which is why the successor matters.
