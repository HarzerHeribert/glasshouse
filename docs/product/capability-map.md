Glasshouse — Implementation Capability Map

> This describes the product. Do not cite it as instruction for how to run a
> worker or a batch — that belongs in `docs/process/`.

Idea

Glasshouse is a lean, project-scoped control plane for existing coding-agent harnesses such as Claude Code, Codex, Antigravity, and later other compatible tools.

Glasshouse does not replace those products, hide them behind a proprietary agent loop, or turn them into invisible subagents.

Glasshouse starts and manages real native harness sessions, keeps every session directly observable and interactive, routes work between sessions and available model resources, records project-specific knowledge, and lets an orchestrator session delegate work to other first-class sessions.

The core principle is:

> **Glasshouse orchestrates agents without hiding them.**

The memory principle is:

> **Memory belongs to the project, not to the model.**

The memory-validity principle is:

> **Remember why a decision was made, but never assume an old decision is still correct merely because it was remembered.**

The implementation-quality principle is:

> **Prefer the simplest secure and scalable implementation that satisfies the current requirements; revisit stale rules instead of building complexity around them.**

The architectural-decision principle is:

> **When a phase records a fixed architectural requirement, implementations must not silently substitute a different architecture. A deviation requires an explicit specification change that records the reason, impact, and migration consequences before implementation.**

The isolation principle is:

> **One Glasshouse instance operates on exactly one project root and must not retrieve memory or session state from another project.**

Description

Running glasshouse inside a project opens a terminal UI scoped to that project.

Glasshouse embeds the native terminal interfaces of supported coding harnesses inside its own TUI, so the user can switch between Claude Code, Codex, Antigravity, or other sessions while still interacting with each product exactly as intended.

Every worker is a real session with its own native context window, provider subscription, lifecycle, and terminal interface.

One normal session can optionally be marked as the orchestrator and receive Glasshouse control tools for spawning, messaging, observing, interrupting, and resuming other sessions.

Glasshouse observes native lifecycle hooks where available, records session activity, maintains small portable checkpoints, and extracts only durable project knowledge such as decisions, constraints, findings, failed approaches, features, and open tasks.

Durable memory must still be validity-aware. A decision made during an alpha prototype, an exploratory architecture session, or an excited late-night design discussion should not automatically constrain a production implementation weeks later. Memories should retain rationale, assumptions, scope, evidence, lifecycle status, and confidence so retrieval can distinguish hard invariants from provisional choices, historical ideas, and decisions whose original assumptions no longer hold.

Routing is session-aware rather than blindly turn-based and considers task suitability, existing relevant context, prompt-cache temperature, quota availability, provider health, switching cost, expected token cost, request-pool pressure, and time until quota reset.

Glasshouse should maintain a normalized capacity view across fundamentally different resource types: subscriptions with opaque usage limits, token-metered APIs, request-limited free routers, credit pools, and unlimited local inference. Exact provider telemetry should be preferred when available; inferred values must be visibly marked as estimates and calibrated from observed usage rather than presented as fact.

Before spending premium agent capacity, Glasshouse may ask a configurable fast and cheap routing model to classify the request, estimate the required capability tier, and choose whether the work can safely be handled by a free/local resource, a low-cost model, an existing warm premium session, or a frontier model. The routing-model role must remain replaceable and may use a model such as GPT-5.6 Luna, a fast free router model, or a local model when it satisfies latency and rate-limit requirements.

cmux is an optional presentation and workspace integration rather than a core dependency; Glasshouse can embed sessions itself and may expose or spawn sessions in cmux panes when cmux is available.

The first implementation should remain deliberately small: one Rust binary, Ratatui, PTYs, SQLite, project isolation, a few harness adapters, event handling, simple memory, and a basic session-aware router.

Glasshouse should be approachable for users who already have one or more coding agents installed. On first launch it should detect supported harnesses and useful local tools, show what was found, let the user enable or ignore each integration, and offer provider/gateway configuration as an optional “configure now” or “do later” step.

Glasshouse should support macOS, Linux, and Windows where the underlying harnesses themselves are supported. Platform-specific terminal/process details should live behind abstractions so project behavior, memory, routing, and configuration remain consistent across operating systems.

Glasshouse may also provide a local gateway mode for compatible harnesses. A launch profile such as Claude / OpenRouter or Claude / Glasshouse Gateway should start the user’s real Claude Code process with environment variables injected only into that child process. The user’s normal claude, codex, and other installations must remain unchanged.

Gateway routing should remain protocol-aware and conservative. The first gateway implementation should prefer pass-through routing between compatible protocols instead of pretending every model perfectly implements every harness protocol. Interactive sessions should remain sticky to a selected backend by default so prompt caches and model-specific context are not destroyed by per-turn routing; failover and explicit task-boundary migration can be added separately.

────────

Implementation Order

Phase 0 — Repository and executable foundation

Fixed architectural requirements

- Glasshouse remains a Rust workspace that produces one primary `glasshouse` executable for V1.
- Do not introduce a web frontend, Electron shell, distributed service topology, or parallel application runtime as an alternative core architecture without an explicit specification revision.

☑ Create Glasshouse as a Rust workspace that builds a single glasshouse executable.
☑ Keep the dependency set limited to libraries required for async execution, terminal UI, PTYs, serialization, SQLite, basic process control, error handling, CLI parsing, logging, OS path conventions, HTTP, hashing, cryptographic randomness, and OS credential storage.
☑ Make glasshouse run without requiring a global daemon, background service, Node installation, or Python environment.
☑ Make all runtime paths configurable so the binary can be used from a user-owned tools directory without a package-manager installation.
☑ Add a glasshouse --version command that prints the binary version.
☑ Add a glasshouse --help command that documents the initial CLI surface.
☑ Add structured application logging that can be enabled for debugging without polluting the interactive TUI.
☑ Add a clean shutdown path that restores the terminal state after normal exit, panic, or interrupt.

Phase 1 — Project-root detection and hard isolation

Fixed architectural requirements

- One running Glasshouse instance belongs to exactly one canonical project root.
- Cross-project isolation must be structural through project-scoped state and storage, not merely a query filter or prompt convention.

☑ Resolve the current project root from the current working directory by using the containing Git repository when one exists.
☑ Add glasshouse --scope <path> to explicitly select a project root when Git-based discovery is not appropriate.
☑ Canonicalize the selected project root before using it for access-control decisions.
☑ Refuse / as an implicit project root unless the user explicitly overrides the safety check.
☑ Refuse the user home directory as an implicit project root unless the user explicitly overrides the safety check.
☑ Refuse obvious multi-project container directories such as a directory containing multiple Git repositories unless the user explicitly selects a narrower scope.
☑ Derive a stable project identifier from the canonical project-root path.
☑ Store each project’s Glasshouse state in a physically separate project-specific state directory.
☑ Store each project’s memory in its own SQLite database instead of sharing one global memory database.
☑ Ensure every spawned harness process starts with its working directory set to the current project root.
☑ Reject any attempt to resume a Glasshouse-managed session whose project identifier differs from the current project identifier.
☑ Add a canonical-path guard that rejects file paths resolving outside the current project root.
☑ Apply the canonical-path guard after resolving symlinks so a project symlink cannot escape the project boundary.
☑ Keep cross-project memory retrieval disabled by design rather than relying only on query filters.
☑ Display the active canonical project root prominently in the TUI.

Phase 2A — Cross-platform runtime

Fixed architectural requirements

- Platform-specific PTY, signal, and process behavior must remain behind stable runtime interfaces.
- Native Windows and WSL are separate runtime environments; Glasshouse must not silently mix their paths, executables, process namespaces, or session state.

☑ Support macOS as a first-class Glasshouse runtime.
☑ Support Linux as a first-class Glasshouse runtime.
☑ Support native Windows as a first-class Glasshouse runtime where the selected harness is available.
☑ Treat WSL as a Linux runtime and do not silently mix Windows and WSL process namespaces.
☑ Hide platform-specific PTY behavior behind a common terminal-process interface.
☑ Use Unix PTY primitives on macOS and Linux through the selected PTY abstraction.
☑ Use Windows ConPTY through the selected PTY abstraction on native Windows.
☑ Hide platform-specific signal and process-termination behavior behind a common process-control interface.
☑ Resolve executable names correctly across Unix executables, Windows .exe, .cmd, and .bat launchers.
☑ Use the operating system’s conventional per-user application-data location for Glasshouse state.
☑ Allow the application-data location to be overridden explicitly for portable installations and tests.
☑ Keep project identifiers stable for a canonical project path within the same operating-system environment.
☑ Normalize path comparisons in a platform-correct way without weakening project-boundary checks.
☑ Add CI builds for macOS, Linux, and Windows before declaring cross-platform support stable.
☑ Add platform-specific PTY smoke tests that start a simple interactive child process and verify input, output, resize, and exit handling.
☑ Make unsupported platform/harness combinations fail with a clear diagnostic rather than a partial broken session.

Phase 2B — Agent and tool auto-detection

☑ Add a non-destructive discovery pass that searches the current PATH for supported harness executables.
☑ Detect Claude Code when a usable claude executable is present.
☑ Detect Codex when a usable codex executable is present.
☑ Detect Antigravity when a supported Antigravity CLI executable is present.
☑ Detect OpenCode when a usable opencode executable is present.
☑ Detect cmux when a usable cmux executable or supported cmux control environment is present.
☑ Detect Ollama when a usable ollama executable or configured local endpoint is present.
☑ Detect common llama.cpp server executables when they are available locally.
☑ Read harness versions using non-interactive version commands when supported.
☑ Never trigger an interactive login merely to determine whether a harness exists.
☑ Never print discovered API-key values during detection.
☑ Detect the presence of relevant provider environment variables without logging their secret contents.
☑ Detect known provider configuration files only when doing so does not require importing or modifying them.
☑ Mark every detected integration as available, configured, unconfigured, unsupported-version, or unknown.
☑ Keep discovery results advisory so the user can manually add a harness that auto-detection missed.
☑ Add glasshouse doctor output that reports discovered harnesses, versions, optional integrations, and actionable setup problems.

Phase 2C — First-run onboarding

☑ Detect whether the current user has completed Glasshouse onboarding before opening the normal TUI for the first time.
☑ Show a concise first-run wizard when onboarding has not been completed.
☑ Show all automatically detected harnesses in the first-run wizard.
☑ Allow the user to enable or ignore each detected harness.
☑ Allow the user to add the path to a harness executable that was not detected automatically.
☑ Explain that Glasshouse launches the user’s existing harness binaries rather than installing replacement copies.
☑ Offer provider and gateway configuration as an optional first-run step.
☑ Offer routing-model configuration as an optional first-run step after providers have been detected or configured.
☑ Offer an Automatic routing-model choice that selects the cheapest sufficiently fast configured resource.
☑ Offer a Choose model routing-model choice for users who want to pin classification to a specific model.
☑ Offer a Do later choice for routing-model configuration and use deterministic routing heuristics until configured.
☑ Provide a clear Configure now choice for provider and gateway setup.
☑ Provide a clear Do later choice that completes onboarding without requiring any API keys.
☑ Allow Glasshouse to be fully useful with only native subscription-backed harnesses configured.
☑ Offer cmux integration only when cmux is detected or the user explicitly asks to configure it.
☑ Show the project-isolation model during onboarding in one concise explanation.
☑ Avoid requiring an account, cloud login, or Glasshouse-hosted service during onboarding.
☑ Persist onboarding choices in user-level Glasshouse configuration.
☑ Allow the onboarding wizard to be reopened later from settings.

Phase 2D — Settings foundation

☑ Add a TUI settings view that can be opened without leaving the current project session permanently.
☑ Add a Harnesses settings section.
☑ Add a Providers settings section.
☑ Add a Launch Profiles settings section.
☑ Add a Routing settings section.
☑ Allow the routing settings to select Automatic, a specific configured model, or deterministic-only classification.
☑ Allow the routing settings to define a maximum acceptable router latency.
☑ Allow the routing settings to define a maximum marginal cost per routing decision.
☑ Allow the routing settings to prefer free resources for routing when they satisfy health and rate-limit requirements.
☑ Allow the routing settings to reserve premium subscription capacity below a configurable remaining-capacity threshold.
☑ Add a Memory settings section.
☑ Add an Integrations settings section for optional tools such as cmux.
☑ Show whether each harness is detected and enabled.
☑ Allow the user to change a harness executable path.
☑ Allow the user to add, edit, disable, test, and remove provider configurations.
☑ Allow the user to create, edit, duplicate, disable, and remove launch profiles.
☑ Allow the user to defer any optional setup item and return to it later.
☑ Separate user-level defaults from project-level overrides visibly in settings.
☑ Require explicit confirmation before writing project-level configuration into the repository.
☑ Never expose stored secret values in full in the settings UI.

Phase 2 — Persistent project state

Fixed architectural requirements

- V1 uses a project-local SQLite database for durable operational state; it does not require a server database.
- Session metadata, project memory, configuration, and credentials are separate domains even when some non-secret records share the same SQLite deployment.

☑ Create the project-specific state directory automatically on first Glasshouse launch.
☑ Create a project-specific SQLite database automatically on first Glasshouse launch.
☑ Persist Glasshouse session metadata independently from the native harness session files.
☑ Persist a mapping between Glasshouse session IDs and native harness session IDs when native IDs are available.
☑ Persist the harness type, creation time, last activity time, role, lifecycle state, and project identifier for every session.
☑ Persist the process presentation mode for every session.
☑ Persist enough metadata to distinguish active, resumable, closed, and failed sessions.
☑ Never store provider credentials directly in the project memory database.
☑ Add a schema-version table so database migrations can be applied deterministically.
☑ Add a small migration mechanism before introducing multiple schema versions.

Phase 3 — TUI shell

Fixed architectural requirements

- The primary V1 product interface is a Ratatui/Crossterm terminal application.
- A browser UI or desktop web container may be considered later but must not become a hidden prerequisite for the core runtime.

☑ Build the main interactive interface with Ratatui and Crossterm.
☑ Create a persistent top bar that shows the project name, project root, and active session.
☑ Create a persistent session bar that lists currently known sessions.
☑ Create a central viewport reserved for the active session terminal.
☑ Create a compact bottom status bar for Glasshouse-level key bindings and status messages.
☑ Allow the user to move to the previous session with a keyboard shortcut.
☑ Allow the user to move to the next session with a keyboard shortcut.
☑ Allow the user to open a session overview from the keyboard.
☑ Allow the user to open a project-memory view from the keyboard.
☑ Allow the user to return from Glasshouse overlays to the active native session without terminating it.
☑ Preserve terminal resize events and propagate the new dimensions to the active embedded terminal.
☑ Keep the visual design text-first and avoid decorative graph visualizations that do not expose actionable state.

Phase 4 — Generic PTY session runtime

Fixed architectural requirements

- Interactive harnesses run as real child processes attached to PTYs or the platform-equivalent terminal abstraction.
- Switching the visible session changes presentation focus only; it must not recreate, emulate, or restart the underlying harness process.

☑ Implement a generic PTY-backed child-process abstraction for interactive harnesses.
☑ Allow the PTY runtime to spawn a command with an explicit working directory and environment.
☑ Stream PTY output continuously into an in-memory terminal buffer.
☑ Forward user keystrokes from the active Glasshouse session to the active PTY.
☑ Forward terminal resize events from Glasshouse to the child PTY.
☑ Support sending text programmatically to a PTY session without requiring the user to focus it.
☑ Support sending interrupt signals to a PTY session.
☑ Detect process exit independently from textual terminal output.
☑ Preserve a bounded terminal scrollback buffer for each live session.
☑ Keep inactive PTY sessions running while the user views another session.
☑ Ensure switching sessions changes only the presentation focus and does not restart the underlying process.
☑ Add a headless presentation mode in which a PTY continues running without occupying the visible session viewport.

Phase 5 — Native terminal embedding

Fixed architectural requirements

- Glasshouse embeds the native harness terminal experience instead of replacing it with a Glasshouse chat composer.
- Native commands, permission flows, model controls, compaction, resume behavior, and tool interfaces remain owned by the harness.

☑ Render ANSI terminal output from the active PTY faithfully enough for native Claude Code and Codex TUIs to remain usable.
☑ Preserve native colors, cursor position, line wrapping, and basic terminal control sequences required by supported harnesses.
☑ Preserve native harness input behavior instead of replacing it with a Glasshouse chat composer.
☑ Allow native slash commands to pass directly to the underlying harness.
☑ Allow native permission prompts to remain interactive.
☑ Allow native compact, resume, model-selection, and tool interfaces to remain accessible when the harness provides them.
☑ Make the embedded native product visually dominant while Glasshouse chrome remains minimal.
☑ Add an escape key sequence that temporarily captures input for Glasshouse-level navigation without permanently stealing input from the harness.

Phase 6 — Harness adapter interface

Fixed architectural requirements

- Glasshouse core depends on a common `HarnessAdapter` contract, not on harness-specific implementation details.
- Commands, lifecycle parsing, hooks, model overrides, protocol declarations, and configuration mechanisms remain isolated inside adapters.

☑ Define a common HarnessAdapter interface for starting, resuming, messaging, interrupting, observing, and describing a harness session.
☑ Make each adapter expose the executable command used to start a new native session.
☑ Make each adapter expose the command or arguments used to resume a native session when supported.
☑ Make each adapter declare whether structured lifecycle hooks are available.
☑ Make each adapter declare whether native session IDs can be discovered.
☑ Make each adapter expose known capabilities such as code editing, shell access, browser use, MCP support, and native subagents when known.
☑ Make each adapter declare which backend wire protocols and model-override mechanisms it supports.
☑ Make each adapter declare whether backend selection is configured through child environment, command-line arguments, an isolated generated configuration, or another explicit launch mechanism.
☑ Make each adapter declare which native communication-style mechanisms it supports and whether changing them requires a new or cleared native session.
☑ Make each adapter declare which native approval/permission modes it supports, including whether a native automatic-review mode exists, and treat an absent mode as unverified rather than substituting a blanket bypass.
☑ Make each adapter identify the harness vendor independently from the model developer and serving provider.
☑ Make the generic PTY runtime independent from any specific harness adapter.
☑ Keep adapter-specific parsing isolated from the core Glasshouse session model.

Phase 7 — Claude Code adapter

Fixed architectural requirements

- The adapter launches the installed first-party `claude` executable; Glasshouse does not reimplement Claude Code's agent loop or terminal UI.

☑ Add a Claude Code adapter that starts the real claude executable inside the current project root.
☑ Capture the native Claude Code session identifier when it can be obtained reliably.
☑ Support resuming a known Claude Code session through Claude Code’s native resume mechanism.
☑ Preserve the complete native Claude Code TUI inside the Glasshouse PTY.
☑ Add Claude Code lifecycle-hook integration for events that Claude exposes structurally.
☑ Translate supported Claude lifecycle events into Glasshouse lifecycle events.
☑ Detect when Claude Code requires user input or permission through structured events when possible.
☑ Detect normal Claude turn completion through hooks rather than terminal-text heuristics when possible.
☑ Record Claude compaction events when they can be observed reliably.
☑ Keep terminal-text parsing only as a fallback for state that cannot be obtained structurally.

Phase 8 — Codex adapter

Fixed architectural requirements

- The adapter launches the installed first-party `codex` executable; Glasshouse does not reimplement Codex's agent loop or couple core behavior to Codex-internal crates.

