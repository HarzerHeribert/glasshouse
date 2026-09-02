# Capability evidence — phase 28

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules). Phase 28 — file-aware memory lookup — had no entry before 2026-08-31; the refusal register's *Phase 28 scoped* section records why (the `memory_files` producer, `record_observed_files`, writes only `observed` associations, and `MemoryStore::for_path` is its only reader). The two lines below are the ones that producer and reader can honestly close; the phase's other lines stay open there.

### Lines 1140 and 1143 — the briefing reads memories observed beside a named file, and the memory-search door answers by path

Package `GH-WIRE-FILE-MEMORY`, 2026-08-31, Sonnet at medium-high (Amber). One reader, `MemoryStore::for_path`, reached from two production callers: `memory::inject::briefing` (the launch briefing gains a labelled, all-or-nothing *file-observed* section built for the files the task names) and the Unix-socket door's `query_memory` (a `QueryMemory` request carrying `path` answers from `for_path` with the association kind on every row; without `path` the verb is byte-for-byte what it was).

### Allow Glasshouse to retrieve memories associated with a file before a new session begins work on that file. (line 1140)

Contract: Given a launch's task text names files and this project's memory_files table holds an observed association for at least one of them, when Glasshouse selects the memory to brief the session with, Glasshouse appends a labelled section built from MemoryStore::for_path in which every row says the association is observed, while preserving the briefing's existing byte ceiling by dropping the whole section (never a partial one) when it would not fit, and leaving the briefing exactly as it was when the task names no file or matches nothing observed.

State: COMPLETE — ruled 2026-08-31 by the orchestrator from the report's five artifacts (`validate_round.py` passed at dispatch; a well-formed facts block; three KILLED mutations with killing test and output quoted; real `test result:` lines; `blast-radius.sh` green over 41 targets in the worktree) and the merged-tree re-run of `cargo fmt --check`, `cargo clippy -D warnings` and four targets (context_injection 15, file_memory_lookup 6, mcp_server 5, memory_query_api 9 — all passed).

Production evidence:
- `src/memory/inject.rs` — `file_observed_memories`
- `src/memory/inject.rs` — `briefing (calls file_observed_memories, passes result to render)`
- `src/memory/inject.rs` — `render (all-or-nothing section budgeting)`
- `src/memory/inject.rs` — `render_entry (association param, assoc= token)`
- `src/memory/inject.rs` — `file_observed_heading`
- `src/routing/session.rs` — `paths_named_in (reused, not modified)`
- `src/memory/search.rs` — `MemoryStore::for_path (reused, not modified)`

Regression evidence:
- `file_memory_lookup::a_task_naming_no_observed_file_adds_no_section`
- `file_memory_lookup::briefing_adds_a_section_for_memories_observed_beside_a_named_file`
- `file_memory_lookup::the_file_observed_section_excludes_memories_already_carried`
- `file_memory_lookup::the_file_observed_section_is_dropped_whole_rather_than_truncated`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/inject.rs: `let file_observed = file_observed_memories(store, task, already_injected, &selected)?;` -> `let file_observed: Vec<MemoryRecord> = Vec::new();` | `drop-the-for-path-call` | **killed** | `file_memory_lookup::briefing_adds_a_section_for_memories_observed_beside_a_named_file` |
| src/memory/inject.rs: `format!(" assoc={}", association.as_str())` -> `format!(" assoc=referenced")` | `label-referenced-not-observed` | **killed** | `file_memory_lookup::briefing_adds_a_section_for_memories_observed_beside_a_named_file` |
| src/memory/inject.rs: `if used + section_bytes <= MAX_INJECTED_BYTES {` -> `if true {` | `exceed-the-byte-ceiling` | **killed** | `file_memory_lookup::the_file_observed_section_is_dropped_whole_rather_than_truncated (via the module's own debug_assert!)` |

> drop-the-for-path-call observed: panicked at crates/glasshouse/tests/file_memory_lookup.rs:134:10 — the file-observed section's own heading must appear (it did not, because file_observed was forced empty)

> label-referenced-not-observed observed: panicked at crates/glasshouse/tests/file_memory_lookup.rs:157:5 — the row must say observed (it said assoc=referenced instead)

