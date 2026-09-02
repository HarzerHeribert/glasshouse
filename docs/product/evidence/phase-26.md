# Phase 26 — Memory query for agents, 6 of 6 closed

Capability map lines 1111–1116. Package `GH-MEMORY-QUERY-API`, worktree
`.worktrees/memory-query-api`; full report in
`.agent-runtime/report-memory-query-api.md`. Integrated 2026-08-29 by the
primary orchestrator, together with `GH-ROUTING-CAPABILITY` in one
`integrate.sh` run.

## What kind of package this was

**A door package.** Three of the four `src/**` files the packet named —
`memory/snapshot.rs`, `memory/search.rs`, `memory/store.rs` — were **not
touched**, and that is the finding rather than an omission. Every producer was
already complete and already project-scoped. Every one of these six lines was
open at the **door**, not in the store.

## Two live defects found that no box asked for

Both were in shipped code and neither is a box line. They are recorded here
because they are the strongest argument for the package having been worth
running:

1. **`query_memory` had no server-side ceiling.** `default_memory_limit()` was
   20 and `limit` passed straight through to `search_grouped`; a caller
   passing `u32::MAX` got `u32::MAX`. Its neighbour `Events` has had
   `MAX_EVENTS_LIMIT` since Phase 42. Three ceilings now exist, all `min`
   rather than rejection.
2. **The machine door leaked the database's absolute path.** Every
   `database::DatabaseError` variant names the database file's absolute path,
   and `ProjectMemory::open` failing reached `Response::err(err)` unfiltered.
   `memory_error_message` now down-casts — a `MemoryStoreError` passes
   through, anything else becomes a class description — with the *unsafe* case
   as the default, so a new error type joining the chain stays suppressed.
   Regression-tested by inducing the failure deterministically and asserting
   the message contains no `/` at all.

## The orchestrator's ruling on 1111's naming

The worker closed 1111 **on the existing `query_memory` verb and added no
second verb**, and flagged the wire spelling as a judgment call rather than
deciding it quietly. I accept the ruling. The basis: every op on this door is
snake_case `<verb>_<noun>`, this codebase's own doc comments already refer to
"Phase 26's `memory.get`" as a *concept* while spelling the wire op
differently, and **nothing in the tree advertises op names to agents** — no
manifest, no tool list, no MCP surface — so there is no caller to break and no
document promising `memory.search`. Had a second verb been added beside
`query_memory`, this entry would record a defect instead of a closure.

## Why 1114's evidence is the strongest in this batch

The worker **verified the `unix.rs:427` trigger claim against the schema
rather than trusting the comment**, as the packet demanded, and found it
accurate: `memories_reject_foreign_project_insert`/`_update` use `IS NOT`
rather than `<>`, so a missing binding row aborts instead of evaluating to
NULL. That is the *write* boundary; the read boundary is `MemoryStore::get`'s
own check, which returns `ForeignProject` — **an error, never `None`**.

The isolation test **carries its own non-vacuity control** (§80): `plant_foreign_memory`
drops only the project trigger, so the FTS5 sync trigger still fires, and the
test asserts the planted row *is* in this project's full-text index under the
very word the door is about to be asked for, and that this project's own
matching memory *does* come back. Without both halves, "the door returned
nothing" would have proven nothing.

---

### Expose a project-scoped memory.search operation to Glasshouse-aware agents. (line 1111)

Contract: Given a Glasshouse-aware agent holding this project's control socket, when it sends a query_memory request, Glasshouse returns that project's ranked memories with their authority, validity state and provenance, while preserving that no memory outside the project the door was opened for can be returned and that the response is bounded server-side.

State: **COMPLETE**

Production evidence:
- `src/api/protocol.rs` — `Request::QueryMemory`
- `src/api/unix.rs` — `query_memory`
- `src/api/unix.rs` — `MAX_MEMORY_LIMIT`
- `src/api/unix.rs` — `provenance_json`