☑ Add a Codex adapter that starts the real codex executable inside the current project root.
☑ Capture the native Codex thread or session identifier when it can be obtained reliably.
☑ Support resuming a known Codex session through Codex’s native resume mechanism.
☑ Preserve the complete native Codex TUI inside the Glasshouse PTY.
☑ Integrate structured Codex events or hooks wherever the installed Codex version exposes them.
☑ Translate supported Codex lifecycle events into Glasshouse lifecycle events.
☑ Detect Codex turn completion structurally when possible.
☑ Detect Codex waiting-for-user and permission states structurally when possible.
☑ Record observed Codex compaction events or compaction-related state when available.
☑ Avoid coupling Glasshouse core logic to Codex-internal Rust crates.

Phase 9 — Antigravity adapter

Fixed architectural requirements

- The adapter launches the real supported Antigravity executable and preserves its native interaction model rather than emulating it.

☑ Add an Antigravity adapter that starts the real supported Antigravity CLI command inside the current project root.
☑ Capture the native Antigravity conversation identifier when it can be obtained reliably.
☑ Support resuming a known Antigravity conversation through its native mechanism when available.
☑ Preserve the native Antigravity terminal experience inside the Glasshouse PTY.
☐ Integrate structured Antigravity lifecycle events where the CLI exposes them.
☐ Translate supported Antigravity lifecycle state into Glasshouse lifecycle events.
☑ Treat unsupported lifecycle information as unknown instead of fabricating certainty from terminal text.

Phase 9A — Harness launch profiles

Fixed architectural requirements

- A launch profile is the authoritative composition of harness, backend resource, model, wire-protocol compatibility, ephemeral child-process overlay, and response profile.
- A provider, direct API, router, or gateway is a backend resource for an installed harness and is never an interactive coding agent by itself.

☑ Introduce a launch-profile abstraction that describes how Glasshouse starts a harness without changing the user’s global harness installation.
☑ Require every interactive Glasshouse session to be operated by a real installed coding harness.
☑ Define a launch profile as the combination of harness, backend resource, model selection, protocol compatibility, child-process configuration overlay, and response profile.
☑ Treat a provider, direct API, router, or gateway as a backend resource for a harness rather than as an interactive coding harness by itself.
☑ Give every harness a Native launch profile that uses the harness’s normal first-party authentication and configuration.
☑ Allow additional launch profiles such as Claude / OpenRouter, Claude / NVIDIA, or Codex / Custom Provider.
☑ Store launch-profile configuration separately from project memory.
☑ Allow a launch profile to inject environment variables only into the child harness process.
☑ Allow a launch profile to inject command-line arguments only into the child harness process.
☑ Allow a launch profile to select the harness's own approval mode, defaulting to its native automatic-review mode where one exists and never to a blanket bypass.
☑ Allow a blanket bypass on a harness that declares no automatic-review mode only after the user has been shown its risk once and acknowledged it, record that acknowledgement per harness, and never downgrade to a bypass silently.
☑ Allow a launch profile to use an isolated generated configuration file when a harness requires file-based provider configuration.
☑ Represent these mechanisms together as an ephemeral child-process launch overlay rather than assuming every harness can be redirected through environment variables alone.
☑ Resolve the launch overlay through the selected HarnessAdapter and refuse unsupported combinations instead of inventing generic environment names.
☑ Never modify the user’s normal global Claude Code or Codex configuration merely to launch a Glasshouse profile.
☑ Prefer temporary or Glasshouse-owned generated configuration over editing third-party config files in place.
☑ Record the launch profile used by every session.
☑ Record the resolved harness, backend resource, model, protocol, pairing class, and response profile used by every session.
☑ Show the active launch profile next to the harness in session details.
☑ Show the resolved launch mechanism and overridden key names for diagnostics while redacting secret values.
☑ Allow the user to select a launch profile when creating a session manually.
☑ Allow the router to select among enabled launch profiles when automatic routing is enabled.
☑ Allow launch profiles to be marked native-subscription, direct-provider, or glasshouse-gateway.
☑ Keep native-subscription profiles available even when gateway providers are configured.
☑ Allow a launch profile to declare which wire protocol it expects from its backend.
☑ Refuse to launch a profile when the selected provider cannot satisfy the required protocol unless an explicit translation adapter exists.

Phase 9B — Scoped harness wrappers and shims

Fixed architectural requirements

- Environment variables, CLI arguments, and generated configuration apply only to the launched process tree.
- Glasshouse must not mutate the user's global harness installation or persistent global harness configuration as a side effect of selecting a profile.

☑ Add glasshouse run <harness> --profile <profile> to launch a real harness with a selected Glasshouse launch profile outside the full Glasshouse TUI.
☑ Make glasshouse run inject configuration only into the spawned process and its descendants.
☑ Preserve the user’s existing shell environment except for explicit launch-profile overrides.
☑ Add an optional command for generating a lightweight user-owned shim such as claude-glasshouse when the user explicitly requests one.
☑ Make generated shims call glasshouse run instead of duplicating provider secrets or routing logic.
☑ Never create shell aliases or modify .zshrc, .bashrc, PowerShell profiles, or other shell startup files without explicit user action.
☑ Allow generated shims to live in a user-selected tools directory instead of requiring a system-wide install.
☑ Make deleting a generated shim sufficient to remove that convenience entry point.
☑ Keep the same launch-profile behavior whether a session is started from the TUI, glasshouse run, or an optional shim.

Phase 9C — Provider protocol model

Fixed architectural requirements

- Protocol compatibility is modeled explicitly and verified before launch.
- V1 prefers protocol pass-through; broad universal protocol translation is not part of the base architecture and requires an explicit pair-specific adapter and tests.

☑ Represent provider compatibility explicitly as anthropic-messages, openai-responses, openai-chat, or another named protocol.
☑ Allow a provider to support more than one protocol.
☑ Store the provider base URL independently for each supported protocol when necessary.
☑ Record whether a provider supports streaming for a protocol.
☑ Record whether a provider supports tool calls for a protocol.
☑ Record whether a provider supports reasoning or thinking controls when known.
☑ Record whether a provider exposes a model-list endpoint.
☑ Record whether a provider exposes usage or rate-limit telemetry.
☑ Treat protocol compatibility as a hard routing constraint before model-quality scoring.
☑ Do not assume OpenAI-compatible chat completion support implies OpenAI Responses API compatibility.
☑ Do not assume an OpenAI-compatible model can transparently satisfy Claude Code’s Anthropic Messages behavior.
☑ Allow explicit protocol-translation adapters later without making translation mandatory for V1.

Phase 9D — Built-in provider templates

☑ Add a built-in OpenRouter provider template.
☑ Add a built-in NVIDIA-compatible provider template for endpoints exposing supported NIM-style APIs.
☑ Add a built-in LiteLLM provider template.
☑ Add a built-in Ollama provider template.
☑ Add a built-in llama.cpp provider template.
☑ Add a generic OpenAI-compatible provider template.
☑ Add a generic Anthropic-compatible provider template.
☑ Allow providers such as UnoRouter, AnyRouter, Kilo, Nous, Z.ai, OpenCode Zen, Cerebras, Groq, or future services to be configured through generic templates when their API shape is compatible.
☑ Keep provider templates declarative so adding a known service does not require branching throughout the core router.
☑ Keep service-specific default URLs and headers overridable by the user.
☑ Allow a provider template to define required environment-variable names without storing the secret in project memory.
☑ Allow the user to test provider connectivity from settings before enabling it for routing.
☑ Allow the user to refresh a provider’s model list manually when the provider exposes model discovery.
☑ Cache discovered model metadata with a timestamp rather than querying remote model catalogs on every Glasshouse start.

Phase 9E — Secret storage

Fixed architectural requirements

- Credentials remain outside project memory, tracked configuration, checkpoints, event payloads, and diagnostic logs.
- Secret storage is accessed through a dedicated `SecretStore` abstraction, preferring native OS-backed secure storage when available.

☑ Define a SecretStore abstraction independent from project memory and provider configuration.
☑ Prefer the macOS Keychain for user-entered provider secrets on macOS when available.
☑ Prefer Windows Credential Manager for user-entered provider secrets on Windows when available.
☐ Prefer a Secret Service-compatible keyring on Linux when available.
☑ Allow environment-variable references as a cross-platform secret source.
☑ Provide a clearly labeled fallback when a native secure secret store is unavailable.
☑ Store only secret references in provider configuration whenever possible.
☑ Never write API keys into tracked .glasshouse project files.
☑ Never include provider secrets in checkpoints, memory extraction input, event logs, debug logs, or crash reports.
☑ Redact recognized secrets from diagnostic output.
☑ Allow the user to delete a stored provider credential from settings.
☑ Allow multiple credentials for the same provider through distinct provider instances.
☑ Allow several credentials for one provider to be held as a pool rather than only as separate provider instances, so a user who has more than one key for the same router can configure them together.

Phase 9F — Direct provider launch profiles

Fixed architectural requirements

- Direct-provider and gateway-backed interactive sessions still run through a real installed coding harness.
- Glasshouse configures the harness's backend but does not take ownership of its agent loop, tools, permissions, compaction, or native session semantics.

☑ Support launching Claude Code directly against a configured Anthropic-compatible gateway by injecting the required child-process environment.
☑ Support ANTHROPIC_BASE_URL injection for Claude Code launch profiles that use a compatible gateway.
☑ Support provider authentication injection for Claude Code without modifying the user’s native Claude authentication.
☑ Allow Claude launch profiles to override default model identifiers when a compatible gateway requires provider-specific model names.
☑ Keep Claude gateway environment variables scoped to the Glasshouse-launched process.
☑ Support Codex custom-provider launch profiles using Glasshouse-owned configuration when the configured provider supports the wire API required by Codex.
☑ Avoid overwriting the user’s normal ~/.codex/config.toml to create a Glasshouse provider profile.
☑ Generate an isolated Codex configuration or environment for Glasshouse-managed custom-provider sessions where supported.
☑ Verify the selected harness, model, provider, and protocol combination before starting an interactive session when a cheap capability check is available.
☑ Require the selected coding harness executable to be installed and usable before offering an interactive direct-provider or gateway-backed launch profile.
☑ Keep the real harness responsible for the agent loop, coding tools, permission flow, compaction, and native session semantics.
☑ Treat environment variables, CLI overrides, and isolated generated configuration as adapter-specific ways to point that harness at the backend.
☑ Fall back to a clear launch error rather than silently using the native paid provider when a requested gateway profile cannot be configured.

Phase 9G — Glasshouse local gateway process

Fixed architectural requirements

- The local gateway is an optional loopback transport, credential, telemetry, reliability, and backend-routing proxy.
- It is not a coding harness, does not own interactive sessions, and must not acquire an autonomous coding loop, repository tool surface, permission system, or compaction system.

☑ Define the local Glasshouse gateway as an optional transport, credential, telemetry, reliability, and backend-routing proxy for requests originating from a real harness.
☑ Never treat the local gateway as a coding harness, agent loop, interactive session owner, or replacement for native harness tools.
☑ Add an optional local Glasshouse gateway that binds only to loopback by default.
☑ Start the local gateway only when at least one active launch profile requires it.
☑ Use an ephemeral local port by default so multiple Glasshouse instances can coexist.
☑ Generate an ephemeral per-instance gateway authentication token for child harnesses.
☑ Never expose provider API keys to a child harness when the local gateway can hold the credential itself.
☑ Keep provider credentials inside the Glasshouse process or secure secret-store boundary.
☑ Expose an Anthropic Messages-compatible ingress for gateway-backed Claude Code profiles when implemented.
☑ Expose an OpenAI Responses-compatible ingress for gateway-backed Codex profiles when implemented.
☑ Expose an OpenAI Chat-compatible ingress for compatible disposable jobs and harnesses when implemented.
☑ Require every interactive gateway ingress to be consumed through a compatible installed harness launch profile.
☑ Preserve streaming end-to-end through the gateway.
☑ Preserve tool-call payloads without lossy rewriting when the backend speaks the same protocol.
☑ Preserve provider error information in structured Glasshouse diagnostics while returning a compatible error to the harness.
☑ Keep the first gateway implementation protocol pass-through wherever possible.
☑ Do not implement broad cross-protocol request translation until concrete harness/provider pairs require it.
☑ Shut down the local gateway when the owning Glasshouse instance exits and no detached sessions depend on it.
☑ Make gateway logs opt-in and redact prompt bodies and secrets by default.

Phase 9H — Sticky gateway routing for harness-backed interactive sessions

Fixed architectural requirements

- Interactive sessions are sticky to a compatible model and backend by default to preserve prompt-cache locality, context continuity, and tool semantics.
- A material model-family change is an explicit migration decision, not transparent per-turn failover.

☑ Assign an interactive harness-backed gateway session to a provider and model when the session starts.
☑ Keep the harness identity and native session semantics explicit even when the backend is routed through a Glasshouse gateway.
☑ Treat the gateway assignment as backend state belonging to the harness-backed session rather than as an independent agent session.
☑ Keep the provider/model assignment sticky across normal turns by default.
☑ Avoid per-turn model switching for interactive sessions solely because another free model is currently available.
☑ Preserve prompt-cache locality as a routing objective for gateway-backed sessions.
☐ Allow an explicit session migration to create a new provider/model assignment at a task boundary.
☑ Allow failover to a compatible backend after a real provider failure when the user or routing policy permits it.
☑ Prefer failover within the same model family and tool semantics before considering a cross-model migration.
☑ Treat a material model-family change as a migration decision rather than a transparent provider failover.
☑ Record when failover changes the provider or model serving a live session.
☑ Warn when failover is likely to invalidate provider-side prompt caching.
☑ Never fail over to a backend that cannot preserve the harness’s required protocol or tool semantics.
☑ Allow the user to pin a gateway-backed session to one provider and disable automatic failover.

Phase 9I — Free-pool routing

Fixed architectural requirements

- Interactive harness routing and bounded internal support jobs are separate policy classes.
- Free capacity may back an interactive profile only when the selected installed harness, protocol, and tool semantics remain compatible.

☑ Allow provider instances to mark selected models as free-tier or zero-marginal-cost resources.
☑ Track request-pool limits separately from token-priced limits when a provider exposes request quotas.
☑ Track per-model free-tier health independently when a router exposes multiple free models.
☑ Prefer free models for bounded Glasshouse support work such as classification, memory extraction, and reranking when quality is sufficient.
☑ Allow explicitly configured free models such as Nemotron variants to participate in disposable-job routing.
☑ Allow compatible free models to back full harness launch profiles only when protocol and tool behavior are adequate for that harness.
☑ Keep interactive harness routing and disposable-support-job routing as separate policy classes.
☑ Avoid consuming scarce free requests on health probes when actual workload can provide health signals.
☑ Apply cooldowns to free models or providers that repeatedly return rate-limit or capacity failures.
☑ Allow the user to order, disable, or pin free resources from settings.
☑ Rotate among a provider's configured credentials when one is rate-limited or exhausted, and treat a single key's exhaustion as that key's limit rather than the provider's.
☑ Track request-pool and quota state per credential rather than only per provider, because two keys for the same router are two separate allowances.
☑ Allow Glasshouse's own automated evaluation and test runs to use configured zero-cost models, and never a metered resource without an explicit opt-in.
☑ Show whether a free resource is being used because of user preference, quota preservation, or fallback.

Phase 9J — Harness-model pairing model

Fixed architectural requirements

- Vendor alignment is an inspectable positive initial soft prior, never proof of quality or a hard routing requirement.
- Reliable local measurements for the exact harness, launch profile, model, backend, and protocol combination must be able to outweigh the prior.

Pairing identity

☑ Store harness vendor independently from model developer, model family, serving provider, gateway, and wire protocol.
☑ Avoid treating the serving provider or reseller as the model developer when they are different entities.
☑ Represent pairing classes at least as vendor-native, vendor-supported, protocol-native, protocol-compatible, protocol-translated, or unknown.
☑ Treat vendor-native as the harness operating a model family produced for that harness vendor’s own coding environment.
☑ Treat vendor-supported as a pairing the harness vendor explicitly supports even when model and harness developers differ.
☑ Treat protocol compatibility separately from model-behavior compatibility and tool-semantic compatibility.
☑ Keep model developer and pairing class unknown for stealth or insufficiently attributed models rather than guessing from behavior or branding.
☑ Allow users to correct or override pairing metadata without changing router code.
☑ Keep pairing metadata declarative and independently updateable as harnesses add or remove official model support.

Pairing prior and evidence

☑ Give a compatible vendor-native harness-model pairing a positive initial routing prior for a fresh session with little local evidence.
☑ Treat the native-pairing preference as a soft prior rather than a hard routing rule or proof of superior performance.
☑ Apply hard protocol, tool, capability, privacy, and user constraints before applying the pairing prior.
☑ Allow the value of a relevant warm session to outweigh the native-pairing prior when continuity evidence is stronger.
☑ Reduce the influence of the pairing prior as reliable local observations accumulate for the exact harness, launch profile, model, and backend combination.
☑ Allow observed task success, usable tool calls, repair rate, effective TTFC, reliability, and user overrides to outweigh the initial pairing prior.
☑ Keep evidence for the same nominal model distinct across different harnesses, gateways, quantizations, model revisions, or protocol translations.
☑ Avoid concluding that a cross-vendor pairing is poor solely because it is cross-vendor.
☑ Avoid concluding that a native pairing is superior when current project evidence contradicts the prior.
☑ Surface the pairing class, current evidence strength, and contribution of the pairing prior in routing explanations.
☑ Allow users to prefer native pairing strongly, weakly, not at all, or as a hard pin for explicitly chosen sessions.

Phase 9K — Harness-aware response profiles

Fixed architectural requirements

- Response profiles govern user-facing communication only and remain independent from reasoning depth, diligence, validation, permissions, safety, and tool use.
- They must prefer native harness mechanisms, must not replace the complete native system prompt, and must not use concision to suppress diagnostics, evidence, or verification.

Profile model

☑ Define response profiles as communication policy rather than model capability, reasoning effort, permission mode, task diligence, or validation policy.
☑ Model verbosity independently as terse, concise, standard, or elaborate.
☑ Model intended audience independently as plain, technical, or executive.
☑ Model progress narration independently as silent, milestones, or detailed.
☑ Model evidence presentation independently as minimal, standard, or audit.
☑ Model final-answer format independently through options such as prose, bullets, or change-summary.
☑ Allow named presets to combine these dimensions without forcing every harness to expose the same native vocabulary.
☑ Provide a concise-technical preset that leads with outcomes, suppresses routine narration, and still reports changed files, verification, risks, and blockers.
☑ Allow separate defaults for orchestrator, worker, reviewer, explainer, and ordinary interactive-session roles.
☑ Resolve response-profile precedence as task override, session, role, project, user default, then harness default.
☑ Keep project response-profile configuration inside the project scope and prevent it from contaminating unrelated projects.

Harness-native application

☑ Prefer a harness’s native output-style or communication-style mechanism when it can represent the selected profile without weakening coding instructions.
☑ Let the HarnessAdapter translate a Glasshouse response profile into the closest safe native harness configuration.
☑ Treat Claude Code output styles, Codex personalities, and future harness-native mechanisms as adapter examples rather than universal Glasshouse concepts.
☑ Record which native mechanism, additive instruction, or fallback was actually applied.
☑ Keep every spawned worker’s response profile explicit because subagents may not inherit the main harness session’s communication style.
☑ Preserve native harness engineering, safety, permission, compaction, and tool-use instructions when applying a response profile.
☑ Never replace the complete native harness system prompt merely to control verbosity, tone, or answer structure.
☑ Do not make gateway-side system-prompt rewriting the default way Glasshouse applies a response profile.
☑ Treat a user-configured gateway prompt transformation as explicit backend metadata and surface that it may interact with harness instructions.

