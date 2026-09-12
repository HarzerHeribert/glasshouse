# Pane project context

## Current workflow additions (2026-09-12)

The implementation now loads global user instructions from
`$XDG_CONFIG_HOME/pane/AGENTS.md`, falling back to `~/.config/pane/AGENTS.md`.
This host-selected read does not grant tools access to the user's home.
Documents are complete UTF-8 text bounded at 64 KiB; failures are reported.
Project commands and complete project skills can be invoked as slash commands;
skill arguments remain literal text and permissions remain unchanged.
See [competitive workflows](competitive-workflows.md) for configuration and
invocation examples, including custom agent templates.

Native `web.fetch`/`web.search` and remote Streamable HTTP MCP now use a
separate host web broker enabled through `[web]`. Domain and public-address
checks apply to network destinations; shell processes gain no network grant.
Search requires an explicitly configured compatible endpoint. Remote MCP
supports JSON and bounded finite SSE POST responses, without redirects or
automatic replay of effectful requests. Legacy SSE GET, OAuth discovery,
server-initiated requests, and resumable streams remain unsupported.
Local stdio MCP retains the subprocess confinement described below.

`--add-dir` grants explicit additional filesystem roots on macOS/Linux, but
does not extend the nested-instruction discovery boundary outside the main
project. Windows and configured filesystem deny-pattern combinations refuse
the flag. `--ask-approval` prompts only for already-admitted foreground
file/shell calls; web, MCP, background work, and subagents are outside that
gate. See [sandbox grants](sandbox-grants.md) for exact limitations.

These additions describe current source and written tests. Their consolidated
verification is tracked in the [competitive checklist](competitive-capability-checklist.md);
this addendum does not assert that the gate has passed.

## Project instruction and context pipeline

At each user task, including a resumed task, Pane refreshes root `AGENTS.md`
and `CLAUDE.md` from the session's existing read profile. Each document is
labelled with its file and directory scope. Deeper scopes apply to their
subtrees; same-directory documents have equal scope. Settings and sandbox
grants remain fixed for the session.

A bounded task-start environment snapshot supplies platform/architecture,
UTC time, execution shell, presence or absence of common executables, absolute project root, shell cwd/reset semantics,
a scratch-path suggestion, permitted Git metadata, sorted top-level entries,
and detected project files. It runs no project executable, creates nothing,
and reports unavailable facts as unknown/refused. It does not guess installed
language versions. The snapshot is stable between inference requests.

A path-only index points to nested guidance. Before a path tool uses an
unseen scope, the runtime stops that cell, delivers the complete applicable
documents, and asks the model to continue. The blocked call and the remainder
of that cell did not run; earlier completed effects remain and are never
replayed automatically. A changed instruction document triggers a fresh
boundary. A successful instruction-file write is explicitly reported as
completed before that boundary. Subagents follow the same rules.

Bash and background shell commands are opaque, so their first call loads all
indexed guidance before spawn. This is guidance delivery, not a filesystem
security boundary: scripts can contain arbitrary paths and intermediate
changes. Generated/auxiliary directories (including `.git`, `.pane`, `target`,
`node_modules`, `.worktrees`, and `.agent-runtime`) are excluded from the index;
explicit structured paths into them still load ancestor guidance. Guidance
outside the project root is not loaded. Incomplete or oversized required
guidance stops the operation with a visible reason.

The loader caps a document at 64 KiB, a batch at 256 KiB/128 documents, and
index discovery at 10,000 entries/32 directory levels. It never presents a
truncated instruction prefix as a complete document. These are host context
bounds, not limits on the model's cell source.

Task context and appended instruction deliveries are saved as rollout
metadata, not chat messages. Resume retains visible chat and refreshes the
next task's base context. No model inference is used to collect these facts;
the supplied context still contributes to provider input tokens.

Project `.mcp.json` stdio servers are callable from cells through `mcp.list()`
and `mcp.call(name, arguments)`. Discovery returns the exact callable name,
original server/tool names, description, and JSON input schema. The model can
inspect those descriptors and retain results as ordinary handles; full MCP
content stays in the runtime while handle previews remain bounded. All MCP
calls are effectful for resume purposes, even if a server claims they are pure.
`isError: true` remains inspectable tool-result data.

Configuration uses `mcpServers: {server: {type?: "stdio", command, args?, env?}}`.
Server identifiers accept ASCII letters, digits, underscores and hyphens, with
no double underscore. Callable components escape each non-alphanumeric UTF-8
byte as `_xx`, including literal underscores, so punctuation cannot collide.
Permissions still use the original `mcp__server__tool` spelling. A matching
allow must exist before discovery starts a server; denies win, including
case-insensitive server denies. Every advertised tool and every invocation is
checked against the session's existing profile. MCP names reserved by the
registry remain unavailable; the separate host `web` API is not an MCP grant.

Stdio discovery starts lazily, after project guidance delivery, and uses the existing
OS sandbox with project cwd and network denial. Ambient credential variables
are removed; explicit project `env` entries are passed only to that server.
Server stderr and protocol diagnostics are not copied into logs or errors, and
call trajectories omit argument values. MCP discovery and invocation use the
same context-firewall PreToolUse/PostToolUse lifecycle as built-in tools,
including refusals. Discovery's pre-event precedes server startup and its
post-event carries a bounded descriptor preview. An instruction-boundary yield
runs neither the operation nor its hooks. Hook arguments are redacted and observed output is a bounded preview;
the complete result stays in its runtime handle. Processes belong to the runtime and
are killed and reaped on teardown, cancellation, timeout, or broken transport.
A cancelled/broken server is not restarted automatically, avoiding replay of
potential effects.

The original transport supports newline-delimited stdio MCP protocol 2024-11-05,
initialize/initialized, paginated tools/list, and tools/call. The current remote
transport is described in the addendum above; server-initiated requests remain
unsupported. Configuration is capped at
1 MiB/16 servers; discovery at 128 tools per server, 16 pages, 32 KiB per schema,
and 2,048 description characters. Frames are capped at 8 MiB and each exchange
has a 10-second deadline; excess content fails explicitly instead of appearing
as a complete truncated result. Windows uses the operational AppContainer
applier with the platform limitations recorded in [sandbox grants](sandbox-grants.md).

The source-context packer returns a complete small file or, for a large file,
a named definition with nearby definitions, imports, callers, and tests. It
recognizes definition boundaries in Python, Rust, JavaScript/TypeScript, Go,
and Java. JavaScript and TypeScript boundaries come from the same parser Pane
uses for cells, including JSX, decorators, template literals, and regular
expressions. Go and Java use conservative lexical scanners that exclude braces
inside comments and language string forms, including Go raw strings and Java
text blocks. Leading documentation comments, decorators, and annotations stay
with their definition.

The packer only marks a definition complete when its language-specific parser
or scanner establishes both boundaries. Malformed or ambiguous syntax uses the
bounded language-agnostic window and records an omission. The existing 1 MiB
source cap, 24,000-byte definition cap, 18 supporting-excerpt cap, traversal
cap, permission checks, version hash, and rendered delivery cap apply equally
to every supported language.
