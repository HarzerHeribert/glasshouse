//! Antigravity.
//!
//! Read from Antigravity CLI 1.1.20 as installed on the development machine on
//! 2026-08-25 — `agy --help` and the package that installed it.
//!
//! # The name
//!
//! Until this install existed, Glasshouse searched `PATH` for `antigravity`
//! and would never have found a real one: the published package ships a
//! binary called `antigravity` but puts it on `PATH` as **`agy`**. Both names
//! are searched now, `agy` first, because that is what an install actually
//! produces. This is the whole argument for deriving adapter declarations
//! from real binaries rather than from plausible-sounding recollection — the
//! previous single-name guess was carefully reasoned and simply wrong.

use super::{
    ApprovalModes, BackendSelection, Backends, Capabilities, Declared, HarnessAdapter,
    HarnessDescription, Invocation, ModelOverride, Vendor,
};
use crate::integrations::IntegrationId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Antigravity;

const MODEL_OVERRIDE: &[ModelOverride] = &[ModelOverride::CommandLine("--model")];

const BACKEND_SELECTION: &[BackendSelection] = &[BackendSelection::CommandLineArguments(
    "--model and --project select what a session runs against",
)];

impl HarnessAdapter for Antigravity {
    fn id(&self) -> IntegrationId {
        IntegrationId::Antigravity
    }

    fn executable_candidates(&self) -> &'static [&'static str] {
        // `agy` first: it is the name the published package links onto
        // `PATH`. `antigravity` second: it is the name of the binary inside
        // that package, so an install that copies it directly, or a future
        // package that links it under its own name, still resolves.
        //
        // Deliberately no shorter alias. `ag` is the-silver-searcher on a
        // great many machines, and a confident wrong detection is worse than
        // a missed one — the user can always configure an explicit path.
        &["agy", "antigravity"]
    }

    fn start(&self) -> Invocation {
        // `agy` with no arguments starts an interactive CLI session; `--print`
        // is the non-interactive mode.
        Invocation::bare()
    }

    fn resume(&self, native_session: &str) -> Option<Invocation> {
        // `agy --help`: `--conversation  Resume a previous conversation by ID`.
        Some(Invocation::of(["--conversation", native_session]))
    }

    fn describe(&self) -> HarnessDescription {
        HarnessDescription {
            vendor: Declared::verified(
                Vendor::Google,
                "the published Antigravity CLI package is Google's, distributed from \
                 antigravity.google",
            ),
            // `agy --help` lists a `plugin` subcommand but documents no
            // lifecycle-hook mechanism.
            hooks: Declared::Unverified,
            // `--conversation <ID>` proves conversation identifiers exist and
            // are accepted. It does not establish that Glasshouse can *find*
            // one: this install exposes no conversation-listing command, and
            // had never been run, so there was no on-disk store to inspect.
            // The map's question is whether identifiers can be discovered, and
            // the honest answer here is that nobody has shown they can.
            session_ids: Declared::Unverified,
            capabilities: Capabilities {
                code_editing: Declared::verified(
                    true,
                    "`agy --help`: `--mode` selects an execution mode, one of which is \
                     `accept-edits`",
                ),
                shell_access: Declared::verified(
                    true,
                    "`agy --help`: `--sandbox` — \"Run in a sandbox with terminal restrictions \
                     enabled\"",
                ),
                browser_use: Declared::Unverified,
                mcp: Declared::verified(
                    true,
                    "`agy --help`: an `mcp` subcommand manages MCP servers",
                ),
                // `--agent` selects an agent for the session and `agents`
                // lists them; neither shows that a session can *spawn* one,
                // which is what the capability means.
                subagents: Declared::Unverified,
            },
            backends: Backends {
                protocols: Declared::Unverified,
                model_override: Declared::verified(
                    MODEL_OVERRIDE,
                    "`agy --help`: `--model  Model for the current CLI session`",
                ),
                selection: Declared::verified(
                    BACKEND_SELECTION,
                    "`agy --help` documents `--model` and `--project` as per-session selectors; \
                     no environment or configuration mechanism is documented there",
                ),
            },
            approvals: ApprovalModes {
                // `agy --help` documents no classifier-style mode.
                automatic_review: Declared::Unverified,
                bypass: Declared::verified(
                    "--dangerously-skip-permissions",
                    "`agy --help`: `--dangerously-skip-permissions` — \"Auto-approve all tool \
                     permission requests without prompting\"",
                ),
                sandbox: Declared::verified(
                    "--sandbox",
                    "`agy --help`: `--sandbox` — \"Run in a sandbox with terminal restrictions \
                     enabled\"",
                ),
            },
            communication_style: Declared::Unverified,
        }
    }
}
