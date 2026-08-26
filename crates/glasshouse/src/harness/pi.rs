//! Pi.
//!
//! Read from Pi 0.73.1 as installed on the development machine on 2026-08-25
//! — `pi --help` and the package that installed it.

use super::{
    ApprovalModes, BackendSelection, Backends, Capabilities, Declared, HarnessAdapter,
    HarnessDescription, Invocation, ModelOverride, SessionIds, Vendor,
};
use crate::integrations::IntegrationId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pi;

const MODEL_OVERRIDE: &[ModelOverride] = &[ModelOverride::CommandLine("--model")];

const BACKEND_SELECTION: &[BackendSelection] = &[
    BackendSelection::CommandLineArguments("--provider selects the provider, --api-key its key"),
    BackendSelection::ChildEnvironment(
        "the API key defaults to the provider's own environment variable when --api-key is absent",
    ),
];

/// Pi is unavailable on `PATH` in this environment, so no current `--help`
/// artifact can establish a native communication-style mechanism. Its package
/// name alone is not evidence, and its prompt flags must not be repurposed as
/// one by inference.
const COMMUNICATION_STYLE: Declared<super::CommunicationStyle> = Declared::Unverified;

impl HarnessAdapter for Pi {
    fn id(&self) -> IntegrationId {
        IntegrationId::Pi
    }

    fn executable_candidates(&self) -> &'static [&'static str] {
        // `pi` is a short name, and short names collide. It is nonetheless
        // the only name the published package installs, so it is the only one
        // searched for — a second guessed alias would add collisions without
        // adding a real install. A user whose `pi` is something else
        // configures an explicit path, which is exactly the escape hatch
        // `crate::session::select` keeps for this.
        &["pi"]
    }

    fn start(&self) -> Invocation {
        // Bare `pi` is interactive; `--print` is the non-interactive mode.
        Invocation::bare()
    }

    fn resume(&self, native_session: &str) -> Option<Invocation> {
        // `pi --help`: `--session <path|id>  Use specific session file or
        // partial UUID`. Note it accepts a *partial* UUID, so a stored
        // identifier resolves whether or not it was truncated.
        Some(Invocation::of(["--session", native_session]))
    }

    fn describe(&self) -> HarnessDescription {
        HarnessDescription {
            vendor: Declared::verified(
                Vendor::Pi,
                "the installed package publishes the `pi` binary as \
                 `@mariozechner/pi-coding-agent`",
            ),
            // `--extension` loads extensions; no lifecycle-hook mechanism is
            // documented.
            hooks: Declared::Unverified,
            session_ids: Declared::verified(
                SessionIds::Discoverable {
                    source: "the session directory, which `--session-dir <dir>` names",
                },
                "`pi --help`: `--session-dir <dir>  Directory for session storage and lookup`, \
                 and `--session <path|id>` resolves a session file or partial UUID from it",
            ),
            capabilities: Capabilities {
                code_editing: Declared::verified(
                    true,
                    "`pi --help` describes itself as an \"AI coding assistant with read, bash, \
                     edit, write tools\"",
                ),
                shell_access: Declared::verified(
                    true,
                    "`pi --help` names `bash` among its built-in tools",
                ),
                browser_use: Declared::Unverified,
                mcp: Declared::Unverified,
                subagents: Declared::Unverified,
            },
            backends: Backends {
                // Pi reaches several providers; which wire protocol it speaks
                // to each is not established by its `--help`.
                protocols: Declared::Unverified,
                model_override: Declared::verified(
                    MODEL_OVERRIDE,
                    "`pi --help`: `--model <pattern>  Model pattern or ID (supports \
                     \"provider/id\")`",
                ),
                selection: Declared::verified(
                    BACKEND_SELECTION,
                    "`pi --help`: `--provider <name>`, and `--api-key <key>  API key (defaults \
                     to env vars)`",
                ),
            },
            // Pi is installed but not on `PATH` on this machine (npm's global
            // prefix is `~/.hermes/node`), so `pi --help` could not be read.
            // Nothing about its approval modes is established — guessing from
            // another harness's flags would be exactly the invented
            // declaration this module exists to prevent.
            approvals: ApprovalModes::UNVERIFIED,
            communication_style: COMMUNICATION_STYLE,
        }
    }
}
