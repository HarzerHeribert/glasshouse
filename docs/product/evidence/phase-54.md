# Capability evidence — phase 54

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 54 — criteria before deeper cmux coupling, 4 of 4 (lines 1892–1895)

Package `GH-CMUX-PRESENTATION`, 2026-08-31, Fable specialist at xhigh, closed
in the same package as Phase 17 because the four criteria are properties of
how Phase 17 was built rather than work beside it: cmux stays optional and
absent-tolerant (1892, 1894), only the documented `cmux workspace` / `send` /
`identify` / `ping` verbs are used with `CMUX_QUIET=1` so a deprecation hint
can never land in an answer (1893), and the surface is expose-and-focus only —
the MCP tool deliberately offers no presentation backend yet (1895).


### Keep cmux optional until repeated usage proves external-pane workflows are essential. (line 1892)

Contract: Given the codebase, when cmux is absent, every core command works unchanged and no dependency is added, while preserving that cmux code runs only on the explicit opt-in paths (the two flags, `sessions focus`, the door's `presentation`, and the NotLive send fallback).

State: COMPLETE — ruled 2026-08-31 by the orchestrator, from the report's five artifacts and the diff of `set_presentation`.

Production evidence:
- `src/integrations/cmux.rs module doc: 'Basic expose-and-focus, and why it stops there (lines 1892, 1895)'`

Regression evidence:
- `cmux_presentation::without_cmux_every_command_runs_embedded_and_says_so`
- `Cargo.toml / Cargo.lock unchanged (no dependency)`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| covered by the 755 and 1894 mutations | `refuse-instead-of-degrade` | **killed** | `cmux_presentation::without_cmux_every_command_runs_embedded_and_says_so` |

> refuse-instead-of-degrade observed: see 755 / 1894

Recorded scope limits — stated by the worker, not discovered later:
- 'until repeated usage proves' is a policy; the tripwire enforces its consequence (no widening without a visible `Subcommand` edit), not a usage measurement

---


### Avoid depending on undocumented cmux internals when a stable command or API surface exists. (line 1893)

Contract: Given the wrapper, when it talks to cmux, it uses only the five documented subcommands (`ping`, `identify --json`, `workspace create`, `workspace select`, `send`) and never the socket protocol, `rpc`, or an internal schema, while preserving that any other cmux verb named in the wrapper's production code fails a test.

State: COMPLETE — ruled 2026-08-31 by the orchestrator, from the report's five artifacts and the diff of `set_presentation`.

Production evidence:
- `src/integrations/cmux.rs` — `Subcommand (ALL, words), CmuxCli::run, caller_workspace_ref (one key by name, no schema)`

Regression evidence:
- `cmux_presentation::the_session_and_shell_layers_never_name_cmux_and_the_wrapper_uses_only_documented_commands`
- `integrations::cmux::tests::every_subcommand_is_spelled_the_way_cmux_documents_it`
- `integrations::cmux::tests::identify_yields_the_callers_workspace_not_the_focused_one`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| cmux.rs: `OsStr::new("--"),` -> `OsStr::new("read-screen"),` | `widen-surface` | **killed** | `cmux_presentation::the_session_and_shell_layers_never_name_cmux_and_the_wrapper_uses_only_documented_commands` |

> widen-surface observed: panicked at crates/glasshouse/tests/cmux_presentation.rs:1233:5 — names cmux verbs its Subcommand does not declare: ["read-screen"]

Recorded scope limits — stated by the worker, not discovered later:
- the verb list in the test is this machine's `cmux --help` on 2026-08-31 (150 entries); a verb cmux adds later is not in it until someone refreshes the list
- `identify --json` is read by locating the `caller` → `workspace_ref` key by name; the document's other keys are never consulted

---


### Keep embedded Glasshouse sessions fully functional even if cmux changes or disappears. (line 1894)

Contract: Given a cmux that changed or disappeared, when an embedded session runs, nothing changes, and when a cmux path is taken the failure is reported and the launch proceeds embedded/headless, while preserving that a cmux command-line change is met in `CmuxCli` alone.

State: COMPLETE — ruled 2026-08-31 by the orchestrator, from the report's five artifacts and the diff of `set_presentation`.

Production evidence:
- `src/main.rs` — `launch_session (HostedBy(Caller) failure arm), focus_session; src/integrations/cmux.rs :: CmuxCli (the one implementation)`

Regression evidence:
- `cmux_presentation::without_cmux_every_command_runs_embedded_and_says_so (Absent and Dead legs, `--presentation-ref caller` leg)`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| main.rs: `eprintln!("glasshouse: {reason}; the session runs {here}");` -> `anyhow::bail!("{reason}");` | `refuse-instead-of-degrade` | **killed** | `cmux_presentation::without_cmux_every_command_runs_embedded_and_says_so` |

> refuse-instead-of-degrade observed: panicked at crates/glasshouse/tests/cmux_presentation.rs:461:9 — the `--presentation-ref caller` launch outside cmux refused instead of running headless

Recorded scope limits — stated by the worker, not discovered later:
- a cmux that keeps answering `ping` but changes `workspace create`'s output shape is reported as `Unreadable` with the output quoted; it is not auto-adapted

---


### Add richer cmux workspace automation only after the basic expose-and-focus workflow proves useful. (line 1895)

Contract: Given the wrapper, when someone wants richer automation, the module doc states what basic expose-and-focus is (open, focus, send) and that anything more waits on use, while preserving that widening is a visible edit to `Subcommand` caught by the tripwire.

State: COMPLETE — ruled 2026-08-31 by the orchestrator, from the report's five artifacts and the diff of `set_presentation`.

Production evidence:
- `src/integrations/cmux.rs module doc section 'Basic expose-and-focus, and why it stops there (lines 1892, 1895)'`

Regression evidence:
- `cmux_presentation::the_session_and_shell_layers_never_name_cmux_and_the_wrapper_uses_only_documented_commands`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| covered by the 1893 mutation | `widen-surface` | **killed** | `cmux_presentation::the_session_and_shell_layers_never_name_cmux_and_the_wrapper_uses_only_documented_commands` |

> widen-surface observed: see 1893

Recorded scope limits — stated by the worker, not discovered later:
- a doc section plus a structural tripwire; whether the basic workflow 'proves useful' is a measurement this package does not make

