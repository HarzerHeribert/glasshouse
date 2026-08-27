//! Claude Code.
//!
//! Every declaration below was read from Claude Code 2.1.245 as installed on
//! the development machine on 2026-08-25 — `claude --help`, the settings
//! document it reads, and the session transcripts it writes. Nothing here is
//! recalled.

use std::ffi::OsString;

use super::{
    ApprovalMode, ApprovalModes, BackendSelection, Backends, Capabilities, CommunicationStyle,
    CredentialPlacement, Declared, DirectProviderPlan, DirectProviderRequest, HarnessAdapter,
    HarnessDescription, HookCommand, HookDestination, HookInstallation, Hooks, Invocation,
    ModelOverride, SessionIds, StyleChange, Vendor, WireProtocol,
    pairing::OfficialModelSupport,
    response::{AdditiveInjection, NativeDelivery, NativeStyle},
};
use crate::integrations::IntegrationId;
use crate::profile::response::{Narration, ResponseProfile, Verbosity};

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

/// The root a Claude Code child reads its Anthropic endpoint from.
///
/// **The root, not an endpoint.** Claude Code 2.1.245 launched with
/// `ANTHROPIC_BASE_URL=http://127.0.0.1:8731` was observed sending
/// `POST /v1/messages?beta=true` to that host: the harness appends
/// `/v1/messages` itself. A provider's declared base URL therefore goes
/// through verbatim, with nothing appended and nothing stripped.
const BASE_URL_ENV: &str = "ANTHROPIC_BASE_URL";

/// The model identifier a Claude Code child uses.
///
/// Observed arriving as `"model":"probe-model-name"` in the request body. An
/// identifier Claude Code does not recognise is a *warning*, not a failure —
/// it assumes a 200k context window and proceeds — which is why a
/// provider-specific model name is safe to pass through.
const MODEL_ENV: &str = "ANTHROPIC_MODEL";

/// Where a credential value has to be written for a Claude Code child to use
/// it.
///
/// **Note the asymmetry, which is deliberate.** Claude Code fixes this
/// variable name, so a provider's own declared variable is where the value is
/// *read from* and this one is where it is *written to*; the two are rarely
/// the same name and nothing may assume they are.
///
/// Observed on 2.1.245: `ANTHROPIC_AUTH_TOKEN=<value>` arrived as
/// `authorization: Bearer <value>`, exactly the injected value, with no
/// `x-api-key` header and **without** the user's own claude.ai credential.
/// Claude Code said so itself — "claude.ai connectors are disabled because
/// ANTHROPIC_API_KEY or another auth source is set and takes precedence over
/// your claude.ai login" — so the injected token wins for that one child
/// while the user's native login is untouched on disk.
const CREDENTIAL_ENV: &str = "ANTHROPIC_AUTH_TOKEN";

/// Extra headers Claude Code sends on every request, one per line.
///
/// Verified on 2.1.245: probed with `X-Glasshouse-Probe: probe-header-value`,
/// the request arrived carrying `x-glasshouse-probe: probe-header-value`. The
/// `\n` separator between several headers is verified too — probed with
/// `$'X-Probe-One: value-one\nX-Probe-Two: value-two'`, both headers arrived
/// (`x-probe-one: value-one`, `x-probe-two: value-two`).
const CUSTOM_HEADERS_ENV: &str = "ANTHROPIC_CUSTOM_HEADERS";

const BACKEND_SELECTION: &[BackendSelection] = &[
    BackendSelection::ChildEnvironment(
        "ANTHROPIC_API_KEY, or a third-party provider's own credentials",
    ),
    BackendSelection::GeneratedConfiguration("--settings <file-or-json>"),
];

