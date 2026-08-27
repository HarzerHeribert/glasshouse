# Capability evidence — phase 10

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 10 — Unified session model, 14 of 14 (lines 641–654)

Contract: every interactive Glasshouse session is a record Glasshouse owns —
started by a real harness, belonging to exactly one project, carrying the
harness, launch profile, backend resource, model, pairing class, wire protocol
and response profile as seven separate facts rather than one agent identifier;
nameable and taggable by the person using it; and retirable without taking the
harness's own history with it.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/database.rs` migration 8 — seven appended columns on
  `sessions`: `model`, `pairing_class`, `protocol`, `response_profile`,
  `response_mechanism`, `display_name`, `purpose`. `ALTER TABLE ADD COLUMN`
  only, in migration 3's shape; no table is rebuilt and no existing row is
  touched.
- `crates/glasshouse/src/session/store.rs` — `SessionRecord` now carries
  eighteen fields of fourteen distinct types. `SessionPairingClass`,
  `SessionProtocol`, `ResponseMechanism`, `AssignedModel`, `ResponseProfile`,
  `SessionName` and `SessionPurpose` are seven types that cannot substitute
  for one another, so the collapse line 645 forbids does not compile.
- `crates/glasshouse/src/session/store.rs: SessionStore::{rename, clear_name,
  set_purpose, clear_purpose, close}` — the three user actions, each writing
  exactly one column.
- `crates/glasshouse/src/session/mod.rs: owning_harness` — line 646's guard,
  called from `SessionStore::create`, which is the only door a session record
  can come through.
- `crates/glasshouse/src/main.rs: launch_session` — **the production writer**.
  It asks `session_pairing` once and reads three separate answers off the
  result (the model, the class, the wire protocol), and records the resolved
  response profile and the mechanism that carried it beside them.
- `crates/glasshouse/src/main.rs: session_pairing` — the pairing question for
  the profile a session is starting under, asked through
  `EffectiveConfig::pairing_queries`, which is the same function
  `glasshouse pairing` prints from.
- `crates/glasshouse/src/cli.rs: Command::Sessions` and `SessionCommand`,
  `crates/glasshouse/src/main.rs` — `glasshouse sessions [show|rename|tag|
  close]`, the surface a person runs. The bare `glasshouse sessions` still
  lists, and the listing gained a `NAME` and a `PURPOSE` column so a tag is
  visible without asking each session in turn.

Line by line:
- **641** — every native harness execution Glasshouse starts is recorded before
  the process exists: `main.rs::launch_session` for the command line and
  `shell/mod.rs::start_session` for the interactive shell. There is no third
  way to start one, because `crate::launch::HarnessLaunch` is the only
  sanctioned launch path and both callers record first.
- **642** — `SessionStore::generate_id` mints 128 bits from SQLite's own
  CSPRNG; `generated_session_identifiers_are_unique_within_a_burst` covers the
  case a clock-derived identifier would fail.
- **643** — `sessions.harness`, `NOT NULL` since migration 2, and since this
  batch it can only hold the slug of an integration whose kind is `Harness`.
- **644** — `sessions.native_session_id` is a separate nullable column and a
  separate identifier space. `SessionStore::new_native_session_id` is
  explicitly *not* derived from the Glasshouse identifier, and the end-to-end
  run below shows the two side by side and different.
- **645** — seven columns, seven types, seven lines of
  `glasshouse sessions show`. Proven three ways: the record round-trips each
  value (`a_session_records_seven_facts_and_no_two_of_them_share_a_column`),
  the raw row holds seven different strings, and the binary prints seven
  different answers. M1, M2, M3 and M16 are the mutations.
- **646** — `owning_harness` refuses `cmux`, `ollama` and `llama-cpp` as
  non-harness integrations and refuses `openai`, `anthropic`, `openrouter`,
  `glasshouse-gateway` and `""` as names no integration has. A direct provider
  and the gateway are not integrations at all, so neither has a spelling this
  check could accept. Enforced in `create` rather than at a caller, so no
  future caller can forget it — M17 deletes the call and dies.
- **647** — all seven states are written by the shipped binary.
  `Starting` at `create`; `Running` and `Failed` in `launch_session` and
  `resume_session`; `Idle` and `WaitingForUser` through `glasshouse hook` →
  `session::lifecycle::observe` → `implied_state`; `Stopped` from
  `ProcessExit::session_state`; and **`Closed` from `glasshouse sessions
  close`, which is new here.** Before this batch `Closed` was representable and
  unreachable — nothing in the shipped binary wrote it.
- **648** — `last_activity_at`, written at `create` and moved by
  `set_lifecycle` and `touch`. Renaming, tagging and closing deliberately do
  **not** move it: those are things the user did to a record, not things the
  session did, and stamping them would push a finished session back to the top
  of a list ordered by when it last ran (M12).
