# Pane competitive capability checklist

This document compares Pane with Claude Code and Codex at the harness and
everyday-workflow levels. It is deliberately a checklist rather than a product
claim: each Pane row should be verified independently and linked to concrete
source, test, or live-harness evidence before its status is upgraded.

The Claude Code and Codex columns are the current comparison baseline supplied
for this audit; they are not independently verified by this document.

## Implementation policy

Target equivalent user outcomes, not identical command names or screens.
Prefer Pane's cells, handles, events, and existing TUI where they already solve
the workflow. Reuse standard Git, MCP, and HTTP interfaces instead of inventing
parallel protocols. A Pane-specific implementation still needs the same
permission, cancellation, recovery, and evidence guarantees; a documented
missing outcome remains a gap, not a design exception.

## Status legend

| Mark | Meaning |
|---|---|
| ✅ | Implemented and presently evidenced |
| 🟡 | Partial, conditional, or materially inconsistent |
| ❌ | Missing |
| ⬜ | Not yet verified |
| 🛠 | Implementation and tests written; consolidated checks pending |

## Sandbox, network, and platform capabilities

| Capability | Claude Code | Codex | Pane current status | Pane evidence or limitation |
|---|---|---|---|---|
| OS-level command sandbox | ✅ | ✅ | 🟡 | Seatbelt / Landlock + inherited seccomp / AppContainer. Native platform guarantees differ. [sandbox_apply](../../../crates/pane/tests/sandbox_apply.rs), [linux_network_isolation](../../../crates/pane/tests/linux_network_isolation.rs). |
| Workspace write restriction | ✅ | ✅ | 🟡 | Project containment exists; active Linux Landlock cannot carve .claude/.pane out of a writable project for arbitrary admitted shell code. Startup warns about future-session config mutation. [sandbox_apply](../../../crates/pane/tests/sandbox_apply.rs), [settings_security](../../../crates/pane/tests/settings_security.rs). |
| Read outside workspace | configurable | generally yes, writes restricted | 🟡 | New host-selected --add-dir grants exact canonical subtrees on macOS/Linux; credential roots remain denied. Windows and arbitrary deny-pattern combinations refuse. [additional_roots](../../../crates/pane/tests/additional_roots.rs). |
| Additional writable roots | ✅ | ✅ | 🟡 | --add-dir is wired through profile checks and macOS/Linux process grants; Windows and configured filesystem-deny combinations refuse. [additional_roots](../../../crates/pane/tests/additional_roots.rs). |
| Network isolation | ✅ | ✅ | 🟡 | macOS denies network; Linux x86_64/aarch64 now deny socket operations via inherited seccomp (host namespace remains); Windows requires Firewall. [linux_network_isolation](../../../crates/pane/tests/linux_network_isolation.rs). |
| Domain allow/deny lists | ✅ | ✅ | ✅ | [web] allow_domains/deny_domains govern broker requests, redirects, DNS, and remote MCP. They do not grant shell egress. [web_capabilities](../../../crates/pane/tests/web_capabilities.rs), [remote_mcp](../../../crates/pane/tests/remote_mcp.rs). |
| Escalate outside sandbox | approval | approval | ❌ | Interactive approval preserves existing grants. No outside-sandbox escalation path has been implemented. |
| Completely unrestricted mode | skip permissions | full access | 🟡 | --yolo remains broad project/Bash access within confinement. True unrestricted execution is not implemented. |
| Shell subprocess inheritance | sandbox propagates | sandbox propagates | 🟡 | Platform confinement is inherited. New Linux tests exercise a forked child; full platform and descriptor inheritance guarantees require native evidence. [linux_network_isolation](../../../crates/pane/tests/linux_network_isolation.rs), [sandbox_apply](../../../crates/pane/tests/sandbox_apply.rs). |
| Web search/fetch | native capabilities | native/configurable | ✅ | web.search/query and web.fetch/URL work through the host broker. Search requires a configured SearXNG-compatible endpoint; fetch returns bounded source text/HTML. [competitive_web_runtime](../../../crates/pane/tests/competitive_web_runtime.rs), [web_capabilities](../../../crates/pane/tests/web_capabilities.rs). |
| MCP/external tools | ✅ | ✅ | 🟡 | Local stdio plus new Streamable HTTP (JSON/finite POST SSE), using web policy. No legacy SSE, OAuth discovery, resumable streams, or server-initiated requests. [project_mcp](../../../crates/pane/tests/project_mcp.rs), [remote_mcp](../../../crates/pane/tests/remote_mcp.rs). |
| Approval policy | ✅ | ✅ | 🟡 | New --ask-approval foreground TUI supports exact once/session/deny; human wait pauses compute deadline. It cannot widen grants and does not cover background/subagent calls. [approval_boundary](../../../crates/pane/tests/approval_boundary.rs), [tui_live](../../../crates/pane/tests/tui_live.rs). |
| Cross-platform | macOS/Linux | macOS/Linux/Windows | 🟡 | All three appliers exist; additional roots, network enforcement, and command compatibility remain unequal. New work needs native platform validation. |

