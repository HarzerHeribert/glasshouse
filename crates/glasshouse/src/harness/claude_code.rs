//! Claude Code.
//!
//! Every declaration below was read from Claude Code 2.1.245 as installed on
//! the development machine on 2026-08-25 — `claude --help`, the settings
//! document it reads, and the session transcripts it writes. Nothing here is
//! recalled.

use super::{
    ApprovalMode, ApprovalModes, BackendSelection, Backends, Capabilities, CommunicationStyle,
    Declared, HarnessAdapter, HarnessDescription, HookCommand, HookDestination, HookInstallation,
    Hooks, Invocation, ModelOverride, SessionIds, StyleChange, Vendor, WireProtocol,
};
use crate::integrations::IntegrationId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaudeCode;

/// Hook event names observed in a real Claude Code settings document.
///
/// Observed, not catalogued: these are the events a live installation was
/// found accepting. Claude Code may well support more.
const HOOK_EVENTS: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "UserPromptSubmit",
    "Stop",
    "StopFailure",
    "SubagentStart",
    "SubagentStop",
];

/// The events Glasshouse asks Claude Code to report.
///
/// A subset of [`HOOK_EVENTS`], and deliberately so: these are the ones that
/// say something about the *session's* state. `PreToolUse` and `PostToolUse`
/// fire many times per turn and would be noise for a lifecycle that only
/// distinguishes running from waiting.
///
/// `SessionStart` is **not** here, and not by oversight: Claude Code 2.1.245
/// does not fire it. A settings document declaring one was installed and the
/// hook never ran, while `UserPromptSubmit` from the same document did.
const REPORTED_EVENTS: &[&str] = &[
    "UserPromptSubmit",
    "PermissionRequest",
    "Stop",
    "StopFailure",
];

/// Seconds a reporting hook may take before Claude Code abandons it.
///
/// Small on purpose. The hook writes one row to a local database; if it cannot
/// do that quickly something is wrong, and a lifecycle note is never worth
/// making the user wait.
const HOOK_TIMEOUT_SECONDS: u32 = 5;

const PROTOCOLS: &[WireProtocol] = &[WireProtocol::AnthropicMessages];

const MODEL_OVERRIDE: &[ModelOverride] = &[
    ModelOverride::CommandLine("--model"),
    ModelOverride::Configuration("the settings document's model key"),
];

const BACKEND_SELECTION: &[BackendSelection] = &[
    BackendSelection::ChildEnvironment(
        "ANTHROPIC_API_KEY, or a third-party provider's own credentials",
    ),
    BackendSelection::GeneratedConfiguration("--settings <file-or-json>"),
];

impl HarnessAdapter for ClaudeCode {
    fn id(&self) -> IntegrationId {
        IntegrationId::ClaudeCode
    }

