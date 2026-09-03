# Capability evidence — phase 60

Phase 60 — Parallel-session file coordination (map lines 2374–2416), promoted from Maybe A, B, C, F and H by the user's steering decision of 2026-09-03 (`design-decisions.md`, *Steering decisions of record*). One vertical capability, not five platforms. Implementation order is the user's: **A+F → B → C → H**.

Entries are bounded by the *Decompression* ruling: the contract, the tests by name, the mutation on each decision, the limits, and the worker's report by path.

**Gating note, and it is now history rather than a standing workaround.** `blast-radius.sh --targeted` used to resolve a change in `src/commands/*.rs` to `--lib commands::<name>`, which selects **zero** tests, because `commands/` lives in the *binary* crate — practice §68's shape inside the gate itself, created for all 21 `commands/*.rs` files by `GH-DECOMP-MAIN` and for `api/`'s seven before that. `GH-CLAIMS-AF` hit it and ran `--bin glasshouse` by hand. **Fixed in `5062267`**: `binary_crate_pkg()` asks whether a file's top-level module is declared in `main.rs` and not `lib.rs`, and routes it to `--bin <pkg>`. Every gate in this phase ran `--bin glasshouse` one way or the other.

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

Limits: adjacency is the only signal — no warning, ranking or conflict verdict. Those are lines 2402–2410.

---

## Packet errors recorded against `GH-CLAIMS-AF`

Written down once, here, rather than retold in five places.