/// Claude Code's native output-style mechanism, read from the settings document
/// observed for Claude Code 2.1.245 on 2026-08-25 and from that session's
/// status-line payload. A Claude Code 2.1.246 `claude --help` run on
/// 2026-08-26 confirmed that the document still reaches the harness through
/// the launch-only `--settings` option; changing it therefore cannot alter an
/// already-running native session.
const COMMUNICATION_STYLE: Declared<CommunicationStyle> = Declared::verified(
    CommunicationStyle {
        mechanism: "output style, supplied in the settings document passed with \
                    `--settings` when the session starts",
        change: StyleChange::NewSession,
    },
    "a Claude Code 2.1.245 session's status-line payload reports its output style; Claude Code \
     2.1.246 `claude --help` documents `--settings <file-or-json>` as a launch option",
);

/// The name of the settings document Glasshouse writes for a Claude Code
/// session.
///
/// One name, used by [`ClaudeCode::hook_installation`] and by
/// [`ClaudeCode::native_response_style`], because Claude Code reads **one**
/// settings document: probed on 2.1.247, `claude --settings A --settings B
/// doctor` validates only `B`, so a second `--settings` silently discards the
/// first. Naming the same file from both places is what lets
/// `crate::session::HarnessSelection::install_session_document` merge them
/// into one document behind one flag.
const SETTINGS_FILE_NAME: &str = "claude-settings.json";

/// The settings key that selects a session's output style.
///
/// Read from the settings schema inside the Claude Code 2.1.247 bundle:
/// `outputStyle: ...optional().describe("Controls the output style for
/// assistant responses")`. Confirmed reachable through `--settings` on the
/// installed binary without starting a session or calling a model —
/// `claude --settings '{"outputStyle": 42}' doctor` answers
/// `Invalid settings ... outputStyle: Expected string, but received number`,
/// so the key is in the schema the `--settings` document is validated against
/// and its type is a string.
const OUTPUT_STYLE_KEY: &str = "outputStyle";

/// One of Claude Code's built-in output styles.
///
/// Deliberately a private type in this file. Line 603 requires an output style
/// to stay an *adapter example* rather than becoming a universal Glasshouse
/// concept, and this type is unreachable from anywhere else in the crate.
struct OutputStyle {
    /// The harness's own name, which is what the settings key takes.
    name: &'static str,
    /// The harness's own description of it.
    description: &'static str,
    /// Whether selecting it keeps Claude Code's coding instructions.
    keeps_coding_instructions: bool,
    /// Whether it governs *communication only*.
    ///
    /// The half of line 601 that has to be judged rather than read: two of the
    /// four built-in styles change what the agent **does**, not how it talks,
    /// and a response profile that selected one would break the phase's first
    /// fixed architectural requirement outright.
    communication_only: bool,
}

/// Claude Code 2.1.247's four built-in output styles, read from the style
/// table inside the installed bundle on 2026-08-27. Each entry there carries
/// `name`, `source: "built-in"`, `description` and `keepCodingInstructions`,
/// and all four declare `keepCodingInstructions: true`.
///
/// `communication_only` is Glasshouse's own judgement of the harness's own
/// description, and it is where two of these four are ruled out:
///
/// - `Learning` — *"Claude pauses and asks you to write small pieces of code
///   for hands-on practice"* changes the work, not the writing.
/// - `Proactive` — *"Claude executes immediately, minimizes interruptions, and
///   prefers action over planning"* is diligence and interruption policy.
///
/// Both are recorded rather than omitted, so that "Glasshouse never selects
/// these two" is a fact a reader can check against the harness's own words
/// instead of an absence.
const BUILT_IN_OUTPUT_STYLES: &[OutputStyle] = &[
    OutputStyle {
        name: "Concise",
        description: "Claude responds tersely, leading with results and skipping preamble and \
                      narration",
        keeps_coding_instructions: true,
        communication_only: true,
    },
    OutputStyle {
        name: "Explanatory",
        description: "Claude explains its implementation choices and codebase patterns",
        keeps_coding_instructions: true,
        communication_only: true,
    },
    OutputStyle {
        name: "Learning",
        description: "Claude pauses and asks you to write small pieces of code for hands-on \
                      practice",
        keeps_coding_instructions: true,
        communication_only: false,
    },
    OutputStyle {
        name: "Proactive",
        description: "Claude executes immediately, minimizes interruptions, and prefers action \
                      over planning",
        keeps_coding_instructions: true,
        communication_only: false,
    },
];