Additive fallback and cache behavior

☑ Define a small stable additive response contract for harnesses that lack an adequate native communication-style mechanism.
☑ Keep the fallback focused on user-facing communication and explicitly state that concision must not reduce analysis, verification, diagnostics, error reporting, or checkpoint completeness.
☑ Inject the fallback through the safest adapter-supported session-start, append-system, instruction-message, hook-context, or equivalent mechanism.
☐ Avoid repeatedly injecting an unchanged response contract on every turn when the harness already retains it.
☑ Prefer selecting a response profile when a session is created so the session’s system-prefix and prompt-cache behavior remain stable.
☑ Let adapters declare whether a live profile change is supported, delayed until a new session, or likely to invalidate prompt caching.
☑ Warn before a profile change that requires clearing or recreating a valuable warm session.
☑ Allow a lightweight in-session communication instruction for a one-turn override when supported without rewriting the system prefix.
☐ Preserve raw native terminal output even when Glasshouse offers optional folding of verbose progress or detail sections.
☐ Do not run a second language model to rewrite every final answer by default because it adds latency, cost, and risk of losing caveats.
☐ Keep arbitrary custom prompt additions separate from named response profiles and require explicit user configuration for them.

Evaluation and safeguards

☐ Measure output-token reduction, time to actionable information, user steering, profile overrides, and perceived cognitive load.
☐ Measure whether concise profiles hide relevant caveats, unresolved risks, verification failures, or required user decisions.
☐ Measure whether elaborate profiles add useful explanation or merely increase token volume and reading time.
☐ Measure profile behavior separately for each harness-model pairing because the same instruction can produce different effects across models.
☑ Allow the user to disable Glasshouse response-profile injection and use the untouched harness default.
☑ Keep the active response profile and application mechanism inspectable from session details.

Phase 10 — Unified session model

Fixed architectural requirements

- Every interactive Glasshouse session is owned by a real harness.
- Harness, launch profile, backend, provider, gateway, model, protocol, pairing class, and response profile remain separately represented rather than collapsed into one ambiguous agent identifier.

☑ Represent every native harness execution as a first-class Glasshouse session.
☑ Assign every Glasshouse session a unique Glasshouse session ID.
☑ Store the harness type for every session.
☑ Store the native session ID separately from the Glasshouse session ID.
☑ Store the harness, launch profile, backend resource, model, pairing class, protocol, and response profile as distinct session metadata.
☑ Never represent a direct API or gateway backend as an interactive Glasshouse session without an owning real harness.
☑ Track session states including starting, running, idle, waiting for user, stopped, failed, and closed.
☑ Track the last known activity timestamp for every session.
☑ Track whether each session is embedded, headless, or externally presented.
☑ Allow the user to rename a session without changing its native session ID.
☑ Allow a session to be tagged with a lightweight purpose such as auth, tests, or research.
☑ Prevent a session from belonging to more than one project.
☑ Keep stopped but resumable sessions visible separately from live processes.
☑ Allow the user to close a Glasshouse session record without deleting the native provider history unless explicitly requested.

Phase 10A — Session supervision

Fixed architectural requirements

- Supervision covers only sessions this project recorded. Glasshouse never adopts, quarantines, or reports on a process it did not start.
- A process that is alive and no longer owned is a distinct condition from one that has stopped, and is never treated as either stopped or healthy.
- Glasshouse reports and refuses; it never ends a session the user did not ask it to end.

☑ Record a durable process identity for every session Glasshouse starts, including the process start time, so that a reused process identifier cannot match a stale record.
☑ Discover, on start, the sessions this project previously recorded whose processes are still running.
☑ Verify a discovered process against its recorded identity before treating it as the session it claims to be.
☑ Adopt a verified live session rather than starting a second session beside it.
☑ Refuse to start a session that would duplicate a live, verified session of the same record.
☑ Detect a recorded session whose process is alive but whose identity no longer matches what was recorded, and mark it quarantined rather than reusing or replacing it.
☑ Refuse to start a replacement while a quarantined process still holds the same resources.
☑ Surface a quarantined session to the user with what is known about it and what it still holds.
☑ Require a started session to become verifiably ready within a bounded time, and record a start that never became ready as a failure with a stated reason rather than as a session.
☑ Restart a session that exits unexpectedly up to a bounded number of consecutive attempts, and stop with a stated reason when that bound is reached.
☑ Reset the consecutive-restart count only when a restarted session has been verified healthy, never when it has merely been started.
☑ Apply session lifecycle changes through a single ordered path so that two concurrent requests cannot interleave into a state neither requested.
☑ Never deliver two inputs to the same session concurrently.

Phase 11 — Session overview

☑ Add a session overview that lists all current project sessions in one screen.
☑ Show the harness name for every session.
☑ Show the user-assigned session name or purpose for every session.
☑ Show the current lifecycle state for every session.
☑ Show the last activity time for every session.
☑ Show whether the native session can be resumed.
☑ Show whether the session is embedded, headless, or external.
☑ Allow the user to focus any live embedded session from the overview.
☑ Allow the user to resume any compatible stopped session from the overview.
☑ Allow the user to interrupt a running session from the overview.

Phase 12 — Unified lifecycle event bus

Fixed architectural requirements

- There is one normalized core lifecycle-event stream shared by the TUI, router, memory, API, and MCP surfaces.
- Adapters translate native observations into core events; consumers must not create competing harness-specific lifecycle architectures.

☑ Define a harness-independent Glasshouse lifecycle-event enum.
☑ Record every translated lifecycle event with session ID and timestamp.
☑ Deliver lifecycle events to the TUI without blocking the harness process.
☑ Deliver lifecycle events to the orchestration layer without coupling orchestration to a specific harness.
☑ Distinguish process exit from successful turn completion.
☑ Distinguish waiting-for-user from idle when the harness provides enough information.
☑ Preserve raw adapter event payloads in debug logs when useful for troubleshooting.
☑ Do not infer successful task completion solely because a child process became quiet.

Phase 13 — Direct session messaging

☑ Add an internal API for sending text to a specific live session.
☑ Add an internal API for sending an interrupt to a specific live session.
☑ Add an internal API for querying the lifecycle state of a specific session.
☑ Add an internal API for listing all sessions in the current project.
☑ Add an internal API for retrieving the most recent terminal output of a session.
☑ Reject messaging attempts targeting sessions from another project.
☑ Record machine-initiated messages separately from user keystrokes in the Glasshouse event log.

Phase 14 — Orchestrator role

☑ Allow exactly one or more sessions to be tagged with the optional orchestrator role without creating a special proprietary agent type.
☑ Keep an orchestrator session otherwise identical to a normal native harness session.
☑ Expose Glasshouse session-management operations to an orchestrator through a local tool interface.
☑ Allow an orchestrator to list current-project sessions.
☑ Allow an orchestrator to spawn a new worker session using a selected harness.
☑ Allow an orchestrator to assign a natural-language task to a newly spawned worker.
☑ Allow an orchestrator to send follow-up instructions to an existing worker.
☑ Allow an orchestrator to interrupt an existing worker.
☑ Allow an orchestrator to query worker lifecycle state.
☑ Allow an orchestrator to retrieve a completed worker result or checkpoint.
☑ Ensure orchestrator tools cannot access sessions belonging to another project.

Phase 15 — Orchestrator wake-up flow

☑ Allow an orchestrator to register interest in completion events from a worker session.
☑ Detect worker turn completion from native lifecycle hooks when available.
☑ Generate a small structured worker-completion event when a watched worker finishes.
☑ Deliver the worker-completion event back into the orchestrator session as a new machine-originated message.
☑ Include the worker session ID, harness, lifecycle result, and concise result summary in the completion notification.
☑ Allow the orchestrator to inspect the worker directly after receiving the notification.
☑ Avoid waking the orchestrator repeatedly for duplicate completion events.
☐ Preserve the user’s ability to enter and modify a worker session before the orchestrator acts on its result.

Phase 16 — Worker transparency

☑ Ensure every worker created by an orchestrator appears immediately in the normal Glasshouse session list.
☑ Allow the user to enter any orchestrated worker while it is running.
☑ Allow direct user input to an orchestrated worker without requiring the orchestrator as an intermediary.
☑ Allow the user to interrupt an orchestrated worker directly.
☑ Record user intervention so the orchestrator can be informed that the worker state may have changed.
☑ Never implement orchestration workers as hidden in-process LLM calls when a native harness session was requested.
☑ Preserve the rule that every worker remains a real session the user can inspect.

Phase 17 — cmux optional integration

☑ Detect whether Glasshouse is running in an environment where cmux control capabilities are available.
☑ Keep all core Glasshouse functionality operational when cmux is absent.
☑ Implement cmux support behind a separate optional integration module.
☑ Allow Glasshouse to spawn a worker in a new cmux pane when the user requests external presentation.
☑ Allow Glasshouse to send text to a known cmux-backed session through the cmux integration.
☑ Allow Glasshouse to focus a cmux pane associated with a session.
☑ Record the cmux surface or pane identifier as optional session presentation metadata.
☑ Allow a session to be created directly in external-cmux presentation mode.
☑ Keep the underlying Glasshouse session abstraction independent from whether presentation is embedded or in cmux.
☑ Treat cmux as a workspace and presentation backend rather than as Glasshouse’s orchestration core.

Phase 18 — Raw event recording

Fixed architectural requirements

- Raw observations remain available as diagnostic source evidence while normalized and derived records remain distinguishable from them.
- Derived interpretation must not overwrite or masquerade as the original event.

☑ Create an append-only project event log for important Glasshouse and harness events.
☑ Record session creation events.
☑ Record session resume events.
☑ Record session stop and failure events.
☑ Record lifecycle-hook events that may later be useful for memory extraction.
☑ Record machine-initiated orchestration messages.
☑ Record detected task-completion boundaries.
☑ Record Git commit identifiers associated with memory events when they can be resolved cheaply.
☑ Keep raw event storage project-scoped.
☑ Treat the raw event stream as reconstructable source material rather than directly injecting it into agent prompts.

Phase 19 — Portable session checkpoints

Fixed architectural requirements

- Glasshouse checkpoints contain portable Glasshouse metadata and bounded handoff context.
- They do not attempt to clone or replace proprietary native harness session formats.

☑ Define a provider-independent checkpoint format for transferring active work between sessions.
☑ Include the current objective in every checkpoint.
☑ Include the current implementation state in every checkpoint.
☑ Include important decisions discovered during the current task in every checkpoint when present.
☑ Include known failed approaches in every checkpoint when present.
☑ Include important files and symbols in every checkpoint when present.
☑ Include test state in every checkpoint when present.
☑ Include explicit next actions in every checkpoint when present.
☑ Include the current Git branch and commit when available.
☑ Keep checkpoints deliberately small enough to bootstrap a fresh session cheaply.
☑ Store checkpoints separately from durable project memory.
☑ Allow the user to request a checkpoint manually for the active session.
☑ Allow Glasshouse to request a checkpoint automatically at selected task boundaries.
☑ Allow a checkpoint created by one harness to bootstrap a fresh session in another harness.

Phase 20 — Minimal durable project memory

Fixed architectural requirements

- Durable memory is project-scoped, minimal, provenance-aware, and stored locally in SQLite for V1.
- It is not a complete transcript archive and must not treat every extracted statement as an enduring requirement.

☑ Create a memory table in the project-specific SQLite database.
☑ Support the memory kind decision.
☑ Support the memory kind constraint.
☑ Support the memory kind feature.
☑ Support the memory kind finding.
☑ Support the memory kind failed_attempt.
☑ Support the memory kind todo.
☑ Store a concise subject for each memory when available.
☑ Store a concise durable body for each memory.
☑ Store the source session ID for each extracted memory when available.
☑ Store the source Git commit for each extracted memory when available.
☑ Store creation and update timestamps for each memory.
☑ Store a lifecycle status for each memory.
☑ Support at least the statuses active, superseded, rejected, resolved, needs_review, and invalidated.
☑ Do not store raw conversation filler as project memory.
☑ Do not store temporary step-by-step plans as durable project memory unless they become an accepted project constraint or decision.
☐ Do not store obvious source-code facts when rereading the source is cheaper and more reliable than maintaining the memory.
☐ Prefer storing information whose rediscovery would require significant exploration or reasoning.

Phase 21 — Memory extraction

☑ Define a structured JSON schema for extracting durable memories from session activity.
☑ Allow a configurable cheap or local model to perform memory extraction.
☑ Feed the extractor bounded session/event chunks rather than entire unbounded session histories.
☑ Require the extractor to classify every emitted memory into one supported memory kind.
☑ Require the extractor to omit speculative claims that were not established during the session.
☑ Require the extractor to distinguish failed approaches from accepted decisions.
☑ Require the extractor to preserve concise rationale when a decision’s rationale is important.
☑ Require the extractor to avoid duplicating an existing active memory when nothing materially changed.
☑ Store the originating session and event references so extracted memory retains provenance.
☑ Allow memory extraction to run after task completion.
☑ Allow memory extraction to run before or around native prompt compaction.
☑ Allow memory extraction to run manually for debugging and evaluation.
☑ Keep memory-extraction failure non-fatal to the coding session.

Phase 21A — Memory authority classes

Fixed architectural requirements

- Hard invariants, accepted decisions, working assumptions, experiments, user preferences, and historical ideas are distinct authority classes.
- Retrieval and injection must preserve those distinctions instead of flattening all memories into equally authoritative text.

☑ Classify durable memory by authority rather than treating every remembered statement as an equally strong rule.
☑ Support the authority class invariant for facts or requirements that should not be violated without explicit review.
☑ Support the authority class constraint for currently binding technical, security, legal, compatibility, or product limits.
☑ Support the authority class decision for an accepted implementation or architecture choice that may later be revisited.
☑ Support the authority class preference for a desired direction that should not force unnecessary complexity.
☑ Support the authority class hypothesis for a belief that still requires validation.
☑ Support the authority class idea for exploratory possibilities that must never be injected as binding instructions.
☑ Support the authority class historical for context that is useful for understanding the project but should not direct current implementation.
☑ Require memory extraction to distinguish a hard requirement from a convenient implementation choice.
☑ Require memory extraction to distinguish an accepted decision from an idea that was merely discussed enthusiastically.
☑ Treat uncertain authority classification conservatively and avoid promoting uncertain memories to invariants automatically.
☑ Allow users or trusted review agents to promote or demote memory authority explicitly.

Phase 21B — Decision provenance and assumptions

☑ Store the rationale behind a durable decision when the rationale materially affects whether the decision remains valid.
☑ Store the project phase in which a decision was made when known, such as prototype, alpha, beta, production, or migration.
☑ Store the task or problem the decision was intended to solve when known.
☑ Store the assumptions that made the decision reasonable when they can be extracted reliably.
☑ Store relevant scale assumptions such as expected user count, request volume, data size, latency target, or deployment topology when they influenced the decision.
☑ Store relevant security assumptions when they influenced the decision.
☑ Store relevant compatibility assumptions when they influenced the decision.
☑ Store relevant operational assumptions such as single-instance versus distributed deployment when they influenced the decision.
☑ Store evidence references such as benchmark results, production incidents, tests, commits, or external requirements when available.
☑ Treat a decision with missing rationale and missing assumptions as lower-confidence than a well-proven decision of the same authority class.
☑ Preserve the original wording or source reference sufficiently to audit how a remembered decision was derived.

Phase 21C — Validity conditions and invalidation

☑ Allow a durable memory to define explicit validity conditions when known.
☑ Allow a durable memory to define explicit invalidation conditions when known.
☑ Mark a memory for review when its assumptions no longer match current project state.
☑ Mark a memory for review when the project phase has changed materially since the memory was created.
☑ Mark a memory for review when a production incident contradicts the assumptions behind the memory.
☑ Mark a memory for review when a newer benchmark or scale measurement invalidates the original performance assumption.
☑ Mark a memory for review when a newer security requirement conflicts with the original design.
☑ Mark a memory for review when current source architecture no longer resembles the architecture on which the decision depended.
☑ Never silently preserve a decision as binding after a known invalidation condition has occurred.
☑ Never silently delete invalidated decisions because historical rationale may still explain the current architecture.
☑ Represent invalidated memories as historical evidence rather than current instructions.

Phase 21D — Memory age and relevance decay

☑ Track the age of every durable memory.
☑ Do not make age alone invalidate a genuine invariant.
☑ Reduce automatic retrieval weight for old ordinary decisions when they have not been reaffirmed against current project state.
☑ Reduce automatic retrieval weight more aggressively for old ideas, hypotheses, and preferences.
☑ Allow recently reaffirmed memories to regain retrieval weight without changing their original creation timestamp.
☑ Track a separate last_validated_at timestamp for memories that have been rechecked.
☑ Prefer a newer validated decision over an older unvalidated decision when both address the same concern.
☑ Avoid resurfacing low-authority stale memories merely because their wording has high lexical similarity to the current task.
☑ Keep historical memories available through explicit history search even when they are excluded from automatic context injection.

Phase 21E — Decision ladder and conflict handling

Fixed architectural requirements

- Recency alone does not determine authority.
- Conflicts are resolved using explicit invariants, current scope, provenance, validity conditions, evidence, and user overrides; uncertain conflicts are surfaced rather than silently guessed.

☑ Build a decision ladder that ranks current instructions by authority, validity, recency, evidence, and scope.
☑ Place explicit current user requirements above historical implementation decisions.
☑ Place current security and correctness invariants above convenience preferences.
☑ Place validated current constraints above older ordinary architecture decisions.
☑ Place ordinary current decisions above stale preferences, hypotheses, and ideas.
☐ Treat current source code and executable tests as stronger evidence of actual behavior than stale memory summaries.
☐ Detect when a new requested implementation directly conflicts with an active remembered decision.
☐ Do not automatically route around a conflicting decision by adding layers, adapters, compatibility shims, or duplicate pathways.
☑ Surface the conflict and ask whether the older decision should be superseded when the conflict is material and cannot be resolved from current evidence.
☐ Allow an implementation agent to supersede an older ordinary decision automatically when current requirements clearly invalidate it and the change is low risk.
☑ Require stronger review before superseding security, legal, data-integrity, or externally imposed invariants.
☑ Record why a decision was superseded so future agents do not resurrect it without context.

Phase 21F — Memory retrieval quality

☑ Retrieve current active invariants and constraints separately from historical decisions.
☑ Inject only memories whose scope overlaps the current task.
☑ Prefer memories validated against the current architecture or project phase.
☐ Penalize memories whose assumptions conflict with current repository state.
☑ Penalize memories that were created during exploratory sessions and never reaffirmed.
☑ Avoid injecting old ideas merely because they mention the same subsystem.
☑ Include memory authority and validity state in machine-facing retrieval results.
☑ Include rationale and invalidation conditions when a remembered decision may constrain implementation.
☑ Allow the receiving agent to challenge a memory explicitly when current evidence contradicts it.
☑ Treat a challenged memory as requiring validation before further automatic injection into the same task.
☑ Record false-positive or harmful memory retrievals so the retrieval policy can be evaluated.

