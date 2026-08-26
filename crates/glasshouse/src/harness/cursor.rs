//! Cursor CLI.
//!
//! Read from Cursor CLI 2026.08.11-e8db854 as installed on the development
//! machine on 2026-08-25 — `cursor-agent --help` and the package that
//! installed it.

use super::{
    ApprovalMode, ApprovalModes, BackendSelection, Backends, Capabilities, Declared,
    HarnessAdapter, HarnessDescription, Invocation, ModelOverride, SandboxSelector, SessionIds,
    Vendor,
};
use crate::integrations::IntegrationId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor;

const MODEL_OVERRIDE: &[ModelOverride] = &[ModelOverride::CommandLine("--model")];

const BACKEND_SELECTION: &[BackendSelection] = &[
    BackendSelection::ChildEnvironment("CURSOR_API_KEY, and CURSOR_API_ENDPOINT for the endpoint"),
    BackendSelection::CommandLineArguments(
        "--api-key, --endpoint, and --header set the same three per invocation",
    ),
];

/// Cursor CLI 2026.08.11-e8db854's `cursor-agent --help` was read on
/// 2026-08-26. Its
/// `--mode ask` and `--mode plan` are read-only execution modes, not a
/// communication-style mechanism: using either would weaken a coding session.
/// No native output-style mechanism was documented, so support remains
/// unknown.
const COMMUNICATION_STYLE: Declared<super::CommunicationStyle> = Declared::Unverified;

impl HarnessAdapter for Cursor {
    fn id(&self) -> IntegrationId {
        IntegrationId::Cursor
    }

    fn executable_candidates(&self) -> &'static [&'static str] {
        // Only the qualified name. Cursor's own usage line calls the command
        // `agent`, and there is an `agent` subcommand, but searching `PATH`
        // for a name that generic would eventually resolve to somebody else's
        // program and start it as if it were a coding harness. The published
        // package links `cursor-agent`, and that is what is searched for.
        &["cursor-agent"]
    }

    fn start(&self) -> Invocation {
        // Bare `cursor-agent` starts the interactive agent; `-p/--print` is
        // the non-interactive mode.
        Invocation::bare()
    }

    fn resume(&self, native_session: &str) -> Option<Invocation> {
        // `cursor-agent --help`: `--resume [chatId]  Select a session to
        // resume`. With an identifier it resumes that chat rather than
        // opening the picker.
        Some(Invocation::of(["--resume", native_session]))
    }

    fn describe(&self) -> HarnessDescription {
        HarnessDescription {
            vendor: Declared::verified(
                Vendor::Cursor,
                "the installed CLI authenticates against Cursor's own service and reads \
                 CURSOR_API_KEY",
            ),
            // `plugin` and `--plugin-dir` exist; no lifecycle-hook mechanism
            // is documented.
            hooks: Declared::Unverified,
            session_ids: Declared::verified(
                SessionIds::Discoverable {
                    source: "`cursor-agent ls`, or `cursor-agent create-chat`, which returns \
                             the ID of a chat it has just created",
                },
                "`cursor-agent --help`: `create-chat  Create a new empty chat and return its \
                 ID`, `ls  Resume a chat session`, and `--resume [chatId]`",
            ),
            capabilities: Capabilities {
                code_editing: Declared::verified(
                    true,
                    "`cursor-agent --help` under `-p/--print`: \"Has access to all tools, \
                     including write and shell\"",
                ),
                shell_access: Declared::verified(
                    true,
                    "`cursor-agent --help` under `-p/--print`: \"Has access to all tools, \
                     including write and shell\"",
                ),
                browser_use: Declared::Unverified,
                mcp: Declared::verified(
                    true,
                    "`cursor-agent --help`: an `mcp` subcommand manages MCP servers, and \
                     `--approve-mcps` approves them",
                ),
                subagents: Declared::Unverified,
            },
            backends: Backends {
                // The endpoint defaults to Cursor's own service. Which wire
                // protocol it speaks is not documented by the CLI.
                protocols: Declared::Unverified,
                model_override: Declared::verified(
                    MODEL_OVERRIDE,
                    "`cursor-agent --help`: `--model <model>  Model to use (e.g., gpt-5, \
                     sonnet-4-thinking)`",
                ),
                selection: Declared::verified(
                    BACKEND_SELECTION,
                    "`cursor-agent --help`: `--api-key` (\"can also use CURSOR_API_KEY env \
                     var\") and `-e/--endpoint` (\"can also use CURSOR_API_ENDPOINT env var\")",
                ),
            },
            approvals: ApprovalModes {
                automatic_review: Declared::verified(
                    ApprovalMode {
                        args: &["--auto-review"],
                        description: "Use Auto-review (Smart Auto): a server classifier \
                                       auto-runs safe tool calls and prompts for the rest",
                    },
                    "`cursor-agent --help`: `--auto-review` — \"Use Auto-review (Smart Auto): \
                     a server classifier auto-runs safe tool calls and prompts for the rest\"",
                ),
                bypass: Declared::verified(
                    ApprovalMode {
                        args: &["--yolo"],
                        description: "Run Everything: force-allow commands unless \
                                       explicitly denied",
                    },
                    "`cursor-agent --help`: `--yolo`, documented as an alias for `--force` \
                     (Run Everything)",
                ),
                sandbox: Declared::verified(
                    SandboxSelector {
                        flag: "--sandbox",
                        values: &["enabled", "disabled"],
                    },
                    "`cursor-agent --help`: `--sandbox <mode>` — \"Explicitly enable or disable \
                     sandbox mode (overrides config)\", choices \"enabled\", \"disabled\"",
                ),
            },
            communication_style: COMMUNICATION_STYLE,
        }
    }
}
