//! `pane`, the Glasshouse native harness.
//!
//! Every declaration below is read from `crates/pane`'s own sources as they
//! stand on 2026-09-06 — `crates/pane/src/wire.rs` (the Anthropic Messages
//! wire format), `crates/pane/src/tools/registry.rs` (the tool set a session
//! may call) and `crates/pane/src/session.rs` (`pane session`'s flag set) —
//! never from the echo-line binary `GH-PANE-KICKOFF` landed in `367d344`,
//! which this file described until now. See the module documentation on
//! [`Declared`] for why an adapter must never declare a capability ahead of
//! the binary that backs it.

use std::ffi::OsString;

use super::{
    ApprovalModes, BackendSelection, Backends, Capabilities, CredentialPlacement, Declared,
    DirectProviderPlan, DirectProviderRequest, HarnessAdapter, HarnessDescription, Invocation,
    ModelOverride, Vendor, WireProtocol,
};
use crate::integrations::IntegrationId;

/// `crates/pane/src/wire.rs`: `send_turn` builds its request with
/// `request_body` and posts it to `{base_url}{MESSAGES_PATH}` carrying
/// `ANTHROPIC_VERSION` — the Anthropic Messages API and nothing else.
const PROTOCOLS: &[WireProtocol] = &[WireProtocol::AnthropicMessages];

/// `crates/pane/src/wire.rs`: `MODEL` is a `pub const`; no flag, environment
/// variable or file changes it — so the override list is verified *and*
/// empty, which is a different fact from `Unverified` (nobody checked).
/// The catalogue invariant allows exactly this for `model_override` and for
/// nothing else (`harness::tests::a_verified_backend_declaration_is_never_an_empty_list`).
const MODEL_OVERRIDE: &[ModelOverride] = &[];

/// The root a pane child reads its Anthropic endpoint from.
///
/// `crates/pane/src/wire.rs::base_url()`: `ANTHROPIC_BASE_URL` if set to a
/// non-empty value, `DEFAULT_BASE_URL` otherwise — the same root, not an
/// endpoint, convention `harness::claude_code::BASE_URL_ENV` documents.
const BASE_URL_ENV: &str = "ANTHROPIC_BASE_URL";

/// Where a credential value has to be written for a pane child to use it as
/// a bearer token.
///
/// `crates/pane/src/wire.rs::credential_header()` reads this variable first
/// and sends it as `Authorization: Bearer <value>`; only when it is absent
/// or empty does it fall back to `ANTHROPIC_API_KEY` as `x-api-key`. A
/// gateway hands out a bearer token (`profile::apply_gateway`'s own doc), so
/// this is the branch a gateway launch takes — exactly the asymmetry
/// `harness::claude_code::CREDENTIAL_ENV` documents for the same reason.
const CREDENTIAL_ENV: &str = "ANTHROPIC_AUTH_TOKEN";

