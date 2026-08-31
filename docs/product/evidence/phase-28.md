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
