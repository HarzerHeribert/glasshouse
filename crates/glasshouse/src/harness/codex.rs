//! Codex.
//!
//! Read from Codex 0.149.0 as installed on the development machine on
//! 2026-08-25 — `codex --help`, `codex resume --help`, `codex login --help`,
//! the hook state it records in its own configuration, and the session
//! rollouts it writes.

use super::{
    BackendSelection, Backends, Capabilities, Declared, HarnessAdapter, HarnessDescription, Hooks,
    Invocation, ModelOverride, SessionIds, Vendor, WireProtocol,
};
use crate::integrations::IntegrationId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Codex;

/// Hook events observed in a real Codex installation's recorded hook state.
///
/// Observed, not catalogued — see [`super::Hooks::verified_events`].
const HOOK_EVENTS: &[&str] = &[
    "session_start",
    "user_prompt_submit",
    "pre_tool_use",
    "post_tool_use",
    "permission_request",
    "pre_compact",
    "post_compact",
    "subagent_start",
    "subagent_stop",
    "stop",
];

const PROTOCOLS: &[WireProtocol] = &[WireProtocol::OpenAiResponses];

const MODEL_OVERRIDE: &[ModelOverride] = &[
    ModelOverride::CommandLine("--model"),
    ModelOverride::Configuration("-c model=<id>"),
];

const BACKEND_SELECTION: &[BackendSelection] = &[
    BackendSelection::CommandLineArguments(
        "-c <key>=<value> overrides any config value; --oss and --local-provider select a \
         local backend",
    ),
    BackendSelection::GeneratedConfiguration(
        "-p/--profile layers $CODEX_HOME/<name>.config.toml over the base user config",
    ),
    BackendSelection::ChildEnvironment("CODEX_HOME relocates the whole configuration root"),
];

impl HarnessAdapter for Codex {
    fn id(&self) -> IntegrationId {
        IntegrationId::Codex
    }

    fn executable_candidates(&self) -> &'static [&'static str] {
        &["codex"]
    }

    fn start(&self) -> Invocation {
        // "If no subcommand is specified, options will be forwarded to the
        // interactive CLI." Bare `codex` is the interactive session.
        Invocation::bare()
    }

    fn resume(&self, native_session: &str) -> Option<Invocation> {
        // `codex resume [OPTIONS] [SESSION_ID] [PROMPT]` — "Session id (UUID)
        // or session name. UUIDs take precedence if it parses."
        //
        // Note the shape difference from Claude Code: a subcommand, not a
        // flag. This is exactly the harness-specific knowledge that would
        // otherwise be a `match` somewhere in core.
        Some(Invocation::of(["resume", native_session]))
    }

    fn describe(&self) -> HarnessDescription {
        HarnessDescription {
            vendor: Declared::verified(
                Vendor::OpenAi,
                "`codex login --help` documents authenticating with OPENAI credentials",
            ),
            hooks: Declared::verified(
                Hooks {
                    mechanism: "a `.codex/hooks.json` inside the project, each entry trusted \
                                per project by hash before it runs",
                    verified_events: HOOK_EVENTS,
                },
                "a real Codex configuration records per-project hook trust keyed by \
                 `<project>/.codex/hooks.json:<event>`; `codex --help` documents \
                 `--dangerously-bypass-hook-trust`",
            ),
            session_ids: Declared::verified(
                SessionIds::Discoverable {
                    source: "$CODEX_HOME/sessions/<yyyy>/<mm>/<dd>/rollout-<timestamp>-<uuid>.jsonl",
                },
                "a real Codex installation writes session rollouts under that path with the \
                 session UUID in the file name, and `codex resume` accepts that UUID",
            ),
            capabilities: Capabilities {
                code_editing: Declared::verified(
                    true,
                    "`codex --help`: the `apply` subcommand applies \"the latest diff produced \
                     by Codex agent\" to the working tree",
                ),
                shell_access: Declared::verified(
                    true,
                    "`codex --help`: `-s/--sandbox` selects \"the sandbox policy to use when \
                     executing model-generated shell commands\"",
                ),
                // Codex 0.149.0's `--help` documents `--search` for web search
                // but names no browser-control capability. Absent evidence is
                // not evidence of absence.
                browser_use: Declared::Unverified,
                mcp: Declared::verified(
                    true,
                    "`codex --help`: an `mcp` subcommand manages external MCP servers, and \
                     `mcp-server` runs Codex as one",
                ),
                subagents: Declared::verified(
                    true,
                    "`codex --help`: an `agents` subcommand browses agent sessions, and its \
                     hook state records subagent_start/subagent_stop events",
                ),
            },
            backends: Backends {
                protocols: Declared::verified(
                    PROTOCOLS,
                    "`codex --help` under `--search`: \"the native Responses `web_search` tool \
                     is available to the model\"",
                ),
                model_override: Declared::verified(
                    MODEL_OVERRIDE,
                    "`codex --help`: `-m/--model <MODEL>`, and `-c model=\"o3\"` is given as an \
                     explicit example of a config override",
                ),
                selection: Declared::verified(
                    BACKEND_SELECTION,
                    "`codex --help`: `-c <key=value>`, `--oss`, `--local-provider`, and \
                     `-p/--profile` (\"Layer $CODEX_HOME/<name>.config.toml on top of the base \
                     user config\")",
                ),
            },
            // Codex 0.149.0's `--help` documents no output-style, persona, or
            // tone mechanism. The capability map anticipates "Codex
            // personalities"; this installation does not expose one, so
            // Glasshouse records that it has not seen one rather than
            // implying the map's example is present.
            communication_style: Declared::Unverified,
        }
    }
}