const BACKEND_SELECTION: &[BackendSelection] = &[BackendSelection::ChildEnvironment(
    "ANTHROPIC_BASE_URL, ANTHROPIC_AUTH_TOKEN (bearer) or ANTHROPIC_API_KEY (x-api-key)",
)];

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
        // `crates/pane/src/session.rs::SessionArgs` parses `--root`,
        // `--task`, `--rollout`, `--session` and `--glasshouse`; none of them
        // is a flag that resumes a *different* prior session by identifier —
        // `--rollout` and `--session` are both this run's own inputs. `None`
        // is the honest answer today, not a placeholder.
        let _ = native_session;
        None
    }

    /// `crates/pane/src/wire.rs::send_turn` speaks only the Anthropic
    /// Messages API — see `PROTOCOLS` — so the same mechanism
    /// `harness::claude_code::ClaudeCode::direct_provider_launch` uses to
    /// hand a child its base URL and credential applies here, minus the
    /// model-environment and custom-header branches pane's wire has no
    /// counterpart for.
    fn direct_provider_launch(
        &self,
        request: &DirectProviderRequest<'_>,
    ) -> Option<DirectProviderPlan> {
        if request.protocol != WireProtocol::AnthropicMessages {
            return None;
        }

        // `crates/pane/src/wire.rs` has no header-forwarding mechanism at
        // all — nowhere to put one, unlike Claude Code's
        // `ANTHROPIC_CUSTOM_HEADERS`. Declaring headers accepted when
        // nothing can carry them would be exactly the invented mechanism
        // this module's own doc warns against.
        if !request.headers.is_empty() {
            return None;
        }

        // `crates/pane/src/wire.rs::MODEL` is a constant nothing can
        // override — see [`MODEL_OVERRIDE`] — so a caller-named model has
        // no destination. `require_model_if_the_harness_selects_through_it`
        // never asks for one here because `direct_provider_requires_model`
        // stays this trait's default `false`.
        let _ = request.model;

        let env = vec![(
            OsString::from(BASE_URL_ENV),
            OsString::from(request.base_url),
        )];
        let mut names = vec![BASE_URL_ENV];

        // A name in, a *destination* out — the value never passes through
        // this adapter, exactly as `harness::claude_code`'s own comment on
        // this asymmetry explains.
        let credential = request
            .credential_var
            .map(|_| CredentialPlacement::Environment(CREDENTIAL_ENV.to_owned()));
        if credential.is_some() {
            names.push(CREDENTIAL_ENV);
        }

        Some(DirectProviderPlan {
            args: Vec::new(),
            env,
            credential,
            // `crates/pane/src/wire.rs` reads its endpoint and credential
            // straight out of its own environment; nothing has to be
            // written down for it — see [`BACKEND_SELECTION`].
            config: None,
            mechanism: format!("child environment: {}", names.join(", ")),
        })
    }

    fn describe(&self) -> HarnessDescription {
        HarnessDescription {
            vendor: Declared::verified(
                Vendor::Glasshouse,
                "crates/pane/src/main.rs and crates/pane/src/lib.rs are Glasshouse's own \
                 sources, built by this workspace as the `pane` binary",
            ),
            // `crates/pane/src/session.rs::SessionArgs` parses `--root`,
            // `--task`, `--rollout`, `--session` and `--glasshouse`; none of
            // them configures a lifecycle-hook document the way Claude
            // Code's `--settings` or Codex's project file do. `--session`
            // names an identifier input, not a hook mechanism.
            hooks: Declared::Unverified,
            // `crates/pane/src/session.rs::SessionArgs::session` is an
            // optional string with no assignment contract (no UUID
            // requirement, no rejection of an arbitrary value the way
            // `harness::claude_code`'s `--session-id` has one) and no
            // discoverable native record Glasshouse could read back
            // afterwards. Pane's own seams instead call *out* to
            // `glasshouse hook --session <id>`
            // (`crates/pane/src/glasshouse.rs`) — the opposite direction
            // from what [`SessionIds`](super::SessionIds) describes.
            session_ids: Declared::Unverified,
            capabilities: Capabilities {
                code_editing: Declared::verified(
                    true,
                    "crates/pane/src/tools/registry.rs: `ALL` has no dedicated edit tool; \
                     `bash` runs under a sandbox profile whose only writable root is the \
                     project, so every write happens through it",
                ),
                shell_access: Declared::verified(
                    true,
                    "crates/pane/src/tools/registry.rs: the `bash` tool runs a command \
                     under the session's sandbox profile",
                ),
                browser_use: Declared::verified(
                    false,
                    "crates/pane/src/tools/registry.rs: `ALL` is `read`, `glob`, `grep` and \
                     `bash`; `NEVER_REGISTERED` names `webfetch` explicitly and \
                     `NETWORK_PROGRAMS` is checked against every declared executable",
                ),
                mcp: Declared::verified(
                    false,
                    "crates/pane/src/tools/registry.rs: `ALL` is `read`, `glob`, `grep` and \
                     `bash`; there is no MCP client code anywhere in the crate",
                ),
                subagents: Declared::verified(
                    false,
                    "crates/pane/src/tools/registry.rs: `ALL` is `read`, `glob`, `grep` and \
                     `bash`; there is no subagent orchestration anywhere in the crate",
                ),
            },
            backends: Backends {
                protocols: Declared::verified(
                    PROTOCOLS,
                    "crates/pane/src/wire.rs: MESSAGES_PATH is `/v1/messages`, \
                     ANTHROPIC_VERSION is sent on every request, and request_body builds \
                     a Messages request",
                ),
                model_override: Declared::verified(
                    MODEL_OVERRIDE,
                    "crates/pane/src/wire.rs: MODEL is a pub const; no argument, \
                     environment variable or file selects a model",
                ),
                selection: Declared::verified(
                    BACKEND_SELECTION,
                    "crates/pane/src/wire.rs: base_url() reads ANTHROPIC_BASE_URL and \
                     credential_header() reads ANTHROPIC_AUTH_TOKEN then ANTHROPIC_API_KEY",
                ),
            },
            // No approval mechanism exists to document: the binary never
            // asks for or bypasses approval of anything.
            approvals: ApprovalModes::UNVERIFIED,
            // No communication-style mechanism exists.
            communication_style: Declared::Unverified,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_declares_the_anthropic_messages_protocol_from_its_own_wire() {
        let description = Pane.describe();
        assert_eq!(
            description.backends.protocols.value().copied(),
            Some(PROTOCOLS)
        );
        let evidence = description
            .backends
            .protocols
            .evidence()
            .expect("declared Verified");
        assert!(evidence.contains("wire.rs"), "{evidence}");
    }

    #[test]
    fn pane_declares_the_two_environment_names_its_wire_reads() {
        assert_eq!(BASE_URL_ENV, "ANTHROPIC_BASE_URL");
        assert_eq!(CREDENTIAL_ENV, "ANTHROPIC_AUTH_TOKEN");

        let request = DirectProviderRequest {
            provider_name: "glasshouse-gateway",
            protocol: WireProtocol::AnthropicMessages,
            base_url: "http://127.0.0.1:9",
            model: None,
            credential_var: Some("IGNORED_NAME"),
            headers: &[],
        };
        let plan = Pane
            .direct_provider_launch(&request)
            .expect("pane speaks the Anthropic Messages API");
        let names: Vec<&str> = plan
            .env
            .iter()
            .map(|(name, _)| name.to_str().unwrap())
            .collect();
        assert!(names.contains(&BASE_URL_ENV), "{names:?}");
        assert_eq!(
            plan.credential,
            Some(CredentialPlacement::Environment(CREDENTIAL_ENV.to_owned()))
        );
    }

    #[test]
    fn a_non_anthropic_protocol_request_finds_no_pane_mechanism() {
        let request = DirectProviderRequest {
            provider_name: "some-provider",
            protocol: WireProtocol::OpenAiChat,
            base_url: "http://127.0.0.1:9",
            model: None,
            credential_var: None,
            headers: &[],
        };
        assert!(Pane.direct_provider_launch(&request).is_none());
    }

    #[test]
    fn pane_has_shell_access_and_edits_through_bash_and_nothing_else() {
        let description = Pane.describe();
        assert!(description.capabilities.shell_access.is_known_present());
        assert!(description.capabilities.code_editing.is_known_present());
        assert!(!description.capabilities.browser_use.is_known_present());
        assert!(!description.capabilities.mcp.is_known_present());
        assert!(!description.capabilities.subagents.is_known_present());
    }
}
