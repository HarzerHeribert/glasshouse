# Capability evidence — phase 21C

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 21C — validity conditions, and a decision that can stop being binding

Contract: Given a durable memory recorded under assumptions that later stop
holding, when a caller judges one of the map's six invalidation conditions to
have occurred, Glasshouse records that judgement with a name and a timestamp
and stops returning the memory as a current instruction — while preserving the
memory itself, its rationale, and its reachability as history.

State: **COMPLETE** for all eleven lines (883–893).

**Migration 10**, five `ALTER TABLE ADD COLUMN`s on `memories`, no FTS change
and no table rebuild: `validity_conditions`, `invalidation_conditions`,
`review_reason` (a six-value `CHECK` enum), `review_marked_at`,
`last_validated_at`. `SUPPORTED_SCHEMA_VERSION` is 10. The shape was fixed by
the orchestrator in the packet rather than designed by the worker, because
migrations are Red tier; the worker applied it unchanged.

Production evidence:
- `memory/store.rs: NewMemory::with_validity_conditions`,
  `MemoryRecord::validity_conditions` / `invalidation_conditions` — lines 883
  and 884, round-tripped through a real database.
- `memory/store.rs: MemoryStore::mark_for_review(id, ReviewReason)` — lines
  885–890. **The six `ReviewReason` values are exactly the map's six lines, one
  each**, and `every_review_reason_the_type_supports_is_one_the_schema_accepts`
  reads them back out of migration 10's own `CHECK` so the enum and the schema
  cannot drift apart.
- `memory/store.rs: MemoryStore::binding` and `SearchScope::Current` both
  filter on `status = Active` — lines 891 and 893. An `Invalidated` memory
  fails that unconditionally.
- **Line 892 is closed structurally: there is no `delete` or `remove` method on
  `MemoryStore` at all.** Re-grepped by the integrator; none exists. An
  invalidated memory cannot be silently deleted because it cannot be deleted.

Regression evidence:
- `tests/memory_validity.rs::validity_and_invalidation_conditions_round_trip_and_absence_stays_none`
- `tests/memory_validity.rs::every_review_reason_can_mark_a_memory_for_review_with_a_stated_cause`
  — all six in one loop, asserting `review_marked_at` is set and `created_at`
  never moves.
- `tests/memory_validity.rs::an_invalidated_memory_is_excluded_from_binding_and_current_search_but_never_deleted`
  — invalidates a `Constraint`/`Invariant`-authority memory, asserts it leaves
  both `binding()` and current search, and that `get()` and
  `count(Invalidated)` still find it.
- `tests/memory_validity.rs::needs_review_and_conflicted_memories_stay_out_of_current_search_but_are_findable_as_history`
  — line 893's two halves in one assertion, and the status is **not** laundered
  back to `Active` on the way out.
- `database.rs::a_version_nine_database_migrates_forward_keeping_its_memories`
  — a real bootstrap rolled back to version 9, a memory recorded, reopened
  through an ordinary `bootstrap()`, body intact. The same test asserts the
  pre-migration row reads `None` for all three new nullable columns, **not `0`
  or `''`** — NULL means "the build that wrote this row recorded nothing", which
  is the rule every migration in this schema follows.
- `database.rs::migration_ten_rejects_an_unrecognized_review_reason` — the
  `CHECK` refuses `'not-a-real-reason'` at the SQL layer.

Failure/isolation evidence:
- Mutation: dropping `review_reason` from `mark_for_review`'s `UPDATE` killed
  `every_review_reason_can_mark_a_memory_for_review_with_a_stated_cause`
  (`left: None, right: Some(ProjectState)`).

Platform/external evidence:
- macOS local suite green; batch 35's full gate and `--windows-vm` cover this
  tree.

**What this phase deliberately does not build.** No automatic detector for "the
project phase changed" or "a production incident occurred". Those conditions
live outside this module — current project phase, incident reports, live
architecture — and recognising them is the same class of judgement that lines
828/829 already established as not the storage layer's business. What Phase 21C
needs *from this module* is the mechanism, and it mirrors exactly how migration
4 shipped all seven `MemoryAuthority` classes before any classifier used them,
leaving Phase 21A to add the classifier on top.

**Fallout the worker correctly refused to fix itself.** Bumping the schema
version broke four pre-existing tests in `session/store.rs`, a file outside its
partition. It reproduced all four, wrote exact patches, and stopped — the
behaviour the `FORBIDDEN FILES` stop condition exists to produce. The
integrator applied them: the credential-surface census gains migration 10's
five columns (with the same "the control is on the producer side" reasoning
migration 6's provenance columns carry), two version pins move 9 → 10, and the
version-7 rollback in `mod phase_10` drops the five `memories` columns before
it drops migration 8's `sessions` columns. **This is the fourth schema-version
bump to break these same tests (4, 8, 9, and now 10.)**
