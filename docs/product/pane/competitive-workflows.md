# Pane everyday workflows

These commands describe the capability-checklist implementation.
The comparison checklist records verification status and remaining limitations.

## Interactive and scripted sessions

For scoped preferences use `/settings`, or `pane config global|local <key> <value>`.
Pane owns `.pane/config.toml` and user-level `pane/config.toml`, including
permissions. `/statusline` offers a persisted layout picker. Legacy Pane and
Claude configuration imports are explicit and previewable; see
[settings experience](settings-experience.md) for the migration and timing rules.

```sh
pane
pane -p "Find the cause of the failing test"
pane exec "Fix the failing test" --output-format json
git diff | pane exec --output-format stream-json
pane --continue
pane --resume SESSION_ID
pane --sessions
pane doctor --json
pane --plan
pane --ask-approval
pane -p "Explain this screenshot" --image screenshot.png
pane --add-dir ../shared-library
pane exec "Run the benchmark task" --full-access
```

`exec` without a task reads all stdin as one task. For positional tasks, place the task
immediately after `exec`, before session flags. The `-p`/`--print` and
`--continue` shortcuts must be the first argument after `pane`; session flags
follow them. For flags-first invocations, use `pane --root PATH --task TASK`
or `pane --root PATH --resume`.

Ordinary piped sessions retain
their existing one-turn-per-line behavior. `json` returns one document;
`stream-json` returns ordered JSONL events, each with `schema_version: 1`, `type`,
and `sequence`. A final `result` reports `success`, `answer`, `error`, and a
typed `telemetry` object. That object records command wall time; executed and
failed cells; main-cell tool calls and failures; provider request, response,
unanswered, and usage-report coverage; and cumulative input, output,
cache-read, and cache-creation tokens split between the parent and helpers and
again by model. Every preflight helper invocation is retained in
`telemetry.preflight_helpers`; stream output also emits it as a `helper` event
with `call_site: "preflight"`. Missing provider usage remains visible through
the coverage counters instead of becoming a measured zero. Machine
output requires a single task. Diagnostics go to stderr, and failures exit
nonzero. Normal task/startup/provider failures produce a result; argument-parser
errors and forced process termination can occur before that result is emitted.
The event stream includes typed messages, completed cells, and completed helper
invocations; it is not a token-by-token provider stream. Tool counts describe
the parent cell trajectory. A helper's internal observations remain in that
helper record rather than being relabelled as parent tool calls.
Failure to write or flush machine output also produces a nonzero exit.

`--plan` starts in the existing planning mode, where model code and tools do not
execute. Switch modes with `/mode` in an interactive session.