/// Where the output-style declarations above were read from.
const OUTPUT_STYLE_EVIDENCE: &str = "Claude Code 2.1.247: the settings schema in the installed bundle declares \
     `outputStyle` as an optional string \"Controls the output style for assistant \
     responses\", and its built-in style table declares Concise, Explanatory, Learning and \
     Proactive with `keepCodingInstructions: true`. `claude --settings '{\"outputStyle\": 42}' \
     doctor` rejects the value by name, so the key reaches the harness through the settings \
     document. Read on 2026-08-27.";

/// Claude Code's mechanism for adding an instruction beside its own system
/// prompt.
///
/// `--append-system-prompt`, not `--system-prompt`. The two sit next to each
/// other in `claude --help` and only one of them is line-607 safe:
/// `--system-prompt <prompt>` is "System prompt to use for the session", while
/// `--append-system-prompt <prompt>` is "Append a system prompt to the default
/// system prompt". Glasshouse declares the second and has no way to reach the
/// first.
const APPEND_SYSTEM_PROMPT: AdditiveInjection = AdditiveInjection {
    mechanism: "an instruction appended to the default system prompt with \
                `--append-system-prompt`",
    evidence: "`claude --help` on Claude Code 2.1.247: `--append-system-prompt <prompt>` — \
               \"Append a system prompt to the default system prompt\", read on 2026-08-27",
    flag: "--append-system-prompt",
};

/// The built-in output style closest to `profile`, or `None`.
///
/// `None` is not a shortcoming: it means no built-in style Glasshouse may
/// safely select expresses that combination of axes, and the additive
/// mechanism covers it instead. Only styles that both keep Claude Code's
/// coding instructions and govern communication only are ever candidates —
/// see [`BUILT_IN_OUTPUT_STYLES`].
///
/// The match reads the harness's own descriptions rather than inventing a
/// correspondence:
///
/// - `Concise` says "responds tersely, leading with results and skipping
///   preamble and narration", which is a terse-or-concise verbosity *and*
///   silent narration. Both, because the description claims both, and a style
///   selected on half of what it says would be applying more than was asked
///   for.
/// - `Explanatory` says "explains its implementation choices", which is
///   `Verbosity::Elaborate`.
///
/// Audience, evidence presentation and answer format are not matched on at
/// all: no built-in style speaks about them, and reading one into a style
/// would be exactly the invention the rest of this file refuses.
fn closest_output_style(profile: &ResponseProfile) -> Option<&'static OutputStyle> {
    let wanted = match (profile.verbosity(), profile.narration()) {
        (Verbosity::Terse | Verbosity::Concise, Narration::Silent) => "Concise",
        (Verbosity::Elaborate, _) => "Explanatory",
        _ => return None,
    };
    BUILT_IN_OUTPUT_STYLES
        .iter()
        .find(|style| style.name == wanted)
        .filter(|style| style.keeps_coding_instructions && style.communication_only)
}