- **649** — `sessions.presentation`, recorded before the process exists and
  then used as the single source of truth for how the session is run, so the
  stored value and the running one cannot disagree. See "Missing evidence" for
  the third value.
- **650** — `SessionStore::rename` writes `display_name` and nothing else.
  `renaming_a_session_leaves_its_native_identifier_alone` reads the identifier
  back afterwards and then resumes the session; the binary prints the
  identifier beside the new name so the promise is visible rather than
  implied. M4.
- **651** — `SessionStore::set_purpose` writes `purpose`, a separate column of
  a separate type, so tagging cannot rename and renaming cannot tag. Free text
  because the map says "such as"; bounded at 32 characters and refused rather
  than truncated.
- **652** — unchanged and re-proved: migration 2's two triggers structurally,
  `open_for_resume` at the resume boundary, and separate database files per
  project. M9 is the mutation the packet asked for — it makes the *check* pass
  (`self.project_id != self.project_id`) while the row is foreign, and
  `resuming_a_session_belonging_to_another_project_is_refused` kills it. The
  binary-level test names the session outright from a second project and every
  one of `show`, `rename`, `tag`, `close` and `resume` refuses.
- **653** — `SessionRecord::disposition` separates `Active` from `Resumable`
  from `Closed` from `Failed`, the listing prints it in the `STATE` column, and
  a stopped session with a recorded native identifier reads `resumable` in the
  shipped binary. M10.
- **654** — `glasshouse sessions close` writes one column. The harness's own
  files are not read, not moved and not deleted, the pointer to that history
  stays recorded, and the command says so on stdout. The test compares every
  byte of the harness's own history directory before and after.

Regression evidence (macOS, `cargo test -p glasshouse`: 1442 passed, 0 failed;
twelve-job local gate below):
- `crates/glasshouse/tests/session_model.rs` — seven tests, **every one
  spawning the shipped binary**, six of them starting a real session with
  `glasshouse launch --headless` against a fake installed harness. No test in
  this file writes a session record by hand, so a build whose launch path
  stopped recording the metadata cannot pass it (§35).
- `crates/glasshouse/src/session/store.rs` — fourteen new unit tests under
  `tests::phase_10`, covering the seven columns, the two labels, the
  vocabularies the schema pins, the 324 response-profile combinations and
  migration 8's forward compatibility.

Mutation evidence — eighteen run, eighteen killed, each named test `ok` before,
`FAILED` mutated, `ok` restored. Every build was forced by touching the mutated
file, so no verdict came from a cached test binary (§16):

| id | mutation | test | result |
|---|---|---|---|
| M1 | the launch path writes the model from the backend resource | `a_launched_session_records_seven_facts_and_the_binary_shows_them_apart` | FAILED |
| M2 | `create` binds the launch profile into the `pairing_class` column | `a_session_records_seven_facts_and_no_two_of_them_share_a_column` | FAILED |
| M3 | the launch path records no pairing class | `a_launched_session_records_seven_facts_and_the_binary_shows_them_apart` | FAILED |
| M4 | a rename also writes the native session identifier | `renaming_a_session_through_the_binary_leaves_its_native_identifier_alone` | FAILED |
| M5 | closing clears the native session identifier | `closing_a_session_keeps_the_pointer_to_its_native_history` | FAILED |
| M6 | closing deletes the row instead of retiring it | `closing_a_record_leaves_the_harnesss_own_history_on_disk` | FAILED |
| M7 | closing removes a directory on disk | `closing_a_record_leaves_the_harnesss_own_history_on_disk` | FAILED |
| M8 | `owning_harness` accepts everything but a harness | `only_a_real_harness_may_own_a_session` | FAILED |
| M9 | the resume project check compares the active project with itself | `resuming_a_session_belonging_to_another_project_is_refused` | FAILED |
| M10 | a stopped session with an identifier is classified `closed` | `a_stopped_but_resumable_session_stays_visible_and_separate` | FAILED |
| M11 | migration 8 back-fills a default response profile | `upgrading_a_version_7_database_preserves_every_existing_session` | FAILED |
| M12 | tagging stamps `last_activity_at` | `naming_or_tagging_a_session_is_not_session_activity` | FAILED |
| M13 | a partial stored profile is completed from defaults | `every_response_profile_round_trips_through_one_column` | FAILED |
| M14 | `harness-default` is stored as a named model | `a_harness_default_is_not_the_same_stored_fact_as_nothing_recorded` | FAILED |
| M15 | a live session may be closed | `a_live_session_cannot_be_closed` | FAILED |
| M16 | the launch path records no response profile | `a_launched_session_records_seven_facts_and_the_binary_shows_them_apart` | FAILED |
| M17 | `create` never calls the harness-ownership guard | `only_a_real_harness_may_own_a_session` | FAILED |
| M18 | `session_pairing` ignores `pairing_queries` and always builds its own | `a_launched_session_records_seven_facts_and_the_binary_shows_them_apart` | FAILED |

