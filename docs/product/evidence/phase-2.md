# Capability evidence — phase 2

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 2 — Persist Glasshouse session metadata independently from the native harness session files

Contract: Given a harness session started by Glasshouse, when the session is
recorded, Glasshouse stores its own session metadata in the project database
and can read it back in a later process, while never parsing, depending on, or
being invalidated by whatever session files the harness keeps for itself.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionStore::create` — the only
  writer of the `sessions` table; reached from `main.rs: launch_session`, which
  records a session before the harness process exists.
- `crates/glasshouse/src/main.rs: session_report` — `glasshouse sessions` reads
  the records back in a separate process.
- `crates/glasshouse/src/database.rs: MIGRATIONS[1]` — the `sessions` table.
  `native_session_id` is nullable, so a record is complete before any harness
  has produced an identifier and stays valid after the harness's own history is
  deleted.

Regression evidence:
- `launching_a_harness_records_a_session_that_a_later_command_reads_back`
  (tests/pty_smoke.rs) — the shipped binary, a real pseudo-terminal, two real
  harness runs, then a second process reading the records. Executed on macOS
  locally and on Linux, macOS and Windows in CI.
- `a_session_is_recorded_and_survives_a_reopen_with_no_harness_involved` —
  the record is complete with no harness identifier and survives a reopen.

Failure/isolation evidence:
- Mutation: making `create` skip its `INSERT` fails the pty_smoke test.
- Mutation: dropping the post-exit `note_lifecycle` call fails it.
- `a_session_write_is_refused_when_the_project_binding_is_missing` — writes are
  refused rather than orphaned when the database has no project bound.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- none.

### Phase 2 — Persist a mapping between Glasshouse session IDs and native harness session IDs when native IDs are available

Contract: Given a harness that reveals its own session identifier, when
Glasshouse records it, the identifier is stored against exactly one Glasshouse
session and can be read back, while a Glasshouse session identifier never
changes and no native session can be claimed by two Glasshouse sessions.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionStore::set_native_session_id`
  — attaches the identifier after creation, which is when harnesses reveal it.
- `crates/glasshouse/src/database.rs: MIGRATIONS[1]` — the partial unique index
  `sessions_native_id` over `(harness, native_session_id)` is what makes the
  column a mapping rather than an annotation.

Regression evidence:
- `a_native_session_identifier_can_be_attached_later_and_read_back`
- `one_native_session_cannot_map_to_two_glasshouse_sessions`
- `two_harnesses_may_use_the_same_native_identifier`
- `many_sessions_may_have_no_native_identifier_at_once`

Failure/isolation evidence:
- Mutation: dropping the unique index lets one native session be claimed twice.
- Mutation: narrowing the index to `(native_session_id)` alone makes two
  harnesses collide.
- Mutation: replacing `NULL` with an empty-string sentinel makes every
  unidentified session collide — the reason the column stays nullable.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- No harness adapter captures a native identifier yet (Phase 7/8), so in
  production the column is currently always `NULL`. The mapping mechanism is
  complete and proven; what feeds it is a later phase.

### Phase 2 — Persist the harness type, creation time, last activity time, role, lifecycle state, and project identifier for every session

Contract: Given any recorded session, when it is read back, every one of those
six fields is present and accurate, while creation time never changes and last
activity time advances on every state change and every recorded interaction.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionRecord`, `SessionStore::create`,
  `SessionStore::set_lifecycle`, `SessionStore::touch`.
- `crates/glasshouse/src/main.rs: launch_session` — moves a real session through
  `Starting` -> `Running` -> `Stopped`/`Failed`.

Regression evidence:
- `every_required_field_is_persisted` — asserted by value against an injected
  clock, not by round-trip.
- `every_role_and_lifecycle_value_round_trips`
- `activity_time_advances_while_creation_time_stays_put`
- `sessions_are_listed_most_recently_active_first`

Failure/isolation evidence:
- Mutation: stopping `set_lifecycle` from touching `last_activity_at` fails the
  activity test.
- Mutation: recording every ended session as `Stopped` fails the pty_smoke
  test, because a failed harness stops being distinguishable.
- `the_schema_rejects_enum_values_it_does_not_define` — `CHECK` constraints
  reject a role, lifecycle, or presentation the schema does not define.
- `an_unrecognized_stored_enum_value_is_reported_rather_than_guessed` — a value
  written by a future build surfaces as a typed error naming the column, not a
  panic or a silent default.
- `touching_an_unknown_session_reports_it_missing_rather_than_inventing_one`

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- none.

### Phase 2 — Persist the process presentation mode for every session

Contract: Given a session presented embedded, headless, or externally, when it
is recorded and read back, its presentation mode is preserved exactly, while an
undefined presentation value cannot be stored at all.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionPresentation`, stored by
  `SessionStore::create` and shown by `main.rs: session_report`.