Phase 21G — Memory revalidation

☐ Add a lightweight revalidation operation that checks selected memories against current repository state and project metadata.
☐ Allow revalidation to run when a project enters a new lifecycle phase such as alpha to beta or beta to production.
☐ Allow revalidation to run before a major architecture refactor.
☐ Allow revalidation to run after a major production incident.
☐ Allow revalidation to run when a memory has not been validated for a configurable period and is about to influence a high-impact change.
☑ Use a stronger model or human review for ambiguous high-impact revalidation instead of trusting a cheap extractor blindly.
☑ Mark a memory reaffirmed, needs_review, superseded, or invalidated after revalidation.
☑ Keep revalidation bounded to relevant memories rather than periodically reprocessing the entire project history.
☐ Avoid automatic revalidation work when the memory is not about to affect any current task.

Phase 21H — Simplicity-first implementation policy

☑ Add a project-level implementation policy that prefers the simplest correct, secure, maintainable, and scalable design satisfying current requirements.
☑ Require agents to revisit a stale ordinary decision before introducing significant complexity solely to preserve it.
☑ Discourage compatibility shims when removing or superseding an obsolete internal rule is cleaner and safe.
☑ Discourage duplicate code paths created only to satisfy contradictory historical memories.
☑ Discourage speculative abstraction that is not justified by current requirements or observed extension pressure.
☑ Prefer existing language, framework, database, and platform primitives over custom mechanisms when they satisfy the requirement cleanly.
☑ Prefer explicit straightforward implementations over clever indirection when both satisfy the same requirements.
☑ Allow smart implementation choices that materially improve correctness, security, scalability, latency, or operational simplicity.
☑ Require the agent to explain unusual complexity when a simpler implementation appears available.
☑ Treat simplicity as a design constraint rather than as permission to ignore real scale or security requirements.

Phase 21I — Production-aware implementation checks

☑ Require implementation planning to consider whether a solution that works on development data remains acceptable at realistic production scale.
☑ Prefer indexed lookup paths for high-cardinality database access when a stable indexed identifier is available.
☑ Flag unindexed scans on large or expected-to-grow tables when they occur on latency-sensitive request paths.
☑ Consider query complexity, index availability, cardinality, and expected access frequency before accepting a database lookup strategy.
☑ Consider concurrency and race behavior before accepting code that is correct only under single-user development conditions.
☑ Consider memory and response-size growth before accepting algorithms whose resource use scales linearly with large datasets.
☑ Consider network round trips before accepting repeated remote calls in hot request paths.
☑ Consider authentication and authorization lookup cost at realistic user counts.
☑ Prefer stable indexed IDs over high-cost ad hoc lookups when the product already has an appropriate identifier.
☑ Do not optimize prematurely where scale is demonstrably irrelevant, but record the assumption if the implementation depends on that fact.
☑ Allow production incidents to promote previously hypothetical scale concerns into validated constraints.

Phase 21J — Implementation review checklist

☑ Before marking a substantial implementation complete, check whether any remembered rule forced avoidable complexity.
☑ Before marking a substantial implementation complete, check whether the design still matches current project requirements rather than historical ones.
☑ Before marking a substantial implementation complete, check correctness under realistic concurrency assumptions.
☑ Before marking a substantial implementation complete, check security boundaries affected by the change.
☑ Before marking a substantial implementation complete, check obvious database and algorithmic scaling characteristics.
☑ Before marking a substantial implementation complete, check whether hot-path database queries use appropriate indexes.
☑ Before marking a substantial implementation complete, check whether a simpler implementation would satisfy the same requirements with less code or fewer moving parts.
☑ Before marking a substantial implementation complete, check whether a clever optimization introduces complexity disproportionate to its demonstrated benefit.
☑ Record material architecture or performance decisions discovered during this review as current memories with rationale and scope.

Phase 21K — Assumption-aware implementation guardrails

Intent

☑ Counter the model-independent failure mode in which an uncertain inference silently becomes a premise for a large implementation and is disproven only after substantial work.
☑ Treat model confidence, repetition, eloquence, and reasoning length as presentation characteristics rather than evidence.
☑ Require concise externalized assumptions and evidence without requesting or storing private chain-of-thought.
☑ Reduce discarded implementation work and time-to-correction rather than attempting to eliminate all uncertainty.
☑ Keep the mechanism harness- and model-independent so the same policy can apply to Claude Code, Codex, Antigravity, gateway-backed agents, and future harnesses.

Risk-based activation

☑ Classify an intended change by uncertainty, reversibility, blast radius, expected implementation cost, security or data-integrity impact, and dependency on unfamiliar behavior.
☑ Let trivial, local, easily reversible edits proceed without an assumption gate.
☑ Trigger a lightweight assumption preflight before substantial architecture changes, broad refactors, migrations, unfamiliar integrations, destructive operations, or changes whose premise is weakly evidenced.
☑ Keep the preflight short enough that it does not become another source of speculative over-planning.
☑ Allow the user to force, skip, or lower the guardrail for a specific task.
☑ Never interpret a long plan as a substitute for validating the few premises on which the plan depends.

Critical-assumption record

☑ Ask for only the small set of critical assumptions whose falsity would materially change or invalidate the proposed implementation.
☑ Represent each critical assumption with a concise claim, current evidence, evidence source, uncertainty, affected scope, and a practical falsification signal.
☑ Distinguish observed facts, explicit user requirements, current repository evidence, externally verified facts, experiment results, and unverified inference.
☑ Record the cheapest useful verification step when an assumption remains uncertain.
☑ Keep transient task assumptions separate from durable project decisions until they have been supported and accepted.
☑ Track task assumptions at least as proposed, probing, supported, refuted, unresolved, or waived-by-user.
☑ Convert a refuted premise into a failed-approach record when preserving it can prevent future repetition.
☑ Promote an assumption into durable project memory only when it becomes a decision, constraint, finding, or validated hypothesis worth retaining.

Evidence before expansion

☑ Prefer direct evidence from current requirements, source code, executable tests, configuration, schemas, runtime behavior, primary documentation, and bounded experiments over a model’s narrative explanation.
☑ Require stronger evidence as implementation cost, irreversibility, security impact, data risk, or architectural blast radius increases.
☑ Verify the highest-leverage premise before broadening an edit across many files or subsystems when verification is practical.
☑ Prefer a read-only inspection, minimal reproduction, executable probe, failing test, walking skeleton, or narrow vertical slice before a large implementation.
☑ Establish a relevant baseline before changing behavior when later success would otherwise be difficult to distinguish from pre-existing state.
☑ Label unresolved inference honestly and time-box exploratory work when direct verification is unavailable.
☑ Do not ask a second model merely whether the first model sounds correct; require a verifier to cite independent repository, runtime, test, or primary-source evidence.
☑ Use a fresh session or different harness for high-impact adversarial verification when independence is worth its additional cost.
☑ Treat reviewer agreement without new evidence as weak confirmation because different agents can share the same mistaken premise.

Bounded implementation and correction

☑ Create a recoverable checkpoint before a high-risk experiment or broad implementation begins.
☑ Define an initial implementation budget using a coarse bound such as files touched, expected tool rounds, elapsed-time class, or milestone before re-evaluation.
☑ Prefer the smallest implementation slice capable of confirming or falsifying the approach.
☑ Re-evaluate critical assumptions when the planned footprint expands materially, verification results contradict the premise, or the initial budget is exceeded.
☑ Pause expansion when an agent begins adding adapters, compatibility layers, or secondary mechanisms primarily to protect an unverified premise.
☑ When a critical premise is refuted, stop compounding the implementation and explicitly choose rollback, repair, re-plan, preserve as an experiment, or ask the user.
☑ Preserve useful evidence and a concise failed-approach record even when the implementation itself is discarded.
☑ Never silently rewrite the task history to make a failed premise appear as though it had always been understood correctly.
☐ Preserve user changes and unrelated worker changes when rolling back or isolating an invalidated experiment.

User and orchestrator visibility

☑ Surface critical assumptions, their evidence state, and unresolved high-impact premises in the task or session view without flooding the normal terminal experience.
☑ Show when an assumption gate was triggered and which risk factor triggered it.
☑ Notify the user or orchestrator when a critical premise becomes refuted or when the implementation budget is materially exceeded.
☑ Offer inspect, continue, verify, checkpoint, handoff, re-plan, and stop as explicit responses to a guardrail event.
☑ Keep advisory warnings non-blocking by default except for separately configured security, destructive-action, or data-integrity policies.
☑ Make every automatic pause, reviewer spawn, or handoff attributable and manually overridable.

Phase 22 — Memory lifecycle and supersession

☑ Allow a new memory to supersede an older memory.
☑ Mark superseded memories as non-current without deleting their history.
☑ Prefer active current memories during normal retrieval.
☑ Allow rejected decisions and failed approaches to remain searchable as historical knowledge.
☑ Allow resolved todos to remain queryable without presenting them as open work.
☑ Record the superseding memory identifier when a direct supersession relationship is known.
☑ Avoid returning mutually contradictory current memories without flagging the conflict.
☑ Add a conflict state for memories whose current truth cannot be resolved automatically.
☑ Require human or stronger-agent review before automatically resolving ambiguous high-impact memory conflicts.

Phase 23 — Memory full-text search

Fixed architectural requirements

- Initial retrieval uses SQLite full-text search over project-local memory.
- V1 must not introduce a vector database merely as speculative infrastructure.

☑ Add an SQLite FTS5 index over memory subjects and bodies.
☑ Add a Glasshouse command for searching project memory with free-form text.
☑ Rank initial memory results with FTS/BM25-style lexical relevance.
☑ Default memory search to current active knowledge while allowing historical search explicitly.
☑ Return source session and commit provenance alongside search results when available.
☑ Keep memory retrieval strictly inside the current project’s SQLite database.
☑ Do not introduce a vector database until lexical retrieval is shown to be insufficient in real usage.

Phase 24 — Memory reranking

Fixed architectural requirements

- Reranking is a bounded, replaceable stage after deterministic candidate retrieval.
- It must preserve source provenance and may not turn a low-authority or invalid memory into a binding instruction merely because it is semantically similar.

☑ Allow the top lexical memory candidates to be reranked by a cheap language model.
☑ Keep reranking optional so memory search still works offline without an LLM.
☑ Limit reranking to a small candidate set to keep latency and token use low.
☑ Ask the reranker to optimize for task relevance, recency, active status, and non-duplication.
☑ Return only a small number of high-value memories for automatic prompt injection.
☑ Record retrieval diagnostics when debug mode is enabled so poor memory selection can be investigated.

Phase 25 — Project knowledge view

☑ Add a project-knowledge TUI view backed by the current project’s memory database.
☑ Show active architecture-related decisions as a simple hierarchical or grouped text view.
☑ Show active decisions in a dedicated section.
☑ Show known constraints in a dedicated section.
☑ Show implemented or planned features in a dedicated section.
☑ Show failed approaches in a dedicated historical section.
☑ Show unresolved todos in a dedicated section.
☑ Allow the user to open a memory item and inspect its rationale, source session, source commit, and lifecycle state.
☑ Show supersession relationships textually when they exist.
☑ Avoid rendering a decorative node graph unless a future concrete use case requires one.

Phase 26 — Memory query for agents

☑ Expose a project-scoped memory.search operation to Glasshouse-aware agents.
☑ Expose a project-scoped memory.get operation for retrieving a selected memory in full.
☑ Expose a project-scoped memory.current operation for retrieving a concise current project snapshot.
☑ Prevent agent memory tools from querying another project’s memory store.
☑ Return concise results rather than dumping the complete memory database into agent context.
☑ Include provenance with machine-retrieved memory so an agent can verify important claims against source or code.

Phase 27 — Context injection

Fixed architectural requirements

- Memory injection is selective, relevance-ranked, authority-aware, and budgeted.
- Glasshouse must not dump the entire memory store into prompts or permanently rewrite native harness system instructions.

☑ Add a context-selection step before Glasshouse automatically sends a routed task to a session.
☑ Query project memory for memories relevant to the routed task.
☑ Inject only a bounded set of high-relevance memories into the target session.
☑ Keep memory injection separate from native harness session history.
☐ Avoid injecting memory when retrieval confidence is low.
☑ Clearly label injected information as Glasshouse project memory rather than user-authored instructions.
☑ Include active constraints and relevant failed approaches preferentially when they can prevent repeated mistakes.
☑ Do not inject stale ordinary decisions as binding instructions when their original assumptions have not been validated against current project state.
☑ Include authority, validity, and rationale metadata when a memory materially constrains the implementation.
☑ Prefer a small number of current high-authority memories over a larger collection of historical decisions.
☑ Avoid repeatedly injecting the same unchanged memory into an already-aware hot session unless needed.

Phase 28 — File-aware memory lookup

☐ Track file paths explicitly referenced by durable memories when extraction can identify them reliably.
☑ Allow Glasshouse to retrieve memories associated with a file before a new session begins work on that file.
☐ Prefer constraints, decisions, and failed approaches when retrieving memory for an intended code edit.
☐ Keep file-aware retrieval advisory and never treat stale memory as stronger evidence than the current source code.
☑ Allow an agent to request the rationale behind a file-related constraint through memory search.

Phase 29 — Memory commits

☑ Define a lightweight memory commit operation that extracts durable project knowledge from recently completed work.
☑ Allow a memory commit to be triggered manually.
☑ Allow a memory commit to be triggered after a successful Git commit.
☑ Allow a memory commit to be triggered after a task-completion event.
☑ Allow a memory commit to be triggered before an intentional native prompt compaction.
☑ Separate durable project memories from transient session checkpoints during a memory commit.
☑ Record the relevant Git commit with memories produced from a code-change boundary.
☑ Make memory commits idempotent enough that rerunning one does not create uncontrolled duplicate knowledge.

Phase 30 — Session context metadata

☐ Track an estimated context-size value for a session when the harness exposes enough information.
☑ Track the number of observed compactions for a session when known.
☑ Track the most recent request or turn time for a session.
☑ Track an estimated prompt-cache state independently from session resumability.
☑ Represent prompt-cache state as at least hot, warm, cold, or unknown.
☑ Treat cache-state estimates as advisory when the provider does not expose authoritative cache telemetry.
☑ Track whether a session has a recent portable checkpoint.
☑ Track a lightweight task-continuity score or flag describing whether the session is still working on the same task.

Phase 31 — Compaction-aware behavior

☐ Never compact a session solely because its prompt cache is estimated to be cold.
☐ Prefer continuing a relevant native session when its contextual value outweighs the cost of rehydrating it.
☑ Prefer creating or refreshing a portable checkpoint before intentional compaction when practical.
☐ Prefer compaction at semantic task boundaries over arbitrary elapsed-time boundaries.
☐ Allow the native harness to perform its own compaction mechanism rather than replacing it with a Glasshouse-specific history format.
☑ Record enough pre-compaction durable memory that important project decisions do not depend solely on a lossy native compact summary.
☐ Allow a fresh session to bootstrap from a checkpoint when a huge cold native session is no longer economically or semantically attractive.

Phase 32 — Resource registry

Fixed architectural requirements

- Subscriptions, metered APIs, free pools, local inference, and gateways are normalized through one resource model without pretending that their native quota semantics are identical.

☑ Create a registry describing model resources available to Glasshouse.
☑ Represent native subscriptions separately from API-key or gateway resources.
☑ Represent local inference resources separately from remote resources.
☑ Allow the registry to describe Claude Code subscription capacity.
☑ Allow the registry to describe Codex or ChatGPT-backed capacity.
☑ Allow the registry to describe Google or Antigravity-backed capacity.
☑ Allow the registry to describe OpenRouter-like gateways.
☑ Allow the registry to describe other user-configured gateways such as UnoRouter, AnyRouter, Kilo, or Nous.
☑ Allow the registry to describe Ollama-backed local models.
☑ Allow the registry to describe llama.cpp-backed local models.
☑ Store secrets through environment references, OS keychain integration, or provider-native authentication rather than plaintext project memory.
☑ Keep resource configuration outside durable project knowledge.

Phase 32A — Unified quota and capacity model

☑ Define a provider-independent CapacityState model for describing how much usable capacity a resource has left.
☑ Allow CapacityState to represent token-limited resources.
☑ Allow CapacityState to represent request-limited resources.
☑ Allow CapacityState to represent credit-limited resources.
☑ Allow CapacityState to represent subscription resources with opaque provider-defined limits.
☑ Allow CapacityState to represent user-defined monetary budgets for metered APIs.
☑ Allow CapacityState to represent effectively unlimited local inference separately from remote quota.
☐ Track input-token budget independently from output-token budget when the provider exposes separate limits.
☐ Track cached-input usage independently when the provider exposes cache telemetry.
☑ Track request count independently from token consumption when both can constrain a resource.
☐ Track provider credits independently from raw tokens when credits are the actual limiting unit.
☑ Track remaining monetary budget independently from provider quota when the user has configured a spending ceiling.
☐ Track the current quota window start when known.
☑ Track the current quota reset time when known.
☑ Track rolling-window capacity separately from fixed calendar-window capacity.
☐ Track concurrent-request limits when they materially affect routability.
☑ Track requests-per-minute limits when known.
☐ Track tokens-per-minute limits when known.
☐ Track requests-per-day or equivalent long-window request pools when known.
☑ Preserve the provider-native quota units alongside any normalized percentage.
☑ Never discard raw telemetry merely because Glasshouse also computes a normalized capacity score.

Phase 32B — Quota telemetry sources

Fixed architectural requirements

- Provider-reported measurements, locally observed measurements, and inferred estimates remain explicitly distinguishable.
- Estimated capacity must never be presented as exact provider truth.

☑ Define quota telemetry sources as authoritative, observed, estimated, manual, or unknown.
☑ Prefer authoritative provider or harness usage telemetry when it is available.
☑ Read rate-limit and usage headers from API and gateway responses when the provider exposes them.
☑ Read provider usage endpoints when they are documented and can be queried without excessive request cost.
☑ Read native harness usage or status information when a stable machine-readable interface exists.
☑ Allow harness adapters to expose subscription-usage telemetry independently from API-provider telemetry.
☑ Allow a user to enter a known plan or manual budget when the provider exposes no usable telemetry.
☑ Never label an inferred subscription percentage as exact.
☑ Attach a confidence value and source description to every estimated capacity value.
☑ Record the timestamp of the last successful quota observation.
☑ Mark quota telemetry stale after a provider-specific configurable age.
☑ Fall back from authoritative telemetry to observed estimates without failing the active coding session.
☑ Treat completely unknown quota as a routing uncertainty rather than as zero or one hundred percent remaining.
☑ Surface the telemetry source in debug and resource views.

Phase 32C — Subscription capacity estimation

☑ Support subscriptions whose providers expose only opaque product limits rather than raw token budgets.
☑ Estimate subscription headroom from observed accepted requests, token usage when visible, throttling events, reset behavior, and historical sessions.
☑ Maintain a separate estimator per provider plan and authenticated account context.
☑ Reset or re-calibrate an estimator when Glasshouse detects a plan change or materially different quota behavior.
☑ Learn observed reset windows from throttling recovery when the provider does not expose an explicit reset timestamp.
☑ Distinguish short-window pressure such as a multi-hour usage window from longer weekly or monthly pressure when evidence allows.
☑ Represent estimated subscription headroom as a range or confidence-weighted percentage when exact usage cannot be known.
☑ Avoid converting opaque subscription usage into fictitious exact token counts.
☑ Allow users to override an obviously incorrect subscription estimate.
☐ Preserve historical estimation data so the scheduler can improve over repeated usage.
☑ Keep estimation history scoped to the authenticated resource and never mix usage observations from unrelated provider accounts.
☑ Allow estimation to be disabled for users who prefer only authoritative usage data.