## Everyday coding-agent capabilities

Pane entries distinguish tested behavior from architecture, intent, and
untested integration assumptions. Platform and workflow limitations are
recorded even when a lower-level building block is implemented.

| Everyday capability | Claude Code | Codex | Pane current status | Importance | Pane evidence or acceptance note |
|---|---|---|---|---:|---|
| Interactive terminal TUI/REPL | ✅ | ✅ | ✅ | **P0** | [tui_live](../../../crates/pane/tests/tui_live.rs): interactive composer, live provider turns, controls, rendering. |
| Start with plain `pane` in repo | ✅ | ✅ | ✅ | **P0** | Bare pane starts current-root TUI; top-level options now default --root to dot. [entrypoint](../../../crates/pane/tests/entrypoint.rs), [cli_workflows](../../../crates/pane/tests/cli_workflows.rs), [tui_live](../../../crates/pane/tests/tui_live.rs). |
| One-shot/headless invocation | ✅ `-p` | ✅ CLI/noninteractive | ✅ | **P0** | pane -p TASK / pane exec TASK use the existing --task loop and exit status. [cli_workflows](../../../crates/pane/tests/cli_workflows.rs), [session_output](../../../crates/pane/tests/session_output.rs). |
| Resume previous session | ✅ `--continue`, `--resume` | ✅ threads/sessions | ✅ | **P0** | --resume [ID], --continue, persisted rollout/checkpoint and refreshed instructions. [session](../../../crates/pane/tests/session.rs), [cli_workflows](../../../crates/pane/tests/cli_workflows.rs). |
| Session picker/history | ✅ | ✅ | 🟡 | **P0** | --sessions lists history and --resume selects it. No interactive history picker or retention/delete workflow. [session](../../../crates/pane/tests/session.rs), [cli_workflows](../../../crates/pane/tests/cli_workflows.rs). |
| Persistent project instructions | ✅ `CLAUDE.md` | ✅ `AGENTS.md` / instructions | ✅ | **P0** | Root AGENTS.md/CLAUDE.md are delivered and refreshed. [scoped_instructions](../../../crates/pane/tests/scoped_instructions.rs), [session](../../../crates/pane/tests/session.rs). |
| Global user instructions | ✅ | ✅ | ✅ | **P0** | Host-owned XDG_CONFIG_HOME/pane/AGENTS.md or ~/.config/pane/AGENTS.md, loaded whole within bounds. [competitive_project_workflows](../../../crates/pane/tests/competitive_project_workflows.rs). |
| Nested/per-directory instructions | ✅ | ✅ | ✅ | **P1** | Applicable directory instructions are delivered before tool access, including writes/renames. [instruction_boundary](../../../crates/pane/tests/instruction_boundary.rs), [scoped_instructions](../../../crates/pane/tests/scoped_instructions.rs). |
| Automatic codebase exploration | ✅ | ✅ | 🟡 | **P0** | Orientation, source-context selection, and optional helper preflight exist. Autonomous quality needs representative task benchmarks. [environment_orientation](../../../crates/pane/tests/environment_orientation.rs), [source_context](../../../crates/pane/tests/source_context.rs). |
| Read/edit/write files directly | ✅ | ✅ | ✅ | **P0** | Built-in `read`, `write`, and `edit` tools are registered. |
| Search code/files | ✅ | ✅ | ✅ | **P0** | Built-in `glob`, `grep`, `rg`, and `fd` tools are registered. |
| Run shell commands | ✅ | ✅ | ✅ | **P0** | Built-in `bash` is permission-controlled and sandboxed. |
| Run tests/build/lint automatically | ✅ | ✅ | ✅ | **P0** | Available through admitted shell commands; benchmark autonomous selection and recovery separately. |
| Git diff/status/log | ✅ | ✅ | ✅ | **P0** | Uses standard admitted Git shell commands, with a new sandboxed status/diff/log workflow test. [competitive_git](../../../crates/pane/tests/competitive_git.rs). |
| Commit changes when asked | ✅ | ✅ | ✅ | **P0** | Uses admitted Git commands; new regression covers local identity, staging, commit, and clean status. No separate Git API. [competitive_git](../../../crates/pane/tests/competitive_git.rs). |
| Create PR / GitHub workflow | ✅ via `gh`/MCP | ✅ via tools/plugins | 🟡 | **P1** | Can use an explicitly configured remote GitHub MCP server through Streamable HTTP. No built-in gh network/auth workflow or tested GitHub PR acceptance yet. |
| Show clean diffs in UI | ✅ | ✅ strong diff UI | ✅ | **P0** | Bounded unified changes in compact/expanded notebook views. [tui_look](../../../crates/pane/tests/tui_look.rs); changes.rs snapshot/diff tests. |
| Review proposed edits before accepting | ✅ | ✅ | 🟡 | **P0** | --ask-approval shows complete checked write/edit arguments before execution; no dedicated before/after diff selector. [competitive_approvals](../../../crates/pane/tests/competitive_approvals.rs), [tui_live](../../../crates/pane/tests/tui_live.rs). |
| Undo/revert agent changes | ✅ workflows/checkpoints | ✅ diff/revert workflows | ✅ | **P0** | changes.rs snapshots and guarded rollback_plan/apply preserve later user edits; /rollback previews then confirms. Rollback is session-local. |
| Plan/read-only mode | ✅ `permission-mode plan` | ✅ read-only / plan workflows | ✅ | **P0** | Existing /mode/Shift-Tab plan executes no code; --plan now selects it at startup. [tui_live](../../../crates/pane/tests/tui_live.rs). |
| Effort/reasoning control | ✅ | ✅ | ✅ | **P0** | Parent effort and per-helper defaults are configurable; the UI labels inherited provider behavior as `default`. |
| Model switching in-session | ✅ | ✅ | ✅ | **P0** | `/models` supports staged model selection by agent tier; retain live UX evidence. |
| Context/token visibility | ✅ some visibility/status | ✅ usage/status | ✅ | **P1** | Context, parent/helper/cache usage and live status rendered by tui/telemetry.rs. [tui_live](../../../crates/pane/tests/tui_live.rs). |
| Cost visibility | varies by auth | ✅ depending on API | 🟡 | **P1** | Token/cache telemetry exists; native monetary cost can remain 'cost unreported'. Gateway metadata is not a universal price estimate. |
| Automatic context compaction | ✅ | ✅ | ✅ | **P0** | Core Pane capability; verify quality and thresholds in competitive tasks. |
| Continue seamlessly after compaction | ✅ | ✅ | ✅ | **P0** | Projected checkpoints retain live handles, task state and instructions. New image overflow regression also preserves current attachments. [session](../../../crates/pane/tests/session.rs), [image_input](../../../crates/pane/tests/image_input.rs). |
| Subagents/delegation | ✅ native | ✅ native | ✅ | **P1** | agent.run starts a separate turn loop and returns a handle; completion arrives as an event. Distinct from Little Helpers. [subagent](../../../crates/pane/tests/subagent.rs). |
| User-created custom agents | ✅ | ✅ skills/subagents | ✅ | **P1** | .pane/agents/NAME.toml instructions/model/effort, runtime snapshots and explicit option overrides; no added grants. [custom_agents](../../../crates/pane/tests/custom_agents.rs). |
| Parallel agents/tasks | ✅ subagents/background | ✅ major workflow | ✅ | **P1** | agent.run uses background threads and returns immediately; event delivery integrates results. Shared workspace, no automatic worktree isolation. [subagent](../../../crates/pane/tests/subagent.rs). |
| Background commands/tasks | ✅ | ✅ | ✅ | **P1** | bg.run/watch/cancel, payloads, bounded lifecycle and teardown. Jobs are task-scoped and are not resumed. [events](../../../crates/pane/tests/events.rs), [session](../../../crates/pane/tests/session.rs). |
| To-do/task-plan tracking | ✅ | ✅ explicit todo/progress | ✅ | **P1** | todo.write/read shows pending/active/done state across cells and checkpoints; resets after the task. [runtime_cells](../../../crates/pane/tests/runtime_cells.rs), [prompt_bytes](../../../crates/pane/tests/prompt_bytes.rs). |
| Progress updates during long work | ✅ | ✅ | ✅ | **P1** | Provider/text/tool deltas, live helper states, elapsed clocks and task snapshots. [tui_live](../../../crates/pane/tests/tui_live.rs). |
| Web search | ✅ | ✅ | ✅ | **P0** | web.search returns structured hits/citations from an explicitly configured search endpoint. [web_capabilities](../../../crates/pane/tests/web_capabilities.rs), [competitive_web_runtime](../../../crates/pane/tests/competitive_web_runtime.rs). |
| Web fetch/read URL | ✅ | ✅ | ✅ | **P0** | web.fetch returns bounded text/HTML/JSON/XML plus final URL and untrusted-content marker. [web_capabilities](../../../crates/pane/tests/web_capabilities.rs), [competitive_web_runtime](../../../crates/pane/tests/competitive_web_runtime.rs). |
| Image/screenshot input | ✅ model-dependent | ✅ explicit CLI support | ✅ | **P1** | Repeat --image PATH (up to four); bounded PNG/JPEG/GIF/WebP blocks survive wire, rollout and overflow. Model must support images. [image_input](../../../crates/pane/tests/image_input.rs). |
| Paste image from clipboard | ✅ depending on terminal/client | ✅ supported UX | ❌ | **P2** | Text paste exists; clipboard image ingestion is not implemented. Attach a saved screenshot with --image. |
| Browser debugging / browser use | limited/product-dependent | ✅ desktop/browser tooling | 🟡 | **P2** | Remote MCP can connect a browser-tool server, but no bundled browser/CDP implementation or verified browser workflow exists. |
| MCP local stdio | ✅ | ✅ | ✅ | **P1** | `.mcp.json`, discovery, and invocation are implemented for stdio servers. |
| MCP remote HTTP/SSE | ✅ | ✅ | 🟡 | **P1** | New Streamable HTTP with JSON/finite POST SSE; no legacy SSE GET/reconnect, OAuth, or server-initiated requests. [remote_mcp](../../../crates/pane/tests/remote_mcp.rs). |
| Discover MCP tools dynamically | ✅ | ✅ | ✅ | **P1** | `mcp.list()` discovers permission-admitted tools and their schemas. |
| Custom slash commands | ✅ | ✅ command/workflow equivalents | ✅ | **P1** | .claude/commands/NAME.md expands to a user task with literal arguments. Built-ins win collisions. [session](../../../crates/pane/tests/session.rs), [project](../../../crates/pane/tests/project.rs). |
| Skills/reusable workflows | ✅ | ✅ | ✅ | **P1** | Explicit /NAME invokes .claude/skills/NAME/SKILL.md through normal task execution and permissions. [competitive_project_workflows](../../../crates/pane/tests/competitive_project_workflows.rs), [project](../../../crates/pane/tests/project.rs). |
| Hooks around tool/session lifecycle | ✅ strong feature | configurable rules/automation | 🟡 | **P1** | Glasshouse lifecycle/context-firewall hooks and Pane event handlers exist. No general user-configured hook format. [seams](../../../crates/pane/tests/seams.rs), [standing_handlers](../../../crates/pane/tests/standing_handlers.rs). |
| Shell command allow rules | ✅ | ✅ | ✅ | **P0** | `Bash(...)` allow/deny admission exists, with deny precedence and segmented command checks. |
| Interactive permission prompts | ✅ | ✅ | 🟡 | **P0** | --ask-approval ships a TUI for already-admitted foreground calls. Hard denies/missing grants do not prompt. [tui_live](../../../crates/pane/tests/tui_live.rs). |
| Allow once / session | ✅ | ✅ | ✅ | **P0** | O once / S exact action for session / D deny. Cancellation, display bounds and human-wait deadline accounting covered. [competitive_approvals](../../../crates/pane/tests/competitive_approvals.rs), [approval_boundary](../../../crates/pane/tests/approval_boundary.rs), [tui_live](../../../crates/pane/tests/tui_live.rs). |
| Full autonomous/yolo mode | ✅ | ✅ | 🟡 | **P0** | `--yolo` broadens project and Bash access but remains sandboxed and is not true full access. |
| Additional working directories | ✅ `--add-dir` | ✅ writable roots/workspaces | 🟡 | **P1** | --add-dir grants explicit canonical macOS/Linux roots while preserving credential denials; Windows and filesystem-deny combinations refuse. [additional_roots](../../../crates/pane/tests/additional_roots.rs). |
| Pipe stdin into agent | ✅ | ✅ | ✅ | **P1** | pane exec reads all stdin as one task. Ordinary sessions retain one-turn-per-line input. [cli_workflows](../../../crates/pane/tests/cli_workflows.rs). |
| JSON/machine-readable output | ✅ | ✅ | ✅ | **P1** | --output-format json produces schema_version=1 result/events on clean stdout; the final result includes per-model parent/helper token classes, request coverage, every preflight helper, wall time, cells and tool failures. Diagnostics stay on stderr. [session_output](../../../crates/pane/tests/session_output.rs). |
| Stream JSON/events for integrations | ✅ | ✅ | ✅ | **P1** | --output-format stream-json emits ordered typed session/message/cell/helper/result events, including preflight helper records and the same final telemetry. Not token deltas. [session_output](../../../crates/pane/tests/session_output.rs). |
| IDE integration | ✅ VS Code/JetBrains ecosystem | ✅ VS Code-family extension | ❌ | **P1/P2** | No bundled editor extension or IDE protocol. Terminal execution alone is not IDE integration. |
| Open current changes in IDE | ✅ integration-dependent | ✅ | ❌ | **P2** | No dedicated editor/open-diff workflow implemented. |
| CI/GitHub Actions usage | ✅ | ✅ | 🟡 | **P1** | Headless invocation, explicit model/auth and machine result contracts now exist; usage in [competitive-workflows.md](competitive-workflows.md). No dedicated Actions package or live CI benchmark yet. |
| Git worktree support | usable | ✅ important workflow | 🟡 | **P1** | Existing worktrees work as independent --root sessions. No Pane-native create/switch/merge workflow or automatic per-agent isolation. |
| Automatic update mechanism | ✅ | ✅ | ❌ | **P2** | No automatic updater implemented; installation remains external. |
| `doctor`/diagnostics command | ✅ | ✅ `codex doctor` | ✅ | **P1** | pane doctor [--json] checks local config, PATH and compiled/available sandbox support without contacting providers. [cli_workflows](../../../crates/pane/tests/cli_workflows.rs). |
| Config profiles | ✅ settings scopes | ✅ TOML/profiles | ✅ | **P1** | --profile NAME overlays [profiles.NAME] recursively; --model still wins. Unknown profiles fail. [competitive_profiles](../../../crates/pane/tests/competitive_profiles.rs). |
| Repo trust / untrusted repo handling | ✅ permissions | ✅ trust level | ❌ | **P1** | No repository-trust store or first-run trust prompt. Project configuration is consumed at session startup; sandbox policy does not substitute for a trust workflow. |
| Rich command/tool-call rendering | ✅ | ✅ | ✅ | **P0** | Typed tool outcomes, execution state, bounded output, timing and diffs in notebook. [tool_call_outcomes](../../../crates/pane/tests/tool_call_outcomes.rs), [tui_live](../../../crates/pane/tests/tui_live.rs), [tui_look](../../../crates/pane/tests/tui_look.rs). |
| Keyboard shortcuts / command palette | ✅ | ✅ | ✅ | **P1** | Slash command discovery, editor navigation, mode/effort controls and modal keys. [tui_live](../../../crates/pane/tests/tui_live.rs). |
| Copy/paste-friendly output | ✅ | ✅ | 🟡 | **P1** | Text paste/editing is tested. Native terminal selection and clipboard export remain incomplete across clients. [tui_live](../../../crates/pane/tests/tui_live.rs). |
| Multi-platform config consistency | decent | strong | 🟡 | **P1** | Shared configuration parser exists; OS grants, extra roots, executable compatibility and network enforcement remain unequal. |