> exceed-the-byte-ceiling observed: panicked at crates/glasshouse/src/memory/inject.rs:420:5 — the debug_assert! `an injected block must never exceed MAX_INJECTED_BYTES bytes` fired, because the heavy always-included section pushed the block over the ceiling

Recorded scope limits — stated by the worker, not discovered later:
- file_observed_memories queries each named path independently and does not re-rank across multiple named paths beyond concatenate-then-truncate to MAX_FILE_OBSERVED_MEMORIES (3); not exercised by any test here, which all name one path.
- The section's heading does not name which file(s) the memories were observed beside, only that they were — kept out to save budget; an agent gets the file back only if a memory's own body mentions it.

---

### Allow an agent to request the rationale behind a file-related constraint through memory search. (line 1143)

Contract: Given the memory-search door's QueryMemory request carries an optional path, when path is present, Glasshouse answers through MemoryStore::for_path instead of the text search, returning the memory body, rationale, and the association kind on every row, while preserving query_memory's existing behavior byte-for-byte when path is absent.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. Same artifacts as 1140, plus the orchestrator's own re-run of `ignore-path-on-the-door` on the merged tree with `scripts/mutate.sh`: **KILLED** by `file_memory_lookup::path_present_answers_from_for_path_with_the_association_kind_on_each_row` (assertion at `tests/file_memory_lookup.rs:464` — the door fell through to the text search and answered *No current memories match "zzz-no-such-token-zzz"*), restored byte-identical.

Production evidence:
- `src/api/protocol.rs` — `Request::QueryMemory (path field, #[serde(default)])`
- `src/api/unix.rs` — `query_memory (branches to query_memory_for_path when path is Some)`
- `src/api/unix.rs` — `query_memory_for_path`
- `src/api/unix.rs` — `file_observed_memory_json`
- `src/api/mcp.rs` — `glasshouse_search_memory tool build (path: None — scope_overflow, not a new argument)`

Regression evidence:
- `file_memory_lookup::path_present_answers_from_for_path_with_the_association_kind_on_each_row`
- `file_memory_lookup::path_absent_leaves_the_verb_unchanged`
- `memory_query_api.rs's existing 9 tests, unmodified, still pass — the byte-for-byte claim for path-absent`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/api/unix.rs: `query_memory(runtime, &query, history, limit, path.as_deref())` -> `query_memory(runtime, &query, history, limit, None)` | `ignore-path-on-the-door` | **killed** | `file_memory_lookup::path_present_answers_from_for_path_with_the_association_kind_on_each_row` |

> ignore-path-on-the-door observed: assertion `left == right` failed: response fell through to the text search and answered {"invariants_and_constraints":[],"other":[],"report":"No current memories match \"zzz-no-such-token-zzz\"...\n"} instead of the path lookup

Recorded scope limits — stated by the worker, not discovered later:
- association is FileAssociation::Observed.as_str() on every row unconditionally — correct because record_observed_files is this build's only writer and can produce no other value (per for_path's own doc comment and the refusal-register's Phase 28 section), not because this door reads a per-row column that could vary.
- The CLI flag (`glasshouse memory search --path`) is not built — main.rs/cli.rs are held by a live worker this round and the packet scoped it out explicitly.

---
- `src/api/mcp.rs` gained `path: None` in the MCP search-memory tool's request literal only to keep compiling after `Request::QueryMemory` grew the field; the MCP tool does NOT expose a path argument. Disclosed by the worker as scope overflow; recorded here as a limit, not a claim.
- The CLI flag (`glasshouse memory search --path`) is not built: `main.rs`/`cli.rs` were held by live workers this round and the packet scoped it out.

Packet error the worker reported (correct): `docs/product/evidence/phase-28.md` — this file — did not exist when the packet named it; the worker used the refusal register's *Phase 28 scoped* section instead, which carries the same producer and reader facts.

## Lines 1139, 1141 and 1142 — file paths a memory references, an edit-intent preference, and advisory freshness (Phase 28 complete)

