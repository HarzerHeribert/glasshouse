# Capability evidence — phase 17

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 17 — cmux optional integration, 10 of 10 (lines 754–763)

Package `GH-CMUX-PRESENTATION`, 2026-08-31, Fable specialist at xhigh. **Ten
lines against one mechanism: cmux is driven through its documented CLI, and a
pane is metadata on the session row, never a second identity.** The ruling it
implements is `design-decisions.md`, *"Phase 17: cmux is driven through its
documented CLI, and a pane is metadata."* Migration 20 adds
`sessions.presentation_ref`, nullable, backfilled `NULL`, undone cleanly.

Twenty mutations, twenty killed, each with its killing test and failure text.
The report's `packet_errors` are honest deviations, all accepted: the recorded
reference is the WORKSPACE ref (`cmux workspace select surface:N` answers
`not_found`, proved live); `--presentation-ref caller` resolves through
`cmux identify --json` because a pane's reference is unknown until `workspace
create` returns; and a session the router *continues* inside a pane is moved
there through one setter, `SessionStore::set_presentation`, rather than by
forcing `--fresh` and overruling the router silently. Its 69-target blast
radius had one red — `events_lifecycle`, outside this package's files —
attributed per practice §34 with two serial re-runs.

At integration the orchestrator merged this beside migration 19 and repaired
two things the worker could not have seen: `UNDO_19` in migration 19's own
proof now drops this migration's column first, and the credential-pin list in
`session/store.rs` keeps migration 19's real column name (`affected`) rather
than the earlier draft name this package had carried verbatim.

### Detect whether Glasshouse is running in an environment where cmux control capabilities are available. (line 754)

Contract: Given Glasshouse running anywhere, when a command needs cmux, Glasshouse reports it available only if CMUX_SOCKET_PATH is set (corroborated by the surface/workspace variables) AND a usable cmux executable resolves AND `cmux ping` answers, while preserving that a set variable in a dead environment reads as absent with the reason.

State: COMPLETE — ruled 2026-08-31 by the orchestrator, from the report's five artifacts and the diff of `set_presentation`.

Production evidence:
- `src/integrations/cmux.rs` — `detect / detect_with / Availability / Absence`
- `src/integrations/mod.rs` — `presence_without_executable_with (now pub(crate), shared with Discovery)`

Regression evidence:
- `integrations::cmux::tests::outside_cmux_is_absent_before_anything_is_resolved_or_pinged`
- `integrations::cmux::tests::an_empty_socket_path_is_outside_cmux`
- `integrations::cmux::tests::inside_cmux_without_an_executable_is_absent_and_names_what_was_tried`
- `integrations::cmux::tests::a_set_variable_whose_cmux_does_not_answer_reads_as_absent`
- `integrations::cmux::tests::presence_with_an_executable_and_an_answer_is_available`
- `cmux_presentation::without_cmux_every_command_runs_embedded_and_says_so (Dead-socket leg: exactly ["ping"] was asked)`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| cmux.rs: `match ping(&cli) {` -> `match Ok::<(), CmuxError>(()) {` | `skip-check` | **killed** | `integrations::cmux::tests::a_set_variable_whose_cmux_does_not_answer_reads_as_absent` |
| same mutation, binary level | `skip-check` | **killed** | `cmux_presentation::without_cmux_every_command_runs_embedded_and_says_so` |
| cmux.rs: `if super::presence_without_executable_with(IntegrationId::Cmux, &env).is_empty() {` -> `if false {` | `skip-check` | **killed** | `integrations::cmux::tests::outside_cmux_is_absent_before_anything_is_resolved_or_pinged` |

> skip-check observed: panicked at crates/glasshouse/src/integrations/cmux.rs:929:22 (`expected NotAnswering, got Available`)

> skip-check observed: panicked at crates/glasshouse/tests/cmux_presentation.rs:431:9 — the launch no longer said `cmux did not answer a ping` and opened a pane

> skip-check observed: assertion failed: matches!(availability, Availability::Absent(Absence::NotInsideCmux))

Recorded scope limits — stated by the worker, not discovered later:
- presence is the same evidence Discovery reports (CMUX_SOCKET_PATH set and non-empty); the corroborating variables are recorded as evidence, not required
- the ping is a real child process on every detect(); it is only made on the cmux paths (the two flags, `sessions focus`, the door's `presentation: cmux`, and the SendMessage NotLive fallback)

---


### Keep all core Glasshouse functionality operational when cmux is absent. (line 755)

Contract: Given cmux absent, dead, or answering, when any core command runs, Glasshouse behaves identically without cmux — a launch asked for a pane says why and runs where it was going anyway, `focus` refuses by name, the door's spawn stays headless and says so — while preserving that nothing in session/** or shell/** depends on cmux.

State: COMPLETE — ruled 2026-08-31 by the orchestrator, from the report's five artifacts and the diff of `set_presentation`.

Production evidence:
- `src/main.rs` — `launch_session (hosted_pane match: Absent arm prints the reason and continues)`
- `src/main.rs` — `focus_session (Absent arm)`
- `src/api/unix.rs` — `spawn_session (presentation_note), send_through_pane (Absent arm)`

Regression evidence:
- `cmux_presentation::without_cmux_every_command_runs_embedded_and_says_so`
- `cmux_presentation::the_session_and_shell_layers_never_name_cmux_and_the_wrapper_uses_only_documented_commands`
- `integration_status (18 passed, unchanged)`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| main.rs: `eprintln!("glasshouse: cmux is not available ({reason}); the session runs {here}");` -> `anyhow::bail!("cmux is not available ({reason})");` | `refuse-instead-of-degrade` | **killed** | `cmux_presentation::without_cmux_every_command_runs_embedded_and_says_so` |

> refuse-instead-of-degrade observed: panicked at crates/glasshouse/tests/cmux_presentation.rs:327:5 — `launched.status.success()` false: the launch refused instead of running headless

Recorded scope limits — stated by the worker, not discovered later:
- `api send`'s CLI client prints `delivered to session …` whether the door or cmux carried it; the door's JSON says which (client.rs is outside this packet's files)

---


### Implement cmux support behind a separate optional integration module. (line 756)

Contract: Given the crate, when cmux support is needed, it lives in one optional module `integrations::cmux` behind the `CmuxControl` trait, while preserving that the only production implementation is `CmuxCli` and the only process start is `CmuxCli::run`.

State: COMPLETE — ruled 2026-08-31 by the orchestrator, from the report's five artifacts and the diff of `set_presentation`.

Production evidence:
- `src/integrations/cmux.rs` — `CmuxControl, CmuxCli, Subcommand`
- `src/integrations/mod.rs` — `pub mod cmux`

Regression evidence:
- `cmux_presentation::the_session_and_shell_layers_never_name_cmux_and_the_wrapper_uses_only_documented_commands (exactly one `Command::new(`)`
- `integrations::cmux::tests::every_subcommand_is_spelled_the_way_cmux_documents_it`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| cmux.rs: `OsStr::new("--"),` -> `OsStr::new("read-screen"),` | `widen-surface` | **killed** | `cmux_presentation::the_session_and_shell_layers_never_name_cmux_and_the_wrapper_uses_only_documented_commands` |

> widen-surface observed: panicked at crates/glasshouse/tests/cmux_presentation.rs:1233:5 — `integrations/cmux.rs names cmux verbs its Subcommand does not declare (line 1893): ["read-screen"]`

Recorded scope limits — stated by the worker, not discovered later:
- optional means absent-tolerant, not a config switch: there is no setting that disables cmux while it is present — a person opts in per launch with `--presentation cmux`

---


### Allow Glasshouse to spawn a worker in a new cmux pane when the user requests external presentation. (line 757)

Contract: Given cmux available, when a person runs `glasshouse launch <harness> --presentation cmux`, Glasshouse issues one `workspace create` with `--cwd <project root>` and `--command` = this same launch (`--scope/--data-dir/--config-dir` resolved, every launch flag forwarded, `--presentation-ref caller`), starts nothing else, waits up to 5 s for the pane's record and prints the session id, while preserving that a launch that would be refused is refused before any pane opens.

State: COMPLETE — ruled 2026-08-31 by the orchestrator, from the report's five artifacts and the diff of `set_presentation`.

Production evidence:
- `src/main.rs` — `open_cmux_pane, pane_global_args, pane_launch_args / PaneLaunch, external_presentation`
- `src/integrations/cmux.rs` — `pane_command, shell_command, shell_quote, CmuxCli::create_workspace, recorded_panes, await_session_at`

Regression evidence:
- `cmux_presentation::an_external_spawn_records_the_pane_as_presentation_metadata`
- `integrations::cmux::tests::the_pane_command_names_the_executable_then_globals_then_the_launch`
- `integrations::cmux::tests::shell_quoting_keeps_a_word_a_word`
- `integrations::cmux::tests::a_create_answer_yields_its_workspace_ref`
- `live probe transcript 1`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| main.rs pane_launch_args: `args.push("caller".into());` -> `args.push("workspace:1".into());` | `wrong-argument` | **killed** | `cmux_presentation::an_external_spawn_records_the_pane_as_presentation_metadata` |

> wrong-argument observed: panicked at crates/glasshouse/tests/cmux_presentation.rs:523:5 — the outer process never saw a record at workspace:7 and printed `has not recorded itself yet` instead of the id

Recorded scope limits — stated by the worker, not discovered later:
- the pane's command carries the resolved data/config directories and the project root; a `--log-file`/`--log-level`/`--log-stderr` the person passed is forwarded, `--allow-unsafe-scope` too; the door's pane command forwards none of the logging flags (the door has no Cli)
- the 5 s wait bounds process start-up in the pane; on expiry the pane is real and reported, the id is not

---


### Allow Glasshouse to send text to a known cmux-backed session through the cmux integration. (line 758)

Contract: Given a session with a recorded pane, when text is sent through the door, Glasshouse delivers through `SessionApi::send_text` if the session is live here and answers `via: door`; only on `NotLive`, and only for a session with a pane, it runs `cmux send --workspace <ref> -- <text>\r` and answers `via: cmux`, while preserving that a session with no pane gets the same `NotLive` refusal it always did and the runtime lock is never held across the cmux child.

State: COMPLETE — ruled 2026-08-31 by the orchestrator, from the report's five artifacts and the diff of `set_presentation`.

Production evidence:
- `src/api/unix.rs` — `dispatch (Request::SendMessage arm), send_through_pane`
- `src/integrations/cmux.rs` — `send_line, CmuxCli::send_line`

Regression evidence:
- `cmux_presentation::focus_and_send_go_through_the_integration_and_the_door_is_preferred`
- `integrations::cmux::tests::send_line_validates_the_stored_ref_and_types_one_line`
- `live probe transcript 2`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| unix.rs: `Ok(()) => Response::ok(serde_json::json!({ "via": "door" })),` -> `Ok(()) => send_through_pane(&store, &id, &text),` | `wrong-branch` | **killed** | `cmux_presentation::focus_and_send_go_through_the_integration_and_the_door_is_preferred` |
| unix.rs: `Err(ApiError::NotLive { .. }) => send_through_pane(&store, &id, &text),` -> `Err(ApiError::NotLive { id }) => Response::err(api_error(ApiError::NotLive { id })),` | `skip-fallback` | **killed** | `cmux_presentation::focus_and_send_go_through_the_integration_and_the_door_is_preferred` |

> wrong-branch observed: assertion `left == right` failed: {"message":"session `4cf7…` is not live in this Glasshouse","status":"error"} — the live session was answered by the pane path

> skip-fallback observed: assertion `left == right` failed: {"message":"session `653d…` is not live in this Glasshouse","status":"error"} — expected via: cmux

Recorded scope limits — stated by the worker, not discovered later:
- `cmux send` interprets `\n`, `\r`, `\t` escape sequences in the text (its documented behaviour); Enter is appended as the two characters `\r`; a line containing such a sequence literally would be translated by cmux
- delivery through cmux types into the pane's terminal, which reaches the harness only through the Glasshouse attached there; it is not the door's own runtime and carries no MessageOrigin

---


### Allow Glasshouse to focus a cmux pane associated with a session. (line 759)

Contract: Given a session with a recorded pane, when a person runs `glasshouse sessions focus <id>`, Glasshouse validates the stored reference and issues exactly one `cmux workspace select <ref>`, while preserving that a session with no pane, an invalid stored reference, or an unavailable cmux is refused by name and cmux is asked nothing.

State: COMPLETE — ruled 2026-08-31 by the orchestrator, from the report's five artifacts and the diff of `set_presentation`.

Production evidence:
- `src/main.rs` — `focus_session; src/cli.rs :: SessionCommand::Focus`
- `src/integrations/cmux.rs` — `focus, PaneRef::parse, CmuxCli::select_workspace`

Regression evidence:
- `cmux_presentation::focus_and_send_go_through_the_integration_and_the_door_is_preferred`
- `cmux_presentation::without_cmux_every_command_runs_embedded_and_says_so`
- `integrations::cmux::tests::focus_validates_the_stored_ref_and_issues_exactly_one_select`
- `integrations::cmux::tests::the_real_cli_refuses_to_select_by_surface_before_asking_cmux`
- `live probe transcript 1`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| cmux.rs focus: `control.select_workspace(&pane)?;` -> `control.send_line(&pane, "")?;` | `wrong-subcommand` | **killed** | `integrations::cmux::tests::focus_validates_the_stored_ref_and_issues_exactly_one_select; cmux_presentation::focus_and_send_go_through_the_integration_and_the_door_is_preferred` |
| cmux.rs PaneRef::parse: `let number_ok = number.is_some_and(|number| {` -> `let number_ok = true; let _ = number.map(|number| {` | `skip-validation` | **killed** | `integrations::cmux::tests::only_workspace_and_surface_refs_are_admitted; cmux_presentation::focus_and_send_go_through_the_integration_and_the_door_is_preferred` |

> wrong-subcommand observed: assertion `left == right` failed: exactly one select, for exactly the recorded workspace: ["ping", "send --workspace workspace:7 -- \\r"]

> skip-validation observed: panicked at crates/glasshouse/src/integrations/cmux.rs:976:13 (`workspace:1; rm -rf /` must be refused) and at tests/cmux_presentation.rs:783:5 (the bogus stored ref was handed to cmux)

Recorded scope limits — stated by the worker, not discovered later:
- a stored `surface:N` is a valid reference for `send` but `workspace select` cannot take it; focus refuses it by name (`NotAWorkspace`) before cmux is asked

---


### Record the cmux surface or pane identifier as optional session presentation metadata. (line 760)

Contract: Given a launch hosted by a pane, when the session is recorded, Glasshouse writes `presentation = external` and `presentation_ref = <workspace ref>` on the session row (migration 20, nullable, backfilled NULL), also for a recorded session the router continues inside the pane, while preserving that the store stores the reference opaquely and never interprets it, and that the migration undoes cleanly.

State: COMPLETE — ruled 2026-08-31 by the orchestrator, from the report's five artifacts and the diff of `set_presentation`.

Production evidence:
- `src/database.rs` — `MIGRATIONS[19] (migration 20), SUPPORTED_SCHEMA_VERSION = 20`
- `src/session/store.rs` — `SessionRecord::presentation_ref, NewSession::with_presentation_ref, SessionStore::create, SessionStore::set_presentation, decode`
- `src/main.rs` — `launch_session (.with_presentation_ref; continue branch set_presentation), session_detail, presented_cell`

Regression evidence:
- `database::tests::migration_20_adds_presentation_ref_and_undoes_cleanly`
- `session::store::tests::an_external_sessions_presentation_ref_round_trips_and_is_never_interpreted`
- `session::store::tests::a_continued_session_can_be_moved_into_a_pane_afterwards`
- `session::store::tests::the_project_database_schema_has_nowhere_to_put_a_credential (column listed and reviewed)`
- `cmux_presentation::an_external_spawn_records_the_pane_as_presentation_metadata (incl. the continuation probe)`
- `session_context (6 passed at schema 20)`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| main.rs: `.with_presentation_ref(hosted_pane.as_ref().map(|pane| pane.as_str().to_owned()))` -> `.with_presentation_ref(None)` | `skip-state-update` | **killed** | `cmux_presentation::an_external_spawn_records_the_pane_as_presentation_metadata` |
| main.rs continue branch: `if let Some(pane) = &hosted_pane {` -> `if let Some(pane) = &None::<cmux::PaneRef> {` | `skip-state-update` | **killed** | `cmux_presentation::an_external_spawn_records_the_pane_as_presentation_metadata` |
| database.rs: `ALTER TABLE sessions ADD COLUMN presentation_ref TEXT;` -> `… TEXT NOT NULL DEFAULT '';` | `wrong-default` | **killed** | `database::tests::migration_20_adds_presentation_ref_and_undoes_cleanly` |

> skip-state-update observed: panicked at crates/glasshouse/tests/cmux_presentation.rs:523:5 — no record named workspace:7, the outer printed `has not recorded itself yet`

> skip-state-update observed: panicked at crates/glasshouse/tests/cmux_presentation.rs:638:5 — the continued session did not show `presentation ref   workspace:13`

> wrong-default observed: assertion `left == right` failed: a row from before the column existed has no pane, not an empty one

Recorded scope limits — stated by the worker, not discovered later:
- a record keeps its pane after the pane closes; `resume_session` (unchanged, like its existing treatment of headless) does not refresh presentation, so a session resumed later from a plain terminal still reads `external workspace:N` until something records otherwise

---


### Allow a session to be created directly in external-cmux presentation mode. (line 761)

Contract: Given cmux available, when a session is created with `--presentation cmux` or `SpawnSession { presentation: "cmux" }`, the session is created directly in external presentation (recorded External + ref by the launch inside the pane), and the door's answer carries the pane and the id, while preserving that an unknown backend is refused by name before anything is recorded and that the absent case answers headless with a note.

State: COMPLETE — ruled 2026-08-31 by the orchestrator, from the report's five artifacts and the diff of `set_presentation`.

Production evidence:
- `src/main.rs` — `run() Launch/Run arm (external_presentation), ExternalPresentation`
- `src/api/protocol.rs` — `Request::SpawnSession.presentation`
- `src/api/unix.rs` — `spawn_session, spawn_in_cmux`
- `src/integrations/cmux.rs` — `Backend::parse`

Regression evidence:
- `cmux_presentation::an_external_spawn_records_the_pane_as_presentation_metadata`
- `cmux_presentation::focus_and_send_go_through_the_integration_and_the_door_is_preferred (door spawn: external / headless-with-note / unknown backend)`
- `cli::tests::parses_a_presentation_backend_and_a_pane_ref_but_never_both`
- `integrations::cmux::tests::every_backend_parses_from_its_own_word_and_nothing_else_does`
- `live probe transcript 2`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| unix.rs: `match presentation.map(cmux::Backend::parse).transpose() {` -> `match None::<&str>.map(cmux::Backend::parse).transpose() {` | `ignore-input` | **killed** | `cmux_presentation::focus_and_send_go_through_the_integration_and_the_door_is_preferred` |

> ignore-input observed: assertion `left == right` failed: {"result":{"session":"5c9b…"},"status":"ok"} — expected presentation: external

Recorded scope limits — stated by the worker, not discovered later:
- a door spawn into cmux is recorded with role `normal`: the launch inside the pane takes no role flag (Phase 14's `--role` on launch is its own box); the answer's `presentation` distinguishes the path
- the MCP `glasshouse_spawn_session` tool does not expose `presentation` yet (mcp.rs passes `None`); its schema grows once use asks

---


### Keep the underlying Glasshouse session abstraction independent from whether presentation is embedded or in cmux. (line 762)

Contract: Given any session, when it is stored or read, the session abstraction knows one nullable opaque string and nothing else about presentation backends, while preserving that no production code in session/** or shell/** names cmux — enforced by a source-scan tripwire.

State: COMPLETE — ruled 2026-08-31 by the orchestrator, from the report's five artifacts and the diff of `set_presentation`.

Production evidence:
- `src/session/store.rs` — `presentation_ref (stored and returned, never parsed)`

Regression evidence:
- `cmux_presentation::the_session_and_shell_layers_never_name_cmux_and_the_wrapper_uses_only_documented_commands`
- `session::store::tests::an_external_sessions_presentation_ref_round_trips_and_is_never_interpreted`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| store.rs: `"UPDATE sessions SET presentation = ?2, presentation_ref = ?3 WHERE id = ?1",` -> `… WHERE id = ?1 -- cmux",` | `cross-layer-dependency` | **killed** | `cmux_presentation::the_session_and_shell_layers_never_name_cmux_and_the_wrapper_uses_only_documented_commands` |

> cross-layer-dependency observed: panicked at crates/glasshouse/tests/cmux_presentation.rs:1201:9 — `src/session/**` must never name cmux in production code (line 762); found it in ["…/src/session/store.rs"]

Recorded scope limits — stated by the worker, not discovered later:
- the scan strips `//` comments and stops at the first `#[cfg(test)]`, so a doc sentence or a test may mention cmux; a dependency cannot
- one new store method (`set_presentation`) beyond the column — a setter over the same two columns, for the continued-session case

---


### Treat cmux as a workspace and presentation backend rather than as Glasshouse’s orchestration core. (line 763)

Contract: Given the architecture, when cmux is used, it is a workspace/presentation backend reached only from `integrations::cmux` by the CLI launch path, `sessions focus`, and the door — never by the session runtime, the session API, or the shell — while preserving that removing cmux changes no core behaviour.

State: COMPLETE — ruled 2026-08-31 by the orchestrator, from the report's five artifacts and the diff of `set_presentation`.

Production evidence:
- `src/integrations/cmux.rs (the only cmux-aware module); callers: src/main.rs, src/api/unix.rs`

Regression evidence:
- `cmux_presentation::the_session_and_shell_layers_never_name_cmux_and_the_wrapper_uses_only_documented_commands`
- `cmux_presentation::without_cmux_every_command_runs_embedded_and_says_so`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| covered by the 762 and 1893 mutations (both tripwire halves) | `cross-layer-dependency` | **killed** | `cmux_presentation::the_session_and_shell_layers_never_name_cmux_and_the_wrapper_uses_only_documented_commands` |

> cross-layer-dependency observed: see 762 and 1893

Recorded scope limits — stated by the worker, not discovered later:
- `session/runtime.rs`, `session/api.rs`, `pty/**` and the shell's production code are untouched; four `#[cfg(test)]` struct literals in shell/state.rs and shell/view.rs gained `presentation_ref: None` (the non-Default field ripple), which names no backend