M3, M16, M17 and M18 are the §35 set: each mutates *the call* on the production
path rather than the callee, and each dies against a test that enters through
the shipped binary rather than through a fixture. M18 is the one that was not
obvious in advance — the configured pairing query and the fallback agree about
the model and the class for a Native profile and disagree about the protocol,
because `pairing_queries` fills a missing protocol in from the adapter's sole
declaration and the fallback cannot.

M7 deserves its own note, because it is the answer to §41. Nothing in the
shipped binary deletes a harness's files, so
`closing_a_record_leaves_the_harnesss_own_history_on_disk` asserts a property
that today's code cannot violate — the test and any mutation of existing logic
would agree, and neither would be testing anything. M7 is therefore a
deliberately synthetic deletion inserted into `close_session`, and it proves
the byte comparison is live rather than decorative. The test is a regression
guard against a future build that starts deleting, and it is named as one.

Migration evidence:
- `upgrading_a_version_7_database_preserves_every_existing_session` creates a
  session under the version-8 schema, winds the database back to exactly what
  version 7 left behind, reopens it through an ordinary bootstrap so migration
  8 really runs, and then compares **all eleven** pre-existing fields against
  what they were. All seven new columns read NULL rather than a default: a
  session recorded before migration 8 ran under a response profile Glasshouse
  never wrote down, which is a different fact from having run the default one.
  M11 is the mutation for exactly that temptation.
- The upgraded database is then exercised, not merely inspected: the migrated
  row is renamed, and its native session identifier is still there.
- Three existing forward-compatibility tests were extended to roll migration
  8's columns back with the row that records them —
  `upgrading_a_version_2_database_preserves_every_existing_session`,
  `a_version_three_database_gains_the_memory_table_with_its_sessions_intact`
  and `a_version_five_database_migrates_forward_keeping_its_memories`. The
  runner resumes from `MAX(version)`, so a rollback that leaves the columns
  behind re-applies migration 8 against a table that already has them; that is
  the trap the version-2 test's own comment records, one migration later.

Platform/external evidence:
- End-to-end against the shipped binary in a real pty (`script -q`, with
  `test -t 1` confirmed inside the session): `glasshouse launch claude-code
  --profile probe` against a fake installed harness, then
  `glasshouse sessions show` printed `model opus`, `pairing class
  vendor-native`, `protocol anthropic-messages`, `response profile
  verbosity=terse audience=executive narration=silent evidence=minimal
  format=bullets` and `response mechanism native` on seven separate lines, with
  the Glasshouse identifier `2a0b24273f98…` and the native identifier
  `d23ab938-6684-…` visibly different. `sessions rename` printed
  *"Its native session id is unchanged: d23ab938-…"*; `sessions tag` and the
  listing showed the name and purpose; `sessions close` printed *"The
  claude-code session `d23ab938-…` was not touched: Glasshouse does not own
  that history and did not delete it"*, and the harness's own transcript file
  had the same md5 before and after. A 35-character purpose was refused with
  *"a session purpose is at most 32 characters; that one is 35"* and exit 1.

Missing evidence:
- **`SessionPresentation::External` has no producer.** It is representable,
  stored, read back and printed, and nothing in the shipped binary creates one,
  because Glasshouse cannot yet present a session anywhere but its own viewport
  or nowhere. Phase 17 line 761 — *"allow a session to be created directly in
  external-cmux presentation mode"* — is the box that gives it one, and it is
  Phase 17's, not this phase's. Line 649 asks Glasshouse to *track* which of
  the three a session is, and for every session it can create it does.
- **A gateway-backed session records the pairing its profile was configured
  with, not the provider the gateway assigned it at start.** Phase 9H already
  records the runtime assignment in `lifecycle_events.gateway_provider` and
  `gateway_model`, so nothing is lost; what is missing is a second read of the
  pairing after `apply_gateway` has bound one. That needs a public function in
  `config::pairing` taking a resolved `LaunchProfile` and a bound provider,
  which was outside this package's ownership.
- **`shell::start_session` records no launch profile, backend resource, model,
  pairing class, protocol or response profile.** It did not record the first
  two before this batch either — the shell starts a session under no named
  profile — so this is not a regression, but a session started with `n` in the
  TUI shows `-` for all seven where one started from the command line shows
  seven answers.
