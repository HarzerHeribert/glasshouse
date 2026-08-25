//! OpenCode.
//!
//! Read from OpenCode 1.18.22 as installed on the development machine on
//! 2026-08-25 — `opencode --help`, `opencode session --help`, and the
//! type definitions its own plugin package ships.

use super::{
    BackendSelection, Backends, Capabilities, Declared, HarnessAdapter, HarnessDescription, Hooks,
    Invocation, ModelOverride, SessionIds, Vendor,
};
use crate::integrations::IntegrationId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenCode;

/// Plugin hook points declared by the installed `@opencode-ai/plugin` package.
///
/// Observed, not catalogued — see [`super::Hooks::verified_events`].
const HOOK_EVENTS: &[&str] = &[
    "tool.execute.before",
    "tool.execute.after",
    "tool.definition",
    "permission.ask",
    "chat.message",
    "chat.params",
    "command.execute.before",
];

const MODEL_OVERRIDE: &[ModelOverride] = &[ModelOverride::CommandLine("--model")];

const BACKEND_SELECTION: &[BackendSelection] = &[BackendSelection::CommandLineArguments(
    "--model takes a provider/model pair, so the provider is chosen with the model",
)];

impl HarnessAdapter for OpenCode {
    fn id(&self) -> IntegrationId {
        IntegrationId::OpenCode
    }

    fn executable_candidates(&self) -> &'static [&'static str] {
        &["opencode"]
    }

    fn start(&self) -> Invocation {
        // `opencode [project]` is the default command — "start opencode tui".
        // The optional positional is a path, which Glasshouse does not pass:
        // the working directory is already the project root, and handing the
        // same fact in twice is a second thing that can disagree.
        Invocation::bare()
    }

    fn resume(&self, native_session: &str) -> Option<Invocation> {
        // `opencode --help`: `-s, --session  session id to continue`.
        Some(Invocation::of(["--session", native_session]))
    }

    fn describe(&self) -> HarnessDescription {
        HarnessDescription {
            vendor: Declared::verified(
                Vendor::OpenCode,
                "the installed plugin package is published as `@opencode-ai/plugin`",
            ),
            hooks: Declared::verified(
                Hooks {
                    mechanism: "a plugin module written against `@opencode-ai/plugin`, \
                                installed with `opencode plugin`",
                    verified_events: HOOK_EVENTS,
                },
                "the installed `@opencode-ai/plugin` type definitions declare these hook \
                 points",
            ),
            session_ids: Declared::verified(
                SessionIds::Discoverable {
                    source: "`opencode session list`",
                },
                "`opencode session --help` documents `list` and `delete <sessionID>`, and \
                 `--session <id>` continues one",
            ),
            capabilities: Capabilities {
                // OpenCode is plainly a coding agent, but the capability map
                // asks for these "when known", and this installation's own
                // interfaces name no built-in tool. Its plugin API exposes
                // `tool.execute.before` and `permission.ask`, which prove
                // *some* tools exist without naming them. Recording that as
                // unverified is the honest reading, and Phase 7-style adapter
                // work against a running session can upgrade it.
                code_editing: Declared::Unverified,
                shell_access: Declared::Unverified,
                browser_use: Declared::Unverified,
                mcp: Declared::verified(
                    true,
                    "`opencode --help`: an `mcp` subcommand manages MCP servers",
                ),
                subagents: Declared::verified(
                    true,
                    "the installed `@opencode-ai/plugin` declares a `subagent_done` \
                     notification sound, and `opencode agent` manages agents",
                ),
            },
            backends: Backends {
                // OpenCode reaches many providers; which wire protocol it
                // speaks to each is not established by anything this
                // installation exposes.
                protocols: Declared::Unverified,
                model_override: Declared::verified(
                    MODEL_OVERRIDE,
                    "`opencode --help`: `-m, --model  model to use in the format of \
                     provider/model`",
                ),
                selection: Declared::verified(
                    BACKEND_SELECTION,
                    "`opencode --help`: the model argument carries the provider, and \
                     `opencode providers` manages provider credentials",
                ),
            },
            communication_style: Declared::Unverified,
        }
    }
}