Phase 32D — Normalized remaining-capacity score

☑ Compute a normalized remaining-capacity score between zero and one for routable resources.
☑ Derive the normalized score from the limiting resource dimension rather than averaging away a hard quota constraint.
☑ Lower the score when short-window request capacity is close to exhaustion.
☑ Lower the score when token or credit capacity is close to exhaustion.
☑ Lower the score when user-defined spending budget is close to exhaustion.
☑ Lower the score when a reset is far away relative to the remaining capacity.
☑ Increase effective availability when a near-term quota reset makes current conservation less important.
☑ Include estimator confidence so low-confidence subscription estimates do not dominate routing decisions.
☐ Treat unlimited local inference as high-capacity but still account for measured latency and concurrency.
☑ Expose the normalized score alongside native units rather than replacing provider-native information.
☑ Allow the routing policy to use capacity bands such as plenty, healthy, tight, reserve, and exhausted.
☑ Keep capacity-band thresholds user-configurable.

Phase 32E — Burn rate and exhaustion forecasting

☑ Record capacity consumption per completed request or observed harness turn when measurable.
☑ Maintain a short moving average of token consumption per task class.
☑ Maintain a short moving average of requests consumed per task class.
☑ Estimate current burn rate for each constrained resource.
☑ Estimate time-to-exhaustion when the remaining capacity and burn rate are sufficiently known.
☑ Estimate whether the resource is likely to survive until its next reset at the current burn rate.
☑ Reduce routing preference for a resource that is forecast to exhaust well before its next reset.
☑ Avoid overreacting to one unusually large request by using robust rolling statistics.
☑ Reset or decay stale burn-rate history after long idle periods or quota resets.
☑ Surface exhaustion forecasts as estimates rather than promises.

Phase 32F — Protected quota reserve

☑ Allow each premium resource to define a protected reserve percentage.
☑ Avoid spending protected reserve on low-tier work while cheaper adequate resources exist.
☑ Allow high-tier tasks to consume protected reserve when their capability requirement justifies it.
☑ Allow the user to override reserve protection for a specific task or session.
☑ Allow reserve policy to become more permissive shortly before a known quota reset.
☑ Allow reserve policy to become more conservative when the next reset is distant.
☑ Keep reserve behavior inspectable in routing explanations.
☐ Avoid moving an almost-complete high-value task to another session solely because a reserve threshold was crossed.

Phase 32G — Provider-aware request-cost estimation

☑ Estimate the marginal input cost of starting a new session on a metered provider.
☑ Estimate the marginal input cost of resuming a cold existing session when context size is known or approximated.
☐ Estimate cached-input cost separately from uncached-input cost when provider pricing supports caching.
☑ Estimate expected output cost from task tier and recent comparable tasks when useful.
☑ Estimate request-pool cost for free providers whose scarce unit is requests rather than tokens.
☐ Estimate local compute cost qualitatively through latency and occupancy instead of pretending local tokens are financially free of all cost.
☑ Include bootstrap context, project memory, checkpoints, and likely repository reads in fresh-session cost estimates when possible.
☑ Treat unknown pricing as unknown instead of assigning a fake zero cost.
☑ Allow provider price metadata to be updated independently from the router implementation.
☑ Record the estimated cost used in a routing decision so later evaluation can compare estimate against actual usage.

Phase 33 — Resource health

☑ Track whether each configured resource is currently available.
☑ Track recent request failures for gateway-backed resources.
☑ Track recent observed latency for gateway-backed resources where measurable.
☑ Track known quota or usage state when a provider or harness exposes it.
☑ Track known quota reset time when it is exposed.
☑ Track recent rate-limit responses separately from transport or model failures.
☑ Track whether a rate-limit failure appears to be provider-wide, model-specific, account-specific, or request-pool-specific when evidence permits.
☑ Feed rate-limit events back into the unified capacity estimator.
☑ Treat provider-declared Retry-After or equivalent cooldown information as authoritative for temporary scheduling blocks.
☑ Treat unavailable quota telemetry as unknown rather than inventing a percentage.
☑ Allow a resource to be temporarily marked degraded after repeated failures.
☑ Allow a degraded resource to recover after successful probes or requests.
☐ Avoid background probing at an aggressive rate that wastes free-request pools.
☑ Keep resource health separate from immediate availability so a healthy paced route can remain temporarily unschedulable without being scored as broken.
☐ Record whether a health observation came from a real task, a retry, a repair attempt, or an explicit probe.

Phase 33A — Routing evidence ledger

☑ Store project-local routing observations as an append-oriented evidence ledger rather than only maintaining current aggregate counters.
☑ Record provider, route, model identity, authenticated quota context, harness, request purpose, and observation timestamp for each measurable turn.
☑ Record dispatch time, first-byte time, time to first real token, time to first tool call, and completion time when the protocol exposes them.
☑ Do not treat whitespace padding, transport keepalives, or reasoning-only deltas as the first generated token.
☑ Record input tokens, output tokens, cached-input tokens, and monetary cost only when they are actually exposed or can be estimated with an explicit confidence label.
☑ Record successful tool rounds, retries, repairs, failovers, and the final user-visible outcome separately.
☑ Preserve raw observations alongside rolling aggregates so a routing decision can be audited and aggregation logic can be recalibrated.
☑ Compute robust rolling summaries such as median, tail latency, exponentially weighted averages, failure rates, and sample counts where useful.
☑ Separate warm-context, cold-context, and unknown-context observations instead of averaging away cache effects.
☑ Keep metrics distinct for materially different model versions, quantizations, routes, or changing stealth-model identities.
☑ Attach source, observation window, sample size, freshness, and confidence to every aggregate used for routing.
☑ Apply conservative priors or keep a metric unknown when the sample is too small to support a routing decision.
☑ Decay or expire stale operational evidence without deleting durable raw observations prematurely.
☑ Treat token volume, request count, context size, and spend as resource telemetry rather than evidence of quality or progress.
☑ Keep the evidence ledger physically project-scoped and require explicit export before observations leave the project.

Phase 33B — Reliability-adjusted agent performance

☑ Treat time to first tool call, TTFC, as the primary responsiveness measure for tool-using agent work when structured tool events are available.
☑ Keep TTFT as a separate measure of generation responsiveness rather than presenting it as agent productivity.
☑ Keep decode tokens per second as a model-serving characteristic rather than presenting it as task progress.
☑ Track successful tool rounds per minute of serving time as an outcome-adjacent agent-system measure.
☐ Define effective TTFC as observed TTFC divided by one minus the relevant failure probability when enough observations exist.
☐ Use reliability-adjusted latency in route comparison so a fast route that frequently fails is not ranked as genuinely fast.
☑ Keep an additive failure penalty available because a failed turn can also stop a harness, lose user attention, or require recovery beyond elapsed time.
☐ Count empty completions, unusable tool calls, stream aborts, and apparently successful but non-actionable turns as distinct unsuccessful outcomes.
☐ Keep raw TTFC, effective TTFC, TTFT, throughput, and rounds per minute visible separately rather than collapsing them into one performance headline.
☐ Avoid comparing TTFC across tasks with materially different tool requirements unless the comparison is explicitly normalized or segmented.
☑ Allow configurable scoring weights and preserve the exact inputs and terms used for every important routing score.
☑ Treat the OX gateway scoring model as implementation evidence and a configurable starting policy rather than a universal Glasshouse constant.
☑ Fall back to coarser process-level latency and outcome observations when a native subscription harness exposes no structured token or tool events.
☐ Never infer precise TTFC or token timing from terminal text when the adapter cannot distinguish protocol events reliably.

Phase 33C — Failure, quota, and route correlation

☑ Classify failures at least as throttle, exhausted quota, upstream 5xx, timeout, stream abort, empty completion, credential failure, request incompatibility, or unknown.
☑ Keep temporary cadence throttling separate from exhausted long-window quota and from provider health failures.
☐ Learn or parse provider cadence separately from Retry-After remainder values when evidence permits.
☑ Reserve known paced capacity at dispatch so concurrent workers do not all consume the same apparent allowance.
☑ Avoid retrying a paced route in place when the current cadence makes the retry predictably unavailable.
☑ Reduce or suppress active probes when probing would consume a material fraction of a scarce request pool.
☑ Measure temporally overlapping failures between routes rather than assuming different front doors are independent providers.
☑ Represent a quota domain separately from a failure domain.
☑ Treat uncorrelated account-level 429 events as evidence of separate quota buckets, not automatically as independent upstreams.
☑ Treat correlated model-specific 5xx events, matching provider metadata, or matching serving behavior as evidence of a shared failure domain.
☑ Preserve route-topology claims as confidence-weighted observations that can change when new evidence arrives.
☑ Use failure-domain diversity when selecting failover candidates so a nominally different route does not provide fictitious resilience.
☑ Require sufficient overlapping observations and expose sample size before presenting a route correlation as meaningful.
☑ Record whether a routing benefit came from independent capacity, independent quota, independent failure handling, or merely a different queue onto the same upstream.
☑ Keep correlation analysis optional for V1 routing and prevent absent evidence from being interpreted as independence.

Phase 34 — Capability registry

☑ Describe each harness and model resource with a small set of capabilities used for routing.
☑ Include code-edit capability in the registry.
☑ Include shell/tool-use capability in the registry.
☑ Include browser-use capability in the registry.
☑ Include large-context capability in the registry.
☑ Include fast-cheap-analysis capability in the registry.
☑ Include repository-review capability in the registry.
☑ Include MCP capability in the registry.
☑ Allow capability descriptions to be updated without changing the core router.
☑ Keep capability scoring simple and inspectable in the first implementation.

Phase 34A — Workload tiers

☑ Define a small ordered workload-tier system that is independent from any specific vendor model name.
☑ Define Tier 0 as deterministic or trivial work that should not require an LLM when simple rules are sufficient.
☑ Define Tier 1 as lightweight classification, extraction, reranking, formatting, and simple factual codebase lookup.
☑ Define Tier 2 as routine coding, bounded debugging, focused review, and small multi-file changes.
☑ Define Tier 3 as difficult debugging, architecture-sensitive changes, broad refactors, and work requiring strong reasoning or long-lived repository context.
☑ Define Tier 4 as frontier work where failure cost or reasoning difficulty justifies the strongest available model or warm premium session.
☑ Allow workload tiers to express required capabilities independently from raw model intelligence.
☑ Allow a task to require a lower reasoning tier but a specific capability such as browser use or a very large context window.
☑ Allow a task to require a minimum harness capability even when a cheap raw model would otherwise score highly.
☑ Keep tier definitions short, inspectable, and configurable rather than encoding opaque proprietary scores.

Phase 34B — Routing-model role

Fixed architectural requirements

- The routing model is a cheap, fast, replaceable decision component, not the orchestrator and not a hidden agent hierarchy.
- It receives a bounded routing schema and cannot independently acquire repository tools or an open-ended coding loop.

☑ Define a dedicated routing_model role separate from interactive coding sessions and memory-extraction models.
☑ Allow the routing model to be a remote paid model.
☑ Allow the routing model to be a free-tier remote model.
☑ Allow the routing model to be a local model.
☑ Allow GPT-5.6 Luna or another inexpensive fast model to be configured for the routing-model role when available to the user.
☑ Never hard-code GPT-5.6 Luna or any other specific model as a mandatory routing dependency.
☐ Prefer a routing model whose marginal decision cost is materially lower than the premium capacity it protects.
☑ Prefer a routing model with sufficient requests per minute to avoid becoming the scheduler bottleneck.
☑ Prefer a routing model with low enough latency that routing does not make interactive use feel slower than direct harness use.
☑ Prefer a routing model that reliably returns the required structured classification schema.
☑ Allow multiple routing-model candidates to form a fallback chain.
☑ Allow deterministic heuristics to remain the final fallback when every routing model is unavailable.
☑ Keep routing-model prompts short and exclude unnecessary repository history.
☑ Do not send secrets, unrelated project memory, or full conversation histories to the routing model.
☑ Allow a user to route classifications through a privacy-preserving local model even when remote models are available.

Phase 34C — Automatic routing-model selection

☑ Let routing_model = auto choose among configured resources dynamically.
☑ Filter automatic candidates by required structured-output reliability.
☑ Filter automatic candidates by current provider health.
☑ Filter automatic candidates by minimum requests-per-minute headroom when known.
☑ Filter automatic candidates by maximum acceptable routing latency.
☑ Filter automatic candidates by maximum marginal routing cost.
☑ Prefer currently free candidates after capability and latency requirements are satisfied.
☑ Prefer local candidates when they satisfy the configured latency and quality requirements.
☑ Prefer a cheap metered model over an unreliable free model when failed routing attempts would cost more time than the price difference.
☐ Avoid using a scarce premium subscription session as the classifier when a cheaper adequate routing resource exists.
☑ Re-evaluate the automatic routing-model choice when its provider becomes degraded or rate-limited.
☑ Keep the selected routing model sticky for a short period to avoid unnecessary provider churn.
☑ Show the currently selected routing model in resource diagnostics.

Phase 34D — Router request schema

☑ Define a small structured input for routing classification containing the user request, minimal current-session metadata, and relevant resource summaries.
☑ Include whether a relevant warm session already exists in the router input.
☑ Include current capacity bands rather than raw provider secrets or unnecessary billing details.
☑ Include required user-specified constraints such as pinned harness or forbidden providers.
☑ Include whether the task is expected to modify code.
☑ Include whether the task needs repository exploration.
☑ Include whether browser or external-tool capability is required.
☑ Include whether the user appears to expect a long-running multi-turn task.
☑ Avoid sending full repository contents to the router.
☑ Avoid sending full session transcripts to the router.
☑ Define structured routing output containing task class, required workload tier, required capabilities, expected duration class, and confidence.
☑ Allow the router output to recommend reuse-session, new-session, or disposable-job as an execution shape.
☑ Treat low-confidence routing classifications as a reason to use conservative deterministic fallback rules.

Phase 34E — Router economics

☑ Measure the number of routing decisions made per interactive hour.
☑ Measure routing-model token and request consumption separately from coding-agent consumption.
☑ Track routing-model spend separately from productive task spend.
☑ Warn when routing overhead becomes a non-trivial fraction of the resources it is intended to save.
☑ Allow repeated low-risk turns in the same sticky session to bypass the routing model.
☑ Re-run classification only when the user starts a new task, requests migration, the current session becomes unsuitable, or resource conditions materially change.
☑ Cache recent classification results for semantically identical task starts when safe.
☑ Prefer deterministic routing for obvious commands such as explicitly selecting a named existing session.
☑ Ensure the scheduler can be useful even if every LLM-based routing call is disabled.

Phase 34F — Model capability and tier calibration

☑ Store model capability metadata as configurable data rather than hard-coded router logic.
☑ Record an initial expected workload ceiling for each configured model.
☑ Record whether a model is suitable for structured routing output.
☑ Record whether a model is suitable for code editing, debugging, architecture work, or only support tasks.
☑ Allow users to manually override a model’s workload ceiling.
☑ Record successful and failed task outcomes by workload tier when enough evidence exists.
☑ Use observed outcomes to suggest calibration changes without silently rewriting the user’s model policy.
☑ Keep capability calibration local to the configured harness, launch profile, model, backend, and relevant protocol path because the same model may behave differently behind different harnesses, gateways, translations, or quantizations.
☑ Store the harness-model pairing class and the current evidence strength alongside capability calibration.
☑ Treat benchmark-derived capability metadata and same-vendor alignment as starting priors rather than proof of performance in the user’s harness.
☑ Keep local quantized-model capability profiles distinct from hosted versions of nominally the same model.

Phase 35 — Lightweight task classification

☑ Add a lightweight task classifier that can run on a cheap, free, or local model.
☑ Classify whether a request requires repository context.
☑ Classify whether a request requires code modification.
☑ Classify whether a request requires shell execution.
☑ Classify whether a request requires browser interaction.
☑ Estimate task complexity on a coarse scale.
☑ Estimate whether the task is likely to require multiple turns.
☑ Assign a required workload tier to the task.
☑ Identify hard capability requirements that cannot be satisfied merely by choosing a stronger text model.
☑ Estimate whether the task is safe for a disposable free or local model.
☑ Estimate whether existing warm context is likely more valuable than a stronger cold model.
☑ Return classification confidence so uncertain tier assignments can be escalated conservatively.
☑ Allow classification to fall back to deterministic heuristics when no cheap model is available.
☑ Keep classification output structured and small.

Phase 35A — Candidate generation

Fixed architectural requirements

- Candidate generation applies hard compatibility and policy constraints before scoring.
- A backend without an owning installed harness, required protocol, required tools, or security compatibility is not a valid interactive candidate regardless of price or capacity.

☑ Generate routing candidates from relevant existing sessions before considering fresh sessions.
☑ Generate fresh native-subscription session candidates from enabled harness launch profiles.
☑ Generate fresh gateway-backed session candidates only as installed-harness launch profiles whose protocol, model, tool semantics, and capability requirements match.
☑ Never generate a direct API or gateway endpoint as a first-class interactive session candidate without an owning installed harness.
☑ Generate disposable-job candidates for tasks that do not need a first-class interactive session.
☑ Exclude candidates below the classified minimum workload tier.
☑ Exclude candidates missing a hard required capability.
☑ Exclude candidates whose provider is unavailable or in an authoritative cooldown.
☑ Exclude candidates whose user-defined spending budget has been exhausted.
☑ Exclude candidates explicitly disabled or forbidden by user policy.
☑ Keep at least one deterministic fallback candidate when a usable native session exists.

Phase 35B — Candidate scoring

Fixed architectural requirements

- Cost, free capacity, and vendor pairing are soft signals after hard constraints.
- Observed task success, tool behavior, effective TTFC, reliability, and user pins may outweigh vendor alignment and nominal token abundance.

☑ Score every routing candidate using an inspectable weighted function.
☑ Include workload-tier fit in candidate scoring.
☑ Include hard capability satisfaction as a prerequisite rather than a soft bonus.
☑ Include existing session affinity in candidate scoring.
☐ Include context quality in candidate scoring.
☐ Include prompt-cache temperature in candidate scoring.
☑ Include normalized remaining capacity in candidate scoring.
☑ Include provider health in candidate scoring.
☑ Include expected marginal cost in candidate scoring.
☑ Include expected latency in candidate scoring.
☑ Include harness-model pairing as an inspectable soft prior for fresh sessions with limited local evidence.
☑ Decay the pairing prior as reliable observations accumulate for the exact harness-profile-model-backend combination.
☐ Prefer observed success and reliability over same-vendor alignment when evidence is sufficient.
☐ Prefer effective TTFC over raw TTFC for tool-using gateway routes when reliability evidence is sufficient.
☐ Include successful tool rounds per minute as supporting evidence without treating it as a universal quality score.
☐ Include cache affinity and the distinction between warm, cold, and unknown observations.
☑ Include current cadence availability separately from general route health.
☑ Include failure-domain diversity when ranking fallback and failover candidates.
☑ Reduce the influence of performance observations with small samples, low confidence, or stale windows.
☑ Include time until quota reset in candidate scoring.
☑ Include protected-reserve policy in candidate scoring.
☑ Include session-switching and bootstrap cost in candidate scoring.
☑ Include user preference and pinning as explicit high-priority policy inputs.
☑ Avoid collapsing hard constraints and soft preferences into one opaque model-generated score.
☑ Return the top candidate plus a concise explanation of the most important reasons it won.

