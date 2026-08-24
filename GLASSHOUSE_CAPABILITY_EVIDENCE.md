# Glasshouse capability evidence ledger

This ledger supports—but never replaces—the authoritative
[`GLASSHOUSE_IMPLEMENTATION_CAPABILITY_MAP.md`](GLASSHOUSE_IMPLEMENTATION_CAPABILITY_MAP.md).
It maps requirements to observable product contracts, production paths, and
non-vacuous regression evidence.

Populate entries incrementally as a capability becomes active or as previously
checked work is reconciled. Do not spend a whole implementation cycle filling
hundreds of future entries speculatively.

## Entry template

```markdown
### <phase and stable short name> — <exact capability text>

Contract: Given <context>, when <trigger>, Glasshouse <observable behavior>,
while preserving <invariant or failure behavior>.

State: NOT STARTED | SCAFFOLDED | PARTIALLY VERIFIED | LOCALLY VERIFIED |
CI VERIFIED | COMPLETE

Production evidence:
- `<file>: <symbol/path>` — why this is a real reachable production path

Regression evidence:
- `<test name>` — behavior proved and platforms actually executed

Failure/isolation evidence:
- `<test or probe>` — negative, fail-closed, cleanup, or boundary behavior

Platform/external evidence:
- `<CI run or runtime probe>` — commit and platforms covered

Missing evidence:
- exact remaining proof or implementation
```

## Evidence rules

- Quote the capability exactly enough to find it in the map.
- Keep the contract to one product sentence.
- Cite symbols and test names, not merely directories.
- State which platform actually executed a test.
- A test-only type or fake caller is not production evidence.
- A checked box requires **COMPLETE**.
- If later evidence contradicts an entry, downgrade it immediately and reopen
  the map checkbox if necessary.

## Active entries

### Phase 1 — Ensure every spawned harness process starts with its working directory set to the current project root

Contract: Whenever Glasshouse invokes an installed harness—including discovery
probes and interactive sessions—the child starts in the active canonical
project root and never inherits an unrelated caller directory.

State: COMPLETE

Production evidence:

- `main::launch_session` is the production consumer: `glasshouse launch
  [harness]` resolves a harness through `session::select::select` and starts it
  through `launch::HarnessLaunch`, which is the only route that reaches PTY
  spawn for a harness. This closes the gap recorded in the previous revision of
  this entry, where `HarnessLaunch` had no production caller at all.
- `session::attach::attach` runs the resulting session against the real
  terminal: raw mode, input/output pumps, resize forwarding, exit propagation,
  and restoration on every exit path.
- `launch::HarnessLaunch::spawn` reaches PTY spawn only through
  `TerminalCommand::for_harness`, which derives the directory from
  `Project::display_root` and is `pub(crate)`.
- `integrations::Discovery::run(&Project)` threads the active project into
  `version::probe_version`, which sets `Command::current_dir` from
  `Project::display_root`.

Regression evidence:

- `the_launch_command_opens_the_configured_harness_inside_the_project_root`
  (pty_smoke, macOS) runs the *shipped binary* in a real pseudo-terminal and
  matches the harness's own report of its working directory against the project
  root by filesystem identity. Glasshouse itself is deliberately run from a
  different directory, so an inherited cwd cannot pass.
- `a_fake_installed_harness_launches_inside_the_discovered_project_root`
  proves the same for the `HarnessLaunch` mechanism directly.
- `version_probe_child_starts_in_the_active_project_root` uses a resolved fake
  probe that prints a version only in the correct child directory.
- `project_configured_executable_wins_over_user_level` and the rest of
  `session::select::tests` pin executable precedence and every refusal path.
- Windows-only tests pin verbatim drive and UNC prefix conversion.

Failure/isolation evidence:

- The end-to-end test installs a *decoy* executable at the user level and the
  real one at the project level; a precedence failure runs the decoy and fails
  the test loudly rather than silently passing.
- `a_failing_configured_executable_never_falls_back_to_path` proves a broken
  configured path is an error, not a silent substitution of another binary.
- `attaching_without_a_terminal_is_refused` fails closed rather than hanging on
  a pty query nothing can answer.
- `PtyProcess::spawn` refuses a working directory that does not exist instead
  of starting the child somewhere else.
- Unsafe Windows-script arguments are rejected before `HarnessLaunch` spawns.

Non-vacuity (mutations actually run, each observed to fail the test):

- Removing the project layer from `EffectiveConfig::executable` → the decoy
  runs; the end-to-end test fails on precedence.
- Making `TerminalCommand::for_harness` use the process cwd instead of the
  project → no harness reports the project root; the test fails.
- Making `exit_code_for` always return success → the test fails on exit-code
  propagation.
- Setting `PTY_ALLOCATION_ATTEMPTS` to 1 → the pty retry test fails.

Platform/external evidence:

- **CI run `32788123095` on commit `f3effe6` is green on `ubuntu-latest`,
  `macos-latest`, and `windows-latest`, plus lint.** The Windows job was
  confirmed to have *actually executed* the tests cited here — 186 lib tests
  and 20 PTY smoke tests — rather than only reporting a green tick;
  `the_launch_command_opens_the_configured_harness_inside_the_project_root`
  and `a_fake_installed_harness_launches_inside_the_discovered_project_root`
  both appear as `ok` in the Windows log.
