//! OpenCode.
//!
//! Read from OpenCode 1.18.22 as installed on the development machine on
//! 2026-08-25 — `opencode --help`, `opencode session --help`, and the
//! type definitions its own plugin package ships.

use std::ffi::OsString;

use super::{
    ApprovalMode, ApprovalModes, BackendSelection, Backends, Capabilities, ConfigPathPlacement,
    CredentialPlacement, Declared, DirectProviderPlan, DirectProviderRequest, GeneratedConfig,
    HarnessAdapter, HarnessDescription, Hooks, Invocation, ModelOverride, SessionIds, Vendor,
    WireProtocol,
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

const BACKEND_SELECTION: &[BackendSelection] = &[
    BackendSelection::CommandLineArguments(
        "--model takes a provider/model pair, so the provider is chosen with the model",
    ),
    BackendSelection::GeneratedConfiguration(
        "a provider is declared only in a configuration document; OPENCODE_CONFIG names an \
         additional one to load",
    ),
];

/// The protocol an OpenCode provider entry built on `@ai-sdk/openai-compatible`
/// speaks.
///
/// **Read off a real request line**, the same standard
/// [`crate::profile::ingress_targets`] holds itself to. OpenCode 1.18.22 was
/// launched against a listener on `127.0.0.1:8731` with a generated
/// configuration declaring exactly the provider entry
/// [`OpenCode::direct_provider_launch`] composes, and the listener recorded:
///
/// ```text
/// POST /v1/chat/completions HTTP/1.1
/// Authorization: Bearer <the value of the named environment variable>
/// User-Agent: opencode/1.18.22 ai-sdk/provider-utils/4.0.23 runtime/bun/1.3.14
/// ```
///
/// Two things follow beyond the protocol itself, and both are relied on
/// below. The base URL goes through **verbatim** — `/v1` came from the
/// provider's own declared URL and `/chat/completions` was appended by the
/// harness, so this adapter neither adds nor strips a path segment, exactly
/// as Codex's does. And the credential arrived as `Authorization: Bearer`
/// having been named in the document as `{env:NAME}` rather than written
/// into it.
const PROTOCOLS: &[WireProtocol] = &[WireProtocol::OpenAiChat];

/// The environment variable that points OpenCode at one additional
/// configuration document.
///
/// **Chosen over `OPENCODE_CONFIG_CONTENT`, which would need no file at
/// all — and the reason is a merge order, not a preference.** Both were
/// probed on OpenCode 1.18.22 against a project holding its own
/// `opencode.json`, with `opencode debug config` reporting the resolved
/// value:
///
/// - `OPENCODE_CONFIG=<file>` merges at **global** scope, *below* the user's
///   own project configuration: the project's value won.
/// - `OPENCODE_CONFIG_CONTENT=<json>` merges at **local** scope, *above* it:
///   Glasshouse's value won.
///
/// Glasshouse adds a provider; it does not get to overrule what the user
/// wrote in their own repository while doing so. The file is the mechanism
/// that adds without overruling, so the file is the mechanism, and the
/// capability map's "prefer temporary or Glasshouse-owned generated
/// configuration" is satisfied by a document that is *both*.
const CONFIG_ENV: &str = "OPENCODE_CONFIG";

/// What Glasshouse calls the document it generates for one OpenCode session.
///
/// Deliberately **not** `opencode.json`. The file lives in the directory
/// Glasshouse owns and is named so that a person who finds one knows at a
/// glance whose it is and that it is not their own configuration.
const CONFIG_FILE_NAME: &str = "opencode-provider.json";

/// The npm package an OpenCode provider entry uses to reach an
/// OpenAI-compatible endpoint.
///
/// OpenCode's own bundle names it in the one-line description it ships for
/// custom providers — "openai-compatible for OpenAI-compatible providers" —
/// and a provider entry declaring it produced the recorded request line in
/// [`PROTOCOLS`].
const OPENAI_COMPATIBLE_NPM: &str = "@ai-sdk/openai-compatible";

/// OpenCode 1.18.22's `opencode --help` was read on 2026-08-26. It documents
/// no native output-style or other communication-style mechanism, so this
/// stays unknown instead of projecting a generic response profile onto it.
const COMMUNICATION_STYLE: Declared<super::CommunicationStyle> = Declared::Unverified;

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

    /// OpenCode is the first harness Glasshouse launches whose provider
    /// configuration is **document-shaped**, and this is the whole of line
    /// 362: an isolated generated configuration file, written where
    /// Glasshouse owns it, for one child process.
    ///
    /// # Why a document at all
    ///
    /// `opencode --help` on 1.18.22 lists every option the binary takes, and
    /// none of them names a base URL, an API key or a provider definition.
    /// `--model <provider>/<model>` *selects* among providers that already
    /// exist; `opencode providers` walks a person through authenticating one
    /// interactively. A provider that is not already configured can only be
    /// brought into existence by a configuration document. That is what
    /// "requires file-based provider configuration" means here, and it was
    /// established from the installed binary rather than assumed.
    ///
    /// # Why the credential is not in the document
    ///
    /// OpenCode substitutes `{env:NAME}` anywhere in a configuration
    /// document's text before parsing it — the bundle's own
    /// `ConfigVariable.substitute` does the replacement out of the child's
    /// environment. So the document names the provider's own credential
    /// variable and the *value* travels the way every other harness's
    /// credential already does, in the child's environment, placed by
    /// [`crate::profile::resolve`]. Probed end to end: a document containing
    /// `"apiKey": "{env:NAME}"` produced `Authorization: Bearer <value>` on
    /// the wire.
    ///
    /// The consequence worth stating plainly: **a generated configuration
    /// file here never contains a secret**, and it cannot start to without
    /// this method being handed one, which [`DirectProviderRequest`] makes
    /// impossible.
    ///
    /// # Three refusals, none of them a substitution
    ///
    /// `None` here means "this harness cannot be launched that way", as
    /// everywhere else, and it is answered when the protocol is not
    /// OpenAI-chat: nothing is translated. A missing **model** is refused one
    /// level up with its own message — see
    /// [`OpenCode::direct_provider_requires_model`] — and so is a file name
    /// that could leave the directory Glasshouse owns, which this adapter
    /// cannot produce because it never names a path at all.
    fn direct_provider_launch(
        &self,
        request: &DirectProviderRequest<'_>,
    ) -> Option<DirectProviderPlan> {
        // Nothing is translated: the provider entry this composes speaks
        // OpenAI chat completions and no other protocol has been read off a
        // request line for OpenCode.
        if request.protocol != WireProtocol::OpenAiChat {
            return None;
        }
        // `--model <provider>/<model>` is how OpenCode is pointed at the
        // provider at all, so a plan without one would configure a provider
        // and leave it unused. Refused above rather than guessed at here.
        let model = request.model?;

        let id = request.provider_name;

        let mut options = serde_json::Map::new();
        // Verbatim — see `PROTOCOLS`. OpenCode appends `/chat/completions`.
        options.insert(
            "baseURL".to_owned(),
            serde_json::Value::String(request.base_url.to_owned()),
        );
        if let Some(var) = request.credential_var {
            // A NAME. The value is placed in the child's environment by
            // `crate::profile::resolve`, under this same variable, and
            // OpenCode reads it back out of there.
            options.insert(
                "apiKey".to_owned(),
                serde_json::Value::String(format!("{{env:{var}}}")),
            );
        }
        if !request.headers.is_empty() {
            // Verified on the wire: a probe declaring
            // `X-Glasshouse-Probe: header-value-here` here arrived as
            // `x-glasshouse-probe: header-value-here`.
            let headers: serde_json::Map<String, serde_json::Value> = request
                .headers
                .iter()
                .map(|(name, value)| (name.clone(), serde_json::Value::String(value.clone())))
                .collect();
            options.insert("headers".to_owned(), serde_json::Value::Object(headers));
        }

        let mut models = serde_json::Map::new();
        models.insert(model.to_owned(), serde_json::json!({ "name": model }));

        let document = serde_json::json!({
            "$schema": "https://opencode.ai/config.json",
            "provider": {
                id: {
                    "npm": OPENAI_COMPATIBLE_NPM,
                    "name": id,
                    "options": serde_json::Value::Object(options),
                    "models": serde_json::Value::Object(models),
                }
            }
        });
        // `serde_json`, not string splicing: a base URL or a header value is
        // whatever the user configured, and a document a harness silently
        // fails to parse would send the session to the default backend
        // instead of refusing.
        let contents = format!("{}\n", serde_json::to_string_pretty(&document).ok()?);

        let credential_note = match request.credential_var {
            Some(var) => format!(", credential read from {var}"),
            None => String::new(),
        };

        Some(DirectProviderPlan {
            // The pair that actually selects the provider. Two argv entries
            // rather than `--model=<pair>` because the two-argument form is
            // the one that was run against the installed binary.
            args: vec![
                OsString::from("--model"),
                OsString::from(format!("{id}/{model}")),
            ],
            // The document's own path is not here and cannot be: it is
            // `<the session's own Glasshouse directory>/<CONFIG_FILE_NAME>`,
            // and no session directory exists while a profile is being
            // resolved. `ConfigPathPlacement` is how the adapter says where
            // the path must go once there is one.
            env: Vec::new(),
            credential: request
                .credential_var
                .map(|var| CredentialPlacement::Environment(var.to_owned())),
            config: Some(GeneratedConfig {
                file_name: CONFIG_FILE_NAME,
                contents,
                path_placement: ConfigPathPlacement::Environment(CONFIG_ENV),
            }),
            // Names and mechanism only: the file name, the variable that
            // points at it, the provider's own name, and the credential
            // *variable*. Never the base URL, never a header value, never a
            // credential.
            mechanism: format!(
                "generated configuration `{CONFIG_FILE_NAME}`, named by {CONFIG_ENV}; provider \
                 `{id}` selected with --model{credential_note}"
            ),
        })
    }

    /// True: see [`OpenCode::direct_provider_launch`]. OpenCode picks its
    /// provider *through* the model, so a profile that names no model would
    /// configure a provider the session never uses.
    fn direct_provider_requires_model(&self) -> bool {
        true
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
                // What OpenCode speaks to a *custom* provider, which is the
                // only kind a launch profile can point it at. Its built-in
                // providers reach many vendors over mechanisms this
                // installation does not expose, and nothing here claims
                // anything about those.
                protocols: Declared::verified(
                    PROTOCOLS,
                    "OpenCode 1.18.22, launched with a generated configuration declaring an \
                     `@ai-sdk/openai-compatible` provider, was recorded sending \
                     `POST /v1/chat/completions` to the base URL that configuration named",
                ),
                model_override: Declared::verified(
                    MODEL_OVERRIDE,
                    "`opencode --help`: `-m, --model  model to use in the format of \
                     provider/model`",
                ),
                selection: Declared::verified(
                    BACKEND_SELECTION,
                    "`opencode --help` (1.18.22, read 2026-08-27): the model argument carries \
                     the provider, `opencode providers` manages provider credentials, and the \
                     complete option list offers no way to name a provider's base URL or key \
                     — that lives only in a configuration document, which the bundle's own \
                     `OPENCODE_CONFIG=/path/to/file.json` loads an additional one of",
                ),
            },
            approvals: ApprovalModes {
                // OpenCode's `--help` documents no classifier-style mode.
                automatic_review: Declared::Unverified,
                bypass: Declared::verified(
                    ApprovalMode {
                        args: &["--auto"],
                        description: "auto-approve permissions that are not explicitly denied \
                                       (dangerous!)",
                    },
                    "`opencode --help`: `--auto` — \"auto-approve permissions that are not \
                     explicitly denied (dangerous!)\". This is approve-unless-denied, not \
                     review, so it is recorded as a bypass and never as automatic review",
                ),
                sandbox: Declared::Unverified,
            },
            communication_style: COMMUNICATION_STYLE,
        }
    }
}