Phase 35C — Capacity-aware tier escalation and downgrade

☑ Prefer the cheapest healthy candidate that satisfies the required workload tier and hard capabilities.
☑ Escalate to a higher tier when lower-tier candidates are unhealthy, exhausted, or repeatedly fail the task.
☑ Escalate to a higher tier when the routing classifier reports low confidence and task failure would be expensive.
☑ Preserve a warm higher-tier session when its existing context makes it cheaper or safer than starting a nominally cheaper cold session.
☑ Downgrade routine support work to free, local, or low-cost resources when premium capacity is tight.
☑ Avoid downgrading work when the expected cost of failure and retry exceeds the premium-resource savings.
☑ Allow retry policy to promote a task by one tier after a clearly attributable model-capability failure.
☑ Cap automatic escalation so a malformed task cannot consume every premium resource without user visibility.
☑ Record escalation and downgrade decisions for later evaluation.

Phase 35D — Routing under subscription pressure

☑ Prefer alternative adequate resources when a premium subscription enters the tight capacity band.
☑ Protect premium subscription reserve for high-tier tasks when the subscription enters the reserve band.
☑ Prefer finishing an already warm high-value session over migrating solely because the same subscription entered tight.
☑ Consider reset proximity before conserving subscription capacity aggressively.
☑ Allow a nearly-resetting subscription to be used more freely when remaining capacity would otherwise expire unused.
☑ Avoid intentionally exhausting a subscription if another adequate zero-cost resource is healthy and the task is low tier.
☑ Make subscription-pressure decisions visible in routing explanations.
☑ Allow users to define different reserve policies for interactive work and background support jobs.

Phase 36 — Session affinity

☑ Compute a session-affinity score for candidate existing sessions.
☑ Increase affinity when the session is already working on the same task or feature.
☑ Increase affinity when the session has recently touched relevant files.
☑ Increase affinity when the native context is still semantically useful.
☑ Increase affinity when the prompt cache is likely hot.
☑ Decrease affinity when the session context has become noisy or unrelated.
☑ Decrease affinity when the session’s quota resource is under significant pressure.
☑ Keep the affinity calculation inspectable so the user can understand why a session was selected.

Phase 37 — Basic session-aware router

☑ Route at task or session boundaries rather than switching providers blindly on every conversational turn.
☑ Prefer an existing relevant session when its affinity outweighs the benefit of starting a new session.
☐ Prefer a fresh session when existing relevant sessions are cold, bloated, or semantically poor and a good checkpoint exists.
☑ Consider harness capability fit when choosing a destination.
☑ Consider session affinity when choosing a destination.
☑ Consider prompt-cache state when choosing a destination.
☑ Consider known quota pressure when choosing a destination.
☑ Consider provider health when choosing a destination.
☑ Consider estimated switching and bootstrap cost when choosing a destination.
☑ Return an inspectable routing explanation in debug or overview mode.
☑ Allow the user to override every automatic routing choice.

Phase 38 — Quota-preserving routing

☑ Allow the router to reserve scarce premium-session capacity for difficult tasks.
☑ Prefer local or free resources for trivial classification and extraction work when suitable.
☐ Prefer cheap resources for simple repository summarization when no valuable warm session already exists.
☑ Prefer premium warm sessions for difficult tasks that benefit strongly from existing context.
☐ Avoid migrating a nearly completed task solely to preserve a small amount of quota.
☑ Avoid spending premium model capacity on internal Glasshouse bookkeeping when a cheap resource can perform it reliably.
☑ Keep quota preservation as a tunable policy rather than a hard-coded model hierarchy.

Phase 39 — Gateway-backed disposable jobs

Fixed architectural requirements

- Disposable jobs are bounded internal model calls for support functions such as classification, extraction, or reranking.
- They are not interactive Glasshouse sessions, hidden harnesses, or autonomous coding workers and receive no unrestricted repository tool surface.

☑ Define disposable jobs as bounded internal LLM calls rather than native interactive sessions or coding harnesses.
☑ Add a simple provider interface for non-interactive disposable LLM jobs.
☑ Allow OpenAI-compatible gateways to be configured through the disposable-job interface.
☑ Allow local Ollama or llama.cpp endpoints to be configured through the disposable-job interface.
☑ Use disposable jobs for classification, memory extraction, reranking, and other bounded support tasks.
☑ Keep disposable jobs distinct from first-class interactive harness sessions.
☑ Do not give disposable jobs an autonomous coding-agent loop, unrestricted repository tools, or native-session identity.
☑ Do not pretend a disposable API call is a user-enterable worker session.
☑ Record which resource performed important memory extraction or classification for debugging.

Phase 40 — Fresh-session handoff

Fixed architectural requirements

- A fresh-session migration uses an explicit bounded handoff with provenance and current task state.
- Glasshouse must not simulate native resume semantics by blindly copying complete transcripts between different harnesses or models.

☑ Allow the router or user to create a fresh session from an existing portable checkpoint.
☑ Include the checkpoint as explicit handoff context rather than replaying the complete old conversation.
☑ Include current Git status and relevant diff references in the handoff when useful.
☑ Include relevant project-memory records in the handoff when useful.
☑ Allow a Claude session to hand off to Codex.
☑ Allow a Codex session to hand off to Claude Code.
☑ Allow either session type to hand off to Antigravity when supported.
☑ Preserve the old session as resumable unless the user explicitly closes it.
☑ Record the handoff relationship between source and destination sessions.

Phase 41 — Project overview

☑ Add a project overview screen that summarizes active sessions, open work, recent memory, and resource state.
☑ Show the current orchestrator session if one is designated.
☑ Show currently running workers.
☑ Show workers waiting for user input.
☑ Show recently completed workers.
☑ Show important active decisions and constraints.
☑ Show unresolved project-memory todos.
☑ Show known resource degradation or quota pressure.
☑ Show normalized remaining-capacity bands for configured resources.
☑ Show whether each displayed capacity value is measured, estimated, manual, or unknown.
☑ Show the next known or estimated reset time for constrained resources.
☑ Show the currently selected routing model and its recent latency.
☑ Show the harness, backend, model, pairing class, and response profile for active sessions when relevant.
☑ Show protected premium reserves when they influence routing.
☑ Keep the overview factual and derived from stored state rather than generating decorative AI commentary by default.

Phase 42 — External control API

Fixed architectural requirements

- The external API controls the same core session, routing, memory, and event services as the TUI.
- It must not introduce a second session manager, duplicate state machine, or alternate agent loop.

☑ Expose a local project-scoped control API for Glasshouse operations.
☑ Allow the API to list sessions.
☑ Allow the API to spawn sessions.
☑ Allow the API to send messages to sessions.
☑ Allow the API to interrupt sessions.
☑ Allow the API to retrieve lifecycle state.
☑ Allow the API to retrieve current resource capacity and quota telemetry.
☑ Allow the API to retrieve the current routing-model selection and health.
☑ Allow the API to request an inspectable routing recommendation without executing it.
☑ Allow the API to query project memory.
☑ Allow the API to request a checkpoint.
☑ Authenticate or restrict the local control channel so unrelated local processes cannot casually control active agent sessions.
☑ Bind every control request to the current Glasshouse project scope.

Phase 43 — MCP surface for orchestrators

Fixed architectural requirements

- The MCP surface is a controlled interface to existing Glasshouse core capabilities.
- It does not create a parallel orchestration runtime or bypass project isolation, session ownership, permissions, or routing constraints.

☑ Expose selected Glasshouse control operations as MCP tools for compatible orchestrator harnesses.
☑ Expose session listing through MCP.
☑ Expose worker spawning through MCP.
☑ Expose session messaging through MCP.
☑ Expose session status through MCP.
☑ Expose worker interruption through MCP.
☑ Expose project-memory search through MCP.
☑ Expose checkpoint retrieval through MCP.
☑ Restrict MCP tools to the active project scope.
☑ Keep dangerous operations explicit enough that native harness permission controls can still be applied where possible.

Phase 44 — User control and override

Fixed architectural requirements

- Routing and automation remain visible, explainable, and overridable.
- Explicit user pins, exclusions, and manual selections are binding constraints until the user changes them or they become technically impossible, in which case Glasshouse must report the conflict.

☑ Allow the user to disable automatic routing for the current Glasshouse instance.
☑ Allow the user to pin a task to a specific harness.
☑ Allow the user to pin a task to a specific existing session.
☑ Allow the user to force a fresh session.
☑ Allow the user to force a checkpoint before migration.
☑ Allow the user to prevent a session from receiving orchestrator-generated messages temporarily.
☑ Allow the user to take over an orchestrated worker directly.
☑ Make user input take precedence over automated orchestration when both target the same session.
☑ Surface automation decisions instead of silently moving work between sessions.

Phase 45 — Failure handling

Fixed architectural requirements

- Glasshouse fails closed for incompatible protocols, missing harnesses, unsafe secret handling, and invalid launch profiles.
- It must not silently fall back to a paid native backend, materially different model family, or weaker tool semantics.

☑ Detect child-process crashes and mark the corresponding session failed.
☑ Preserve terminal output and event history after a worker crashes.
☑ Preserve the most recent checkpoint after a worker crashes.
☑ Allow a failed task to be resumed in the same native session when possible.
☑ Allow a failed task to be handed off to a fresh session when appropriate.
☑ Avoid automatically retrying destructive tasks on another harness without sufficient task-state information.
☑ Detect gateway failure separately from harness-process failure.
☑ Degrade unhealthy gateway resources without affecting unrelated native subscriptions.
☑ Ensure one failed worker cannot terminate unrelated sessions or the entire Glasshouse instance.

Phase 46 — Security and contamination tests

☑ Add automated tests proving one project database cannot be queried through another project’s Glasshouse instance.
☑ Add automated tests proving a session from project A cannot be resumed from project B.
☑ Add automated tests proving canonicalized paths cannot escape the project root through ...
☑ Add automated tests proving symlink targets outside the project root are rejected by Glasshouse-controlled file operations.
☑ Add automated tests proving cmux session metadata cannot bypass project-scope validation.
☑ Add automated tests proving MCP operations remain bound to the active project.
☑ Add automated tests proving memory extraction cannot write into another project’s database.
☑ Add automated tests proving each project’s Glasshouse state is physically separated, so that deleting one project’s state directory removes only that project’s state.

Phase 47 — Observability without spectacle

Fixed architectural requirements

- Telemetry exists for diagnosis, routing, reliability analysis, and product evaluation rather than token-spend gamification.
- Raw token counts, cost, TTFT, TTFC, throughput, errors, outages, and correlations remain measurable, while inferred quality conclusions remain labeled as derived evidence.

☑ Add a debug view showing why the router chose a session or resource.
☑ Add a debug view showing recent lifecycle events for a session.
☑ Add a debug view showing which memories were retrieved for a routed task.
☑ Add a debug view showing estimated cache temperature and the evidence used for that estimate.
☑ Add a debug view showing quota information and whether it is measured, inferred, or unknown.
☑ Add an optional compact route-evidence table showing sample count, TTFC, effective TTFC, TTFT, decode throughput, successful rounds per minute, and observation window when available.
☑ Show failure counts by class instead of presenting one unexplained error percentage.
☑ Show whether latency evidence came from warm, cold, or unknown context.
☑ Show route health, immediate availability, cadence, quota reset, and failure-domain evidence as separate concepts.
☑ Show the strongest measured factors behind the most recent routing decision in concise text.
☑ Show correlations with their sample size and confidence instead of implying precise independence from sparse data.
☑ Keep lifetime token and spend totals out of the default project overview and never present them as achievement counters.
☑ Add a debug view showing memory-extraction inputs and outputs when explicitly enabled.
☑ Keep diagnostic views optional and do not turn them into the normal user experience.
☑ Prefer inspectable text and tables over animated knowledge-graph visualizations.

Phase 48 — CLI ergonomics

☑ Make bare glasshouse open the current project’s interactive TUI.
☑ Add glasshouse session list for a non-interactive project-session summary.
☑ Add glasshouse session new <harness> for starting a project session from the shell.
☑ Add glasshouse memory search <query> for non-interactive project-memory search.
☑ Add glasshouse status for a concise project and resource summary.
☑ Add glasshouse doctor for checking harness executables, optional cmux support, database health, and configuration.
☑ Keep CLI commands project-scoped unless an explicitly administrative command is clearly global.
☑ Avoid requiring a separate initialization command for normal Git repositories unless persistent project configuration becomes necessary.

Phase 49 — Configuration

☑ Support a small user-level Glasshouse configuration file for harness executable paths and optional resource definitions.
☑ Support an optional project-level Glasshouse configuration file for project-specific behavior.
☑ Keep project-level configuration inside the project root when the user explicitly chooses to create it.
☑ Keep secrets out of tracked project configuration.
☑ Make sensible defaults sufficient for Claude Code and Codex when their native executables are already usable from the shell.
☑ Allow automatic routing and memory extraction to be disabled independently.
☑ Allow the user to configure provider-specific quota overrides when automatic telemetry is unavailable.
☑ Allow the user to configure a monthly or rolling monetary budget for metered providers.
☑ Allow the user to configure protected reserve percentages for premium subscriptions.
☑ Allow the user to configure the routing-model fallback chain.
☑ Allow the user to configure workload-tier ceilings for individual models.
☑ Allow the user to configure native-pairing preference strength without hard-coding vendor-specific routing rules.
☑ Allow named response profiles and separate role defaults for orchestrator, worker, reviewer, explainer, and ordinary sessions.
☑ Allow response-profile injection to be disabled independently from automatic routing and memory extraction.
☑ Allow cmux integration to be disabled even when cmux is detected.
☑ Keep configuration schema small until real usage demonstrates a need for additional options.

Phase 50 — Tracked project knowledge as an optional feature

Fixed architectural requirements

- Git-tracked project knowledge is optional and contains only explicitly approved portable knowledge.
- Operational SQLite state, secrets, private raw transcripts, and unredacted event data do not become tracked files.

☑ Keep runtime memory outside the source repository by default.
☑ Add an explicit opt-in command for creating tracked .glasshouse project knowledge.
☑ Export selected durable decisions and constraints into human-readable files only when tracked knowledge is enabled.
☑ Never export raw session histories into the repository automatically.
☑ Never export credentials or provider metadata into tracked project knowledge.
☑ Treat tracked human-readable memory as a projection of canonical project memory rather than requiring it for Glasshouse operation.
☑ Allow teams to review tracked architecture decisions through normal Git workflows when this mode is enabled.

Phase 51 — Evaluation hooks

☐ Measure how many repository exploration operations occur before and after relevant project memory exists.
☑ Measure how often retrieved memory is actually useful to the receiving agent.
☑ Measure how often stale or incorrect memory is retrieved.
☑ Measure how often an old decision causes an agent to add unnecessary implementation complexity.
☑ Measure how often revalidation correctly identifies a decision whose original assumptions no longer hold.
☑ Measure how often agents challenge a remembered decision and whether the challenge was justified.
☑ Measure how often superseded memories are incorrectly resurfaced as current guidance.
☐ Measure whether production-aware checks catch expensive query patterns or scaling assumptions before deployment.
☐ Measure how often one harness successfully continues work from another harness’s checkpoint.
☑ Measure how often automatic routing is overridden by the user.
☑ Measure how often warm-session reuse is chosen over fresh-session creation.
☑ Measure how often memory prevents repetition of a recorded failed approach.
☑ Measure memory-extraction cost separately from interactive coding cost.
☑ Measure routing-model cost and request consumption separately from interactive coding cost.
☑ Measure how often workload-tier classification predicts successful execution without escalation.
☑ Measure how often a low-cost or free route succeeds compared with the premium route it displaced.
☑ Measure the accuracy of estimated subscription headroom against observed throttling and resets.
☐ Measure how often protected quota remains available for high-tier tasks when needed.
☐ Measure how often a critical assumption is refuted before broad implementation versus after substantial edits.
☐ Measure elapsed time, tool rounds, and changed-file churn between the first unsupported premise and its correction.
☐ Measure how much implementation work is discarded or substantially rewritten after an assumption is invalidated.
☐ Measure how often assumption preflights identify the actual premise that later determines success or failure.
☐ Measure false alarms, unnecessary pauses, verification overhead, reviewer cost, and cases where the guardrail increases over-planning.
☐ Compare guardrails disabled, advisory, and risk-gated modes on comparable tasks.
☐ Measure whether fresh-session or cross-harness verification contributes independent evidence rather than agreement alone.
☐ Measure native versus cross-vendor harness-model pairings by task success, usable tool calls, repair loops, effective TTFC, reliability, and user overrides.
☐ Measure how quickly local pairing evidence becomes more predictive than the initial same-vendor prior.
☐ Measure output-token reduction, time to actionable information, profile overrides, missing caveats, and additional steering for each response profile.
☐ Measure response-profile effects separately by harness-model pairing and application mechanism.
☑ Measure routing latency added before interactive task execution.
☐ Measure whether effective TTFC predicts usable agent turns better than raw TTFC, TTFT, or decode throughput.
☑ Measure how often failure-domain evidence prevents a failover onto the same unhealthy upstream.
☑ Measure how often nominally different routes provide separate quota capacity but not independent failure resilience.
☐ Measure how much scarce capacity is consumed by probes and whether passive observations can replace them.
☑ Measure how often sparse, stale, or incorrectly segmented evidence causes a poor routing decision.
☑ Measure estimated versus actual marginal token or request consumption when telemetry permits.
☑ Keep evaluation data local and project-scoped unless the user explicitly exports it.

Phase 52 — Criteria before adding semantic/vector retrieval

Fixed architectural requirements

- Semantic or vector infrastructure is deferred until measured retrieval failures demonstrate that SQLite full-text search and reranking are insufficient.
- It must not be added solely for hypothetical scale or architectural fashion.

☑ Do not add vector retrieval until FTS5 retrieval failures are observed and recorded in real projects.
☐ Define concrete retrieval cases that lexical search cannot solve before selecting an embedding system.
☐ If semantic retrieval is added, combine it with lexical retrieval rather than replacing lexical retrieval.
☐ Keep project isolation physically intact when adding embeddings.
☐ Ensure semantic retrieval respects memory lifecycle status and does not resurrect superseded knowledge as current truth.
☐ Evaluate semantic retrieval on real Glasshouse queries before making it part of the default path.

Phase 53 — Criteria before adding graph storage

Fixed architectural requirements

- Graph storage is deferred until concrete multi-hop relationship queries cannot be served adequately by the existing relational model.
- No graph database is introduced as speculative infrastructure.

☐ Do not add a graph database solely to visualize project memory.
☑ Add explicit typed relationships in SQLite first when relationships become useful.
☑ Introduce relationships such as supersedes, affects, and implemented_by only when they improve real queries.
☐ Evaluate whether SQLite relations are insufficient before adopting a dedicated graph database.
☑ Keep the user-facing project-knowledge view useful even if no graph database is ever added.

Phase 54 — Criteria before deeper cmux coupling

Fixed architectural requirements

- cmux remains an optional presentation integration.
- Core session ownership, PTY management, routing, memory, and lifecycle semantics must not depend on cmux.

