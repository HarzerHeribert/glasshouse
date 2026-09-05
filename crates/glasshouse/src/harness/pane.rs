//! `pane`, the Glasshouse native harness.
//!
//! Read from `crates/pane`'s own source on 2026-09-05, landed in `367d344`
//! (`GH-PANE-KICKOFF`): a binary that reads one line from stdin, echoes it
//! back, and exits `Ok`. It takes no arguments, reads no configuration, and
//! writes no session record of its own — every fact below is `Verified`
//! against that source rather than guessed at what a later sub-phase (61C
//! and on) will add. See the module documentation on [`Declared`] for why an
//! adapter must never declare a capability ahead of the binary that backs it.

use super::{
    ApprovalModes, Backends, Capabilities, Declared, HarnessAdapter, HarnessDescription,
    Invocation, Vendor,
};
use crate::integrations::IntegrationId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pane;

impl HarnessAdapter for Pane {
    fn id(&self) -> IntegrationId {
        IntegrationId::Pane
    }

    fn executable_candidates(&self) -> &'static [&'static str] {
        &["pane"]
    }

    fn start(&self) -> Invocation {
        // `crates/pane/src/main.rs`: `main` takes no arguments and reads
        // directly from `stdin`. Bare `pane` is the only invocation there is.
        Invocation::bare()
    }

    fn resume(&self, native_session: &str) -> Option<Invocation> {
        // `crates/pane/src/main.rs` and `crates/pane/src/lib.rs` parse no
        // arguments at all, so there is no resume flag to name. `None` is
        // the honest answer today, not a placeholder for one 61C might add.
        let _ = native_session;
        None
    }

    fn describe(&self) -> HarnessDescription {
        HarnessDescription {
            vendor: Declared::verified(
                Vendor::Glasshouse,
                "crates/pane/src/main.rs and crates/pane/src/lib.rs are Glasshouse's own \
                 sources, built by this workspace as the `pane` binary",
            ),
            // No lifecycle-hook mechanism exists to read: the binary parses
            // no configuration and no arguments.
            hooks: Declared::Unverified,
            // No session identifier mechanism exists: the binary writes no
            // record of its own and accepts no identifier argument.
            session_ids: Declared::Unverified,
            capabilities: Capabilities {
                code_editing: Declared::verified(
                    false,
                    "crates/pane/src/lib.rs: `echo_line` reads one line and writes it back \
                     unchanged; there is no file or tool access anywhere in the crate",
                ),
                shell_access: Declared::verified(
                    false,
                    "crates/pane/src/lib.rs and src/main.rs contain no process-spawning code",
                ),
                browser_use: Declared::verified(
                    false,
                    "crates/pane/src/lib.rs and src/main.rs contain no browser-facing code",
                ),
                mcp: Declared::verified(
                    false,
                    "crates/pane/src/lib.rs and src/main.rs contain no MCP client code",
                ),
                subagents: Declared::verified(
                    false,
                    "crates/pane/src/lib.rs and src/main.rs contain no subagent orchestration",
                ),
            },
            // No backend, model, or provider is reached at all: the binary
            // echoes its input and never dials out.
            backends: Backends::UNVERIFIED,
            // No approval mechanism exists to document: the binary never
            // asks for or bypasses approval of anything.
            approvals: ApprovalModes::UNVERIFIED,
            // No communication-style mechanism exists.
            communication_style: Declared::Unverified,
        }
    }
}