## Verification record

Settings UX follow-up (implemented and verified):
[Global/Project settings, advanced config commands, and status-line persistence](settings-experience.md).
Native global/project TOML, `/settings` scope tabs, typed `/config` and CLI
commands, and persisted `/statusline` are integrated. Claude permissions are
explicit-import only; legacy Pane configuration has a preserved-source migration.
The workspace audit exercised 5,524 tests (six existing manual/live skips).
Its five failures were fixed and the affected 315-test correction gate passed,
including all 23 live-terminal cases. Native Linux's 69-test settings/sandbox
batch, workspace lint/format checks, and five documentation tests passed.
Full details and log paths are in the linked settings record. This does not
upgrade Linux inner-directory isolation or unverified Windows behavior to parity.

Implementation and new tests were written before the consolidated checks, as
requested. The initial, pre-settings macOS Pane batch passed on 2026-09-12:

```sh
cargo test -p pane --no-fail-fast
cargo clippy -p pane --all-targets -- -D warnings
cargo fmt -p pane --check
git diff --check
```

All commands passed. The full test run includes 307 library unit tests,
20 live-terminal tests, 42 rendering tests, and documentation tests. One
pre-existing wire integration test remains ignored. The first run exposed
the corrected host-Git configuration issue and a five-second hook-timing
failure; the latter passed unchanged in the final full run. Local logs are
`/tmp/pane-competitive-final-tests.log` and `/tmp/pane-competitive-clippy.log`.