☑ Keep cmux optional until repeated usage proves external-pane workflows are essential.
☑ Avoid depending on undocumented cmux internals when a stable command or API surface exists.
☑ Keep embedded Glasshouse sessions fully functional even if cmux changes or disappears.
☑ Add richer cmux workspace automation only after the basic expose-and-focus workflow proves useful.

Phase 54A — Setup and portability completion criteria

☑ Consider onboarding usable when a new user can launch Glasshouse and see installed supported harnesses without manually editing a config file.
☑ Consider onboarding usable when the user can skip all provider configuration and still use native detected harnesses.
☑ Consider settings usable when the user can return later and configure a provider without rerunning the entire setup.
☑ Consider launch profiles usable when the same installed Claude Code binary can be started natively or with an alternate compatible provider without modifying the user’s normal Claude configuration.
☑ Consider interactive gateway use valid only when the session is operated by an installed compatible harness and Glasshouse does not create a replacement agent loop.
☑ Consider response profiles minimally usable when at least one supported harness can apply a selected profile through a native mechanism or the bounded additive fallback while preserving coding instructions.
☑ Consider gateway mode usable when two concurrent Glasshouse instances can run isolated local gateways without port or credential collisions.
☑ Consider provider setup usable when OpenRouter, one generic OpenAI-compatible endpoint, and one generic Anthropic-compatible endpoint can be configured and tested.
☑ Consider free-pool support usable when at least one configured zero-cost or free-tier model can perform a disposable Glasshouse support job.
☑ Consider cross-platform support stable only after PTY/session smoke tests pass on macOS, Linux, and native Windows CI runners.

Phase 55 — V1 completion definition

Fixed architectural requirements

- V1 remains a local terminal control plane built around installed native harnesses, project-local SQLite, adapter boundaries, PTY sessions, and basic evidence-aware routing.
- V1 does not require a Glasshouse cloud service, replacement coding harness, browser frontend, microservice topology, vector database, graph database, or broad protocol-translation layer.

☑ Consider V1 usable when Glasshouse can start in a Git project and isolate all state to that project.
☑ Consider V1 usable when Claude Code can run as a fully interactive embedded native session.
☑ Consider V1 usable when Codex can run as a fully interactive embedded native session.
☑ Consider V1 usable when the user can switch between multiple live native sessions without restarting them.
☑ Consider V1 usable when one session can be designated as orchestrator and spawn at least one visible worker session.
☑ Consider V1 usable when every interactive native, direct-provider, or gateway-backed session records a real owning harness and launch profile.
☑ Consider V1 usable when a compatible vendor-native pairing receives an inspectable initial prior without overriding stronger observed evidence or user choice.
☑ Consider V1 usable when a response profile can control user-facing communication without reducing verification or replacing native harness coding instructions.
☑ Consider V1 usable when a worker completion event can reliably wake or notify the orchestrator.
☑ Consider V1 usable when the user can enter and directly control any orchestrated worker.
☑ Consider V1 usable when project-specific durable memory can store the six initial memory kinds.
☑ Consider V1 usable when project memory can be searched with FTS5.
☑ Consider V1 usable when a small portable checkpoint can hand work from one harness to another.
☑ Consider V1 usable when a simple router can choose between an existing relevant session and a fresh session using inspectable rules.
☑ Consider V1 usable when at least one authoritative or observed provider quota can be displayed in native units.
☑ Consider V1 usable when opaque subscription capacity can be represented as unknown or estimated without fabricating exact token counts.
☑ Consider V1 usable when a configurable cheap/free/local routing model can assign workload tiers with deterministic fallback.
☑ Consider V1 usable when protected premium reserve can influence a routing decision.
☑ Consider V1 usable when a substantial high-risk task can record a small set of critical assumptions with evidence state and create a checkpoint before broad implementation.
☑ Consider V1 usable when routing explanations show workload tier, session affinity, resource capacity, and the primary reason for selection.
☑ Consider V1 usable when at least one gateway-backed route records classified success and failure outcomes plus TTFT or TTFC and can cite that evidence in a routing explanation.
☑ Consider V1 usable when cmux integration can expose or spawn a session externally without being required for normal operation.
☑ Consider V1 complete only after project-isolation and cross-contamination tests pass.

Phase 56 — Harness–subscription decoupling: choose the harness, route the subscription and model

Recorded 2026-08-31 from the user's instruction of record: *"I want to be able to choose the harness, not the provider, because some harnesses are more efficient in different tasks."* A coding harness (Claude Code, Codex, Gemini's CLI, and the others Phase 9 adapts) is today bound in practice to its vendor's subscription and model. Phase 9J already stores harness vendor, model developer, serving provider and wire protocol as independent facts; this phase makes that independence usable: a subscription is a routing resource with its own rules, Glasshouse's bundled gateway serves any supported harness from any subscription or model it can translate to the harness's native protocol, and the choice of harness is made on evidence of which harness is efficient at which kind of task. Line 497's standing rule — no broad cross-protocol translation until concrete pairs require it — is not repealed; this phase is the concrete requirement it was waiting for, one named pair at a time, each behind an end-to-end test. Phase 55's fixed requirements hold: no replacement harness, no cloud service.

☑ Allow a user to choose the coding harness for a task independently of which provider, subscription, or model serves it.
☑ Treat a subscription — a Claude, ChatGPT/Codex, or Gemini plan, or an API key — as a routing resource with its own rules, separate from any harness that consumes it.
☑ Allow a subscription rule to state which harnesses, workload tiers, and job kinds the subscription may serve, and which it must never serve.
☑ Serve any supported harness through Glasshouse's bundled API gateway from any subscription or model whose wire protocol the gateway can translate to the harness's native protocol.
☑ Translate between wire protocols at the gateway for concrete harness/provider pairs as each is required, recording every supported pairing and every refused one by name.
☑ Keep a harness's native tooling — editing, shell, repository, and tool-call behaviour — intact when it is served by a non-native provider, and refuse the pairing by name when it cannot be kept.
☑ Record per-harness task efficiency — tokens, wall-clock, request count, and outcome by task class — so that harness choice can rest on evidence rather than on which vendor bills for it.
☑ Prefer, for a stated task the user has not assigned a harness to, the harness with the better observed efficiency for that task class, and say why.
☑ Give the routing candidate set a subscription and model axis, so the same harness is ranked across every subscription allowed to serve it.
☑ Never charge a task to a subscription the user's rules did not allow for that harness or tier, and announce which subscription served each session.
☑ Keep the decoupling opt-in per launch profile, so an existing profile keeps its native pairing until the user changes it.
☑ Cover each supported harness/provider/protocol pairing with an end-to-end test through the shipped binary against a fixture upstream before offering it.

Phase 56A — Entitlement pool and subscription broker: several accounts, one scheduler

Recorded 2026-08-31 from the user's instruction of record, refining Phase 56: *"rather than one 20x account and one enormous bucket, two 5x entitlements the scheduler consumes evenly — optimising around reset boundaries; workers 1–7 on Claude subscription A, 8–14 on B, 15–17 on Codex, 18–20 on OpenRouter, without the orchestrator caring which entitlement produced the inference; the architecture is Entitlement → Provider → Protocol → Harness rather than treating 'Claude' as one provider; pooling API credits and subscription entitlements."* The unit of capacity becomes the **entitlement** — a specific subscription or credit account with its own authentication, remaining capacity and reset — and a **broker** stands between every harness and the pool, so no harness process is bound to one account's quota. Today's per-resource machinery (Phase 32's bands and reset proximity, Phase 33's throttle and health, Phase 35D's reserve rules, Phase 36's affinity) becomes per-entitlement; Phase 56's rules (1946–1947) and announcement (1954) apply to each entitlement. The layering is explicit and each layer separately replaceable: harness → protocol adapter → authentication → entitlement → inference model. Nothing here invents a number the provider does not expose.

☑ Model an entitlement — a specific subscription or API-credit account such as Claude Max A, Claude Max B, ChatGPT Pro, OpenRouter credits, or an API key — as the unit of capacity, distinct from the vendor, the provider adapter, the wire protocol, and the harness.
☑ Allow several entitlements of the same vendor and plan to coexist in one pool, each with its own authentication, remaining capacity, and reset time.
☑ Keep the layering explicit and separately replaceable: harness, protocol adapter, authentication, entitlement, inference model.
☑ Track, per entitlement, remaining capacity, time until reset, recent throttling, and the models it can serve, from the telemetry the provider actually exposes.
☑ Score entitlements for a new job by available capacity, time until reset, recent throttling, session affinity, and model availability, and choose by that score rather than by round-robin.
☑ Burn an entitlement aggressively when its reset is near and its remainder would otherwise expire, and preserve one whose reset is far.
☑ Distribute independent workers across the pool while keeping a long-running session sticky to the entitlement that holds its context and cache, unless a rule or exhaustion forces a move.
☑ Present the whole pool to every harness through the broker, so that a single harness process is no longer bound to one account's quota.
☑ Fall back across the pool in a stated order on exhaustion or throttling — subscription to subscription to API credits — and record every fallback with its reason.
☑ Let the user state per-entitlement rules — allowed harnesses, tiers, job kinds, and spend ceilings — and never let the broker exceed them.
☑ Show the pool in one inspectable view — each entitlement's capacity, reset, throttle history, and what it served — and announce the entitlement that served each session.
☑ Keep every entitlement's credential isolated: tokens and keys never mixed across accounts, never logged, never written into a project file.
☑ Cover the broker with an end-to-end test against fixture entitlements, including a reset boundary and an exhaustion fallback, before offering it.

Phase 57 — Context firewall: tool-output compaction between harness and model

Recorded 2026-09-01 from the user's instruction of record: a coding model should not automatically receive tens of thousands of tokens of grep hits, logs, test output, and generated files when a fraction is relevant — but a false negative costs a wrong engineering decision while a false positive only costs tokens, so the firewall optimizes hard against false negatives, preserves every original byte for recovery, and makes no substantive coding decision. It reduces and ranks; it never generates. Reduction runs a ladder — passthrough, deterministic compaction, then optional semantic reduction through the existing disposable-job and provider machinery — and the whole subsystem is off by default, provider-agnostic, harness-abstracted, fail-open, and measured from day one. The semantic reducer is a disposable support job: Phase 39's job-kind roster, Phase 9I's free-pool routing, and Phase 56A's per-entitlement job-kind rules apply to it unchanged, and `disposable_interface.rs`'s variant-roster tripwire firing on the new job kind is that design working, not a regression.

☑ Normalize harness tool results into a harness-agnostic form before any reduction, so the firewall core never depends on one harness's JSON shapes.
☑ Pass small tool results through untouched below a configurable raw-passthrough threshold.
☑ Reduce oversized tool output deterministically before any model is asked — duplicate hits, repeated log, stack, and progress lines, blank-line runs, and safely identifiable generated noise.
☑ Never let deterministic reduction drop content it cannot positively classify as redundant; uncertain material always survives.
☑ Preserve every reduced result's original bytes locally, addressable by a stable per-session reference the session can later expand.
☑ Reconstruct every forwarded result verbatim from original bytes — reduction selects, ranks, and annotates candidates; it never generates replacement evidence text.
☑ Annotate each reduced result with one compact provenance header stating original and forwarded sizes, retained-candidate counts, and the raw-result reference.
☑ Record per-reduction telemetry — raw, deterministic, and forwarded token counts, mode, tool, and any bypass reason — through the existing routing-evidence ledger rather than a parallel metrics store.
☑ Track raw-expansion requests as the primary recall signal, so a recall regression is measurable before any savings claim is believed.
☑ Restrict reduction to an explicitly configurable tool-eligibility list, defaulting to search, read, and log-shaped outputs and never to edits, writes, permission or security results, small outputs, error details, or unknown shapes.
☑ Preserve exit status, stderr, interruption, and failure semantics when reducing command output, and pass through unchanged whenever an adapter cannot guarantee a tool's semantics.
☑ Support four firewall modes — off, shadow, safe, aggressive — where off is byte-identical to no firewall and shadow runs the full pipeline while always forwarding the original.
☑ Keep semantic reduction disabled unless the user's configuration explicitly names a reducer, in every mode.
☑ Bridge Claude Code through its native post-tool hook with per-session registration that never edits the user's own settings files and never disturbs unrelated hooks.
☑ Verify at session start that the installed harness supports tool-output replacement, and fall back to shadow mode with a stated reason when it does not.
☑ Adapt each supported Claude Code tool's output shape explicitly and pass unknown or unsupported shapes through unchanged.
☑ Keep firewall state and raw-result stores separated per session, so concurrent sessions and subagents never observe one another's reductions.
☑ Route semantic reduction as a disposable support job through the existing provider abstraction and per-entitlement job-kind rules, never through a firewall-private provider client.
☑ Give the reducer only the stated task, tool query, and candidate output — never the conversational transcript — and keep its prompt small.
☑ Require structured candidate-selection output from the reducer and rebuild the final result from trusted original candidates by id.
☑ Bias thresholds toward inclusion: safe mode retains uncertain candidates by default, and aggressive mode states plainly that it trades recall for reduction.
☑ Fail open on every reducer failure — timeout, transport, rate limit, schema, validation, or outage — forwarding the original output with a recorded bypass reason and never an empty result.
☑ Support pinned models and free-router aliases through the existing provider and free-model configuration, validating reducer output regardless of which model a router answered with.
☑ Respect existing secret handling and privacy policy before any external reduction: local-only operation, path and tool exclusions, secret-file defaults, and no transmission to a provider the user has not configured.
☑ Let the session expand a suppressed result — whole, by candidate, by file, or by range — through a supported Glasshouse surface rather than an invented side channel.
☑ Compare shadow-mode reductions against forwarded originals so recall and savings claims rest on recorded evidence rather than on the compression ratio alone.
☑ Show the firewall's mode and per-session aggregate savings in the existing status and settings surfaces without cluttering the primary workflow.

Phase 58 — Context economy: cache-stable translation, entitlement-aware reduction, and a measured token budget

Recorded 2026-09-02 from the user's instruction of record, after a side-by-side comparison with Headroom (headroomlabs-ai/headroom, Apache-2.0, a local context-compression proxy): *"take everything which would benefit us in a meaningful way and ingest it — make sure this is documented going forward so future orchestrators won't forget."* `docs/product/design-decisions.md` (*"Headroom, compared"*) records the comparison, what was taken, what was refused by name, and the order of work. The lines below are the taken half. Nothing here weakens the relay's byte-for-byte promise, the firewall's fail-open rule, or the response profiles' native-mechanism route; each line embeds in the mechanism Glasshouse already owns.

Cache-stable translation — the gap the comparison exposed: a default Claude Code launch on any translated pairing is refused today because its prompt-cache markers are refused by field name.

☑ Carry a harness's prompt-cache markers across a translated pairing where the target protocol has an equivalent, and strip them with a recorded reason where it does not, instead of refusing the request.
☑ Keep a default Claude Code launch usable on every supported translated pairing without the user disabling prompt caching.
☑ Serialize translated requests deterministically — stable tool order and stable JSON Schema key order — so an unchanged prompt prefix stays byte-identical across turns.
☑ Never alter the bytes of a message already sent upstream in an earlier turn of the same session on a translated pairing, as the relay already guarantees for native ones.
☑ Supply a stable per-session prompt-cache key on targets that accept one when the harness did not set its own.
☑ Measure prompt-cache read and creation tokens per exchange where the provider reports them, and show the per-session cache ratio beside the routing evidence.

Entitlement-aware reduction — a subscription pays in rate limits and context window, a metered key pays in tokens, local inference pays in latency; one threshold cannot serve all three.

☑ Key the context firewall's reduction policy on the serving entitlement's kind, with per-kind thresholds that default to today's values.
☑ Allow a launch profile or an entitlement to declare its reduction policy explicitly, overriding the kind's default.

A local reducer the user installs — Headroom's compressors are more developed than the deterministic ladder and run locally; use them, do not rewrite them.

☑ Allow the semantic reducer to be a local out-of-process tool the user installs, selected by configuration beside the model-backed reducer, with the same provenance header, raw preservation, and expansion path.
☑ Treat a local reducer's absence, timeout, or failure as a bypass with a stated reason, never as an error the session sees.
☑ Record which reducer produced each reduction, so savings and recall are attributable per reducer.

A savings readout that is a query, not an estimate.

☑ Report token savings by purpose — firewall reduction, response profile, translation — from the evidence ledger's own rows with denominators, so a savings claim is a query over recorded exchanges.
☑ Provide a seeded, offline proof fixture for the firewall's deterministic ladder so its reduction ratios are reproducible without any provider.

Effort and learning — evaluate before offering, and export what memory already knows.

☑ Evaluate a clamp-only per-turn effort reduction on translated pairings for turns that only resume after a tool result, never raising effort and never touching the byte-for-byte relay, before offering it.
☑ Offer an opt-in export of remembered constraints and failed approaches into a marker-delimited block of the harness's native local instruction file, gitignored by default, replacing only its own block on re-export.



────────

Maybe / Experimental Capabilities

These capabilities are intentionally outside the required V1 implementation path. Each should be implemented only after its prerequisite core behavior is stable and real usage provides a measurable problem, baseline, and evaluation method.

Maybe A — Cross-session file claims

☐ Consider adding a project-scoped file-claim registry shared by all active Glasshouse sessions.
☐ Allow a session to claim a file when it begins an edit-oriented operation on that file.
☐ Release a session’s file claim automatically when the relevant turn completes.
☐ Release abandoned file claims when the owning session exits, fails, or exceeds a safe stale-claim timeout.
☐ Associate every file claim with the owning Glasshouse session ID rather than only a process ID.
☐ Keep file claims project-scoped so a claim can never affect another project.
☐ Surface active file claims in the session overview when they are relevant to parallel work.
☐ Treat file claims as coordination metadata rather than as source-control ownership.
☐ Prefer soft claims and warnings before implementing hard filesystem locks.
☐ Do not modify repository file permissions merely to represent an agent claim.
☐ Do not rely on operating-system file locking as the canonical coordination mechanism unless a concrete need is demonstrated.

Maybe B — Cross-session edit-intent hooks

☐ Consider using harness hooks or tool-call events to detect when a session is about to read or edit a file.
☐ Record an edit_intent event before a session performs a file-modifying operation when the harness exposes enough information.
☐ Record a read_intent event when useful for detecting high-risk concurrent work.
☐ Allow the coordination layer to compare new edit intent with active file claims from other sessions.
☐ Allow the coordination layer to compare new edit intent with recently modified but not yet reconciled files from other sessions.
☐ Keep intent detection best-effort when a harness does not expose structured pre-tool hooks.
☐ Avoid pretending terminal-output inference is equivalent to a structured pre-edit hook.
☐ Preserve the user’s ability to bypass coordination when Glasshouse cannot determine intent confidently.

Maybe C — Parallel conflict prediction

☐ Consider predicting likely conflicts before two sessions modify overlapping files.
☐ Treat two simultaneous edit intents for the same file as a high-confidence conflict risk.
☐ Treat edits to adjacent files with shared interfaces as a lower-confidence conflict risk.
☐ Allow the router to use task plans, touched-file history, Git diffs, and current claims as conflict-prediction inputs.
☐ Keep conflict prediction advisory when the expected touched files are inferred rather than observed.
☐ Avoid expensive whole-repository analysis merely to predict every possible overlap.
☐ Show the user which files or interfaces caused a conflict warning.
☐ Distinguish direct file overlap from broader semantic overlap in warnings.