- Local macOS: `cargo fmt --check`, `cargo clippy -D warnings`, unit and PTY
  smoke tests, MSRV 1.85.0 `cargo check --locked`, `git diff --check`, and
  live CLI probes of `glasshouse launch` (help, no-terminal refusal, unknown
  harness, non-harness integration).
- An independent spawn-site inventory, re-run after the merge, found three
  production spawn sites, all project-bound, and zero production callers of
  the generic `TerminalCommand::new`.

What CI caught that local evidence had not:

- `cmd.exe` cannot open the verbatim `\\?\` path that resolving an executable
  produces on Windows, so **no `.cmd`-shimmed harness could start there at
  all** — and npm installs most of them that way. Fixed in `4aa31ad`.
- A prior revision of this entry cited a Windows job that had never run
  `tests/pty_smoke.rs`: when the lib target fails, cargo never reaches the
  integration tests, so the `.cmd` and verbatim-path claims were unproven
  while the entry implied otherwise. Confirming *execution*, not just the
  job's conclusion, is part of this evidence and not a formality.

Remaining caveats (recorded, not blocking):

- Native Windows UNC project roots are still refused rather than supported;
  `cmd.exe` cannot reliably hold a UNC working directory. This is a
  documented limitation of the contract, not a gap in it.


### Phase 2B — Detect cmux when a usable cmux executable or supported cmux control environment is present

Contract: Given a machine where the `cmux` executable is not on `PATH` but
Glasshouse is running inside a cmux surface, when discovery runs, cmux is
reported as detected and configured with the evidence that proves it, while
never being reported as launchable and never recording any environment
variable's value.

State: COMPLETE

Production evidence:

- `integrations::presence_without_executable` reads the cmux control
  environment (`CMUX_SOCKET_PATH`, corroborated by `CMUX_SURFACE_ID` and
  `CMUX_WORKSPACE_ID`).
- `integrations::detect_one_with` consults it in the `ResolveOutcome::NotFound`
  arm only, yielding `IntegrationStatus::Configured` with `executable: None`.
  The `Usable` and `Unusable` arms are untouched.

Regression evidence:

- `cmux_socket_path_set_yields_evidence_naming_it`,
  `cmux_corroborating_variables_are_also_named`,
  `empty_cmux_socket_path_counts_as_unset`, `no_cmux_variables_yields_no_evidence`
  pin the decision, over injected lookups so no test mutates the environment.
- `absent_executable_but_presence_evidence_is_configured_not_launchable` proves
  the wiring, including that `is_usable()` stays **false** — a detected
  integration with no executable must never be mistaken for a launchable one.
- `absent_executable_with_no_presence_evidence_stays_not_found` pins the
  unchanged behaviour for everything else.

Failure/isolation evidence:

- `evidence_notes_never_contain_a_value_only_names` fills every consulted
  variable with sentinels and asserts no note contains one.
  `CMUX_SOCKET_CAPABILITY` is a capability token and is never read at all.
- Non-vacuity (mutation actually run): making the `NotFound` arm ignore the
  presence evidence makes the wiring test fail.

Platform/external evidence:

- Live probe on macOS inside a real cmux surface, with `cmux` removed from
  `PATH`: `glasshouse doctor` reports `cmux [configured]` with the notes
  "candidates tried: cmux", "CMUX_SOCKET_PATH is set", "CMUX_SURFACE_ID is set".
- CI green on `ubuntu-latest`, `macos-latest`, `windows-latest`, and lint. The
  behaviour is environment-variable based with no `cfg` gating, so all three
  platforms execute the same code.

### Phase 2B — Detect Ollama when a usable ollama executable or configured local endpoint is present

Contract: Given a machine with no `ollama` executable on `PATH` but a
configured local endpoint, when discovery runs, Ollama is reported as detected
and configured, while never being reported as launchable and never recording
the endpoint's value — which can carry credentials.

State: COMPLETE

Production evidence:

- `integrations::presence_without_executable` treats a set, non-empty
  `OLLAMA_HOST` as a configured endpoint, wired through the same
  `detect_one_with` seam as cmux above.
- Deliberately no network request: discovery stays non-destructive and adds no
  HTTP dependency. The capability asks whether an endpoint is *configured*, not
  whether a server is answering.

Regression evidence:

- `ollama_host_set_unset_and_empty` pins set, unset, and empty-as-unset.
- The same wiring and non-launchability tests listed above cover Ollama, which
  is the integration they are written against.

Failure/isolation evidence:

- `evidence_notes_never_contain_a_value_only_names` covers `OLLAMA_HOST`.
- Live probe: `glasshouse doctor` with
  `OLLAMA_HOST=http://user:SUPERSECRET@127.0.0.1:11434` reports
  `Ollama [configured]` with the note "OLLAMA_HOST is set", and the whole
  report contains **zero** occurrences of the secret.

Platform/external evidence:

- Same CI run as the cmux entry above; no `cfg` gating, so all three platforms
  execute this code.

Missing evidence:

- None for the stated contract. Whether a configured endpoint is actually
  *reachable* is deliberately out of scope and would need a network probe.