/// The model families Anthropic produces for Claude Code, as `claude --help`
/// spells them. Families rather than ids: the help text presents these as
/// aliases for "the latest model" of each line, which is exactly what a
/// family is.
const NATIVE_FAMILIES: &[&str] = &["opus", "sonnet", "fable"];

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
        let file_name = SETTINGS_FILE_NAME;
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

    /// Up to four environment variables on one child process, and no
    /// arguments at all — the mechanism `BACKEND_SELECTION` already declares
    /// as [`BackendSelection::ChildEnvironment`]. The fourth,
    /// `CUSTOM_HEADERS_ENV`, is present only when the provider declares
    /// headers at all.
    ///
    /// Nothing here writes to `~/.claude` or to any settings document: the
    /// overlay this plan becomes reaches exactly one process's environment
    /// and dies with it.
    fn direct_provider_launch(
        &self,
        request: &DirectProviderRequest<'_>,
    ) -> Option<DirectProviderPlan> {
        // Claude Code is Anthropic's own client and speaks only the Anthropic
        // Messages API. A provider serving something else is not something to
        // translate for — see `crate::provider::translation_available`.
        if request.protocol != WireProtocol::AnthropicMessages {
            return None;
        }

        let mut env = vec![(
            OsString::from(BASE_URL_ENV),
            OsString::from(request.base_url),
        )];
        let mut names = vec![BASE_URL_ENV];

        if let Some(model) = request.model {
            env.push((OsString::from(MODEL_ENV), OsString::from(model)));
            names.push(MODEL_ENV);
        }

        // A name in, a *destination* out. The provider's own variable name is
        // where the value is read from; this is where it is written to.
        let credential = request
            .credential_var
            .map(|_| CredentialPlacement::Environment(CREDENTIAL_ENV.to_owned()));
        if credential.is_some() {
            names.push(CREDENTIAL_ENV);
        }

        // `Name: value` per header, joined by a real newline — see
        // `CUSTOM_HEADERS_ENV`'s doc for the verification. Absent entirely
        // rather than an empty string when the provider declares none.
        if !request.headers.is_empty() {
            let rendered = request
                .headers
                .iter()
                .map(|(name, value)| format!("{name}: {value}"))
                .collect::<Vec<_>>()
                .join("\n");
            env.push((OsString::from(CUSTOM_HEADERS_ENV), OsString::from(rendered)));
            names.push(CUSTOM_HEADERS_ENV);
        }

        Some(DirectProviderPlan {
            args: Vec::new(),
            env,
            credential,
            // Variable names only. The value that fills `CREDENTIAL_ENV` is
            // never in this string, and never in this type.
            mechanism: format!("child environment: {}", names.join(", ")),
        })
    }

    fn resume(&self, native_session: &str) -> Option<Invocation> {
        // `-r, --resume [value]` — "Resume a conversation by session ID, or
        // open interactive picker with optional search term". The identifier
        // is passed as its own argument rather than glued on with `=`, so a
        // value that begins with a dash cannot be re-read as a flag.
        Some(Invocation::of(["--resume", native_session]))
    }

    fn official_model_support(&self) -> OfficialModelSupport {
        OfficialModelSupport {
            native_families: Declared::verified(
                NATIVE_FAMILIES,
                "`claude --help`: `--model <model>` — \"Provide an alias for the latest model \
                 (e.g. 'fable', 'opus', or 'sonnet') or a model's full name (e.g. \
                 'claude-fable-5')\", read from Claude Code 2.1.246 on 2026-08-27",
            ),
            // Claude Code's own help names no model developed by anyone
            // else. That is not the same as Anthropic supporting none, and
            // it must not be recorded as one — an `Unverified` here means a
            // cross-vendor model reaching Claude Code is classified by its
            // wire protocol rather than being refused a class it might
            // deserve.
            supported_models: Declared::Unverified,
        }
    }

    fn native_response_style(&self, profile: &ResponseProfile) -> Option<NativeStyle> {
        let style = closest_output_style(profile)?;
        Some(NativeStyle {
            mechanism: "the session's output style, set in the settings document passed with \
                        `--settings`",
            selection: style.name,
            selection_description: style.description,
            evidence: OUTPUT_STYLE_EVIDENCE,
            delivery: NativeDelivery::SettingsKey {
                file_name: SETTINGS_FILE_NAME,
                flag: "--settings",
                key: OUTPUT_STYLE_KEY,
                value: style.name,
            },
        })
    }

    fn additive_response_injection(&self) -> Option<AdditiveInjection> {
        Some(APPEND_SYSTEM_PROMPT)
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
            communication_style: COMMUNICATION_STYLE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::response::{AnswerFormat, Audience, EvidenceDetail};

    /// Every one of the 324 combinations of the five axes.
    fn every_profile() -> Vec<ResponseProfile> {
        let mut all = Vec::new();
        for verbosity in Verbosity::ALL {
            for audience in Audience::ALL {
                for narration in Narration::ALL {
                    for evidence in EvidenceDetail::ALL {
                        for format in AnswerFormat::ALL {
                            all.push(ResponseProfile::new(
                                *verbosity, *audience, *narration, *evidence, *format,
                            ));
                        }
                    }
                }
            }
        }
        all
    }

    #[test]
    fn no_profile_ever_selects_a_style_that_changes_what_the_agent_does() {
        // `Learning` makes Claude stop and set exercises; `Proactive` makes it
        // prefer action over planning. Both keep the coding instructions and
        // both are still forbidden, because a response profile governs
        // user-facing communication only — the phase's first fixed
        // architectural requirement.
        for profile in every_profile() {
            if let Some(style) = closest_output_style(&profile) {
                assert!(
                    style.communication_only,
                    "{profile:?} selected `{}`, which is not communication policy",
                    style.name
                );
                assert!(
                    style.keeps_coding_instructions,
                    "{profile:?} selected `{}`, which would weaken the coding instructions",
                    style.name
                );
            }
        }
    }

    #[test]
    fn the_built_in_style_table_matches_what_the_harness_says_about_itself() {
        // A guard on the declaration rather than on the behaviour: if this
        // table is ever edited to claim a style Claude Code does not ship, or
        // to relabel one of the two excluded styles as communication-only, the
        // change has to be made deliberately here.
        let names: Vec<&str> = BUILT_IN_OUTPUT_STYLES
            .iter()
            .map(|style| style.name)
            .collect();
        assert_eq!(names, ["Concise", "Explanatory", "Learning", "Proactive"]);
        let usable: Vec<&str> = BUILT_IN_OUTPUT_STYLES
            .iter()
            .filter(|style| style.communication_only && style.keeps_coding_instructions)
            .map(|style| style.name)
            .collect();
        assert_eq!(
            usable,
            ["Concise", "Explanatory"],
            "exactly two of Claude Code's four built-in styles are communication policy"
        );
    }

    #[test]
    fn a_concise_profile_selects_concise_and_an_elaborate_one_explanatory() {
        let concise = ResponseProfile::new(
            Verbosity::Concise,
            Audience::Technical,
            Narration::Silent,
            EvidenceDetail::Standard,
            AnswerFormat::ChangeSummary,
        );
        assert_eq!(closest_output_style(&concise).unwrap().name, "Concise");

        let elaborate = ResponseProfile::new(
            Verbosity::Elaborate,
            Audience::Plain,
            Narration::Milestones,
            EvidenceDetail::Standard,
            AnswerFormat::Prose,
        );
        assert_eq!(
            closest_output_style(&elaborate).unwrap().name,
            "Explanatory"
        );
    }

    #[test]
    fn a_concise_profile_that_still_wants_narration_gets_no_native_style() {
        // Claude Code's own description of `Concise` claims two things —
        // terse answers *and* skipped narration. A profile that asked for only
        // the first would be given more than it asked for, so it falls through
        // to the additive mechanism instead.
        let profile = ResponseProfile::new(
            Verbosity::Concise,
            Audience::Technical,
            Narration::Detailed,
            EvidenceDetail::Standard,
            AnswerFormat::Prose,
        );
        assert!(closest_output_style(&profile).is_none());
    }

    #[test]
    fn the_style_and_the_hooks_name_the_same_settings_document() {
        // Claude Code 2.1.247 honours only the last `--settings`, so two
        // documents would mean the second silently discarding the first.
        let profile = ResponseProfile::new(
            Verbosity::Terse,
            Audience::Technical,
            Narration::Silent,
            EvidenceDetail::Minimal,
            AnswerFormat::Bullets,
        );
        let style = ClaudeCode.native_response_style(&profile).unwrap();
        match style.delivery {
            NativeDelivery::SettingsKey { file_name, .. } => {
                assert_eq!(file_name, SETTINGS_FILE_NAME);
            }
            NativeDelivery::Arguments(_) => panic!("Claude Code selects its style in a document"),
        }
    }

    #[test]
    fn the_additive_mechanism_appends_and_never_replaces() {
        let injection = ClaudeCode.additive_response_injection().unwrap();
        assert_eq!(injection.flag, "--append-system-prompt");
        assert_ne!(injection.flag, "--system-prompt");
    }
}