Regression evidence:
- `memory_query_api::query_memory_carries_authority_validity_rationale_and_invalidation_conditions`
- `memory_query_api::every_retrieval_verb_carries_provenance_sufficient_to_locate_the_source`
- `memory_query_api::no_memory_verb_can_be_made_to_read_another_projects_memory`
- `memory_query_api::an_absurd_limit_still_gets_a_bounded_response_from_every_memory_verb`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/search.rs: 'AND memories.project_id = ?2 \' -> 'AND (memories.project_id = ?2 OR 1=1) \' | `drop-project-scope-predicate` | **killed** | `memory_query_api::no_memory_verb_can_be_made_to_read_another_projects_memory` |

> drop-project-scope-predicate observed: the planted foreign memory was returned in the result set; the assertion that named it fired

Recorded scope limits — stated by the worker, not discovered later:
- The wire op stays 'query_memory'; no verb spelled 'memory.search' exists. This is a ruling on the door's snake_case convention, not a proof that the map meant a conceptual name.
- Does not prove the CLI and the door agree; that is memory_search.rs's subject and it was already proven.

---

### Expose a project-scoped memory.get operation for retrieving a selected memory in full. (line 1112)

Contract: Given an agent that has a memory identifier or an unambiguous prefix of one, when it sends get_memory, Glasshouse returns that one memory whole — uncut body, full provenance, supersession and lifecycle — while preserving that a row bound to another project is refused as an error rather than reported absent.

State: **COMPLETE**

Production evidence:
- `src/api/protocol.rs` — `Request::GetMemory`
- `src/api/unix.rs` — `get_memory`
- `src/api/unix.rs` — `memory_full_json`

Regression evidence:
- `memory_query_api::get_memory_returns_one_selected_memory_in_full_over_the_socket`
- `memory_query_api::every_retrieval_verb_carries_provenance_sufficient_to_locate_the_source`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/store.rs: 'Some(record) if record.project_id != self.project_id =>' -> 'Some(record) if false =>' | `disable-read-boundary` | **killed** | `memory_query_api::no_memory_verb_can_be_made_to_read_another_projects_memory` |

> disable-read-boundary observed: status ok with the foreign project's memory body in the result — 'The alpha kestrel export must never write partial files.'

Recorded scope limits — stated by the worker, not discovered later:
- Does not prove behaviour for an identifier prefix that is ambiguous between two local memories; resolve_id's own tests cover that.

---

### Expose a project-scoped memory.current operation for retrieving a concise current project snapshot. (line 1113)

Contract: Given an agent orienting itself in a project, when it sends current_memory, Glasshouse returns the snapshot's sections — one per memory kind, active memories only, each reporting what it omitted — while preserving that the response size does not grow with how much the project has accumulated.

State: **COMPLETE**

Production evidence:
- `src/api/protocol.rs` — `Request::CurrentMemory`
- `src/api/unix.rs` — `current_memory`
- `src/api/unix.rs` — `snapshot_section_json`
- `src/api/unix.rs` — `snapshot_entry_json`

Regression evidence:
- `memory_query_api::current_memory_returns_the_snapshots_sections_not_a_flattened_dump`
- `memory_query_api::an_absurd_limit_still_gets_a_bounded_response_from_every_memory_verb`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/api/unix.rs: 'const MAX_SNAPSHOT_SECTION_LIMIT: usize = 50;' -> '= 10_000;' | `raise-section-ceiling` | **killed** | `memory_query_api::an_absurd_limit_still_gets_a_bounded_response_from_every_memory_verb` |

> raise-section-ceiling observed: budget.per_section_limit came back as 10000 instead of 50, with 125 entries in the finding section

Recorded scope limits — stated by the worker, not discovered later:
- Does not add a second production caller of snapshot() — it adds the first one inside src/api/. shell/mod.rs:1357 was already one; see packet_errors.

---

### Prevent agent memory tools from querying another project’s memory store. (line 1114)