    fn executable_candidates(&self) -> &'static [&'static str] {
        &["claude"]
    }

    fn start(&self) -> Invocation {
        // `claude` with no arguments opens an interactive session in the
        // current directory: "starts an interactive session by default, use
        // -p/--print for non-interactive output". Glasshouse has already made
        // that directory the project root.
        Invocation::bare()
    }

    fn hook_installation(&self, report: &HookCommand) -> Option<HookInstallation> {
        // The shape is the one a real Claude Code settings document uses:
        // each event maps to a list of entries, each entry holds a list of
        // `{type, command, timeout}` hooks. Tool events additionally carry a
        // `matcher`; none of the events below is a tool event, so none does.
        let file_name = "claude-settings.json";
        Some(HookInstallation {
            file_name,
            contents: super::hooks_document(REPORTED_EVENTS, report, HOOK_TIMEOUT_SECONDS),
            // `--settings` loads *additional* settings, so the user's own
            // hooks keep running alongside these rather than being replaced.
            args: Invocation::of([
                std::ffi::OsString::from("--settings"),
                report.file(file_name).into_os_string(),
            ]),
            events: REPORTED_EVENTS,
            // Claude Code's `--settings` flag points the harness at a file
            // Glasshouse chooses; nothing of the user's project is touched.
            destination: HookDestination::GlasshouseOwned,
        })
    }

    fn assign_session_id(&self, native_session: &str) -> Option<Invocation> {
        // `--session-id <uuid>` — "Use a specific session ID for the
        // conversation (must be a valid UUID)". The requirement is enforced by
        // the binary, not merely documented: `claude --session-id not-a-uuid`
        // answers "Error: Invalid session ID. Must be a valid UUID." before
        // doing anything else.
        //
        // Claude Code names each conversation's transcript after this
        // identifier, so assigning it is also what lets Glasshouse find the
        // transcript of a session it started.
        Some(Invocation::of(["--session-id", native_session]))
    }

    fn resume(&self, native_session: &str) -> Option<Invocation> {
        // `-r, --resume [value]` — "Resume a conversation by session ID, or
        // open interactive picker with optional search term". The identifier
        // is passed as its own argument rather than glued on with `=`, so a
        // value that begins with a dash cannot be re-read as a flag.
        Some(Invocation::of(["--resume", native_session]))
    }

    fn describe(&self) -> HarnessDescription {
        HarnessDescription {
            vendor: Declared::verified(
                Vendor::Anthropic,
                "`claude --help` describes Anthropic authentication (ANTHROPIC_API_KEY, \
                 apiKeyHelper) as its first-party mechanism",
            ),
            hooks: Declared::verified(
                Hooks {
                    mechanism: "the `hooks` section of a settings document, which \
                                `--settings <file-or-json>` can supply per session",
                    verified_events: HOOK_EVENTS,
                },
                "a real Claude Code settings document carries a `hooks` map keyed by these \
                 event names; `claude --help` documents `--settings` and, for print mode, \
                 `--include-hook-events`",
            ),
            session_ids: Declared::verified(
                SessionIds::Assigned {
                    flag: "--session-id",
                },
                "`claude --help`: `--session-id <uuid>` — \"Use a specific session ID for \
                 the conversation (must be a valid UUID)\". Transcripts are also written \
                 per session under the user's Claude Code project directory",
            ),
            capabilities: Capabilities {
                code_editing: Declared::verified(
                    true,
                    "`claude --help`: `--tools` names built-in tools \"Bash,Edit,Read\"",
                ),
                shell_access: Declared::verified(
                    true,
                    "`claude --help`: `--allowedTools` is documented with the example \
                     \"Bash(git *)\"",
                ),
                browser_use: Declared::verified(
                    true,
                    "`claude --help`: `--chrome` — \"Enable Claude in Chrome integration\"",
                ),
                mcp: Declared::verified(
                    true,
                    "`claude --help`: `--mcp-config`, `--strict-mcp-config`, and an `mcp` \
                     subcommand",
                ),
                subagents: Declared::verified(
                    true,
                    "`claude --help`: `--agents <json>` defines custom agents, and its \
                     settings document accepts SubagentStart/SubagentStop hook events",
                ),
            },
            backends: Backends {
                protocols: Declared::verified(
                    PROTOCOLS,
                    "Claude Code is Anthropic's own client and `--betas` is documented as \
                     \"Beta headers to include in API requests\", i.e. the Anthropic \
                     Messages API",
                ),
                model_override: Declared::verified(
                    MODEL_OVERRIDE,
                    "`claude --help`: `--model <model>`; the same key is settable in a \
                     settings document",
                ),
                selection: Declared::verified(
                    BACKEND_SELECTION,
                    "`claude --help` under `--bare`: \"Anthropic auth is strictly \
                     ANTHROPIC_API_KEY or apiKeyHelper via --settings ... 3P providers \
                     (Bedrock/Vertex/Foundry) use their own credentials\"",
                ),
            },
            approvals: ApprovalModes {
                automatic_review: Declared::verified(
                    ApprovalMode {
                        args: &["--permission-mode", "auto"],
                        description: "auto permission mode for the session",
                    },
                    "`claude --help`: `--permission-mode <mode>` — \"Permission mode to use for \
                     the session\", choices \"acceptEdits\", \"auto\", \"bypassPermissions\", \
                     \"manual\", \"dontAsk\", \"plan\". The `auto-mode` subcommand inspects the \
                     classifier configuration and does not select the mode for a session.",
                ),
                bypass: Declared::verified(
                    ApprovalMode {
                        args: &["--dangerously-skip-permissions"],
                        description: "Bypass all permission checks",
                    },
                    "`claude --help`: `--dangerously-skip-permissions` — \"Bypass all \
                     permission checks\"; the same effect is also reachable with \
                     `--permission-mode bypassPermissions`",
                ),
                sandbox: Declared::Unverified,
            },
            communication_style: Declared::verified(
                CommunicationStyle {
                    mechanism: "output style, supplied in the settings document passed with \
                                `--settings` when the session starts",
                    change: StyleChange::NewSession,
                },
                "Claude Code reports an output style as part of its status-line payload, and \
                 `--settings` is read when the process starts",
            ),
        }
    }
}