1. **The packet quoted none of its seven box lines** while claiming they were quoted "above"; its body named only 2394–2398. `new-packet.sh --lines` exists precisely to avoid this.
2. **The migration-ripple grep was scoped to `crates/glasshouse/tests` and should have been `crates/`.** Eight of the nine `SUPPORTED_SCHEMA_VERSION` pins that had to move live under `src/` (`database/tests.rs` ×4, `session/store/tests.rs` ×4), and 24 rollback fixtures across eight files additionally needed `DROP TABLE IF EXISTS file_claims`. The worker found and fixed all of it; the packet was simply wrong about where the ripple lands.
3. **`--lib session::store` and `--lib database` are name-substring filters, not module selectors** (practice §68's family). Both selected non-zero tests so their results stand, but `--lib database`'s 55 include `session::store::tests::*` and are not "the database module's tests".
4. **The `commands/` binary-crate blind spot in `blast-radius.sh --targeted`** — see the gating note at the top of this file. Found by this package; a filter that matches nothing is indistinguishable from a pass, which is why the successor matters.

---

## Edit intent (Maybe B) — lines 2402–2405

All four lines closed by one package: `GH-EDIT-INTENT` (Opus, **Red** — it changes what every Claude Code launch installs), worktree `.worktrees/edit-intent`, packet `.agent-runtime/packet-edit-intent.md`, report **`.agent-runtime/report-edit-intent.md`**.

**Contract as delivered:** *Before a Claude Code session performs a file-modifying operation, Glasshouse records an edit intent for that path, compares it with other live sessions' claims, tells the session and the model when another one holds the same file — and **always** answers `allow`, on every path including every error path.*

**Proven end to end against the real installed Claude Code 2.1.259**, not only in tests. Two live sessions in a fixture project; `aaaa…` held a claim on `src/main.rs`; a real `claude -p` session registered for `bbbb…` was told to overwrite it. The model's own answer: *"a Glasshouse file-coordination hook warned that src/main.rs was already claimed by another session … noting this is advisory rather than a lock and that the write went ahead anyway."* The file was written; both sessions held a claim afterwards. That is steering decision 4's MVP behaviour, demonstrated rather than argued.

**Two harness facts captured empirically before any code was written**, with a throwaway `PreToolUse` hook teeing stdin to a file: `tool_input` arrives carrying an absolute `file_path` (the captured document is pinned verbatim as a fixture), and **a regex matcher is honoured** — with `"matcher": "Edit|Write"` a session told to `Read` one file and `Write` another produced two `Write` events and no `Read` event. So the hook uses `Edit|Write|MultiEdit|NotebookEdit` rather than the firewall's `"*"`, which is the difference between a per-tool-call cost and a per-*edit* cost. **Measured: 5.3 ms median per writing tool call**, 3.7 ms of it bare process spawn; read-only tools never invoke it.

Gates on the merged tree, re-run by the orchestrator: fmt, clippy `-D warnings`, rustdoc `-D warnings` clean; `--test edit_intent` 13/13; `--test file_claims` 20/20; `--test context_firewall` 13/13; `--test firewall_bridge` 17/17; `--lib firewall` 108; `--bin glasshouse`; `blast-radius.sh --targeted` exit 0; size ratchet clean.

### Record an edit_intent event before a session performs a file-modifying operation when the harness exposes enough information. (line 2402)

Contract: Given a Claude Code session about to run a writing tool, when the `PreToolUse` hook fires, Glasshouse records an edit intent for each named path inside the project, while never delaying or altering the operation.

State: **COMPLETE** — ruled 2026-09-03 by the orchestrator. Production: `firewall/adapter.rs :: parse_pre_tool_use_event`, `:: edit_intent_paths`; `commands/hook.rs :: edit_intent_hook`, `:: edit_intent_conflict`; `commands/launch.rs :: install_edit_intent_hook`; `harness/claude_code.rs :: edit_intent_hook_entry`, `:: edit_intent_command_line`, `:: merge_edit_intent_hook`. Regression: `edit_intent::a_write_records_an_edit_intent_for_the_path_it_names`, `::a_read_records_nothing_and_still_allows`, `::a_path_outside_the_project_is_not_recorded_and_still_allows`, `::writing_the_same_file_twice_renews_one_intent`, `firewall::adapter::tests::the_real_captured_pre_tool_use_write_event_parses_with_its_file_path`, `harness::claude_code::tests::pre_tool_use_is_never_a_reported_lifecycle_event`.

Limits: a file changed by `Bash` (`sed`, a heredoc) records no intent — inferring a path from a shell command line is the guessing line 2404 forbids. No `edit_intent` row in `lifecycle_events`; the durable record is the `file_claims` row (see the ruling below).

### Compare new edit intent with active file claims from other sessions. (line 2403)

Contract: Given another live session already holding a claim on the same path, when a session expresses edit intent, Glasshouse names that session and the path to both the user and the model, while letting the operation proceed.

State: **COMPLETE** — ruled 2026-09-03 by the orchestrator. Production: `commands/hook.rs :: edit_intent_conflict`, `firewall/adapter.rs :: pre_tool_use_response`, `session/store/claims.rs :: SessionStore::active_claims` (existing, unmodified). Regression: `edit_intent::a_second_session_editing_the_same_file_is_told_who_holds_it`, `::a_session_does_not_conflict_with_its_own_claim`, `::a_stopped_sessions_claim_is_not_reported_as_a_conflict`, `firewall::adapter::tests::a_conflict_is_reported_and_still_allowed`.

Limits: direct same-path overlap only — adjacent-interface or semantic overlap is line 2410 and is not attempted. The comparison is exact string equality on the canonical path spelling; case is not folded, matching `memory_files`.

### Keep intent detection best-effort when a harness does not expose structured pre-tool hooks, and say so rather than inferring intent from terminal output. (line 2404)

Contract: Given a harness with no verified structured pre-tool hook, when a user asks what Glasshouse can do, `glasshouse doctor` states that file coordination is unavailable for that harness, while substituting no terminal-output inference for it.

State: **COMPLETE** — ruled 2026-09-03 by the orchestrator. Production: `harness/mod.rs :: structured_pre_tool_hook`, `integrations/mod.rs :: write_adapter_report`, `commands/launch.rs :: install_edit_intent_hook` (the non-Claude-Code early return). Regression: `edit_intent::doctor_says_which_harnesses_have_no_pre_tool_hook`, `harness::tests::a_named_pre_tool_hook_is_one_the_adapter_itself_declares`, `harness::tests::claude_code_is_the_only_harness_with_a_verified_pre_tool_bridge`, `integrations::tests::the_doctor_report_shows_each_adapters_declarations`.

Limits: Claude Code is the only harness with a bridge today; the other six are **declared unavailable rather than investigated**. That is the line's own instruction — say so rather than infer — not a gap.

### Preserve the user's ability to bypass coordination when Glasshouse cannot determine intent confidently. (line 2405)

Contract: Given a user who wants coordination off, when `[edit_intent] mode = "off"` is set in user or project configuration, Glasshouse installs no `PreToolUse` hook at all — and on every other path, including every conflict and every internal failure, it answers `allow`.

State: **COMPLETE** — ruled 2026-09-03 by the orchestrator. Production: `config/firewall.rs :: EditIntentMode`, `:: EditIntentConfig`; `config/effective.rs :: EffectiveConfig::edit_intent_mode`; `firewall/adapter.rs :: pre_tool_use_response`; `commands/launch.rs :: install_edit_intent_hook`. Regression: `edit_intent::a_conflict_never_denies_the_operation`, `::every_failure_path_allows_and_stays_silent`, `::the_hook_always_exits_zero`, `::the_configured_bypass_is_off_and_the_default_is_on`, `firewall::adapter::tests::no_input_to_the_builder_ever_produces_deny_or_ask`, `config::firewall::tests::the_edit_intent_table_round_trips_and_refuses_a_spelling_it_does_not_know`.

Mutation `invert-decision`, on the one invariant the whole phase rests on: `"permissionDecision": PRE_TOOL_USE_DECISION,` → `if conflict.is_some() { "deny" } else { … }` — **KILLED** by `a_conflict_never_denies_the_operation`; panicked at `tests/edit_intent.rs:347:5`, *"soft coordination: a conflict is told, never enforced"*. Run twice, before and after `cargo fmt`; killed both times, restore byte-identical. **`mode = "off"` installs no hook at all rather than an inert one**, which is why "off" costs nothing.

Limits: `mode = "off"` is proven by unit test over the resolver plus the early return; no test launches a real harness with the hook absent.

## Three rulings, the orchestrator's at integration

1. **The default is `on`, and it stands.** The worker chose it and flagged it as the one decision with launch-wide blast radius. Upheld, and the distinction it drew is the right one: `FirewallMode` defaults to `Off` because the firewall can change *what the model is shown*; this hook cannot — it never denies, never delays, never alters a tool's input or result, and the mutation above is what proves that rather than asserts it. Line 2405's own word is **bypass**, which presupposes something otherwise happening, and steering decision 4's MVP behaviour is unobservable if it ships off. The cost is bounded and measured: 5.3 ms per *writing* tool call, and read-only tools never spawn it. Reversing it is one constant.

2. **No migration, and therefore no `edit_intent` row in `lifecycle_events`.** The packet said the conflict should be surfaced *"in the event log"*; the worker refused, with a better reason than the packet's instruction. `lifecycle_events` carries `CHECK ((kind = 'file_touched') = (path IS NOT NULL))`, so an `edit_intent` kind carrying a path is a table-rebuild migration plus roughly six more files — and the packet's own REQUIRED BEHAVIOR permitted skipping it if the claims table already carries what an intent needs. It does: a `file_claims` row **is** the record "session S is about to change path P at time T". A parallel event row would be a second source of truth for one fact, which CLAUDE.md rule 8 forbids and `LifecycleEvent::SessionStarted`'s own doc refuses. The conflict is not a new fact either — it is a *query* over claims. **Named successor if a durable event kind is ever wanted**: migration 28 plus the `edit_intent` kind, with line 2409 (conflict prediction) as the first consumer that would actually read it.

3. **Line 2392's tick was ahead of its producer, and this package is the producer.** The worker flagged it rather than quietly benefiting from it: when `cfbb432` closed 2392 (*"claim a file when a session begins an edit-oriented operation"*), nothing in the shipped binary called `claim_file` except the `glasshouse claim` verb a person types. The evidence entry above recorded that honestly as a limit — *"the claim is declared by a CLI verb; edit-intent detection is Maybe B"* — but the line's own words describe an automatic trigger, and there was none. **There is now**, in the same phase and within hours, so the tick is left standing rather than reverted and re-applied; this entry is where that is written down. The general lesson is the one `cluster-b.py` exists for: a box whose only caller is a CLI verb a human runs is a box worth re-reading.