Package `GH-FILE-AWARE-MEMORY`, 2026-09-03, Opus 5 high (Red: migration 26, the context-firewall hook path). Worker report: `.agent-runtime/report-file-aware-memory.md` — the record; this entry is the bounded summary the *Decompression* ruling asks for. Design: `design-decisions.md`, *File paths a memory explicitly references*. Three packet errors and four scope-overflow files (`shell/state.rs`'s exhaustive match, `memory/export_local.rs`'s parameter, two test files' mechanical arguments), all accepted.

### Track file paths explicitly referenced by durable memories when extraction can identify them reliably. (line 1139)

Contract: Given a Claude Code session whose context-firewall PostToolUse hook sees an Edit/Write/MultiEdit/NotebookEdit call, when the session's memory is later extracted, Glasshouse records the edited file as a file_touched lifecycle event, shows it to the extraction model as `edited <path>`, keeps only the paths the model names that are byte-equal to paths the session demonstrably edited, and stores them as `referenced` file associations, while preserving that observed associations stay `observed`, that no path outside the project root is ever stored, and that the hook's response is unaffected by recording.

State: **COMPLETE** — ruled 2026-09-03 by the orchestrator on the report's five artifacts (`.agent-runtime/report-file-aware-memory.md`; `validate_round.py` passed at dispatch; 6/6 KILLED with output; `blast-radius.sh --targeted` exit 0 over 35 targets), the merged-tree gate (`/private/tmp/claude-501/-Users-eneas-projects-glasshouse/1ee4f96b-92d2-4ca9-a6ef-3ac50e9ba8d4/scratchpad/gate-fam.log`: the migrate-in-place test first, then clippy, `--lib database` ×2, six lib modules, twenty targets, the bin, no `version, 25` pin left) and one mutation re-run on the merged tree (`guard-off`, KILLED by `a_path_the_session_never_edited_is_never_stored_however_confidently_the_model_names_it`). *Reliably* is the byte-equality guard against the session's own `file_touched` set, not the model's word; the producer is Phase 57's `PostToolUse` hook, which the register's refusal predates (design-decisions, *File paths a memory explicitly references*). Migration 26 touches `lifecycle_events` only.

Production evidence:
- `src/database.rs` — `MIGRATIONS[25] (migration 26 -- lifecycle_events rebuilt for kind `file_touched` and column `path`)`
- `src/database.rs` — `SUPPORTED_SCHEMA_VERSION = 26`
- `src/database.rs` — `LIFECYCLE_EVENT_KINDS (twelfth value)`
- `src/database.rs` — `MEMORY_FILE_PROVENANCE (observed, referenced)`
- `src/events/mod.rs` — `LifecycleEvent::FileTouched`
- `src/events/mod.rs` — `LifecycleEvent::kind / implied_state`
- `src/events/log.rs` — `payload_columns / ALL_COLUMNS / read_row (path)`
- `src/firewall/eligibility.rs` — `is_writing_tool`
- `src/harness/claude_code.rs` — `context_firewall_command_line (--session)`
- `src/cli.rs` — `ContextFirewallCommand::Hook::session`
- `src/main.rs` — `record_file_touches`
- `src/main.rs` — `project_relative_path`
- `src/main.rs` — `context_firewall_hook (calls record_file_touches before firewall::process)`
- `src/main.rs` — `install_context_firewall_hook (passes &record.id)`
- `src/memory/extract/lifecycle.rs` — `describe (FileTouched -> `edited <path>`)`
- `src/memory/extract/lifecycle.rs` — `chunk_for_session (touched set over the surviving window)`
- `src/memory/extract/chunk.rs` — `SessionChunk::with_touched_paths / touched_paths`
- `src/memory/extract/schema.rs` — `ExtractedMemory::paths, RawMemory::paths, RESPONSE_SCHEMA, PROMPT_CONTRACT rule 14`
- `src/memory/extract/mod.rs` — `Extractor::record_referenced_paths (the byte-equality guard)`
- `src/memory/extract/mod.rs` — `ExtractionOutcome::paths_dropped`
- `src/memory/extract/diagnostics.rs` — `ExtractionDiagnostics::paths_dropped`
- `src/memory/store.rs` — `FileAssociation::Referenced / strongest / record_referenced_files / record_file_associations`
- `src/memory/search.rs` — `MemoryStore::for_path (provenance_words), strongest_association, RetrievalResult::association`
- `src/memory/inject.rs` — `render_entry (assoc=), file_observed_memories`
- `src/api/unix.rs` — `file_observed_memory_json (per-row association)`
- `src/shell/state.rs` — `describe_event (FileTouched arm)`

Regression evidence:
- `file_aware_memory::an_edit_through_the_hook_becomes_a_referenced_association_on_the_memory_that_names_it`
- `file_aware_memory::a_path_the_session_never_edited_is_never_stored_however_confidently_the_model_names_it`
- `file_aware_memory::a_path_outside_the_project_root_is_never_stored_and_backslashes_fold`
- `file_aware_memory::one_tool_call_naming_a_file_twice_records_it_once`
- `file_aware_memory::the_hook_response_is_identical_across_every_recording_outcome`
- `file_aware_memory::a_memory_carrying_both_rows_for_one_file_reports_referenced`
- `file_aware_memory::a_schema_25_database_migrates_in_place_with_every_seq_preserved`
- `glasshouse(bin)::tests::record_file_touches_never_propagates_a_failure`
- `database::tests::every_file_association_the_type_supports_is_one_the_schema_records`
- `events::log::tests::each_kind_fills_exactly_its_own_columns`
- `events::log::tests::the_kind_names_are_the_ones_the_schema_allows`
- `firewall::eligibility::tests::every_hard_blocked_editing_tool_is_a_writing_tool_whatever_its_case`
- `firewall::eligibility::tests::a_read_shaped_or_security_shaped_tool_is_not_a_writing_tool`
- `harness::claude_code::tests::the_command_line_carries_the_glasshouse_session_in_every_mode`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/main.rs: `    if !glasshouse::firewall::eligibility::is_writing_tool(&event.tool_name) {` -> `    if true {` | `record-nothing` | **killed** | `file_aware_memory::the_hook_response_is_identical_across_every_recording_outcome` |
| src/memory/extract/mod.rs: `            if touched.iter().any(|seen| seen == path) {` -> `            if true {` | `guard-off` | **killed** | `file_aware_memory::a_path_the_session_never_edited_is_never_stored_however_confidently_the_model_names_it` |
| src/memory/search.rs: `        self.associations.get(id).copied()` -> `        self.associations.get(id).map(|_| FileAssociation::Observed)` | `label-observed` | **killed** | `file_aware_memory::a_memory_carrying_both_rows_for_one_file_reports_referenced` |

> record-nothing observed: assertion `left == right` failed: the recorded case must actually have recorded, or the comparison proves nothing -- and three more tests failed with it, including an_edit_through_the_hook_becomes_a_referenced_association_on_the_memory_that_names_it at line 281 (no file_touched event, so no referenced row)

> guard-off observed: assertion `left == right` failed: ExtractionOutcome { trigger: Manual, model: "fake/canned", session_id: "s-guard-1", recorded: [MemoryId("8cf42999...")], ..., paths_dropped: 0, ... } -- the drop count went to zero and a `referenced` row appeared for a file the session only READ

> label-observed observed: assertion `left == right` failed at tests/file_aware_memory.rs:566 -- Some(Observed) where Some(Referenced) was required; the end-to-end and CLI tests failed with it

Recorded scope limits — stated by the worker, not discovered later:
- The `tracing::warn!` on a failed append is proven only in process (the binary's bootstrap refuses to start on an unwritable database, so a hook invocation can never be launched into that state); the hook's response invariance is proven over the four outcomes a real invocation can reach.
- `--activity` extraction can produce no referenced association at all: SessionChunk::build leaves the touched set empty and only chunk_for_session fills it.
- paths_dropped is a run-level sum, not per memory.
- Rule 14 is enforced only against false positives. A model that returns an empty list for a memory that IS about a file loses the association silently, and nothing signals it.
- tests/file_memory_lookup.rs's `!contains("referenced")` assertion still passes but its stated premise -- that the association is unbuildable -- is what this package retired.

---

### Prefer constraints, decisions, and failed approaches when retrieving memory for an intended code edit. (line 1141)

Contract: Given a file-aware retrieval for an intended code edit, when Glasshouse orders the memories associated with that file, Glasshouse puts constraints, decisions and failed attempts ahead of features, findings and todos inside each ladder rung, while preserving Phase 21E's rung as the primary key and leaving the Lookup order byte-for-byte unchanged.

State: **COMPLETE** — same artifacts and gate as 1139. `RetrievalIntent::CodeEdit` orders `Constraint`/`Decision`/`FailedAttempt` ahead of the other kinds *between* the ladder rung and the weight, so Phase 21E's rung stays primary and a heavy weight cannot outvote the preference; `Lookup` is byte-for-byte the previous order (`tests/memory_path_lookup.rs` unchanged in its assertions). The briefing's file section — built for the files the task names, the intended edit — asks with `CodeEdit`; the door stays `Lookup`; `memory search --path --for-edit` exposes both. The generic evidence the register row 478 warned against is not what ticks this: the file-scoped door is.

Production evidence:
- `src/memory/search.rs` — `RetrievalIntent`
- `src/memory/search.rs` — `RetrievalIntent::prefers`
- `src/memory/search.rs` — `rank (the preference key between rung and weight)`
- `src/memory/search.rs` — `MemoryStore::for_path (intent parameter)`
- `src/memory/inject.rs` — `file_observed_memories (asks with CodeEdit)`
- `src/api/unix.rs` — `query_memory_for_path (asks with Lookup)`
- `src/main.rs` — `memory_path_report (--for-edit -> CodeEdit)`
- `src/cli.rs` — `MemoryCommand::Search::for_edit`

Regression evidence:
- `file_aware_memory::code_edit_puts_a_constraint_ahead_of_a_finding_that_outweighs_it_and_lookup_does_not`
- `file_aware_memory::the_kind_preference_never_reaches_across_a_ladder_rung`
- `file_aware_memory::for_edit_without_a_path_is_an_error_that_names_the_flag_it_needs`
- `memory_path_lookup (all 7, unchanged, pinning the Lookup order)`
- `file_memory_lookup (all 8, unchanged)`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/search.rs: `            Self::CodeEdit => matches!(` -> `            Self::CodeEdit => false && matches!(` | `kind-preference-dropped` | **killed** | `file_aware_memory::code_edit_puts_a_constraint_ahead_of_a_finding_that_outweighs_it_and_lookup_does_not` |

> kind-preference-dropped observed: assertion `left == right` failed: for an intended edit, the constraint comes first -- and the Lookup half of the same test kept passing, which is what says the preference and not something else moved the row

Recorded scope limits — stated by the worker, not discovered later:
- The preference is proven on one pair in one rung. It is a fixed kind class rather than a number, so there is no threshold to get wrong, but no test enumerates all six kinds against each other.
- The two memories are separated by a phase penalty rather than by a tie, deliberately: a tie would have been decided by two random ids and was flaky.
- The socket door stays Lookup by design (1143's caller asked what a file is associated with); nothing tests that a CodeEdit door would be wrong, only that this one is Lookup.

---

### Keep file-aware retrieval advisory and never treat stale memory as stronger evidence than the current source code. (line 1142)

Contract: Given a file-aware retrieval, when Glasshouse presents a memory associated with a file, Glasshouse labels the result advisory and states a freshness derived only from commit order -- stale when the memory's source commit is a strict ancestor of the file's last change, current when the last change is that commit or an ancestor of it, unknown otherwise -- while preserving that the label never withholds, reorders or rescores a memory and that no source code is read or compared.

State: **COMPLETE** — same artifacts and gate. Every file-aware result is advisory by label (the briefing heading, the door's `advisory: true`, the CLI) and carries a commit-order freshness — `stale` when `source_commit` is a strict ancestor of the file's last-change commit, `current`, or `unknown` — computed by `checkpoint::git`'s two new questions with `GIT_DIR`/`GIT_WORK_TREE` scrubbed from the child. It never withholds, reorders or rescores (`stale-withholds` KILLED). No source is read and no conflict is judged, which is the line the register's refusals of 828/829/862/932 protect; the label is per memory, not per claim.

Production evidence:
- `src/checkpoint/git.rs` — `last_change_commit`
- `src/checkpoint/git.rs` — `is_ancestor`
- `src/checkpoint/git.rs` — `git_output (env scrub, no shell, stdin null)`
- `src/checkpoint/git.rs` — `Freshness / Freshness::compare / as_str`
- `src/memory/inject.rs` — `FileAwareMemory`
- `src/memory/inject.rs` — `file_observed_memories (one git log per path)`
- `src/memory/inject.rs` — `file_observed_heading (the advisory sentence)`
- `src/memory/inject.rs` — `render_entry (freshness= token)`
- `src/memory/inject.rs` — `briefing / briefing_traced / select_briefing / select_briefing_traced (project_root)`
- `src/api/unix.rs` — `query_memory_for_path (advisory: true, freshness per row)`
- `src/main.rs` — `memory_path_report (the advisory line and per-row tokens)`
- `src/main.rs` — `brief_launch_session / estimated_project_memory_tokens / memory_search_explain (pass the project root)`

Regression evidence:
- `file_aware_memory::freshness_is_commit_order_and_unknown_is_not_current`
- `file_aware_memory::no_repository_answers_unknown_rather_than_current`
- `file_aware_memory::a_stale_memory_is_shown_in_its_rank_and_labelled_rather_than_withheld`
- `file_aware_memory::a_briefing_with_no_project_root_labels_every_row_unknown_and_withholds_nothing`
- `file_aware_memory::memory_search_by_path_prints_the_association_the_freshness_and_the_advisory_line`
- `file_aware_memory::a_path_with_no_associations_says_so_without_claiming_the_project_is_empty`
- `file_aware_memory::a_search_with_no_path_is_unchanged`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/checkpoint/git.rs: the inner `                Some(true) => Self::Stale,` -> `                Some(true) => Self::Current,` | `stale-never` | **killed** | `file_aware_memory::freshness_is_commit_order_and_unknown_is_not_current` |
| src/memory/inject.rs: `        if used + section_bytes <= MAX_INJECTED_BYTES {` -> `        if used + section_bytes <= MAX_INJECTED_BYTES && !file_observed.iter().any(|r| r.freshness == Freshness::Stale) {` | `stale-withholds` | **killed** | `file_aware_memory::a_stale_memory_is_shown_in_its_rank_and_labelled_rather_than_withheld` |

> stale-never observed: assertion `left == right` failed at tests/file_aware_memory.rs:751 -- Current where Stale was required on a real two-commit repository; a_stale_memory_is_shown_in_its_rank_and_labelled_rather_than_withheld and the CLI test failed with it

> stale-withholds observed: panicked at tests/file_aware_memory.rs:858:6 -- the whole file-aware section vanished, so `expect("a task naming an associated path must inject something")` fired

Recorded scope limits — stated by the worker, not discovered later:
- Freshness is about the FILE changing, never about whether the memory's claim is still true. Nothing here reads a line of source -- which is the register's refusal at 828/829/862/932 kept, not worked around.
- Freshness::compare costs up to TWO merge-base calls, not the one the packet specified: one is not enough to separate `current by ancestry` from `divergent branches`, and reporting the common current case as unknown would have been worse. Zero calls when the commits are equal; one for the ordinary Current; two only to separate Stale from Unknown. Still one `git log` per path.
- checkpoint/git.rs's module-level `No subprocess` doctrine now has a documented exception. All three of its stated reasons are answered in the header (not the checkpoint path; None when git is absent; GIT_DIR/GIT_WORK_TREE/GIT_COMMON_DIR/GIT_INDEX_FILE removed from every child) rather than waived -- but this is a real widening of that module's contract.
- The briefing caps at MAX_FILE_OBSERVED_MEMORIES (3) per briefing and MAX_OBSERVED_PATHS (8) paths, unchanged; freshness does not alter either.
- The section heading had to get SHORTER to fit MAX_INJECTED_BYTES (900) once every row grew a `freshness=` token. A further token on that line will not fit.

---
