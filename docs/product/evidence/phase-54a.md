# Capability evidence — phase 54A

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules). Phase 54A — setup and portability completion criteria — had no entry before 2026-08-31.

### Lines 1899–1908 — onboarding, settings, profiles, gateway isolation, provider setup, free-pool, and the platform legs

Package `GH-PROVE-IT-54A`, 2026-08-31, Sonnet at medium (Green — tests and mutations only; no production change). One new file, `tests/v1_criteria_setup.rs`, ten tests, 9/9 mutations KILLED on the closed lines; **1908 stays open** (its words name CI runners). Written under the corrected prove-it discipline: the map line quoted as the criterion, seams as suggestions only.

### Consider onboarding usable when a new user can launch Glasshouse and see installed supported harnesses without manually editing a config file. (line 1899)

Contract: Given a fresh project with detectable harnesses on PATH and no config file, when Glasshouse runs `doctor`, it lists them as installed while creating no config file, while preserving that detection needs no manual editing.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 54A line is a completion CRITERION over shipped setup/portability behaviour; this test is the criterion's tripwire through the shipped binary, and the report quotes which evidence entry proves the underlying mechanism. Artifacts: `test result:` lines with counts, one KILLED mutation per line with output quoted.

Production evidence:
- `src/integrations/mod.rs` — `doctor_report`
- `src/integrations/mod.rs` — `detect_one_with_prober`