Maybe D — Queue conflicting work

☐ Consider allowing Glasshouse to queue a worker turn when it would edit a file actively claimed by another session.
☐ Keep queuing disabled by default until the user explicitly enables parallel-work coordination.
☐ Allow the router to choose a non-conflicting alternative task while a worker waits.
☐ Wake a queued worker automatically when the conflicting claim is released.
☐ Preserve the queued worker’s existing native session instead of restarting it.
☐ Show queued state clearly so the user understands why a worker is not progressing.
☐ Avoid queueing read-only work merely because another session is editing the same file unless experiments show this is necessary.

Maybe E — User override and reconcile-later mode

☐ Allow the user to override a conflict warning and let both sessions continue.
☐ Allow the user to choose queue, continue anyway, or reconcile later when Glasshouse predicts a direct edit conflict.
☐ Record user conflict overrides in the event log.
☐ Mark files edited concurrently under an override as requiring reconciliation.
☐ Surface unresolved reconciliation markers in the project overview.
☐ Allow the orchestrator to receive a reconciliation task after both conflicting workers finish.
☐ Allow the user to assign reconciliation to a new worker session.
☐ Never silently discard one worker’s changes merely because another worker claimed the file first.

Maybe F — Turn-scoped coordination

☐ Prefer turn-scoped claims over long-lived session ownership so agents can work side by side without unnecessarily serializing the project.
☐ Consider releasing edit claims at successful turn completion even when the native session remains open.
☐ Allow a session to renew a claim when its next turn continues work on the same file.
☐ Avoid treating an entire feature branch or directory as locked when only one file is actively being modified.
☐ Consider directory- or interface-level claims only after real usage demonstrates repeated cross-file conflicts.
☐ Keep coordination granular enough that unrelated workers can continue in parallel.

Maybe G — Read visibility during active edits

☐ Experiment with warning readers when another session is actively editing the requested file.
☐ Prefer allowing reads of the last committed or current filesystem state with an explicit stale-or-changing warning before implementing hard read blocking.
☐ Consider exposing the owning session and claim age to the reader.
☐ Consider allowing a reader to request the latest checkpoint or diff from the owning session instead of reading unstable intermediate state.
☐ Only consider hard read blocking if experiments show that reading mid-edit state causes meaningful agent failures.
☐ Never make hard read blocking the default without evidence that its coordination benefit outweighs lost parallelism.

Maybe H — Orchestrator-aware conflict handling

☐ Consider notifying the orchestrator when two workers are likely to touch the same files.
☐ Allow the orchestrator to re-plan work before a predicted direct conflict occurs.
☐ Allow the orchestrator to serialize only the conflicting portion of otherwise parallel tasks.
☐ Allow the orchestrator to instruct one worker to work on tests, documentation, or analysis while another owns the conflicting implementation file.
☐ Deliver file-claim release events to a waiting orchestrator when relevant.
☐ Keep conflict handling transparent so the user can inspect why the orchestrator changed a worker’s plan.

Maybe I — Evaluation criteria for file coordination

☐ Measure how often parallel sessions actually produce overlapping file edits before enabling automatic file coordination by default.
☐ Measure how often a conflict warning predicts a real Git or semantic merge conflict.
☐ Measure how much wall-clock time is lost to unnecessary queueing.
☐ Measure whether soft file claims reduce failed parallel work without materially reducing concurrency.
☐ Measure whether hard read blocking improves outcomes before considering it for production use.
☐ Measure how often users override warnings and whether those overrides later require reconciliation.
☐ Keep the feature experimental until conflict prevention provides a measurable benefit over ordinary Git-based reconciliation.

Maybe L — Convergent co-editing of a contended file

Intent and boundaries

☐ Consider letting two sessions work on the same file concurrently in isolated buffers, rather than serializing them behind a claim, when both genuinely need it.
☐ Treat this as a third option beside queueing and reconcile-later, not a replacement for either, and keep queueing as the default until measurement justifies otherwise.
☐ Keep the isolated buffer a real working tree the session can compile and test in, because a change nobody can verify is not an implementation.
☐ Make convergent co-editing a setting the user can turn off, whose off state is ordinary queueing and remains a coherent way to work.
☐ Make the coordination mode inspectable per contended file, so a user can see whether a file was queued, co-edited, or reconciled after the fact.
☐ Record which mode produced each reconciliation, so the two can be compared on real work rather than argued about.
☐ Do not build automatic semantic merge; a merged file neither author would accept is worse than a queue.
☐ Do not make this part of MVP or a V1 completion requirement.

Mutual visibility

☐ Allow a session to see another session's in-progress changes to a file both are editing, before either has finished.
☐ Present another session's in-progress change as an unfinished proposal rather than as committed truth.
☐ Allow a session to adapt its own pending change in response to what another session is doing to the same file.
☐ Prefer a single read at finalization over continuous mutual adaptation, so two sessions cannot oscillate against each other's stale state.
☐ Record when a session adapted its change because of another session's work, so the benefit can be measured rather than assumed.

The join barrier and reconciliation

☐ Hold reconciliation until every session editing a contended file has declared its work on that file finished.
☐ Reconcile the buffers into the real file only when the result preserves what each session intended.
☐ Escalate to the orchestrator or the user, with both versions visible, when reconciliation cannot preserve both intents.
☐ Never silently invent a merge that neither session wrote.
☐ Never silently discard one session's change because another session finished first.
☐ Allow a session that is blocked at the barrier to continue with its other files rather than idling.
☐ Surface a contended file's barrier state so the user can see why reconciliation has not happened.

Evaluation before promotion

☐ Measure how often two sessions genuinely require the same file, since a partition that avoids the case entirely is cheaper than any protocol for it.
☐ Measure how often convergent editing yields a reconciliation both sessions accept without escalation.
☐ Measure wall-clock against simply queueing the second session, which is the honest baseline.
☐ Measure how often mutual visibility caused a session to adapt in a way that later proved wrong.
☐ Measure escalation rate and the cost of an escalation, including the reviewer's time.
☐ Promote this beyond experimental status only if it beats queueing on real work without producing merges an author would disown.

Maybe J — Experimental agent diagnostic feedback bus

Intent and scope

☐ Treat this as an explicitly post-MVP experiment whose quality impact and latency cost must be measured before it can influence default behavior.
☐ Use deterministic repository diagnostics to catch newly introduced mechanical errors before they survive until a CI run.
☐ Target deterministic findings across source code, configuration, schemas, infrastructure definitions, queries, documentation, generated artifacts, dependency metadata, and repository-specific policy rather than assuming diagnostics are limited to conventional programming-language lint.
☐ Return concise structured diagnostics to the responsible agent while the edit is still recent enough to repair cheaply.
☐ Treat visual squiggles as an optional human-facing projection of the same diagnostic records rather than as the canonical capability.
☐ Keep the agent-facing diagnostic protocol useful in a terminal-only workflow even when Glasshouse has no source-code editor view.
☐ Do not present diagnostics as proof that behavior, architecture, or requirements are correct merely because code compiles and lint passes.
☐ Do not replace repository CI, task-boundary validation, tests, or human review.
☐ Do not make this capability part of the MVP or a V1 completion requirement.

Hook placement and validation tiers

☐ Use PreToolUse only for checks that can be decided cheaply from the proposed operation, such as protected or generated files, project-scope escape, known file claims, obvious secret material, forbidden paths, or unexpectedly large edits.
☐ Avoid running ordinary compilers or linters in PreToolUse because the proposed file state does not yet exist unless an adapter can safely construct a temporary overlay.
☐ Use PostToolUse, a structured file-change event, or the closest reliable harness-specific equivalent for syntax, lint, type, import, and language-server diagnostics after an edit succeeds.
☐ Allow PostToolUse diagnostics to be returned as concise model-visible feedback so the same agent can repair newly introduced problems.
☐ Run cheap deterministic checks synchronously only when they remain inside a strict configurable latency budget.
☐ Debounce and coalesce rapid successive edits before running semantic or project-aware diagnostics.
☐ Run slower lint, typecheck, compiler, and test commands asynchronously when the result can safely arrive at the next agent decision point.
☐ Reserve full-project validation and relevant test suites for semantic task boundaries, stop checks, explicit user requests, or pre-commit and pre-push gates.
☐ Prefer persistent project-scoped language servers or validator processes when they materially reduce repeated startup cost.
☐ Do not introduce a global validator daemon or allow diagnostic state to cross project boundaries.
☐ Define a language-, tool-, and ecosystem-independent validator adapter interface rather than hard-coding a catalogue of linters or compilers.
☐ Allow adapters to ingest Language Server Protocol diagnostics, SARIF, structured compiler or analyzer output, repository-defined machine-readable formats, and carefully configured command output parsers.
☐ Support validators that report file ranges, whole-file findings, project-wide findings, cross-file relationships, or findings without a meaningful source location.
☐ Discover candidate validators from explicit Glasshouse configuration and, when safe and unambiguous, from repository manifests, task runners, pre-commit configuration, and CI definitions.
☐ Treat tools such as Ruff, ESLint, TypeScript language services, rust-analyzer, and cargo check only as examples and conformance fixtures for the generic adapter contract.
☐ Allow the same adapter contract to cover languages, build systems, SQL and schema validation, API specifications, infrastructure as code, security and policy scanners, documentation checks, and project-specific deterministic validators.
☐ Keep arbitrary repository commands opt-in, sandboxed through the active harness policy, time-bounded, and explicit about the files or project scope they inspect.

Diagnostic identity and delivery

☐ Represent a diagnostic with scope, severity, message, producing validator, observation time, stable rule or error code when available, optional project-relative file and source range, and the relevant file revision, content hash, or project revision when available.
☐ Associate diagnostics with the Glasshouse session and edit event that caused their observation when attribution is reliable.
☐ Distinguish diagnostics introduced by the current edit from pre-existing repository debt.
☐ Prefer reporting newly introduced errors and relevant warnings instead of dumping the repository’s complete diagnostic backlog into agent context.
☐ Discard, replace, or visibly mark diagnostics stale when their recorded file revision no longer matches the current file.
☐ Prevent late asynchronous results from being attributed to a newer edit or a different worker.
☐ Deduplicate repeated unchanged diagnostics across consecutive edits.
☐ Keep model-visible feedback short and provide a separate inspectable view for full details.
☐ Show optional squiggles only in a future Glasshouse diff or code view that can map diagnostics to exact source positions.
☐ Fall back to a compact terminal list containing scope, optional file and location, rule, and message when no visual code surface exists.
☐ Never auto-apply a linter or compiler suggestion that changes source behavior without an explicit agent action or user-approved policy.

Safety, gating, and latency targets

☐ Keep diagnostic feedback advisory and non-blocking by default during the experiment.
☐ Allow high-confidence deterministic errors to request immediate agent repair without silently undoing the completed edit.
☐ Require explicit opt-in before a diagnostic class can block an edit, stop task completion, or prevent a commit or push.
☐ Keep permission and security policy checks separate from ordinary code-quality warnings.
☐ Target a warm PreToolUse policy-check overhead below 5 ms at p95 on representative projects.
☐ Target synchronous per-edit syntax or incremental diagnostic overhead below 100 ms at p95 on representative projects.
☐ Record cold-start cost separately from steady-state cost for language servers and validation tools.
☐ Disable or demote a validator automatically for the current session when repeated timeouts or crashes make it disruptive, while surfacing the degradation.
☐ Make per-validator timeout, debounce interval, severity threshold, and blocking policy configurable.
☐ Ensure one slow validator cannot stall unrelated sessions or the whole Glasshouse instance.

Evaluation criteria before promotion

☐ Measure how many newly introduced deterministic findings across supported validator families are caught before CI or another downstream validation gate.
☐ Measure the reduction in CI failures attributable to deterministic issues that the feedback bus was capable of detecting.
☐ Measure how often an agent repairs a reported diagnostic successfully on its next edit.
☐ Measure additional tool calls, tokens, and wall-clock time caused by diagnostic feedback.
☐ Measure synchronous hook overhead and end-to-end turn overhead separately at p50, p95, and p99.
☐ Measure cold-start overhead and steady-state overhead separately.
☐ Measure false positives, stale-result delivery, incorrect edit attribution, duplicate feedback, and diagnostics ignored by agents.
☐ Measure how often inherited repository debt is incorrectly presented as a regression caused by the active worker.
☐ Measure coverage and usefulness separately across languages, configuration and schema validation, infrastructure, documentation, security and policy analysis, and repository-specific validators.
☐ Measure whether immediate feedback improves task success rather than merely increasing diagnostically clean intermediate states.
☐ Compare advisory-only, agent-repair, and blocking modes before enabling any gate by default.
☐ Promote the capability beyond experimental status only if it measurably reduces avoidable failures without materially increasing turn latency, context noise, or agent repair loops.

Maybe K — Experimental session drift and rework detector

Intent and boundaries

☐ Experiment with detecting when an active agent session appears to be compounding an invalid premise, drifting from the requested task, or repeatedly repairing its own avoidable changes.
☐ Base detection on observable project and lifecycle evidence rather than attempting to infer private model reasoning.
☐ Keep detection model- and harness-independent and avoid naming any vendor model as the source of the general failure mode.
☐ Treat the detector as an advisory early-warning system rather than an autonomous judge of implementation quality.
☐ Do not make the detector part of MVP or a V1 completion requirement.

Candidate observable signals

☐ Consider repeated edits and reversions in the same files or symbols without corresponding acceptance-criterion progress.
☐ Consider rapid growth in touched files, dependencies, abstractions, or compatibility layers beyond the task’s recorded initial scope.
☐ Consider repeated test, compiler, diagnostic, or runtime failures that contradict the active approach rather than merely expose a local typo.
☐ Consider multiple checkpoint summaries that materially change the claimed root cause or implementation direction.
☐ Consider repeated attempts to preserve an assumption after direct repository or runtime evidence contradicts it.
☐ Consider long sequences of tool activity without a successful probe, passing milestone, reduced uncertainty, or completed acceptance criterion.
☐ Consider an implementation budget overrun together with unresolved critical assumptions as a stronger signal than either condition alone.
☐ Keep each signal inspectable and avoid combining weak signals into an unexplained proprietary score.
☐ Attribute observations to concrete revisions, tool events, tests, files, and checkpoints when possible.
☐ Distinguish productive iteration on a difficult problem from circular churn by requiring multiple independent signals before escalating.

Response policy

☐ Start with a quiet session marker and concise evidence summary rather than interrupting the agent immediately.
☐ Escalate to an orchestrator or user warning only when signal strength, accumulated cost, or risk crosses a configurable threshold.
☐ Suggest the cheapest next action among verify premise, run a focused probe, inspect diff, checkpoint, ask user, obtain fresh review, hand off, re-plan, or stop.
☐ Allow a fresh-context reviewer to examine the premise and current evidence without inheriting the original session’s full persuasive narrative.
☐ Require any reviewer recommendation to cite observable evidence and identify what would falsify its conclusion.
☐ Never discard, revert, kill, or migrate a session automatically solely because the detector reports likely drift.
☐ Preserve the user’s ability to continue deliberately when exploration is expected or the detector is wrong.
☐ Decay or clear drift state after a validated milestone, supported premise, successful correction, or explicit user acknowledgement.

Evaluation before promotion

☐ Measure precision and recall against human-labelled cases of genuine wasted rework, productive exploration, and ordinary debugging.
☐ Measure how much earlier the detector identifies a refuted premise than the unassisted session does.
☐ Measure avoided discarded work, wall-clock time, tool rounds, model usage, and changed-file churn.
☐ Measure interruption cost, false alarms, ignored warnings, premature abandonment, and cases where intervention makes performance worse.
☐ Measure whether detector explanations lead to successful verification or merely cause another model-generated planning loop.
☐ Compare user-only review, same-session self-review, fresh-session review, cross-harness review, and deterministic evidence checks.
☐ Promote the detector only if it reduces costly rework without materially suppressing useful exploration or increasing supervisory burden.

Explicit Non-Goals for V1

☐ Do not build a new coding model.
☐ Do not build a replacement for Claude Code, Codex, or Antigravity.
☐ Do not build a proprietary hidden subagent runtime.
☐ Do not build a terminal multiplexer.
☐ Do not require cmux.
☐ Do not build a global personal-memory system.
☐ Do not build a cloud account system.
☐ Do not build team synchronization.
☐ Do not build a vector database before demonstrated need.
☐ Do not build a graph database before demonstrated need.
☐ Do not build decorative knowledge-graph visualizations.
☐ Do not put a universal model proxy underneath native subscription harnesses in V1.
☐ Do not automatically replay full old conversations when a small checkpoint is sufficient.
☐ Do not allow memory from one project to enter another project’s retrieval path.
☐ Do not hide orchestration decisions from the user.
☐ Do not make an autonomous swarm the default interaction model.

────────

Product Rules

☐ Every worker must remain a real session the user can enter.
☐ Every interactive Glasshouse session must be operated by a real installed coding harness.
☐ A provider, direct API, router, or gateway is a backend resource for a harness, not an interactive coding harness by itself.
☐ Glasshouse must not build a replacement agent loop merely because a model is reached through an API or gateway.
☐ Native harness behavior must remain available unless Glasshouse has a concrete technical reason to intercept it.
☐ Project isolation must be enforced structurally rather than by convention.
☐ Durable memory must capture expensive-to-rediscover knowledge rather than conversation volume.
☐ Current source code remains stronger evidence than stale project memory.
☐ Historical decisions must retain rationale and context but must not become timeless constraints by default.
☐ Explicit current requirements may supersede ordinary historical decisions without forcing compatibility complexity.
☐ Security, correctness, and externally imposed invariants require stronger evidence before being superseded.
☐ Glasshouse should prefer revisiting an obsolete decision over engineering around it.
☐ Implementation agents should optimize for the simplest secure and production-appropriate design, not merely the smallest diff.
☐ A solution that works on toy data is not complete when obvious production scale changes its complexity characteristics.
☐ Routing must account for the value of existing context instead of comparing models in isolation.
☐ Glasshouse must distinguish measured quota from estimated subscription headroom.
☐ Glasshouse must never invent exact token balances for opaque subscriptions.
☐ A cheap routing model should protect premium capacity only when the routing overhead is materially smaller than the resources it saves.
☐ Workload tier and hard capability requirements must be determined before price optimization.
☐ Free resources should be used aggressively for suitable work but never treated as equivalent merely because their monetary price is zero.
☐ Routing decisions must remain inspectable and manually overridable.
☐ Same-vendor harness-model alignment is a useful initial prior, not proof of superior performance.
☐ Local evidence for the exact harness-profile-model-backend combination should outweigh vendor alignment when sufficiently strong.
☐ Response profiles control communication rather than reasoning effort, diligence, verification, permissions, or implementation quality.
☐ Model confidence, verbosity, repeated agreement, and reasoning length are not evidence that an implementation premise is correct.
☐ High-impact implementation premises should be tied to current evidence or an explicit bounded verification step before broad expansion.
☐ Token volume, context size, request count, and spend are resource measurements, not proxies for quality, progress, or agent productivity.
☐ Native prompt compaction and project memory must remain separate concepts.
☐ User control must take precedence over automatic orchestration.
☐ Optional integrations must not become hidden hard dependencies.
☐ Glasshouse should remain understandable enough that the user can inspect why it made an important decision.
