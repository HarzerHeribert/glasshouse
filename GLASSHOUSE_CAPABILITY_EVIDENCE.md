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

## Active reconciled example

### Phase 1 — Ensure every spawned harness process starts with its working directory set to the current project root

Contract: Whenever Glasshouse invokes an installed harness—including discovery
probes and interactive sessions—the child starts in the active canonical
project root and never inherits an unrelated caller directory.

State: PARTIALLY VERIFIED

Production evidence:

- `integrations::Discovery::run(&Project)` threads the active project into the
  real `version::probe_version` production subprocess.
- `version::probe_version` sets `Command::current_dir` from
  `Project::display_root`.
- `launch::HarnessLaunch::spawn` reaches PTY spawn through project-bound
  `TerminalCommand::for_harness`, but currently lacks a production interactive
  session consumer.

Regression evidence:

- `version_probe_child_starts_in_the_active_project_root` uses a resolved fake
  probe that prints a version only in the correct child directory.
- `a_fake_installed_harness_launches_inside_the_discovered_project_root` proves
  the PTY launch mechanism by filesystem identity.
- Windows-only drive/UNC prefix tests pin `strip_verbatim_prefix` behavior.

Failure/isolation evidence:

- The version-probe test fails with no parsed version if the child inherits the
  test runner's directory.
- Unsafe Windows-script arguments are rejected before `HarnessLaunch` spawns.

Platform/external evidence:

- Local macOS gates are recorded in `GLASSHOUSE_HANDOFF.md`.
- Earlier baseline CI predates the current commits and is not evidence for the
  new Windows branches.

Missing evidence:

- A correct reachable production interactive-session consumer of
  `HarnessLaunch`, including I/O, DSR, exit, signal, and terminal-restoration
  lifecycle.
- Current-commit `windows-latest` execution of the `.cmd` and verbatim-path
  tests.

The authoritative Phase 1 checkbox therefore remains unchecked.