- Vocabulary is the map's own (Phase 10/11: "embedded, headless, or externally
  presented"), not invented here.

Regression evidence:
- `every_presentation_mode_is_persisted` — all three modes.
- `launching_a_harness_records_a_session_that_a_later_command_reads_back`
  asserts the presentation column reaches the listing.

Failure/isolation evidence:
- `the_schema_rejects_enum_values_it_does_not_define` covers `presentation`.
- `stored_values_honour_format_width_so_listings_align` — the `Display` impls
  use `Formatter::pad`, so the listing's columns cannot silently go ragged.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- Only `Embedded` occurs in production today, because `glasshouse launch` is the
  only session producer. `Headless` and `External` arrive with Phase 4's
  headless mode and Phase 17's cmux panes.

### Phase 2 — Persist enough metadata to distinguish active, resumable, closed, and failed sessions

Contract: Given the stored metadata alone, when Glasshouse classifies a
session, it separates active, resumable, closed, and failed without consulting
any harness, while never reporting a session resumable when nothing was
recorded to resume it to.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/session/store.rs: SessionRecord::disposition` — derived
  from lifecycle plus the presence of a native identifier, deliberately not a
  second stored column that could disagree with the first.
- `crates/glasshouse/src/main.rs: session_report` — the STATE column of
  `glasshouse sessions`.

Regression evidence:
- `the_four_dispositions_are_distinguishable_from_stored_metadata` — all seven
  lifecycle states, with and without a native identifier.
- `launching_a_harness_records_a_session_that_a_later_command_reads_back` — a
  clean exit reads as `closed` and a failing one as `failed`, end to end.

Failure/isolation evidence:
- Mutation: treating a stopped session with no native identifier as resumable
  fails the disposition test.
- `a_stopped_session_with_no_native_identifier_is_not_resumable` and
  `a_live_session_is_not_resumable` — the refusals `open_for_resume` enforces.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- none.

### Phase 2 — Never store provider credentials directly in the project memory database

Contract: Given the project database at any schema version this build produces,
when its full schema is enumerated, there is no column and no key/value slot in
which a provider credential could be stored, while any future schema change
that adds one fails the build's tests until it is deliberately reviewed.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/database.rs: MIGRATIONS` — the complete schema is three
  tables: `project_metadata`, `schema_migrations`, `sessions`. None has a
  credential column.
- `crates/glasshouse/src/session/store.rs: NewSession` — the only way to create
  a session, and it has no field a secret could be passed through.

Regression evidence:
- `the_project_database_schema_has_nowhere_to_put_a_credential` — asserts the
  exact `(table, column)` list. Deliberately an allowlist rather than a name
  pattern: `project_metadata.key` would false-positive on any name match, and a
  credential column could just as easily be called `value`. Any new column
  fails this test until someone updates the list, which is the moment to ask
  whether it can hold a secret.
- `project_metadata_holds_only_the_project_identifier` — the one key/value table
  is pinned to its single known key, closing the route by which a secret could
  be stored without a schema change.

Failure/isolation evidence:
- The test fails by construction on any schema addition; it is an exact
  equality, so it cannot pass vacuously.

Platform/external evidence:
- CI `32815286487` on `3d606e3` — green on Linux, macOS, Windows and lint,
  with the Windows job confirmed to have executed 228 lib and 22 PTY tests
  (a green tick is not proof the suite ran: when the lib target fails,
  cargo never reaches the integration tests).

Missing evidence:
- Provider credentials do not exist yet (Phase 9E). This entry proves the
  project database is not where they can land; it does not yet prove where they
  do land.