Regression evidence:
- `v1_criteria_setup::v1_1899_a_fresh_project_shows_installed_harnesses_with_no_config_file_created`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| for d in discovery.harnesses() { -> for d in std::iter::empty::<&DetectedIntegration>() { | `hide-detected-harnesses` | **killed** | `v1_criteria_setup::v1_1899_a_fresh_project_shows_installed_harnesses_with_no_config_file_created` |

> hide-detected-harnesses observed: panicked at crates/glasshouse/tests/v1_criteria_setup.rs:395:9 (no Claude Code row in doctor report)

Recorded scope limits — stated by the worker, not discovered later:
- only claude-code and codex are exercised, not every IntegrationId

---

### Consider onboarding usable when the user can skip all provider configuration and still use native detected harnesses. (line 1900)

Contract: Given zero `[providers.*]` configuration, when the user launches a native detected harness, Glasshouse starts and records the session, while preserving that no provider lookup is required on the native path.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 54A line is a completion CRITERION over shipped setup/portability behaviour; this test is the criterion's tripwire through the shipped binary, and the report quotes which evidence entry proves the underlying mechanism. Artifacts: `test result:` lines with counts, one KILLED mutation per line with output quoted.

Production evidence:
- `src/profile/mod.rs` — `resolve (BackendResource::Native arm)`
- `src/main.rs` — `launch_session`

Regression evidence:
- `v1_criteria_setup::v1_1900_a_native_launch_needs_no_provider_configuration`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| BackendResource::Native => { push mechanism note } -> BackendResource::Native => { panic!(...) } | `panic-native-resolution` | **killed** | `v1_criteria_setup::v1_1900_a_native_launch_needs_no_provider_configuration` |

> panic-native-resolution observed: panicked at crates/glasshouse/tests/v1_criteria_setup.rs:459:5; child process panicked at profile/mod.rs:970:13

Recorded scope limits — stated by the worker, not discovered later:
- does not prove every harness's native path, only claude-code's

---

### Consider settings usable when the user can return later and configure a provider without rerunning the entire setup. (line 1901)

Contract: Given onboarding already completed, when the user edits `config.toml` directly to add a provider, Glasshouse's very next `doctor` run sees it, while preserving that setup is never rerun.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 54A line is a completion CRITERION over shipped setup/portability behaviour; this test is the criterion's tripwire through the shipped binary, and the report quotes which evidence entry proves the underlying mechanism. Artifacts: `test result:` lines with counts, one KILLED mutation per line with output quoted.

Production evidence:
- `src/integrations/mod.rs` — `doctor_report (Configured providers section)`
- `src/config/mod.rs` — `EffectiveConfig::provider_names`

Regression evidence:
- `v1_criteria_setup::v1_1901_a_provider_added_later_is_seen_without_rerunning_setup`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| let names = effective.provider_names(); -> let names: Vec<String> = Vec::new(); | `discard-provider-names` | **killed** | `v1_criteria_setup::v1_1901_a_provider_added_later_is_seen_without_rerunning_setup` |

> discard-provider-names observed: panicked at crates/glasshouse/tests/v1_criteria_setup.rs:528:5 (v1901-provider absent from doctor report)

Recorded scope limits — stated by the worker, not discovered later:
- proves the read path only; does not exercise the settings TUI's own provider-add flow

---

### Consider launch profiles usable when the same installed Claude Code binary can be started natively or with an alternate compatible provider without modifying the user’s normal Claude configuration. (line 1902)

Contract: Given the same installed Claude Code binary, when it is launched natively and then under an alternate direct-provider profile, Glasshouse never modifies the harness's own `$HOME/.claude` configuration, while preserving byte-identical state across both launches.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 54A line is a completion CRITERION over shipped setup/portability behaviour; this test is the criterion's tripwire through the shipped binary, and the report quotes which evidence entry proves the underlying mechanism. Artifacts: `test result:` lines with counts, one KILLED mutation per line with output quoted.

Production evidence:
- `src/profile/mod.rs` — `apply_direct_provider`
- `src/harness/claude_code.rs` — `direct_provider_launch`

Regression evidence:
- `v1_criteria_setup::v1_1902_native_and_alternate_provider_launches_never_touch_claudes_own_config`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| install_quiet_harness's script: exit 0 -> also writes a sentinel into $HOME/.claude/settings.json | `fixture-sensitivity-check` | **killed** | `v1_criteria_setup::v1_1902_native_and_alternate_provider_launches_never_touch_claudes_own_config` |

> fixture-sensitivity-check observed: assertion left == right failed at crates/glasshouse/tests/v1_criteria_setup.rs:636:5

Recorded scope limits — stated by the worker, not discovered later:
- the guarantee is an absence with no production guard to invert (nothing writes there); the mutation instead proves the byte-identical assertion is non-vacuous, per practice §17

---

### Consider interactive gateway use valid only when the session is operated by an installed compatible harness and Glasshouse does not create a replacement agent loop. (line 1903)

Contract: Given a gateway-backed launch profile, when Glasshouse starts a session, it spawns only the installed harness child pointed at the local gateway over environment variables, while preserving that Glasshouse runs no agent loop of its own.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 54A line is a completion CRITERION over shipped setup/portability behaviour; this test is the criterion's tripwire through the shipped binary, and the report quotes which evidence entry proves the underlying mechanism. Artifacts: `test result:` lines with counts, one KILLED mutation per line with output quoted.

Production evidence:
- `src/harness/claude_code.rs` — `BASE_URL_ENV / direct_provider_launch`
- `src/main.rs` — `launch_session (gateway backend arm)`
- `src/gateway/mod.rs` — `start_if_required, accept_loop`

Regression evidence:
- `v1_criteria_setup::v1_1903_a_gateway_backed_launch_hands_the_session_to_the_installed_harness_alone`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| const BASE_URL_ENV: &str = "ANTHROPIC_BASE_URL"; -> "ANTHROPIC_BASE_URL_DISABLED" | `rename-base-url-env` | **killed** | `v1_criteria_setup::v1_1903_a_gateway_backed_launch_hands_the_session_to_the_installed_harness_alone` |

> rename-base-url-env observed: panicked at crates/glasshouse/tests/v1_criteria_setup.rs:725:28 (no ANTHROPIC_BASE_URL in child's environment)

Recorded scope limits — stated by the worker, not discovered later:
- the no-agent-loop architectural claim is cited from phase-9g.md, not re-derived by this test

---

### Consider response profiles minimally usable when at least one supported harness can apply a selected profile through a native mechanism or the bounded additive fallback while preserving coding instructions. (line 1904)

Contract: Given at least one configured response profile, when a supported harness is launched, Glasshouse applies the profile through the harness's native mechanism while preserving the coding system prompt.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 54A line is a completion CRITERION over shipped setup/portability behaviour; this test is the criterion's tripwire through the shipped binary, and the report quotes which evidence entry proves the underlying mechanism. Artifacts: `test result:` lines with counts, one KILLED mutation per line with output quoted.

Production evidence:
- `src/harness/claude_code.rs` — `install_session_document / --settings composition`
- `src/session/select.rs` — `HarnessSelection::install_session_document`

Regression evidence:
- `v1_criteria_setup::v1_1904_a_launched_harness_receives_the_response_profile_through_its_native_settings_flag`
- `response_profiles::the_launch_carries_exactly_one_settings_flag_and_keeps_the_lifecycle_hooks`
- `response_profiles::the_launch_appends_to_the_system_prompt_and_never_replaces_it`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| std::ffi::OsString::from("--settings") -> "--settings-disabled" | `rename-settings-flag` | **killed** | `v1_criteria_setup::v1_1904_a_launched_harness_receives_the_response_profile_through_its_native_settings_flag` |

> rename-settings-flag observed: panicked at crates/glasshouse/tests/v1_criteria_setup.rs:836:31

Recorded scope limits — stated by the worker, not discovered later:
- only claude-code's native mechanism is exercised at the launch-shaped level

---

### Consider gateway mode usable when two concurrent Glasshouse instances can run isolated local gateways without port or credential collisions. (line 1905)

Contract: Given two concurrent Glasshouse instances each with a gateway-backed profile, Glasshouse binds each to its own OS-chosen port and mints its own credential, while preserving that neither instance accepts the other's token.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 54A line is a completion CRITERION over shipped setup/portability behaviour; this test is the criterion's tripwire through the shipped binary, and the report quotes which evidence entry proves the underlying mechanism. Artifacts: `test result:` lines with counts, one KILLED mutation per line with output quoted.

Production evidence:
- `src/gateway/mod.rs` — `Gateway::start_with_degrade_sink, start_if_required, GatewayToken`

Regression evidence:
- `v1_criteria_setup::v1_1905_two_concurrent_gateways_get_different_ports_and_reject_each_others_credential`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| const EPHEMERAL_PORT: u16 = 0; -> = 58231; | `pin-fixed-port` | **killed** | `v1_criteria_setup::v1_1905_two_concurrent_gateways_get_different_ports_and_reject_each_others_credential` |

> pin-fixed-port observed: panicked at crates/glasshouse/tests/v1_criteria_setup.rs:902:6 (second gateway's bind collided with the first's)

Recorded scope limits — stated by the worker, not discovered later:
- seam-level (start_if_required) rather than two real `glasshouse launch --profile gateway` processes

---

### Consider provider setup usable when OpenRouter, one generic OpenAI-compatible endpoint, and one generic Anthropic-compatible endpoint can be configured and tested. (line 1906)

Contract: Given OpenRouter, one generic openai-compatible provider and one generic anthropic-compatible provider, when the user tests connectivity, Glasshouse actually reaches each configured base URL with the configured credential attached, while preserving that each template's own credential-header convention is used.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 54A line is a completion CRITERION over shipped setup/portability behaviour; this test is the criterion's tripwire through the shipped binary, and the report quotes which evidence entry proves the underlying mechanism. Artifacts: `test result:` lines with counts, one KILLED mutation per line with output quoted.

Production evidence:
- `src/provider/mod.rs` — `templates, template`
- `src/provider/resources.rs` — `probe_provider, telemetry_probe`
- `src/provider/discovery.rs` — `connectivity_with_headers`
- `src/main.rs` — `resources_report`

Regression evidence:
- `v1_criteria_setup::v1_1906_openrouter_and_both_generic_templates_are_configured_and_actually_probed`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| name: "openai-compatible".to_owned() -> "openai-compatible-disabled".to_owned() | `break-generic-template-name` | **killed** | `v1_criteria_setup::v1_1906_openrouter_and_both_generic_templates_are_configured_and_actually_probed` |

> break-generic-template-name observed: panicked at crates/glasshouse/tests/v1_criteria_setup.rs:1045:9

Recorded scope limits — stated by the worker, not discovered later:
- proves the connectivity test only, not a real interactive launch against any of the three providers

---

### Consider free-pool support usable when at least one configured zero-cost or free-tier model can perform a disposable Glasshouse support job. (line 1907)

Contract: Given a provider naming a zero-cost model under `free_models` and `routing.model = automatic`, when Glasshouse classifies a request, the free model performs the disposable job, while preserving that the routing policy — not a shortcut around it — chose it.

State: COMPLETE — ruled 2026-08-31 by the orchestrator. A Phase 54A line is a completion CRITERION over shipped setup/portability behaviour; this test is the criterion's tripwire through the shipped binary, and the report quotes which evidence entry proves the underlying mechanism. Artifacts: `test result:` lines with counts, one KILLED mutation per line with output quoted.

Production evidence:
- `src/main.rs` — `disposable_candidates, automatic_classification_model`
- `src/routing/disposable.rs` — `DisposableRouting::choose (exercised, not mutated)`

Regression evidence:
- `v1_criteria_setup::v1_1907_a_free_tier_model_performs_the_classification_support_job`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| let free_models = provider_config.free_models(); -> let free_models: &[String] = &[]; | `discard-free-models` | **killed** | `v1_criteria_setup::v1_1907_a_free_tier_model_performs_the_classification_support_job` |

> discard-free-models observed: panicked at crates/glasshouse/tests/v1_criteria_setup.rs:1203:5

Recorded scope limits — stated by the worker, not discovered later:
- exercises classification only, the one disposable job shape currently wired

---

### Consider cross-platform support stable only after PTY/session smoke tests pass on macOS, Linux, and native Windows CI runners. (line 1908)

Contract: Cross-platform support is stable only after PTY/session smoke tests pass on macOS, Linux, and native Windows CI runners.

State: PARTIALLY VERIFIED — ruled 2026-08-31 by the orchestrator, agreeing with the worker's `open`: the line's own words require macOS, Linux AND native Windows CI runners. The macOS smoke leg is green locally (quoted in the report); the Linux and native-Windows CI legs are the missing clauses. Not a refusal: closes on CI evidence once the runners are available (the local gate has been the mirror until September per practice §27); ~~pushing for CI is the orchestrator's standing job.~~

**STALE as of 2026-09-01, and it now contradicts a user ruling — do not act on
it.** The user instructed: *"this projects CI is way too demanding to be ran on
github fully … only run in the github CI in the future when absolutely
necessary"*, then *"if ci is good on this machine then just skip it on github."*
`.github/workflows/ci.yml` is therefore **`workflow_dispatch` only**; a push
fires nothing. **Pushing for CI is no longer anyone's standing job**, and an
orchestrator who reads the sentence above and dispatches the matrix is spending
a monthly allowance the user asked to conserve.

**This leaves 1908 needing a ruling, not a package, and the ruling is the
user's** because it trades a map line's literal words against their own cost
instruction. Two defensible readings:

- **The gate is the gate.** `scripts/ci-local.sh --macos --linux --windows-vm`
  runs macOS natively, Linux in a container, and drives a **real Windows VM**.
  That is an automated three-platform smoke gate; the line's intent — do not
  call cross-platform support stable until all three are exercised — is
  satisfiable today, on this machine, at no cost.
- **"CI runners" means hosted CI.** Then the line needs a GitHub Actions run on
  all three, which is exactly what the user restricted, and the line cannot
  close without spending that allowance.

**What to do:** do not reinterpret the map line unilaterally, and do not
dispatch the GitHub matrix to satisfy it. Put the choice to the user. If they
take the first reading, the evidence path is one clean
`ci-local.sh --macos --linux --windows-vm` run — **on a quiet machine**, since
concurrent `cargo` load makes that gate report its own CPU contention as a
Linux PTY failure.

Regression evidence:
- `pty_smoke (71 tests, macOS only, this worktree)`

Recorded scope limits — stated by the worker, not discovered later:
- no Linux or native Windows CI run exists in any evidence-ledger entry this packet's READ ONLY THIS names; this worktree cannot produce either

---

---

## Line 1908 — OPEN, and for the first time on evidence rather than absence (2026-09-02)

**The user's ruling:** *"Local ci-local.sh run with --windows-vm satisfies
1908 -- close it on that evidence."* So the three-leg run
`scripts/ci-local.sh --macos --linux --windows-vm` is the runner the line
names, and the line ticks on a green one.

**The run, on `2edce1b`:**

| leg | result |
|---|---|
| macOS lint (fmt, clippy, rustdoc, doc boundary, evidence coverage, script tests) | PASS; README progress was stale (regenerated since) |
| macOS test | 4 probe timeouts in `shell::settings_persistence_tests`, on a machine running four worker builds — **18/18 twice alone**, load |
| Linux build+test, clippy, MSRV | PASS |
| **Windows build, test, MSRV** | **FAIL — does not compile.** `dfaf27f` added `api::mute`/`unmute` calls to `main.rs:622/625` while `api/mod.rs` exports them only under `cfg(unix)`; plus a `cfg(unix)` test helper called unconditionally and unused-import errors under `-D warnings` in fourteen test files |

**`GH-WINDOWS-TEST-BUILD` (Opus 5 high, Red) fixed the build**: `cfg(not(unix))`
twins for `mute`/`unmute` returning the existing `no_unix_socket()` refusal —
a missed case in a pattern `api/mod.rs` already had for `serve`, `send_message`,
`interrupt`, `read_output`; every test straggler gated to the cfg its only user
carries, no `#[allow]` anywhere. Mutation KILLED by the cross-check itself (a
compile error is the observable). On the real ARM64 VM: `build` exit 0, `msrv`
exit 0, and the new `#[cfg(windows)]` refusal test observed passing.

**The Windows test leg then ran for the first time ever, and seven tests fail
in three families**, identical across three runs:

- **A — "the fake harness never exited"** (4): `shell::shell_entitlement_scrub_tests::…never_carry…`
  and three in `tests/entitlement_shell_scrub.rs`, all at a 20-second
  `poll_exits` deadline. A session/process-exit-observation defect on Windows,
  or the `.cmd` fixture never exiting. **Red packet: `GH-WINDOWS-EXIT-OBSERVATION`.**
- **B — firewall hook registration count** (2): `tests/firewall_bridge.rs:418`
  and `:504`, `left: 0, right: 1`. Undiagnosed; Amber.
- **C — an unescaped Windows path in a test fixture's TOML** (1):
  `tests/tier_ceiling.rs:668`, `C:\Users` read as a unicode escape.
  `session_supervision.rs:57` already escapes; this fixture does not. Green,
  cheapest.
- Plus **about one flake per full run, a different test each time** — runs 1
  and 2 ran byte-identical binaries and disagreed. The rate is the finding.

**The finding that outranks the failures: the Windows runner can run stale
binaries and report them as the tree's result.** `install-source.ps1`
extracts with `tar`, which restores archive mtimes, and `C:\ci\target`
persists, so a changed source file older than a previous artifact is judged
fresh — two consecutive `test` runs compiled nothing (`0.58s`, `0.47s`) and
silently did not run the test the packet had just added. One `touch` made the
VM recompile and report 65 tests instead of 64. Fixed the same day in the
runner (stamp the extracted tree to *now*); until a run proves the stamp, any
Windows-only evidence from this leg must be read with §16's question — *did
it compile the tree it reports on?*

**State: NOT STARTED → SCAFFOLDED** for the Windows leg (the build exists and
is green; the smoke tests do not pass). The tick needs a three-leg green run on
a merged tree after the three families close.

---

## Families B and C closed on the VM — 2026-09-02 (`GH-WINDOWS-FIXTURES`, Amber, Sonnet medium); 1908 still open

Both were the test fixtures' own defects, and no production line changed:

- **Family C** (`tests/tier_ceiling.rs`): the calibration test interpolated
  a Windows path into TOML unescaped, so `C:\Users` read as a broken
  `\u` escape. Escaped the way `session_supervision.rs:57` already does.
  Mutation on the VM: revert the escape → the exact original
  *too few unicode value digits* error returns. KILLED.
- **Family B** (`tests/firewall_bridge.rs`): the packet's Phase −1 read the
  wrong assertion — both `left: 0, right: 1` failures are
  `harness_invocations().len()`, the fake harness's own argv log, not the
  settings-document count. Root cause, reproduced by hand on the VM with the
  byte-identical script: `install_fake_claude`'s `.cmd` interpolates
  `"2.1.252 (Claude Code)"` inside a parenthesised `if (...)` block, and
  cmd.exe's block parser eats the literal `)`, so the fake harness never
  ran to completion and never wrote its log. Fix: escape `(`/`)` as
  `^(`/`^)` in the Windows branch. Not product behaviour.

VM evidence: run 1 (Family C only) — `tier_ceiling` 8/8, `firewall_bridge`
still red, as expected; runs 2 and 3 (both fixes) — 8/8 and 11/11, twice.
Every other red in those runs was Family A (`GH-WINDOWS-EXIT-OBSERVATION`,
in flight) or the known roughly-one-flake-per-run (`evaluation_producers`,
`gateway_failure_taxonomy`, `v1_criteria_routing` — a different test each
run, none regressing). Cross-check `cargo check --tests --target
x86_64-pc-windows-gnu` clean. **Operational finding:** the runner refuses a
busy VM tree (*Could not replace CI source tree*) rather than clobbering it,
so two concurrent runs fail fast instead of corrupting each other; it still
has no queue.

**1908 stays open** until the three-leg `ci-local.sh --macos --linux
--windows-vm` is green on a tree carrying both Windows packages.
