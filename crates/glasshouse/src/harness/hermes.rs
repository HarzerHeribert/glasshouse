//! Hermes Agent.
//!
//! Read from Hermes Agent 0.15.1 as installed on the development machine on
//! 2026-08-25 — `hermes --help`, `hermes hooks --help`, and `hermes version`.

use super::{
    ApprovalModes, BackendSelection, Backends, Capabilities, Declared, HarnessAdapter,
    HarnessDescription, Hooks, Invocation, ModelOverride, SessionIds, Vendor,
};
use crate::integrations::IntegrationId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hermes;

/// Hermes declares hooks in its own configuration and gates each one behind a
/// first-use consent allowlist. `hermes hooks list` shows each hook's matcher
/// and consent status; the event *names* a matcher can carry are not
/// enumerated by the command-line interface, so none are claimed here.
const HOOK_EVENTS: &[&str] = &[];

const MODEL_OVERRIDE: &[ModelOverride] = &[
    ModelOverride::CommandLine("--model"),
    ModelOverride::Environment("HERMES_INFERENCE_MODEL"),
    ModelOverride::Configuration("the model.provider key in config.yaml"),
];

const BACKEND_SELECTION: &[BackendSelection] = &[
    BackendSelection::CommandLineArguments(
        "--provider overrides the provider for one invocation, --model the model",
    ),
    BackendSelection::ChildEnvironment("HERMES_INFERENCE_MODEL selects the model for the child"),
    BackendSelection::GeneratedConfiguration(
        "the persistent provider lives in config.yaml, and --ignore-user-config falls back to \
         built-in defaults",
    ),
];

impl HarnessAdapter for Hermes {
    fn id(&self) -> IntegrationId {
        IntegrationId::Hermes
    }

    fn executable_candidates(&self) -> &'static [&'static str] {
        &["hermes"]
    }

    fn start(&self) -> Invocation {
        // Bare `hermes` opens an interactive session. Deliberately *not*
        // `--tui`: Hermes has two interactive front ends and picks between
        // them from the user's own `display.interface` setting, so forcing
        // one would override a choice the user already made in their own
        // configuration — which is the opposite of what a control plane that
        // "does not hide or replace" its harnesses should do.
        Invocation::bare()
    }

    fn resume(&self, native_session: &str) -> Option<Invocation> {
        // `hermes --help`: `--resume SESSION, -r SESSION  Resume a previous
        // session by ID or title`.
        Some(Invocation::of(["--resume", native_session]))
    }

    fn describe(&self) -> HarnessDescription {
        HarnessDescription {
            vendor: Declared::verified(
                Vendor::Hermes,
                "`hermes version` identifies the installation as Hermes Agent, which is its \
                 own publisher",
            ),
            hooks: Declared::verified(
                Hooks {
                    mechanism: "shell hooks declared in Hermes's own config.yaml, each \
                                allowlisted on first use",
                    verified_events: HOOK_EVENTS,
                },
                "`hermes hooks --help`: \"Inspect shell-script hooks declared in \
                 ~/.hermes/config.yaml, test them against synthetic payloads, and manage the \
                 first-use consent allowlist\"",
            ),
            session_ids: Declared::verified(
                SessionIds::Discoverable {
                    source: "`hermes sessions`",
                },
                "`hermes --help`: a `sessions` subcommand, `--resume SESSION` by ID or title, \
                 and `--pass-session-id` to put the identifier in the agent's system prompt",
            ),
            capabilities: Capabilities {
                code_editing: Declared::verified(
                    true,
                    "`hermes --help`: `-t/--toolsets` selects tool sets and a `tools` \
                     subcommand manages them",
                ),
                shell_access: Declared::verified(
                    true,
                    "`hermes --help`: `--accept-hooks` auto-approves \"unseen shell hooks\", \
                     and `hermes hooks` manages shell-script hooks",
                ),
                browser_use: Declared::verified(
                    true,
                    "`hermes --help`: a `computer-use` subcommand, and `postinstall` bootstraps \
                     a browser among its non-Python dependencies",
                ),
                mcp: Declared::verified(true, "`hermes --help`: an `mcp` subcommand"),
                subagents: Declared::Unverified,
            },
            backends: Backends {
                // Hermes reaches several providers and can also *serve* an
                // OpenAI-compatible proxy. What it serves is not what it
                // speaks, so no wire protocol is claimed from that.
                protocols: Declared::Unverified,
                model_override: Declared::verified(
                    MODEL_OVERRIDE,
                    "`hermes --help`: `-m/--model` (\"Also settable via \
                     HERMES_INFERENCE_MODEL\") and a persistent `model.provider` in config.yaml",
                ),
                selection: Declared::verified(
                    BACKEND_SELECTION,
                    "`hermes --help`: `--provider PROVIDER` (\"e.g. openrouter, anthropic\"), \
                     and `--ignore-user-config`",
                ),
            },
            approvals: ApprovalModes {
                // Hermes's `--help` documents no classifier-style mode.
                automatic_review: Declared::Unverified,
                bypass: Declared::verified(
                    "--yolo",
                    "`hermes --help`: `--yolo` — \"Bypass all dangerous command approval \
                     prompts\"",
                ),
                sandbox: Declared::Unverified,
            },
            communication_style: Declared::Unverified,
        }
    }
}