Contract: Given a second project's memory row present in this project's database file by any route the write trigger never saw, when an agent names it through get_memory, query_memory or current_memory, Glasshouse refuses or omits it and never returns its content, while preserving that the refusal is distinguishable from both an empty result and an absent memory.

State: **COMPLETE**

Production evidence:
- `src/api/unix.rs` — `get_memory`
- `src/api/unix.rs` — `query_memory`
- `src/api/unix.rs` — `current_memory`
- `src/memory/store.rs` — `MemoryStore::get`
- `src/memory/search.rs` — `MemoryStore::search`

Regression evidence:
- `memory_query_api::no_memory_verb_can_be_made_to_read_another_projects_memory`
- `project_isolation::one_project_database_cannot_be_queried_through_another_projects_glasshouse_instance`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/store.rs: 'Some(record) if record.project_id != self.project_id =>' -> 'Some(record) if false =>' | `disable-read-boundary` | **killed** | `memory_query_api::no_memory_verb_can_be_made_to_read_another_projects_memory` |
| src/memory/search.rs: 'AND memories.project_id = ?2 \' -> 'AND (memories.project_id = ?2 OR 1=1) \' | `drop-project-scope-predicate` | **killed** | `memory_query_api::no_memory_verb_can_be_made_to_read_another_projects_memory` |

> disable-read-boundary observed: get_memory answered status ok with the foreign row's full JSON

> drop-project-scope-predicate observed: the planted id appeared in the returned id list

Recorded scope limits — stated by the worker, not discovered later:
- The refusal message names both ProjectIds, which are '<directory-basename>-<hash>'. That is the existing vocabulary project_isolation.rs asserts on, and it is the one identifying string this door emits by design.
- Proven for the memory verbs only. Session and checkpoint verbs have their own boundary (SessionApi) and their own tests; this package did not re-derive them.
- The snapshot half of the proof rests on the planted row being active and of a kind with a section; a foreign row in a non-current status would be omitted for a second reason and would prove less.

---

### Return concise results rather than dumping the complete memory database into agent context. (line 1115)

Contract: Given a caller that asks for more than the door will give, when it passes any limit to query_memory or current_memory, Glasshouse answers with the server's own ceiling rather than the number asked for, while preserving that a caller may still lower any ceiling and that nothing is dropped without being counted.

State: **COMPLETE**

Production evidence:
- `src/api/unix.rs` — `MAX_MEMORY_LIMIT`
- `src/api/unix.rs` — `MAX_SNAPSHOT_SECTION_LIMIT`
- `src/api/unix.rs` — `MAX_SNAPSHOT_BODY_CHARS`

Regression evidence:
- `memory_query_api::an_absurd_limit_still_gets_a_bounded_response_from_every_memory_verb`
- `memory_query_api::current_memory_returns_the_snapshots_sections_not_a_flattened_dump`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/api/unix.rs: 'const MAX_MEMORY_LIMIT: usize = 100;' -> '= 10_000;' | `raise-search-ceiling` | **killed** | `memory_query_api::an_absurd_limit_still_gets_a_bounded_response_from_every_memory_verb` |
| src/api/unix.rs: 'const MAX_SNAPSHOT_BODY_CHARS: usize = 2000;' -> '= 100_000;' | `raise-body-ceiling` | **killed** | `memory_query_api::an_absurd_limit_still_gets_a_bounded_response_from_every_memory_verb` |

> raise-search-ceiling observed: 125 memories returned to a caller that asked for 4294967295; the ceiling assertion fired

> raise-body-ceiling observed: a 2861-character body came back whole with body_truncated false

Recorded scope limits — stated by the worker, not discovered later:
- Bounds the count and the per-entry body length, not the total byte size of a response: 100 memories with very long bodies is still a large line of JSON, because query_memory does not truncate bodies (memory.current is the verb that does).
- The ceilings are constants, not configuration. Nothing lets an operator raise them either, which is deliberate but untested as a requirement.