`--ask-approval` requires a live terminal. Before an admitted foreground file/shell tool
call executes, the modal shows its exact checked arguments. Press **O** for
once, **S** to remember that exact action for the session, or **D**/**Escape** to
deny it. Paste and Enter cannot approve. Oversized actions cannot be approved
through a truncated preview. Hard denials remain denials; this mode does not
grant additional filesystem access or escape the sandbox. Web, MCP, background
jobs and subagents are outside this approval gate. Human waiting pauses the
compute deadline without resetting time already used; each prompt expires
after ten minutes, and cancellation removes stale prompts.

`--image` attaches up to four PNG/JPEG/GIF/WebP files, each at most 5 MiB.
Attachments follow the first actual task, survive resume and compaction, and
require a model that accepts image blocks. Machine events include image media
types rather than base64 contents. Clipboard image paste is not implemented.

`--add-dir` grants read/write access to an existing canonical directory for the
session, resolving relative paths from the main project. It preserves
credential exclusions and does not grant access to siblings. macOS/Linux
support is implemented; Windows and combinations with configured filesystem
deny patterns refuse explicitly. Linux's existing inner-directory write-deny
limitations still apply. This flag does not modify the saved permission file.

`--full-access` is one flag for the widest session: the `--yolo` admission
profile, the `full` rung, and no OS confinement of Pane's own. It is accepted
on macOS, Linux and Windows. `--dangerously-bypass-os-sandbox` remains the
name of that third half alone; it cannot be saved in configuration, never
activates as a fallback, and is rejected without `--yolo`. Pane keeps command/path admission and credential
stripping but does not install Landlock/seccomp for child tools; startup and
tool results say that the surrounding container or VM is now the only process
boundary. Do not use this mode for an ordinary host session.

## Git inside the sandbox

Use ordinary `git status`, `git diff`, `git log`, and `git commit` through
admitted shell commands. Shell descendants default `GIT_CONFIG_GLOBAL` and
`GIT_CONFIG_SYSTEM` to the platform null device: host configuration and
credential helpers are not implicitly imported, and unreadable home config
does not break local Git operations. Repository configuration and explicit
`git -c user.name=... -c user.email=... commit ...` remain available.
This does not grant network access for push, fetch, or GitHub authentication.

## Web access

Configure `.pane/config.toml` (or global Pane configuration):

```toml
[web]
enabled = true
allow_domains = ["docs.example.org", "search.example.org"]
deny_domains = ["private.docs.example.org"]
search_endpoint = "https://search.example.org/search"
max_response_bytes = 1048576
timeout_seconds = 20
```

Use a real SearXNG-compatible endpoint that returns JSON search results. Pane
does not choose a search provider or supply credentials automatically. Omit
`search_endpoint` for fetch-only access. An empty `allow_domains` permits public
domains; `*.example.org` matches subdomains, while a bare name matches exactly.
Deny wins. HTTPS is required unless `allow_http = true` is explicitly set.

Inside a model cell:

```typescript
const results = web.search("query");
const page = web.fetch(results.results[0].url);
return {citation: page.citation, excerpt: page.content.slice(0, 1000)};
```

Requests and redirects are checked against domain policy. The actual connection
resolver rejects private, loopback, link-local, and other nonpublic addresses.
Ambient HTTP proxies and credentials are not used. Fetch returns bounded UTF-8
text, HTML, JSON, or XML with its source URL and an untrusted-content marker.
HTML is returned as source, without browser execution or article extraction.
Cancellation stops waiting and prevents subsequent redirect requests; an
already dispatched request can finish within its bounded transport timeout.

Web policy governs the host broker; it does not grant network access to shell
commands. Linux confinement now adds inherited seccomp restrictions to
Landlock: socket operations and alternate socket interfaces are refused on
x86_64/aarch64. The host network namespace remains present. Local Unix-socket
build daemons are also unavailable under this conservative policy.

## Remote MCP

The existing `mcp.list()` / `mcp.call()` cell interface now also supports
Streamable HTTP servers. Configure `.mcp.json` with a server such as:

```json
{"mcpServers":{"remote":{"type":"http","url":"https://tools.example.org/mcp"}}}
```

The server's tools still need `mcp__remote__TOOL` allow rules in the session's
permission profile, and `[web]` must enable and admit the endpoint's domain.
Optional headers may reference values supplied explicitly in that server's
`env` object with `${NAME}`; ambient environment secrets are never imported.
Do not commit credential values to a shared repository.

The transport implements the MCP 2025-03-26 Streamable HTTP request path with
JSON or bounded finite SSE POST responses and session headers. It refuses
redirects and does not automatically retry effectful requests. Legacy SSE GET
transport, OAuth discovery, resumable streams, server-initiated requests and
automatic session recreation are not implemented. Cancellation stops queued
dispatch; effects already submitted to a remote server cannot be undone.

## Instructions, skills, and agent templates

Global instructions are loaded from `$XDG_CONFIG_HOME/pane/AGENTS.md`, or
`~/.config/pane/AGENTS.md` when XDG configuration is absent. Project and nested
`AGENTS.md`/`CLAUDE.md` instructions retain their existing scope handling.
Global documents are loaded whole within a 64 KiB limit; an oversized or invalid
document produces an explicit diagnostic.

Project commands in `.claude/commands/NAME.md` are invoked with `/NAME`.
Skills in `.claude/skills/NAME/SKILL.md` are also invoked with `/NAME arguments`.
Skill arguments remain literal text, and a skill grants no additional
permissions. Its relative references resolve from its own directory.

Custom agent templates live in `.pane/agents/NAME.toml`:

```toml
instructions = "Review the supplied change. Explain defects using source evidence."
model = "your-concrete-model-id"
effort = "high"
```

Select one with `agent.run(task, {profile: "NAME"})`. Explicit `model` and
`effort` options override the template. Templates use the existing subagent
mode, depth, cancellation, and sandbox rules. Their definitions are captured
when the runtime is created.

## Named configuration profiles

```toml
[model]
parent = "your-normal-model-id"

[profiles.review.model]
parent = "your-review-model-id"

[profiles.review.helpers.effort]
check = "high"
```

Start with `pane --profile review`. The selected table overlays base settings
recursively. Unknown profile names fail explicitly; selecting one does not edit
the base configuration. `--model` remains the highest-priority startup model
override.

## CI baseline

Install Pane and its configured provider/gateway dependencies in the CI image.
Supply provider authentication through the job's secret environment, choose an
explicit model, and run `pane exec TASK --output-format json`. Capture stdout as
the result artifact and stderr as diagnostics. Check both the process exit code
and the result's `success` field. Preserve the final `telemetry` object rather
than reconstructing usage from prose or a session total: its per-model parent
and helper rows are the evidence for mixed-model runs. A successful agent exit
is not a substitute for independently running the repository's tests.

Use the same clean worktree, task, model, permission settings, and external
fixtures for every comparative benchmark. Record missing capabilities as
failures or exclusions by name, never as successful empty responses.