Platform-specific and integration limitations remain 🟡 even when local tests
pass. This is not a full parity claim or a completed competitive benchmark.
Usage and configuration examples are in
[competitive-workflows.md](competitive-workflows.md). Release installation uses
`scripts/install-local.sh` after commit, retaining prior installed versions.

### Native Linux checks — 2026-09-12

Rust 1.98, Docker Linux aarch64, non-root UID 1000, Landlock ABI 8;
source copied into an isolated container directory, not executed on the macOS
bind mount. The initial network/sandbox batch passed 51 tests:

```sh
cargo test --locked -p pane --test linux_network_isolation --test sandbox_apply --test additional_roots --test web_capabilities --test remote_mcp -- --nocapture
```

After fixing Git's literal `/dev/null` write grant and machine-output delivery
errors, the correction batch passed 34 tests:

```sh
cargo test --locked -p pane --test competitive_git --test session_output --test sandbox_apply --test linux_network_isolation -- --nocapture
cargo test --locked -p pane --lib machine_delivery
cargo test --locked -p pane --lib failed_stream_delivery
```

The counts overlap between batches; they are not a unique-test total.
Real confined Git operations and descendant socket denial ran on aarch64.
x86_64 seccomp had structural policy coverage only; no native Windows run or
live external search/MCP service acceptance is claimed.

## Audit rule

For each Pane row, record at least one of the following before changing ⬜ or
🟡 to ✅:

1. A focused automated test that exercises the user-visible behavior.
2. A live-harness transcript or screenshot with the exact build identifier.
3. A source reference plus a negative/edge-case test when the capability is a
   security boundary.

A capability is not complete merely because it can be approximated through an
unrestricted shell. The documented Pane workflow, permissions, rendering,
failure behavior, and cross-platform contract are part of the capability.