---

### Include provenance with machine-retrieved memory so an agent can verify important claims against source or code. (line 1116)

Contract: Given an agent that has retrieved a memory through this door, when it needs to check the claim, Glasshouse supplies the commit, session and event slice that locate its source plus all ten Phase 21B provenance fields, while preserving that an unrecorded field is null rather than empty and that no credential or filesystem path travels with it.

State: **COMPLETE**

Production evidence:
- `src/api/unix.rs` — `provenance_json`
- `src/api/unix.rs` — `memory_result_json`
- `src/api/unix.rs` — `snapshot_entry_json`
- `src/api/unix.rs` — `memory_error_message`

Regression evidence:
- `memory_query_api::every_retrieval_verb_carries_provenance_sufficient_to_locate_the_source`
- `memory_query_api::a_memory_verb_that_cannot_open_the_database_says_so_without_naming_the_file`
- `memory_query_api::current_memory_returns_the_snapshots_sections_not_a_flattened_dump`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/api/unix.rs: 'None => "the project's memory database could not be opened".to_owned(),' -> 'None => err.to_string(),' | `leak-database-path` | **killed** | `memory_query_api::a_memory_verb_that_cannot_open_the_database_says_so_without_naming_the_file` |

> leak-database-path observed: the response message carried the absolute path of the project database; the no-slash assertion fired for all three verbs

Recorded scope limits — stated by the worker, not discovered later:
- Proves the fields are carried and correlated with what was stored. It does not prove a named commit or session actually exists — provenance is a reference, not a foreign key, by MemoryRecord's own design.
- Snapshot entries carry only source_session_id and source_commit; the other eleven fields are reachable only through get_memory.
- The no-credential property rests on the producer-side screen in memory::extract::schema::judge, which this package did not re-derive.

---

### Worker-reported packet errors and gates (transcribed at closure)

**Packet errors the worker reported — read these BEFORE its results.**
Thirteen consecutive rounds a worker corrected its packet and was right:
- FEASIBILITY says memory::snapshot::snapshot() has 'zero non-test call sites in crates/glasshouse/src' and that line 1113's door is 'the production caller snapshot() has never had'. It has had one since Phase 41: crates/glasshouse/src/shell/mod.rs:1357, in build_project_overview_memory, production code (the file's first #[cfg(test)] is at line 2538). shell/mod.rs:3598 states this explicitly. Reading (b) therefore does not apply; the packet's requested post-condition (a snapshot( call site inside src/api/ outside #[cfg(test)]) is satisfied anyway at src/api/unix.rs:1205.
- EXPECTED FILES lists src/memory/snapshot.rs, src/memory/search.rs and src/memory/store.rs. None needed editing: every one of the six lines was open at the door, not in the store. Reported because the file list implied producer work that does not exist.
- The packet asks to 'prove the boundary holds even when the caller supplies a crafted project id, an empty one, or one differing only by case or trailing space'. No request on this door has a project field (api/mod.rs: 'the door itself is the scope'), so the crafted-id case is proven as inertness — an extra project/scope key changes nothing — and the case/whitespace case is proven on the memory identifier, which is the only caller-supplied identifier that exists.

Gates the worker ran (re-run the decisive ones yourself):
- cargo build: ok
- cargo test --test memory_query_api: 9 passed (3 consecutive runs, no flake)
- cargo test --test project_isolation: 7 passed
- cargo test --test memory_provenance: 13 passed
- cargo test --test capacity_api: 3 passed
- cargo clippy --all-targets --all-features -- -D warnings: clean
- cargo fmt --all -- --check: clean
- cargo doc --no-deps: clean; --bin glasshouse --document-private-items: 8 unresolved links, all pre-existing
- scripts/check-doc-boundary.sh: clean
- scripts/blast-radius.sh: every traced target passed (20 targets incl. memory_search, session_model, pty_smoke, terminal_loss, --bin glasshouse)

