//! The launch-profile abstraction and its resolution into a per-launch
//! overlay.
//!
//! Three things live here, and they are deliberately not the same type:
//!
//! - A [`LaunchProfile`] is **inert configuration** — a name, a harness, a
//!   backend resource, an optional model, an optional expected protocol, and
//!   an approval selection. Nothing about it has touched a real adapter yet.
//! - A [`LaunchOverlay`] is the **ephemeral, per-launch result** of asking
//!   one [`HarnessAdapter`] whether a profile can actually be honoured. It
//!   applies to exactly one child process and is consumed by
//!   [`LaunchOverlay::apply`].
//! - [`resolve`] is the only place allowed to turn a profile's declaration
//!   into arguments or environment for a child process, and it **refuses
//!   rather than invents**: a combination the adapter does not declare comes
//!   back as a [`Refusal`], never as a best-effort substitute.
//!
//! # Why this module never imports `crate::config` or `crate::database`
//!
//! A launch profile is configuration, not project memory. It is read from
//! [`crate::config`], resolved here, and applied to one child process; none
//! of that touches the project's SQLite database, and it must not start to.
//! Only a *reference* to which profile a session ran under belongs in the
//! database — see `session/store.rs` — and a reference is not a definition.
//!
//! [`crate::provider`] and [`crate::secret`] *are* imported, because a
//! direct-provider profile cannot be resolved without knowing what the
//! provider serves and where its credential comes from. [`crate::config`]
//! still is not: the **caller** looks a configured provider up by name and
//! hands the resolved [`crate::provider::Provider`] in through
//! [`Resolution::provider`]. That keeps resolution a pure function of what it
//! was given — no file, no ambient environment, no configuration search — and
//! `harness::resolving_a_launch_profile_touches_no_files` enforces it.
//!
//! # The credential boundary
//!
//! [`resolve`] is the **only** place in Glasshouse where a
//! [`crate::secret::Secret`] exists. It is held in a local, moved into the
//! overlay's environment, and dropped there. No type in this module stores
//! one: not [`LaunchProfile`], not [`MechanismNote`], and not any
//! [`Refusal`]. A [`crate::harness::DirectProviderPlan`] cannot hold one
//! either — an adapter is handed variable *names* and never a value, so the
//! boundary is structural rather than a habit.

use std::ffi::OsString;
use std::fmt;

use crate::gateway::upstream::UpstreamBackend;
use crate::gateway::{Gateway, Route, Upstream};
use crate::harness::{
    ApprovalKind, ApprovalMode, CredentialPlacement, CredentialVarProblem, Declared,
    DirectProviderRequest, HarnessAdapter, WireProtocol,
};
use crate::integrations::IntegrationId;
use crate::launch::HarnessLaunch;
use crate::provider::{ProtocolCompatibleProviders, Provider};
use crate::routing::{AssignedModel, Cost, CredentialId, ToolSemantics};
use crate::secret::{SecretRef, SecretStore};

/// The protocols the local gateway's ingress knows how to serve.
///
/// All three, and the list is here rather than in [`mod@crate::gateway`]
/// because that module is structurally forbidden from naming
/// [`crate::harness`] — see its own header — and a protocol enum lives
/// there.
///
/// **This is a capability, not a promise.** It says what the ingress can
/// carry; what a *running* gateway actually carries is narrower, because a
/// route exists only for a protocol the one configured provider declared a
/// base URL for. [`Gateway::served_protocols`] is that narrower answer, and
/// it — never this constant — is what `apply_gateway` refuses against. A
/// profile checked against this list alone would launch a harness at an
/// ingress that would answer its first request with a `404`.
///
/// The order matters in one place only: [`gateway_upstream`] builds routes
/// in it, so it is the order a diagnostic lists protocols in.
pub const GATEWAY_INGRESS_PROTOCOLS: &[WireProtocol] = &[
    WireProtocol::AnthropicMessages,
    WireProtocol::OpenAiResponses,
    WireProtocol::OpenAiChat,
];

/// The request-target path prefixes that belong to each ingress protocol.
///
/// This is the whole of "the request target decides the protocol", and it
/// lives here for the same reason [`GATEWAY_INGRESS_PROTOCOLS`] does: the
/// gateway cannot name a [`WireProtocol`], so the module that can composes
/// the table and hands it over as [`Route`]s. The gateway owns the
/// *matching* — including the fact that a leading `/v1` is not part of the
/// answer, see `crate::gateway::upstream`'s `VERSION_SEGMENT`.
///
/// Each entry is a prefix matched at a path-segment boundary, so one entry
/// covers a protocol's whole surface: `/messages` places
/// `/v1/messages?beta=true` and `/v1/messages/count_tokens` alike.
///
/// **Every prefix here was read off a real request line**, from a harness
/// run against a listener that recorded it — Claude Code 2.1.245 sends
/// `POST /v1/messages?beta=true`, Codex 0.149.1 sends `POST /responses` —
/// or, for OpenAI Chat, off the endpoint the provider templates in
/// [`mod@crate::provider`] already document. Nothing here is a guess at a
/// path nobody has seen, which is the same rule that module applies to base
/// URLs.
const fn ingress_targets(protocol: WireProtocol) -> &'static [&'static str] {
    match protocol {
        WireProtocol::AnthropicMessages => &["/messages"],
        WireProtocol::OpenAiResponses => &["/responses"],
        WireProtocol::OpenAiChat => &["/chat/completions"],
    }
}

/// The name the gateway presents itself to an adapter under.
///
/// Letters, digits and `-` only, so it survives
/// [`crate::harness::unsafe_provider_name_char`] — an adapter may
/// interpolate it into a command line, and Codex puts it in a dotted TOML
/// path. It is the same string [`BackendResource::slug`] already uses, so a
/// session record and a launch mechanism name the same thing.
const GATEWAY_PROVIDER_NAME: &str = "glasshouse-gateway";

/// The variable name a gateway-backed launch associates its token with.
///
/// [`DirectProviderRequest::credential_var`] serves two purposes at once in
/// the adapters that read it: Claude Code uses it only as "there is a
/// credential, put it in my own fixed variable", while Codex writes it out
/// as the `env_key` the child will read. This name is therefore a
/// *destination*, and it is a real one for either adapter — it is not the
/// name of a variable anything reads a value **from**, because the gateway
/// token is minted in memory and has no source variable at all.
const GATEWAY_TOKEN_VAR: &str = "GLASSHOUSE_GATEWAY_TOKEN";

/// The profile every harness has before any configuration ever names one.
pub const NATIVE_PROFILE_NAME: &str = "native";

/// Which kind of backing a profile uses. A backend resource is a backend
/// *for a harness*; it is never an interactive coding agent by itself, and
/// nothing in Glasshouse can start a session from one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendResource {
    /// The harness's own first-party authentication and configuration.
    Native,
    /// A configured provider or router reached directly by the harness.
    DirectProvider { provider: String },
    /// The local Glasshouse gateway.
    GlasshouseGateway,
}

impl BackendResource {
    /// A stable, human-readable name for diagnostics and session records.
    ///
    /// Never a credential: a [`BackendResource::DirectProvider`]'s payload is
    /// the provider's own *name*, which is configured separately from any
    /// secret — see [`crate::config`]'s "No secrets here" section. This is
    /// exactly the kind of reference `session/store.rs`'s `backend_resource`
    /// column exists to hold.
    pub fn slug(&self) -> String {
        match self {
            BackendResource::Native => "native".to_owned(),
            BackendResource::DirectProvider { provider } => format!("direct-provider:{provider}"),
            BackendResource::GlasshouseGateway => "glasshouse-gateway".to_owned(),
        }
    }

    /// What kind of thing this backend is, for a refusal message that names
    /// what was asked for without repeating the whole variant.
    fn kind_description(&self) -> &'static str {
        match self {
            BackendResource::Native => "its native backend",
            BackendResource::DirectProvider { .. } => "a direct provider",
            BackendResource::GlasshouseGateway => "the Glasshouse gateway",
        }
    }
}

/// How a profile is backed, for the user's own accounting.
///
/// This is [`BackendResource`] collapsed to its three kinds — the map calls
/// it out as its own concept ("native-subscription, direct-provider, or
/// glasshouse-gateway") because it is what a user picks between, while
/// [`BackendResource`] additionally carries the concrete provider identity a
/// `DirectProvider` needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileClass {
    NativeSubscription,
    DirectProvider,
    GlasshouseGateway,
}

/// Which approval mode a profile asks the harness for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalSelection {
    /// Nothing was asked for. Resolves to the harness's automatic-review mode
    /// where it has one, and otherwise to no approval argument at all.
    Default,
    /// Automatic review was asked for explicitly. Refused on a harness that
    /// declares none — never downgraded.
    AutomaticReview,
    /// A blanket bypass, allowed only with a recorded acknowledgement.
    Bypass,
}

/// A launch profile: inert configuration naming how a session *should* be
/// opened, before anything has checked whether the named harness can
/// actually honour it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchProfile {
    pub name: String,
    pub harness: IntegrationId,
    pub backend: BackendResource,
    pub model: Option<String>,
    pub expected_protocol: Option<WireProtocol>,
    pub approval: ApprovalSelection,
    /// Phase 9H line 518: pin a gateway-backed session to the provider it is
    /// assigned at start, and turn automatic failover off.
    ///
    /// A property of the **profile** rather than a live command, because that
    /// is where the user can actually say it today: a running gateway lives
    /// inside one launch's stack frame, with no surface pointed at it. A
    /// person who wants this session to stay on one backend, whatever
    /// happens, records it once and every session started through the profile
    /// honours it.
    ///
    /// It pins to *whichever* provider the session is assigned rather than to
    /// a named one. Naming one would need the profile to choose a backend,
    /// which is the decision `gateway_upstream` makes from the user's own
    /// configuration order — and a pin that disagreed with it would be a
    /// second, silent way to select a provider.
    ///
    /// Meaningless on a `Native` or `DirectProvider` backend, where there is
    /// no gateway to pin: `apply_gateway` is the only reader.
    pub pin_gateway_backend: bool,
}

impl LaunchProfile {
    /// Every harness has this one, by construction rather than by a
    /// configuration entry, so adding gateway profiles can never remove it.
    pub fn native(harness: IntegrationId) -> Self {
        Self {
            name: NATIVE_PROFILE_NAME.to_owned(),
            harness,
            backend: BackendResource::Native,
            model: None,
            expected_protocol: None,
            approval: ApprovalSelection::Default,
            pin_gateway_backend: false,
        }
    }

    pub fn class(&self) -> ProfileClass {
        match &self.backend {
            BackendResource::Native => ProfileClass::NativeSubscription,
            BackendResource::DirectProvider { .. } => ProfileClass::DirectProvider,
            BackendResource::GlasshouseGateway => ProfileClass::GlasshouseGateway,
        }
    }
}

/// One resolved mechanism, for diagnostics. Carries key *names* only — never
/// a value — so it can be rendered without redacting anything at the call
/// site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MechanismNote {
    pub category: &'static str,
    pub detail: String,
}

/// The ephemeral result of resolving a profile against one adapter. Applies
/// to exactly one child process.
///
/// Construction goes through [`resolve`] only — every combination this type
/// can hold has already passed the resolution rules, so there is
/// deliberately no other public constructor.
pub struct LaunchOverlay {
    args: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
    mechanisms: Vec<MechanismNote>,
}

impl LaunchOverlay {
    fn empty() -> Self {
        Self {
            args: Vec::new(),
            env: Vec::new(),
            mechanisms: Vec::new(),
        }
    }

    pub fn args(&self) -> &[OsString] {
        &self.args
    }

    pub fn env(&self) -> &[(OsString, OsString)] {
        &self.env
    }

    pub fn mechanisms(&self) -> &[MechanismNote] {
        &self.mechanisms
    }

    /// Apply this overlay to `launch`: its arguments, then its environment
    /// operations, in that order. This is how the overlay reaches the child
    /// process and the **only** way it may — nothing else in Glasshouse may
    /// copy an overlay's env pairs onto a [`HarnessLaunch`] by hand.
    pub fn apply<'a>(self, launch: HarnessLaunch<'a>) -> HarnessLaunch<'a> {
        let mut launch = launch.args(self.args);
        for (key, value) in self.env {
            launch = launch.env(key, value);
        }
        launch
    }
}

impl fmt::Debug for LaunchOverlay {
    /// Manual, like [`HarnessLaunch`]'s own: an environment *value* must
    /// never reach a `Debug` rendering, only the count of operations and
    /// each key's name.
    ///
    /// **This is now load-bearing rather than tidy.** Since Phase 9F an
    /// overlay's environment can carry a real provider credential — see
    /// [`resolve`]'s direct-provider path — so a derived `Debug` here would
    /// print a live key into any log, panic message or test failure that
    /// formatted an overlay. Do not derive this impl, and do not add a field
    /// to this struct whose own `Debug` would render an environment value.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let env_keys: Vec<&OsString> = self.env.iter().map(|(key, _)| key).collect();
        f.debug_struct("LaunchOverlay")
            .field("arg_count", &self.args.len())
            .field("env_keys", &env_keys)
            .field("mechanisms", &self.mechanisms)
            .finish()
    }
}

/// Why a profile could not be resolved against an adapter.
///
/// Every message names the harness and what was asked for. No variant here
/// may be treated as a fallback to a different mode — a `Refusal` is handed
/// back to the caller, which reports it and starts nothing.
#[derive(Debug, thiserror::Error)]
pub enum Refusal {
    #[error(
        "launch profile `{profile}` for {} is backed by the local Glasshouse gateway, but no \
         gateway is running for this launch",
        .harness.display_name(),
    )]
    GatewayNotRunning {
        profile: String,
        harness: IntegrationId,
    },

    #[error(
        "launch profile `{profile}` is backed by the local Glasshouse gateway, but {} cannot \
         be pointed at any protocol that gateway's ingress serves ({})",
        .harness.display_name(),
        .protocols.join(", "),
    )]
    GatewayProtocolUnserved {
        profile: String,
        harness: IntegrationId,
        /// The protocol **slugs** this gateway is actually serving — what
        /// its one configured provider declared a base URL for, not the
        /// whole of [`GATEWAY_INGRESS_PROTOCOLS`].
        protocols: Vec<String>,
    },

    #[error(
        "launch profile `{profile}` is backed by the local Glasshouse gateway, but {} declares \
         nowhere to put the gateway's own token; the session would reach the gateway \
         unauthenticated and be refused by it",
        .harness.display_name(),
    )]
    GatewayTokenUnplaceable {
        profile: String,
        harness: IntegrationId,
    },

    #[error(
        "launch profile `{profile}` for {} is backed by the provider `{provider}`, which is \
         not configured",
        .harness.display_name(),
    )]
    ProviderNotConfigured {
        profile: String,
        harness: IntegrationId,
        provider: String,
    },

    #[error(
        "launch profile `{profile}` is backed by the provider `{provider}`, which serves \
         {served}, but {} needs {needed}",
        .harness.display_name(),
    )]
    ProviderProtocolUnsupported {
        profile: String,
        harness: IntegrationId,
        provider: String,
        /// What the provider serves, as a comma-separated list of protocol
        /// slugs — or a sentence saying it serves none.
        served: String,
        /// What the harness can be pointed at, the same way.
        needed: String,
    },

    #[error(
        "launch profile `{profile}` is backed by the provider `{provider}`, which declares no \
         base URL for {protocol}; set one before launching against it"
    )]
    ProviderBaseUrlMissing {
        profile: String,
        provider: String,
        protocol: WireProtocol,
    },

    #[error(
        "refusing launch profile `{profile}`: the provider name `{provider}` contains \
         `{offending}`, which a harness would interpret rather than pass through. Provider \
         names may use letters, digits, `-` and `_` only"
    )]
    UnsafeProviderName {
        profile: String,
        provider: String,
        offending: char,
    },

    #[error(
        "refusing launch profile `{profile}`: the provider `{provider}` names `{variable}` as \
         its credential variable, but {problem}; credential variable names may use letters, \
         digits and `_` only, and may not start with a digit"
    )]
    UnsafeCredentialVariable {
        profile: String,
        provider: String,
        variable: String,
        problem: CredentialVarProblem,
    },

    #[error(
        "launch profile `{profile}` asks {} to use the provider `{provider}` over {protocol}, \
         but {} declares no way to be pointed at a direct provider",
        .harness.display_name(),
        .harness.display_name(),
    )]
    NoDirectProviderMechanism {
        profile: String,
        harness: IntegrationId,
        provider: String,
        protocol: WireProtocol,
    },

    #[error(
        "launch profile `{profile}` needs the credential for the provider `{provider}`, but \
         the environment variable it names ({}) has no value; set it and try again. \
         Glasshouse will not start {} against its own native account instead",
        .variables.join(" and "),
        .harness.display_name(),
    )]
    CredentialUnavailable {
        profile: String,
        harness: IntegrationId,
        provider: String,
        /// The environment variable **names** the provider declares. Never a
        /// value — a `Refusal` is printed, and this one is printed precisely
        /// when the variable is empty.
        variables: Vec<String>,
    },

    #[error(
        "launch profile `{profile}` asks {0} for its automatic-review mode, but {0} declares \
         none",
        .harness.display_name(),
    )]
    NoAutomaticReview {
        profile: String,
        harness: IntegrationId,
    },

    #[error(
        "launch profile `{profile}` asks {0} to bypass its approval checks ({description}), \
         which has not been acknowledged for {0} yet; acknowledge the risk once (in \
         `glasshouse setup`), then this profile can run",
        .harness.display_name(),
    )]
    BypassNotAcknowledged {
        profile: String,
        harness: IntegrationId,
        description: &'static str,
    },

    #[error(
        "launch profile `{profile}` asks {0} to bypass its approval checks, but {0} declares \
         no bypass mode",
        .harness.display_name(),
    )]
    NoBypass {
        profile: String,
        harness: IntegrationId,
    },

    #[error(
        "launch profile `{profile}` names a model, but {} declares no way to override its \
         model",
        .harness.display_name(),
    )]
    NoModelOverride {
        profile: String,
        harness: IntegrationId,
    },

    #[error(
        "launch profile `{profile}` expects the {protocol} protocol, but {}'s backend cannot \
         serve it",
        .harness.display_name(),
    )]
    ProtocolMismatch {
        profile: String,
        harness: IntegrationId,
        protocol: WireProtocol,
    },

    #[error(
        "launch profile `{profile}` asks {0} for its automatic-review mode while backed by \
         {backend}, but that mode depends on a server-side capability the selected backend \
         may not serve, so the session would come up with its tools blocked; select one of \
         {0}'s own explicit modes instead",
        .harness.display_name(),
    )]
    AutomaticReviewNeedsNativeBackend {
        profile: String,
        harness: IntegrationId,
        backend: &'static str,
    },

    #[error(
        "launch profile `{profile}` for {} is backed by {backend}, but {}'s executable is not \
         installed and usable ({detail}); install it, or point Glasshouse at it, before this \
         profile can be offered",
        .harness.display_name(),
        .harness.display_name(),
    )]
    HarnessExecutableUnavailable {
        profile: String,
        harness: IntegrationId,
        backend: &'static str,
        detail: String,
    },
}

/// Everything [`resolve`] needs besides the profile itself.
///
/// One context struct rather than a positional parameter list, because this
/// list grows with every backend kind Glasshouse learns to launch, and a
/// fifth positional `bool` would be a defect waiting to happen.
///
/// It holds **no credential value**. [`Resolution::secrets`] is a store that
/// can produce one on request; the request is made inside [`resolve`], for
/// one variable, at the last possible moment.
pub struct Resolution<'a> {
    pub adapter: &'a dyn HarnessAdapter,
    pub acknowledged_bypass: bool,
    /// The configured provider a [`BackendResource::DirectProvider`] backend
    /// names, looked up by the CALLER from configuration. `None` for a
    /// [`BackendResource::Native`] profile.
    ///
    /// The caller does the lookup so this module never has to import
    /// [`crate::config`] — see the module documentation.
    pub provider: Option<&'a Provider>,
    pub secrets: &'a dyn SecretStore,
}

/// Resolve `profile` against `cx.adapter`, producing the overlay for exactly
/// one child process — or refusing, which starts nothing.
///
/// [`Resolution::acknowledged_bypass`] is the caller's answer to "has this
/// harness's blanket-bypass risk been shown to and accepted by the user",
/// read from user-level configuration only — see
/// [`crate::config::EffectiveConfig::bypass_acknowledged`].
///
/// # The one place a credential exists
///
/// A [`crate::secret::Secret`] is minted here, moved into the returned
/// overlay's environment, and dropped. It is never stored on a profile, a
/// plan, a mechanism note or a refusal — see the module documentation.
///
/// # Why automatic review depends on the backend
///
/// Claude Code's `--permission-mode auto` is decided by a **safety
/// classifier, which is itself a model call**. Pointed at the harness's own
/// backend that call is served; pointed at a gateway it is a request the
/// gateway receives and cannot answer as Anthropic would, and auto mode fails
/// closed — the session comes up with its tools blocked.
///
/// The evidence, stated at its real strength: a working multi-gateway
/// launcher on the development machine drives Claude Code through exactly the
/// four variables this module injects and deliberately does **not** select
/// auto mode, its own comment giving that reason; and Claude Code 2.1.245's
/// bundle references no separate classifier endpoint, every API path in it
/// being an ordinary one. That is a strong reading corroborated by a working
/// implementation. It is **not** a controlled experiment, and nothing here
/// should be read as one.
///
/// So the approval arm is keyed on the **backend**, not on the harness — it
/// is a property of "this approval mechanism is served by whatever the
/// harness talks to", which is equally true of a
/// [`BackendResource::DirectProvider`] and of a
/// [`BackendResource::GlasshouseGateway`]:
///
/// - [`ApprovalSelection::Default`] contributes no approval argument, exactly
///   as it already does for a harness declaring no automatic-review mode, and
///   records a [`MechanismNote`] saying so.
/// - [`ApprovalSelection::AutomaticReview`] is **refused**. A default that
///   falls back is not a request that is refused.
/// - [`ApprovalSelection::Bypass`] is unchanged, acknowledgement and all:
///   nothing about a backend relaxes that.
///
/// [`BackendResource::Native`] behaviour does not change by one byte.
pub fn resolve(profile: &LaunchProfile, cx: &Resolution<'_>) -> Result<LaunchOverlay, Refusal> {
    resolve_with_gateway(profile, cx, None)
}

/// [`resolve`], for a caller that has a running local gateway to offer.
///
/// # Why this is a second entry point rather than a field on [`Resolution`]
///
/// A gateway is not a property of the profile or of the adapter: it is a
/// *process* the caller started, and only a caller that decided to start one
/// has anything to pass. Callers that never can — the configuration tests
/// that resolve a Native profile to check a lookup — keep the argument-free
/// [`resolve`] and are unaffected.
///
/// `None` here is not "no gateway configured"; it is "this call site has no
/// gateway to give". A gateway-backed profile therefore refuses with
/// [`Refusal::GatewayNotRunning`], which is the honest thing to say, rather
/// than being silently resolved against something else.
///
/// # What a gateway-backed profile resolves into
///
/// Exactly what a direct-provider profile resolves into, through the same
/// adapter method, with two substitutions: the base URL is the gateway's own
/// loopback address, and the credential written into the child is the
/// **gateway's token** rather than any provider key. That is line 2 of Phase
/// 9G in one sentence — the provider credential stays in this process, held
/// by the gateway, and the child is given something that is worthless
/// anywhere else.
///
/// Reusing [`HarnessAdapter::direct_provider_launch`] is deliberate. The
/// variables Claude Code reads are the harness's own declared knowledge, and
/// naming `ANTHROPIC_BASE_URL` here instead would put that knowledge in a
/// second place, where the two copies could disagree.
pub fn resolve_with_gateway(
    profile: &LaunchProfile,
    cx: &Resolution<'_>,
    gateway: Option<&Gateway>,
) -> Result<LaunchOverlay, Refusal> {
    let adapter = cx.adapter;

    if profile.model.is_some() {
        let can_override_model = adapter.describe().backends.model_override.value().is_some();
        if !can_override_model {
            return Err(Refusal::NoModelOverride {
                profile: profile.name.clone(),
                harness: profile.harness,
            });
        }
        // A model on a `Native` profile is still only *validated* here: with
        // no provider identity there is no way to know which of a harness's
        // several declared model mechanisms is the right one, and picking
        // one anyway is the invention this module exists to refuse. The
        // direct-provider path below has that identity, so there it becomes
        // a real override.
    }

    if let Some(expected) = profile.expected_protocol {
        let can_serve = adapter
            .describe()
            .backends
            .protocols
            .value()
            .is_some_and(|protocols| protocols.contains(&expected));
        if !can_serve {
            return Err(Refusal::ProtocolMismatch {
                profile: profile.name.clone(),
                harness: profile.harness,
                protocol: expected,
            });
        }
    }

    let mut overlay = LaunchOverlay::empty();

    match &profile.backend {
        BackendResource::DirectProvider { provider } => {
            apply_direct_provider(profile, provider, cx, &mut overlay)?;
        }
        BackendResource::GlasshouseGateway => {
            let Some(gateway) = gateway else {
                return Err(Refusal::GatewayNotRunning {
                    profile: profile.name.clone(),
                    harness: profile.harness,
                });
            };
            apply_gateway(profile, gateway, adapter, &mut overlay)?;
        }
        BackendResource::Native => {}
    }

    // An automatic-review mode is not necessarily served by whatever the
    // harness is pointed at — see `automatic_review_depends_on_the_backend`.
    let native_backend = matches!(profile.backend, BackendResource::Native);

    match profile.approval {
        ApprovalSelection::Default => {
            // A harness with no automatic review gets no approval argument
            // at all here — never a silent bypass. `approval_args` already
            // answers `None` for a mode a harness lacks; there is nothing
            // else to try.
            match adapter.approval_args(ApprovalKind::AutomaticReview) {
                Some(args) if native_backend => {
                    append_approval(adapter, &mut overlay, &args, "automatic review");
                }
                Some(_) => {
                    // The same "no approval argument at all" a harness
                    // without the mode already gets, for the same reason:
                    // nothing else is safe to substitute. Recorded so a user
                    // reading the mechanisms can see it was a decision.
                    overlay.mechanisms.push(MechanismNote {
                        category: "approval mode",
                        detail: format!(
                            "automatic review withheld: it depends on a server-side \
                             capability {} may not serve",
                            profile.backend.kind_description()
                        ),
                    });
                }
                None => {}
            }
        }
        ApprovalSelection::AutomaticReview => {
            let Some(args) = adapter.approval_args(ApprovalKind::AutomaticReview) else {
                return Err(Refusal::NoAutomaticReview {
                    profile: profile.name.clone(),
                    harness: profile.harness,
                });
            };
            // A default that falls back is not a request that is refused: an
            // explicit ask for a mode the backend may not serve is refused
            // rather than quietly dropped, because a session whose tools are
            // silently blocked is worse than one that never started.
            if !native_backend {
                return Err(Refusal::AutomaticReviewNeedsNativeBackend {
                    profile: profile.name.clone(),
                    harness: profile.harness,
                    backend: profile.backend.kind_description(),
                });
            }
            append_approval(adapter, &mut overlay, &args, "automatic review");
        }
        ApprovalSelection::Bypass => {
            let Some(args) = adapter.approval_args(ApprovalKind::Bypass) else {
                return Err(Refusal::NoBypass {
                    profile: profile.name.clone(),
                    harness: profile.harness,
                });
            };
            if !cx.acknowledged_bypass {
                let description = bypass_description(adapter);
                return Err(Refusal::BypassNotAcknowledged {
                    profile: profile.name.clone(),
                    harness: profile.harness,
                    description,
                });
            }
            append_approval(adapter, &mut overlay, &args, "bypass");
        }
    }

    Ok(overlay)
}

/// [`resolve_with_gateway`], plus Phase 9F line 466's precondition: refuse a
/// direct-provider or gateway-backed profile before doing anything else if
/// `harness_executable` says the harness's executable is not installed and
/// usable. [`BackendResource::Native`] is unaffected — this check is never
/// even consulted for one, so a `Native` profile's behaviour cannot change by
/// so much as which branch runs.
///
/// # Why this takes the answer rather than finding it
///
/// [`resolve`] and [`resolve_with_gateway`] stay pure functions of the values
/// in [`Resolution`] — no real `PATH` lookup as a side effect of resolving a
/// profile whose caller never asked for one. That is not incidental: every
/// existing caller of those two functions (`main.rs`'s own production launch
/// path, `config`'s and `onboarding`'s tests, `tests/pty_smoke.rs`, and this
/// module's own test suite) constructs profiles naming real harnesses —
/// `Codex`, `Pi` — that are not all installed on every machine those tests
/// run on, and none of them expects a `PATH` search to happen underneath it.
/// A third, additional entry point that takes the executable check as a
/// value keeps that guarantee intact while still letting a production caller
/// opt in.
///
/// A caller that has already resolved the harness's executable — as
/// `main.rs`'s `session::select::select` already does, before any launch
/// profile is resolved — should hand back the [`crate::harness::ExecutablePresence::Usable`]
/// it already established rather than pay for a second search. A caller that
/// has not should call [`crate::harness::ExecutablePresence::detect`] itself,
/// which performs the real check this precondition asks for.
pub fn resolve_checked(
    profile: &LaunchProfile,
    cx: &Resolution<'_>,
    gateway: Option<&Gateway>,
    harness_executable: &crate::harness::ExecutablePresence,
) -> Result<LaunchOverlay, Refusal> {
    if !matches!(profile.backend, BackendResource::Native) && !harness_executable.is_usable() {
        return Err(Refusal::HarnessExecutableUnavailable {
            profile: profile.name.clone(),
            harness: profile.harness,
            backend: profile.backend.kind_description(),
            detail: harness_executable.detail(profile.harness),
        });
    }
    resolve_with_gateway(profile, cx, gateway)
}

/// Point one child process at the local gateway, or refuse.
///
/// The three things that make this different from a direct provider, and
/// nothing else is:
///
/// 1. the base URL is the gateway's own loopback address rather than a
///    provider's;
/// 2. the credential written into the child is the gateway's token, which is
///    already in memory, so **no [`crate::secret::Secret`] is resolved here
///    at all** — the provider's key was resolved once, at gateway start, and
///    lives in the gateway;
/// 3. no provider headers are forwarded, because the child is not talking to
///    a provider. A provider's own extra headers are the gateway's business
///    on the hop the gateway makes.
///
/// Everything else — the arguments, the environment, the credential's
/// destination variable — comes from the adapter's own declaration, so a
/// harness that changes how it is pointed at a backend changes it in one
/// place.
fn apply_gateway(
    profile: &LaunchProfile,
    gateway: &Gateway,
    adapter: &dyn HarnessAdapter,
    overlay: &mut LaunchOverlay,
) -> Result<(), Refusal> {
    let harness_protocols: &[WireProtocol] = adapter
        .describe()
        .backends
        .protocols
        .value()
        .copied()
        .unwrap_or(&[]);

    // What this *running* gateway carries, which is
    // `GATEWAY_INGRESS_PROTOCOLS` narrowed to the protocols the configured
    // provider declared a base URL for. Refusing against the constant
    // instead would launch a harness at an ingress with no route for it, and
    // the mismatch would surface as a `404` on the first request rather than
    // as a refusal naming what is missing.
    //
    // Compared by slug because the gateway may not name a `WireProtocol` —
    // see `Gateway::served_protocols`. The slug is `WireProtocol::slug`'s own
    // output on both sides of the comparison, so there is one spelling, not
    // two.
    let served = gateway.served_protocols();
    let ingress_serves =
        |protocol: &WireProtocol| served.iter().any(|slug| *slug == protocol.slug());

    // An explicit ask is a constraint, never a hint — the same rule
    // `choose_protocol` applies to a direct provider. A profile expecting a
    // protocol the ingress does not serve is refused rather than quietly
    // given one that exists.
    let protocol = match profile.expected_protocol {
        Some(expected) => ingress_serves(&expected).then_some(expected),
        None => harness_protocols.iter().copied().find(ingress_serves),
    };
    let Some(protocol) = protocol.filter(|protocol| harness_protocols.contains(protocol)) else {
        return Err(Refusal::GatewayProtocolUnserved {
            profile: profile.name.clone(),
            harness: profile.harness,
            // What the ingress serves, for a message that names the mismatch
            // from the side the user cannot change. Slugs, because that is
            // what the gateway could tell us.
            protocols: served.iter().map(|slug| (*slug).to_owned()).collect(),
        });
    };

    // Phase 9H lines 505, 506 and 507. This is the moment a session gets a
    // backend, and it is the only place that knows all three of the harness,
    // the protocol resolved for it, and the model the profile named — the
    // gateway knows where to forward bytes and nothing else. Recording it
    // here rather than at gateway start is what makes the assignment
    // *belong to the harness-backed session* instead of to the process.
    let model = match &profile.model {
        Some(model) => AssignedModel::Named(model.clone()),
        None => AssignedModel::HarnessDefault,
    };
    gateway.routing().bind(
        profile.harness.slug(),
        protocol.slug(),
        model,
        gateway.upstream(),
    );

    // Phase 9H line 518, applied where the assignment was just made. A pin
    // recorded on the profile is the user's own statement, so it is honoured
    // before the first request rather than after a failure has already moved
    // the session.
    if profile.pin_gateway_backend
        && let Some(pinned) = gateway.routing().pin_to_serving_provider()
    {
        overlay.mechanisms.push(MechanismNote {
            category: "gateway pin",
            detail: format!("pinned to `{pinned}`; automatic failover is off"),
        });
    }

    let base_url = gateway.base_url();
    let request = DirectProviderRequest {
        provider_name: GATEWAY_PROVIDER_NAME,
        protocol,
        base_url: &base_url,
        model: profile.model.as_deref(),
        credential_var: Some(GATEWAY_TOKEN_VAR),
        headers: &[],
    };

    let Some(plan) = adapter.direct_provider_launch(&request) else {
        return Err(Refusal::NoDirectProviderMechanism {
            profile: profile.name.clone(),
            harness: profile.harness,
            provider: GATEWAY_PROVIDER_NAME.to_owned(),
            protocol,
        });
    };

    let Some(CredentialPlacement::Environment(destination)) = plan.credential else {
        return Err(Refusal::GatewayTokenUnplaceable {
            profile: profile.name.clone(),
            harness: profile.harness,
        });
    };

    overlay.args.extend(plan.args);
    overlay.env.extend(plan.env);
    // The gateway's token, not a provider key. This is the only value that
    // reaches the child, and it authenticates against exactly one loopback
    // listener owned by exactly one Glasshouse instance.
    overlay.env.push((
        OsString::from(destination),
        OsString::from(gateway.token().expose()),
    ));

    // The loopback address and the adapter's own variable names. Not the
    // token, and not the provider's credential — neither of which any
    // mechanism note has ever carried.
    overlay.mechanisms.push(MechanismNote {
        category: "glasshouse gateway",
        detail: format!("{base_url} over {protocol} — {}", plan.mechanism),
    });

    // Phase 9H line 505, said out loud. `gateway_upstream`'s documentation
    // argues that choosing a backend is legitimate here *because* the choice
    // is announced, pinnable and reversible rather than silent; this is the
    // announcement. Names only — `Assignment::label` is built from
    // `CredentialId::label`, which is a provider and a variable name.
    if let Some(assignment) = gateway.routing().assignment() {
        overlay.mechanisms.push(MechanismNote {
            category: "gateway backend",
            detail: assignment.label(),
        });
    }

    Ok(())
}

/// Which configured providers the local gateway may forward to: the one it
/// assigns now, and the ones a real provider failure may move a session to.
///
/// # Phase 9G refused to choose here. Phase 9H chooses, and says so.
///
/// The previous version of this function refused a configuration in which more
/// than one provider served the ingress, with a message explaining that
/// *"choosing a backend per session is sticky routing rather than something a
/// launch profile decides"*. That was right at the time and it is the line
/// this phase exists to cross.
///
/// The objection Phase 9G actually raised was to a **silent** choice: *"a
/// gateway that picked the alphabetically first of three routers would be
/// making exactly the routing decision the map defers"*. What makes the
/// choice legitimate now is not that this phase is allowed to be arbitrary —
/// it is that the choice is no longer invisible or final:
///
/// - it is **recorded** as a [`crate::routing::interactive::Assignment`] the
///   moment a profile binds a session, and reported in the launch's own
///   mechanism note (Phase 9H lines 505 and 507);
/// - the user can **pin** the session to one provider and turn automatic
///   failover off (line 518);
/// - the user can **migrate** the session to another provider at a task
///   boundary (line 511);
/// - and every change is **recorded** with its cache consequence (lines 515
///   and 516).
///
/// A choice that is announced, pinnable and reversible is a different thing
/// from a choice made behind the user's back, and the refusal was about the
/// second. The order is the order the caller presents the providers in, which
/// is the user's own configuration order; nothing here ranks providers on
/// quality, because that is Phase 9J's job and it has no evidence yet.
///
/// **This is a judgement, and it is the one thing in this batch most worth
/// disagreeing with.** The alternative — keep refusing until a launch profile
/// can name its gateway provider — is defensible and costs a field on
/// `BackendResource::GlasshouseGateway`. It was not taken because, with the
/// refusal in place, a user with two configured routers cannot start a
/// gateway-backed session at all, and every one of Phase 9H's failover lines
/// is unreachable in production by construction.
///
/// # Several credentials for one provider are several backends
///
/// Phase 9E's last line allows *"several credentials for one provider to be
/// held as a pool"*, and [`Provider::credential_env`] has always been a list.
/// The previous version took *"the first that currently resolves"* and
/// discarded the rest. Each one that resolves is now its own backend, which
/// is what makes Phase 9I line 537's rotation — *"treat a single key's
/// exhaustion as that key's limit rather than the provider's"* — something
/// the gateway can actually do rather than something it can only describe.
///
/// # One provider, every protocol it serves
///
/// Unchanged from Phase 9G, and its reasoning is unchanged with it. A provider
/// is a candidate if it serves at least one ingress protocol with a base URL,
/// and it gets a [`Route`] for **each** protocol it serves. Requiring all
/// three would refuse every real configuration — no built-in template serves
/// more than two.
///
/// Every credential is resolved here, once, at start, and moved into the
/// [`Upstream`]. Unlike [`resolve`]'s direct-provider path they do not end at
/// a child process: they stay in this process for the gateway's lifetime,
/// which is the entire point of holding them here instead.
pub fn gateway_upstream(
    providers: &[Provider],
    secrets: &dyn SecretStore,
) -> Result<Upstream, GatewayUpstreamRefusal> {
    // This is the routing constraint before any selection. Its result has a
    // distinct type so a future model-quality scorer can only be handed
    // providers which have already passed it; it cannot accidentally rank the
    // raw configured-provider slice and filter afterward.
    let candidates =
        ProtocolCompatibleProviders::for_any_protocol(providers, GATEWAY_INGRESS_PROTOCOLS);

    if candidates.is_empty() {
        return Err(GatewayUpstreamRefusal::NoProviderServesTheIngress {
            protocols: GATEWAY_INGRESS_PROTOCOLS.to_vec(),
            served: describe_provider_protocols(providers),
        });
    }

    let mut backends = Vec::new();
    let mut variables = Vec::new();
    for candidate in candidates.iter() {
        let provider = candidate.provider();
        variables.extend(provider.credential_env.iter().cloned());
        // Every credential that resolves, in the order the provider declared
        // them. A variable with no value is skipped rather than refused: a
        // user who has one of two keys set has one working backend, and
        // failing the whole launch over the other would be worse than using
        // what is there.
        for var in &provider.credential_env {
            let reference = SecretRef::Environment { var: var.clone() };
            let Some(credential) = secrets.resolve(&reference) else {
                continue;
            };
            backends.push(UpstreamBackend::new(
                provider.name.clone(),
                gateway_routes(provider),
                credential,
                CredentialId::new(provider.name.clone(), reference),
                // Nobody has marked anything free on this path yet: the
                // free-tier marking of Phase 9I line 527 lives in
                // `crate::config`'s provider entry, and reaches routing through
                // the disposable-job path rather than through the gateway's
                // interactive one. `Cost::Metered` is the fail-closed default
                // and this is where a marking would arrive.
                Cost::Metered,
            )?);
        }
    }

    if backends.is_empty() {
        return Err(GatewayUpstreamRefusal::CredentialUnavailable {
            provider: candidates
                .iter()
                .map(|candidate| candidate.name().to_owned())
                .collect::<Vec<_>>()
                .join(", "),
            variables,
        });
    }

    Ok(Upstream::with_failover(backends)?)
}

/// One [`Route`] per ingress protocol `provider` actually serves, in the
/// ingress's own order.
///
/// A protocol it does not serve gets no route, which is what makes a request
/// for it a refusal rather than a request sent to some other protocol's base
/// URL.
fn gateway_routes(provider: &Provider) -> Vec<Route> {
    GATEWAY_INGRESS_PROTOCOLS
        .iter()
        .filter_map(|protocol| {
            declared_base_url(provider, *protocol).map(|base_url| {
                Route::new(
                    protocol.slug().to_owned(),
                    ingress_targets(*protocol),
                    base_url,
                )
                .with_tools(tool_semantics(provider, *protocol))
            })
        })
        .collect()
}

/// What a provider declares about tool calls on one protocol, as the three
/// states a routing policy needs.
///
/// [`Declared`] carries an evidence string that routing has no use for, and
/// [`Declared::is_known_present`] collapses "verified absent" into "nobody
/// checked" — which is exactly the distinction Phase 9H line 517 turns on. So
/// the translation is explicit here rather than done with an existing helper
/// that answers a different question.
fn tool_semantics(provider: &Provider, protocol: WireProtocol) -> ToolSemantics {
    match provider.serves(protocol).map(|support| &support.tool_calls) {
        Some(Declared::Verified { value: true, .. }) => ToolSemantics::Verified,
        Some(Declared::Verified { value: false, .. }) => ToolSemantics::KnownAbsent,
        Some(Declared::Unverified) | None => ToolSemantics::Unverified,
    }
}

/// The base URL `provider` declares for `protocol`, or `None` when it
/// declares none — or declares an empty one.
///
/// An empty base URL is not a base URL: the generic templates in
/// [`mod@crate::provider`] ship one so the user can supply their own, and
/// launching against `""` must never happen. The same rule
/// `apply_direct_provider` already applies, in one place both can be read
/// from.
fn declared_base_url(provider: &Provider, protocol: WireProtocol) -> Option<&str> {
    provider
        .serves(protocol)
        .map(|support| support.base_url.as_str())
        .filter(|base_url| !base_url.is_empty())
}

/// Why the local gateway could not be given an upstream to forward to.
///
/// Separate from [`Refusal`] because it is answered before any profile is
/// resolved against any adapter: there is no harness in the question yet,
/// and a refusal that had to invent one would be naming something it did not
/// check. Every variant carries names only.
#[derive(Debug, thiserror::Error)]
pub enum GatewayUpstreamRefusal {
    #[error(
        "the local Glasshouse gateway can forward requests for {}, but no configured provider \
         serves any of them with a base URL; configured declarations: {served}. Configure one \
         before launching a gateway-backed profile",
        protocol_list(.protocols),
    )]
    NoProviderServesTheIngress {
        /// Every protocol the ingress offers — what the user could configure
        /// a provider for, not one of them picked out.
        protocols: Vec<WireProtocol>,
        /// The configured providers' protocol declarations, including an
        /// explicit marker for an empty base URL. This makes an empty
        /// candidate set explain both what the ingress required and what was
        /// actually declared.
        served: String,
    },

    #[error(
        "the local Glasshouse gateway needs the credential for the provider `{provider}`, but \
         the environment variable it names ({}) has no value; set it and try again. \
         Glasshouse will not start a harness against its own native account instead",
        .variables.join(" and "),
    )]
    CredentialUnavailable {
        provider: String,
        /// The environment variable **names** the provider declares. Never a
        /// value — this refusal is printed precisely when there is none.
        variables: Vec<String>,
    },

    #[error(transparent)]
    Unusable(#[from] crate::gateway::UpstreamError),
}

/// `a`, `b` and `c` — a list of protocols for a message a user reads.
///
/// Names only, and every one of them is a [`WireProtocol::slug`], so nothing
/// user-written reaches a diagnostic through here.
fn protocol_list(protocols: &[WireProtocol]) -> String {
    protocols
        .iter()
        .map(|protocol| protocol.slug())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The configured protocol declarations for a refusal after compatibility
/// filtering left no candidates.
///
/// An empty base URL is named rather than elided: it is a declaration with no
/// destination, which is precisely why it could not pass the filter.
fn describe_provider_protocols(providers: &[Provider]) -> String {
    if providers.is_empty() {
        return "no configured providers".to_owned();
    }

    providers
        .iter()
        .map(|provider| {
            let protocols = if provider.protocols.is_empty() {
                "no protocol at all".to_owned()
            } else {
                provider
                    .protocols
                    .iter()
                    .map(|support| {
                        if support.base_url.is_empty() {
                            format!("`{}` (no base URL)", support.protocol)
                        } else {
                            format!("`{}`", support.protocol)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            format!("`{}` declares {protocols}", provider.name)
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Point one child process at `provider_name`, or refuse.
///
/// The resolution order is fixed, and each step refuses rather than
/// substituting:
///
/// 1. the provider is not configured;
/// 2. its name would be unsafe to interpolate into a command line;
/// 3. no protocol is common to what the provider serves and what the harness
///    can be pointed at;
/// 4. the provider declares no base URL for the chosen protocol;
/// 5. the harness declares no direct-provider mechanism at all;
/// 6. the credential the provider names cannot be resolved.
///
/// Step 6 is the one that matters most: a harness launched without the
/// credential would reach for the user's own native, paid account, so a
/// missing value is a refusal rather than a launch.
fn apply_direct_provider(
    profile: &LaunchProfile,
    provider_name: &str,
    cx: &Resolution<'_>,
    overlay: &mut LaunchOverlay,
) -> Result<(), Refusal> {
    let Some(provider) = cx.provider else {
        return Err(Refusal::ProviderNotConfigured {
            profile: profile.name.clone(),
            harness: profile.harness,
            provider: provider_name.to_owned(),
        });
    };

    // Untrusted input reaching a command line. A provider name is a
    // configuration table key, so it is whatever the user typed, and Codex
    // interpolates it into a dotted TOML path where `.` is a separator
    // rather than a character to escape. Refused before a request is built,
    // so no adapter ever sees an unsafe one — see
    // `crate::harness::unsafe_provider_name_char` for why the check lives
    // here rather than in each adapter.
    if let Some(offending) = crate::harness::unsafe_provider_name_char(&provider.name) {
        return Err(Refusal::UnsafeProviderName {
            profile: profile.name.clone(),
            provider: provider.name.clone(),
            offending,
        });
    }

    let protocol = choose_protocol(profile, cx.adapter, provider)?;
    let base_url = declared_base_url(provider, protocol)
        .expect("the protocol was chosen from protocol-compatible candidates");

    // A provider declaring several credential variables is a **pool**, and
    // choosing between them on cost, quota or region is a routing decision
    // this phase does not make: take the first that currently resolves, and
    // refuse only if none does. `Provider::secret_refs` deliberately refuses
    // to choose for the same reason.
    let resolvable = provider
        .credential_env
        .iter()
        .find(|var| {
            cx.secrets
                .resolve(&SecretRef::Environment {
                    var: (*var).clone(),
                })
                .is_some()
        })
        .map(String::as_str);
    // The name handed to the adapter when nothing resolves: the first
    // declared one. The request stays well-formed, so a harness that
    // declares no mechanism at all still refuses with *that* reason rather
    // than being masked by a missing value.
    let credential_var = resolvable.or_else(|| provider.credential_env.first().map(String::as_str));

    if let Some(var) = credential_var
        && let Some(problem) = crate::harness::unusable_credential_var(var)
    {
        return Err(Refusal::UnsafeCredentialVariable {
            profile: profile.name.clone(),
            provider: provider.name.clone(),
            variable: var.to_owned(),
            problem,
        });
    }

    let request = DirectProviderRequest {
        provider_name: &provider.name,
        protocol,
        base_url,
        model: profile.model.as_deref(),
        credential_var,
        headers: &provider.headers,
    };

    let Some(plan) = cx.adapter.direct_provider_launch(&request) else {
        return Err(Refusal::NoDirectProviderMechanism {
            profile: profile.name.clone(),
            harness: profile.harness,
            provider: provider.name.clone(),
            protocol,
        });
    };

    overlay.args.extend(plan.args);
    overlay.env.extend(plan.env);

    if let Some(CredentialPlacement::Environment(destination)) = plan.credential {
        let Some(var) = resolvable else {
            return Err(Refusal::CredentialUnavailable {
                profile: profile.name.clone(),
                harness: profile.harness,
                provider: provider.name.clone(),
                variables: provider.credential_env.clone(),
            });
        };
        // The whole lifetime of a credential in Glasshouse: minted here,
        // moved into the overlay below, dropped at the end of this block.
        // Nothing between those two lines may print, store or copy it.
        let Some(secret) = cx.secrets.resolve(&SecretRef::Environment {
            var: var.to_owned(),
        }) else {
            return Err(Refusal::CredentialUnavailable {
                profile: profile.name.clone(),
                harness: profile.harness,
                provider: provider.name.clone(),
                variables: provider.credential_env.clone(),
            });
        };
        overlay
            .env
            .push((OsString::from(destination), OsString::from(secret.expose())));
    }

    // Names, the provider and the protocol — the adapter's own `mechanism`
    // is variable and override *key* names only, and this adds nothing but
    // identity to it.
    overlay.mechanisms.push(MechanismNote {
        category: "direct provider",
        detail: format!("`{}` over {} — {}", provider.name, protocol, plan.mechanism),
    });

    Ok(())
}

/// Which protocol this launch will use.
///
/// With no `expected_protocol`, it is **the first protocol the harness
/// declares that has a compatible provider candidate** — deterministic by the
/// harness's own declared order, which is the harness's own preference and
/// not an ordering invented here. A declaration with no base URL is excluded
/// before that choice, so it cannot beat a later protocol that is routeable.
/// With an expectation, it is that protocol and no other: an explicit request
/// is a constraint, never a hint, and a provider that does not route it is
/// refused rather than quietly given a neighbouring one.
///
/// The harness's ability to serve an explicitly expected protocol has already
/// been checked by [`resolve`], which refuses with [`Refusal::ProtocolMismatch`].
fn choose_protocol(
    profile: &LaunchProfile,
    adapter: &dyn HarnessAdapter,
    provider: &Provider,
) -> Result<WireProtocol, Refusal> {
    let harness_protocols: &[WireProtocol] = adapter
        .describe()
        .backends
        .protocols
        .value()
        .copied()
        .unwrap_or(&[]);

    let compatible = |protocol| {
        ProtocolCompatibleProviders::for_protocol(std::slice::from_ref(provider), protocol)
            .only()
            .is_some()
    };

    let chosen = match profile.expected_protocol {
        Some(expected) => compatible(expected).then_some(expected),
        None => harness_protocols
            .iter()
            .copied()
            .find(|protocol| compatible(*protocol)),
    };

    if let Some(protocol) = chosen {
        return Ok(protocol);
    }

    // Preserve the more specific diagnostic when the provider declared a
    // matching protocol but gave it no destination. It still did not survive
    // the compatibility filter above, and therefore cannot be selected or
    // ranked.
    let missing_base_url = match profile.expected_protocol {
        Some(expected) => provider
            .serves(expected)
            .is_some_and(|support| support.base_url.is_empty())
            .then_some(expected),
        None => harness_protocols.iter().copied().find(|protocol| {
            provider
                .serves(*protocol)
                .is_some_and(|support| support.base_url.is_empty())
        }),
    };
    if let Some(protocol) = missing_base_url {
        return Err(Refusal::ProviderBaseUrlMissing {
            profile: profile.name.clone(),
            provider: provider.name.clone(),
            protocol,
        });
    }

    Err(Refusal::ProviderProtocolUnsupported {
        profile: profile.name.clone(),
        harness: profile.harness,
        provider: provider.name.clone(),
        served: describe_protocols(
            &provider
                .protocols
                .iter()
                .map(|p| p.protocol)
                .collect::<Vec<_>>(),
        ),
        needed: match profile.expected_protocol {
            Some(expected) => format!("`{expected}`, which the profile asked for"),
            None => describe_protocols(harness_protocols),
        },
    })
}

/// What Phase 9F line 465 calls "a cheap capability check" for one launch
/// profile: the exact request [`crate::provider::discovery::connectivity`]
/// should send to prove "this credential, at this base URL, for this
/// protocol, answers" — or, when the profile's backend gives no fixed
/// combination to test, an honest reason there is nothing to check.
///
/// This never sends the request. [`crate::provider::discovery::connectivity`]
/// blocks its calling thread for as long as
/// [`crate::provider::discovery::ProbeTimeouts::default`] allows, so a caller
/// that wants to check before starting an interactive session must run it off
/// whatever thread draws the terminal — `shell::spawn_provider_probe`
/// already does exactly that for a different Phase 9D line, and is the
/// pattern a caller here should follow. [`capability_probe`] only decides
/// whether a check is possible and, if so, what to send.
#[derive(Debug)]
pub enum CapabilityProbe {
    /// No fixed protocol/base-URL combination exists to test. **Not a
    /// failure** — the launch proceeds exactly as it would if this function
    /// did not exist, and the caller's own report should say so rather than
    /// treating this as an error.
    Unavailable { reason: &'static str },
    /// A request [`crate::provider::discovery::connectivity`] can make.
    Available(crate::provider::discovery::ProbeRequest),
}

/// Build the [`CapabilityProbe`] for `profile`, or say why none is possible.
///
/// # Why a `Native` or gateway-backed profile always answers `Unavailable`
///
/// A [`BackendResource::Native`] profile talks to the harness's own account
/// through a mechanism this crate never sees the credential or base URL
/// for — there is nothing here to build a request from.
///
/// A [`BackendResource::GlasshouseGateway`] profile talks to Glasshouse's
/// own local listener, not to a provider directly, and which upstream
/// provider actually answers behind it is Phase 9H's sticky-routing
/// decision — made per session, not by this profile. Probing the gateway's
/// own loopback address would only prove the gateway this process just
/// started is listening, which is not "this credential, at this base URL,
/// for this protocol, answers" in the sense line 465 asks for; it is
/// reported as unavailable rather than as a check that answers a different
/// question than the one asked.
///
/// # Why a resolvable [`BackendResource::DirectProvider`] is always available
///
/// Once a protocol and base URL can be chosen at all — the same choice
/// `apply_direct_provider` makes — [`crate::provider::discovery::ProbeTarget::BaseUrl`]
/// is always a valid target, even when the provider has no established
/// model-list endpoint. So "no check available" for a direct-provider
/// profile only ever means the combination itself could not be resolved
/// (unconfigured provider, no shared protocol, no base URL) — the same
/// conditions under which [`resolve`] would refuse for an entirely separate
/// reason, so there is nothing new for a probe to report either.
///
/// The credential is resolved the same way `apply_direct_provider` does — the
/// first declared variable that currently has a value — but unlike there, a
/// probe with none is still built: [`crate::provider::discovery`] sends no
/// credential header when given `None`, and the resulting outcome
/// ("reachable" or "unreachable" with no credential involved) is still
/// information a report can use.
pub fn capability_probe(profile: &LaunchProfile, cx: &Resolution<'_>) -> CapabilityProbe {
    use crate::provider::discovery::{ProbeRequest, ProbeTarget};

    let BackendResource::DirectProvider { .. } = &profile.backend else {
        return CapabilityProbe::Unavailable {
            reason: match profile.backend {
                BackendResource::Native => {
                    "a native profile uses the harness's own account, which this crate holds \
                     no protocol, base URL or credential for"
                }
                BackendResource::GlasshouseGateway => {
                    "a gateway-backed profile talks to Glasshouse's own local listener; which \
                     upstream provider actually answers is decided per session, not by this \
                     profile, so there is no fixed combination to test yet"
                }
                BackendResource::DirectProvider { .. } => {
                    unreachable!("matched above")
                }
            },
        };
    };

    let Some(provider) = cx.provider else {
        return CapabilityProbe::Unavailable {
            reason: "the profile's provider is not configured",
        };
    };

    let Ok(protocol) = choose_protocol(profile, cx.adapter, provider) else {
        return CapabilityProbe::Unavailable {
            reason: "no protocol is common to what the provider serves and what the harness \
                      can be pointed at",
        };
    };

    let Some(base_url) = declared_base_url(provider, protocol) else {
        return CapabilityProbe::Unavailable {
            reason: "the provider declares no base URL for the chosen protocol",
        };
    };

    let target = if provider.model_list_endpoint.is_known_present() {
        ProbeTarget::ModelList
    } else {
        ProbeTarget::BaseUrl
    };

    // The same search `apply_direct_provider` performs: the first declared
    // credential variable that currently resolves. `None` is not refused
    // here the way it is there — a probe with no credential still answers a
    // real question about the base URL and protocol.
    let credential = provider.credential_env.iter().find_map(|var| {
        cx.secrets
            .resolve(&SecretRef::Environment { var: var.clone() })
    });

    CapabilityProbe::Available(ProbeRequest::new(
        provider.name.clone(),
        protocol,
        base_url.to_owned(),
        target,
        provider.headers.clone(),
        credential,
    ))
}

/// Render what a capability check found, in the wording Phase 9F line 465
/// asks for: distinguishing "it refused the credential" from "it never
/// answered" rather than flattening either to "check failed".
pub fn describe_probe_outcome(outcome: &crate::provider::discovery::ProbeOutcome) -> String {
    use crate::provider::discovery::ProbeOutcome;

    match outcome {
        ProbeOutcome::Reached { status } => format!("reached (status {status})"),
        ProbeOutcome::Rejected { status } => {
            format!("reachable, but it rejected the credential (status {status})")
        }
        ProbeOutcome::Unexpected { status } => {
            format!("reachable, but answered unexpectedly (status {status})")
        }
        ProbeOutcome::TimedOut { waited_ms } => {
            format!("did not answer within {waited_ms}ms")
        }
        ProbeOutcome::Unreachable { reason } => format!("never answered: {reason}"),
    }
}

/// A comma-separated list of protocol slugs, or an honest sentence when the
/// list is empty — "nothing" and "an empty list" read the same way to a user
/// and neither is served by printing `[]`.
fn describe_protocols(protocols: &[WireProtocol]) -> String {
    if protocols.is_empty() {
        return "no protocol at all".to_owned();
    }
    protocols
        .iter()
        .map(|protocol| format!("`{protocol}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The harness's own description of its bypass mode, falling back to a
/// generic label in the (should-not-happen) case that `approval_args`
/// answered `Some` while `describe()` disagrees — never a panic on a launch
/// path.
fn bypass_description(adapter: &dyn HarnessAdapter) -> &'static str {
    adapter
        .describe()
        .approvals
        .bypass
        .value()
        .map(|mode: &ApprovalMode| mode.description)
        .unwrap_or("bypass all approval checks")
}

/// Append `args` to `overlay` and record what was resolved, for diagnostics.
fn append_approval(
    adapter: &dyn HarnessAdapter,
    overlay: &mut LaunchOverlay,
    args: &[&'static str],
    label: &'static str,
) {
    overlay
        .args
        .extend(args.iter().map(|arg| OsString::from(*arg)));

    let description = match label {
        "bypass" => bypass_description(adapter),
        _ => adapter
            .describe()
            .approvals
            .automatic_review
            .value()
            .map(|mode: &ApprovalMode| mode.description)
            .unwrap_or(label),
    };
    overlay.mechanisms.push(MechanismNote {
        category: "approval mode",
        detail: format!("{label}: {description} ({})", args.join(" ")),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{Declared, adapter_for};
    use crate::provider::ProtocolSupport;

    fn profile_for(harness: IntegrationId) -> LaunchProfile {
        LaunchProfile::native(harness)
    }

    /// A [`SecretStore`] holding values in memory.
    ///
    /// Deliberately not [`crate::secret::EnvironmentSecretStore`]: a test
    /// that set a real environment variable would make the credential
    /// visible to every other test in this process and to anything that
    /// inspected it, which is exactly the exposure this phase exists to
    /// prevent. It also keeps these tests free of `std::env`, which
    /// `harness::resolving_a_launch_profile_touches_no_files` forbids in
    /// this module's production code.
    struct FakeSecrets(Vec<(String, String)>);

    impl FakeSecrets {
        fn empty() -> Self {
            Self(Vec::new())
        }

        fn holding(var: &str, value: &str) -> Self {
            Self(vec![(var.to_owned(), value.to_owned())])
        }
    }

    impl crate::secret::SecretStore for FakeSecrets {
        fn resolve(&self, reference: &SecretRef) -> Option<crate::secret::Secret> {
            // Forced by `SecretRef` gaining `OsCredential`: this fake holds
            // variable names, so a reference naming the OS store is one it
            // has nothing to answer with. No production line in this module
            // changed.
            let SecretRef::Environment { var } = reference else {
                return None;
            };
            self.0
                .iter()
                .find(|(name, _)| name == var)
                .map(|(_, value)| crate::secret::Secret::mint_for_test(value))
        }

        fn is_present(&self, reference: &SecretRef) -> bool {
            let SecretRef::Environment { var } = reference else {
                return false;
            };
            self.0.iter().any(|(name, _)| name == var)
        }

        fn describe(&self) -> &'static str {
            "in-memory test store"
        }
    }

    /// The context every pre-9F test used implicitly: one adapter, no
    /// provider, no credential.
    fn native_cx<'a>(
        adapter: &'a dyn HarnessAdapter,
        acknowledged_bypass: bool,
        secrets: &'a dyn SecretStore,
    ) -> Resolution<'a> {
        Resolution {
            adapter,
            acknowledged_bypass,
            provider: None,
            secrets,
        }
    }

    fn provider_serving(name: &str, protocol: WireProtocol, base_url: &str) -> Provider {
        Provider {
            name: name.to_owned(),
            protocols: vec![ProtocolSupport {
                protocol,
                base_url: base_url.to_owned(),
                streaming: Declared::Unverified,
                tool_calls: Declared::Unverified,
                reasoning: Declared::Unverified,
            }],
            model_list_endpoint: Declared::Unverified,
            usage_telemetry: Declared::Unverified,
            credential_env: Vec::new(),
            headers: Vec::new(),
        }
    }

    fn direct_profile(harness: IntegrationId, provider: &str) -> LaunchProfile {
        let mut profile = LaunchProfile::native(harness);
        profile.name = "gateway".to_owned();
        profile.backend = BackendResource::DirectProvider {
            provider: provider.to_owned(),
        };
        profile
    }

    fn env_value<'a>(overlay: &'a LaunchOverlay, key: &str) -> Option<&'a std::ffi::OsStr> {
        overlay
            .env()
            .iter()
            .find(|(name, _)| name == std::ffi::OsStr::new(key))
            .map(|(_, value)| value.as_os_str())
    }

    fn rendered_args(overlay: &LaunchOverlay) -> Vec<String> {
        overlay
            .args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    // --- 1. every harness has a Native profile that adds nothing ---------

    #[test]
    fn a_native_profile_exists_for_every_harness_and_adds_nothing() {
        for &id in IntegrationId::ALL {
            let Some(adapter) = adapter_for(id) else {
                continue;
            };
            let profile = LaunchProfile::native(id);
            assert_eq!(profile.harness, id);
            assert_eq!(profile.backend, BackendResource::Native);
            assert_eq!(profile.class(), ProfileClass::NativeSubscription);

            let overlay = resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty()))
                .unwrap_or_else(|err| panic!("{}'s native profile must resolve: {err}", id.slug()));

            // Whether or not the harness declares automatic review, the
            // *native* profile only ever contributes what the harness's own
            // automatic-review mode would add — for the harnesses that have
            // none, that is nothing at all.
            let expects_args = adapter
                .approval_args(ApprovalKind::AutomaticReview)
                .is_some();
            assert_eq!(
                !overlay.args().is_empty(),
                expects_args,
                "{}'s native profile args did not match its declared automatic review",
                id.slug()
            );
            assert!(
                overlay.env().is_empty(),
                "{} native profile added env",
                id.slug()
            );
        }
    }

    // --- 2. explicit automatic review, refused where none exists ---------

    #[test]
    fn an_explicit_automatic_review_request_is_refused_on_a_harness_without_one() {
        // OpenCode declares no automatic review (only a blanket `--auto`).
        let adapter = adapter_for(IntegrationId::OpenCode).expect("a harness");
        let mut profile = profile_for(IntegrationId::OpenCode);
        profile.approval = ApprovalSelection::AutomaticReview;

        let err = resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty()))
            .expect_err("must be refused");
        match &err {
            Refusal::NoAutomaticReview {
                profile: name,
                harness,
            } => {
                assert_eq!(name, NATIVE_PROFILE_NAME);
                assert_eq!(*harness, IntegrationId::OpenCode);
            }
            other => panic!("expected NoAutomaticReview, got {other:?}"),
        }
        let message = err.to_string();
        assert!(message.contains("OpenCode"), "{message}");
        assert!(message.contains("automatic-review"), "{message}");
    }

    // --- 3. a defaulted profile adds no approval argument on such a harness

    #[test]
    fn a_defaulted_profile_on_such_a_harness_adds_no_approval_argument() {
        let adapter = adapter_for(IntegrationId::OpenCode).expect("a harness");
        let profile = profile_for(IntegrationId::OpenCode); // ApprovalSelection::Default

        let overlay = resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty())).unwrap();
        assert!(
            overlay.args().is_empty(),
            "a defaulted profile must add no approval argument at all: {:?}",
            overlay.args()
        );

        // Explicitly: not the bypass argument either, acknowledged or not.
        let bypass_args = adapter
            .approval_args(ApprovalKind::Bypass)
            .expect("OpenCode declares a bypass mode");
        for arg in &bypass_args {
            assert!(
                !overlay
                    .args()
                    .iter()
                    .any(|a| a == std::ffi::OsStr::new(arg)),
                "a defaulted profile must never carry the bypass argument `{arg}`"
            );
        }

        let overlay_acknowledged =
            resolve(&profile, &native_cx(adapter, true, &FakeSecrets::empty())).unwrap();
        assert!(overlay_acknowledged.args().is_empty());
    }

    // --- 4. bypass refused until acknowledged, per harness ----------------

    #[test]
    fn a_bypass_is_refused_until_it_is_acknowledged_for_that_harness() {
        let adapter = adapter_for(IntegrationId::Hermes).expect("a harness");
        let mut profile = profile_for(IntegrationId::Hermes);
        profile.approval = ApprovalSelection::Bypass;

        let err = resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty()))
            .expect_err("unacknowledged bypass is refused");
        let description = match &err {
            Refusal::BypassNotAcknowledged {
                profile: name,
                harness,
                description,
            } => {
                assert_eq!(name, NATIVE_PROFILE_NAME);
                assert_eq!(*harness, IntegrationId::Hermes);
                *description
            }
            other => panic!("expected BypassNotAcknowledged, got {other:?}"),
        };
        assert!(!description.is_empty());
        assert!(err.to_string().contains(description));

        let overlay = resolve(&profile, &native_cx(adapter, true, &FakeSecrets::empty()))
            .expect("acknowledged bypass resolves");
        let expected_args = adapter.approval_args(ApprovalKind::Bypass).unwrap();
        let rendered: Vec<String> = overlay
            .args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(rendered, expected_args);

        // Acknowledging Hermes must not acknowledge a different harness.
        let other_adapter = adapter_for(IntegrationId::Antigravity).expect("a harness");
        let mut other_profile = profile_for(IntegrationId::Antigravity);
        other_profile.approval = ApprovalSelection::Bypass;
        let err = resolve(
            &other_profile,
            &native_cx(other_adapter, false, &FakeSecrets::empty()),
        )
        .expect_err("Hermes's acknowledgement must not carry over to Antigravity");
        assert!(matches!(err, Refusal::BypassNotAcknowledged { .. }));
    }

    // --- 5. the gateway backend, and what it resolves into ---------------

    /// The gateway is a *process a caller started*, so a call site with none
    /// to offer cannot resolve a profile that needs one. It refuses by
    /// saying exactly that, and starts nothing.
    ///
    /// This is also what keeps [`resolve`]'s one-argument form honest: it
    /// forwards `None`, so every existing caller behaves as it always did.
    #[test]
    fn a_gateway_backed_profile_is_refused_when_no_gateway_is_running() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let mut profile = profile_for(IntegrationId::ClaudeCode);
        profile.backend = BackendResource::GlasshouseGateway;

        let err = resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty()))
            .expect_err("no gateway was supplied");
        match &err {
            Refusal::GatewayNotRunning { harness, .. } => {
                assert_eq!(*harness, IntegrationId::ClaudeCode);
            }
            other => panic!("expected GatewayNotRunning, got {other:?}"),
        }
        let message = err.to_string();
        assert!(message.contains("Claude Code"), "{message}");
        assert!(message.contains("gateway"), "{message}");
    }

    /// Phase 9H lines 505, 506 and 507: resolving a gateway-backed profile —
    /// which is what `main.rs`'s `launch_session` does, through this exact
    /// function — is what gives the session its backend assignment.
    ///
    /// **This test exists because a mutation survived without it.** Deleting
    /// `apply_gateway`'s call to `Gateway::routing().bind` broke nothing: the
    /// gateway's own conformance tests bind the assignment themselves, so
    /// every one of them passed against a build in which the production
    /// launch path recorded no assignment at all. A capability whose only
    /// caller can be deleted silently does not have a caller.
    #[test]
    fn resolving_a_gateway_backed_profile_assigns_the_session_a_provider_and_a_model() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let gateway = gateway_serving(&[WireProtocol::AnthropicMessages]);
        let mut profile = profile_for(IntegrationId::ClaudeCode);
        profile.backend = BackendResource::GlasshouseGateway;
        profile.model = Some("a-named-model".to_owned());

        assert!(
            gateway.routing().assignment().is_none(),
            "a gateway that no profile has resolved through has assigned nothing"
        );

        let overlay = resolve_with_gateway(
            &profile,
            &native_cx(adapter, false, &FakeSecrets::empty()),
            Some(&gateway),
        )
        .expect("the gateway serves the protocol this harness speaks");

        let assignment = gateway
            .routing()
            .assignment()
            .expect("resolving a gateway-backed profile assigns its backend");
        assert_eq!(assignment.harness(), IntegrationId::ClaudeCode.slug());
        assert_eq!(assignment.provider(), "fixture");
        assert_eq!(
            assignment.protocol(),
            WireProtocol::AnthropicMessages.slug()
        );
        assert_eq!(
            assignment.backend().model(),
            &AssignedModel::named("a-named-model")
        );

        // And the choice is announced rather than silent — the argument
        // `gateway_upstream`'s own documentation rests on.
        let announced = overlay
            .mechanisms
            .iter()
            .find(|note| note.category == "gateway backend")
            .expect("the assignment is reported in the launch's mechanism notes");
        assert!(announced.detail.contains("a-named-model"), "{announced:?}");
        assert!(announced.detail.contains("fixture"), "{announced:?}");
        assert!(
            !announced.detail.contains(PLANTED_CREDENTIAL),
            "a mechanism note must name a credential and never carry one: {announced:?}"
        );
    }

    /// A profile that names no model assigns none, and says so rather than
    /// leaving a reader unable to tell "no model" from "we forgot".
    #[test]
    fn a_gateway_backed_profile_with_no_model_assigns_the_harnesss_own_default() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let gateway = gateway_serving(&[WireProtocol::AnthropicMessages]);
        let mut profile = profile_for(IntegrationId::ClaudeCode);
        profile.backend = BackendResource::GlasshouseGateway;
        profile.model = None;

        resolve_with_gateway(
            &profile,
            &native_cx(adapter, false, &FakeSecrets::empty()),
            Some(&gateway),
        )
        .expect("a profile need not name a model");

        let assignment = gateway.routing().assignment().expect("assigned");
        assert_eq!(assignment.backend().model(), &AssignedModel::HarnessDefault);
        assert!(
            assignment.label().contains("the harness's own default"),
            "{}",
            assignment.label()
        );
    }

    /// Phase 9H line 518, on the path a real launch takes: a profile that
    /// records a pin turns automatic failover off before the session's first
    /// request, and says so in the launch's own mechanism notes.
    ///
    /// The pin lives on the profile because that is where a user can state it
    /// today — see [`LaunchProfile::pin_gateway_backend`]. A pin nobody can
    /// set is not a capability.
    #[test]
    fn a_profile_that_records_a_pin_turns_automatic_failover_off_at_session_start() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let gateway = gateway_serving(&[WireProtocol::AnthropicMessages]);
        let mut profile = profile_for(IntegrationId::ClaudeCode);
        profile.backend = BackendResource::GlasshouseGateway;
        profile.pin_gateway_backend = true;

        let overlay = resolve_with_gateway(
            &profile,
            &native_cx(adapter, false, &FakeSecrets::empty()),
            Some(&gateway),
        )
        .expect("the gateway serves the protocol this harness speaks");

        assert_eq!(
            gateway.routing().pin().provider(),
            Some("fixture"),
            "a profile that records a pin pins the session it starts"
        );
        let note = overlay
            .mechanisms
            .iter()
            .find(|note| note.category == "gateway pin")
            .expect("a pin is a mechanism worth reporting");
        assert!(note.detail.contains("fixture"), "{note:?}");
        assert!(note.detail.contains("failover is off"), "{note:?}");
    }

    /// And a profile that records no pin does not pin, so the default is the
    /// behaviour every profile written before the field existed already had.
    #[test]
    fn a_profile_without_a_pin_leaves_automatic_failover_on() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let gateway = gateway_serving(&[WireProtocol::AnthropicMessages]);
        let mut profile = profile_for(IntegrationId::ClaudeCode);
        profile.backend = BackendResource::GlasshouseGateway;

        let overlay = resolve_with_gateway(
            &profile,
            &native_cx(adapter, false, &FakeSecrets::empty()),
            Some(&gateway),
        )
        .expect("resolvable");

        assert_eq!(gateway.routing().pin().provider(), None);
        assert!(
            !overlay
                .mechanisms
                .iter()
                .any(|note| note.category == "gateway pin")
        );
    }

    /// A running gateway serving `protocols`, for the tests below. Its
    /// upstream never has to answer: resolution reads the gateway's address
    /// and token and opens no connection at all.
    ///
    /// Which protocols it serves is the parameter because that is now the
    /// thing under test: a gateway serves what its one configured provider
    /// declared a base URL for, and `apply_gateway` refuses against exactly
    /// that.
    fn gateway_serving(protocols: &[WireProtocol]) -> crate::gateway::Gateway {
        let profiles = [{
            let mut profile = profile_for(IntegrationId::ClaudeCode);
            profile.backend = BackendResource::GlasshouseGateway;
            profile
        }];
        crate::gateway::start_if_required(&profiles, || {
            Ok(Upstream::new(
                "fixture".to_owned(),
                protocols
                    .iter()
                    .map(|protocol| {
                        Route::new(
                            protocol.slug().to_owned(),
                            ingress_targets(*protocol),
                            "https://provider.example/api",
                        )
                    })
                    .collect(),
                crate::secret::Secret::mint_for_test(PLANTED_CREDENTIAL),
                crate::routing::CredentialId::new(
                    "fixture",
                    crate::secret::SecretRef::Environment {
                        var: "FIXTURE_API_KEY".to_owned(),
                    },
                ),
            )?)
        })
        .expect("loopback is bindable")
        .expect("a gateway-backed profile asks for a gateway")
    }

    /// A running gateway serving the protocol Claude Code speaks — the shape
    /// every test written before the ingress served more than one assumes.
    fn running_gateway() -> crate::gateway::Gateway {
        gateway_serving(&[WireProtocol::AnthropicMessages])
    }

    /// Phase 9G's OpenAI Responses ingress, at the resolution layer: a
    /// gateway whose provider serves Responses **resolves a Codex profile**,
    /// which is the line's whole point.
    ///
    /// Codex 0.149.1 removed `wire_api = "chat"` — confirmed against the
    /// installed binary, which answers
    /// ``Error loading config.toml: `wire_api = "chat"` is no longer
    /// supported.`` — so Responses is the only protocol that can ever back a
    /// Codex profile, and this ingress is therefore the only gateway path to
    /// one. The same binary pointed at a path-less base URL was observed
    /// sending `POST /responses`, which is why
    /// [`ingress_targets`] declares the bare form.
    ///
    /// Lose this and the Responses ingress can exist in the gateway while
    /// remaining unreachable from the only harness that speaks it.
    #[test]
    fn a_gateway_serving_responses_resolves_a_codex_profile() {
        let adapter = adapter_for(IntegrationId::Codex).expect("a harness");
        let gateway = gateway_serving(&[WireProtocol::OpenAiResponses]);
        let mut profile = profile_for(IntegrationId::Codex);
        profile.backend = BackendResource::GlasshouseGateway;

        let overlay = resolve_with_gateway(
            &profile,
            &native_cx(adapter, false, &FakeSecrets::empty()),
            Some(&gateway),
        )
        .expect("a Codex profile resolves against a gateway that serves Responses");

        let rendered = format!("{:?}", overlay.args());
        assert!(
            rendered.contains(&format!("http://{}", gateway.address())),
            "the child was not pointed at this gateway: {rendered}"
        );
        assert!(
            rendered.contains("responses"),
            "the child was not configured for the Responses wire API: {rendered}"
        );

        // The gateway's own token reaches the child, and the provider
        // credential the gateway holds does not — the same rule the Claude
        // Code path already carries, asserted again on the path that did not
        // exist when it was written.
        let env: Vec<(String, String)> = overlay
            .env()
            .iter()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
            .collect();
        assert!(
            env.iter()
                .any(|(_, value)| value == gateway.token().expose()),
            "the gateway's token did not reach the child: {env:?}"
        );
        for (key, value) in &env {
            assert!(
                !value.contains(PLANTED_CREDENTIAL),
                "the provider credential reached the child in {key}"
            );
        }
    }

    /// A gateway serving every protocol its ingress knows how to carry
    /// resolves both harnesses that declare one — and hands each the
    /// protocol it actually speaks, never the first one the list happens to
    /// name.
    ///
    /// Lose this and `apply_gateway` can go back to picking
    /// `GATEWAY_INGRESS_PROTOCOLS[0]`, which now silently means "Anthropic
    /// Messages for everyone".
    #[test]
    fn each_harness_is_given_the_protocol_it_speaks_not_the_first_one_served() {
        let gateway = gateway_serving(GATEWAY_INGRESS_PROTOCOLS);
        assert_eq!(
            gateway.served_protocols(),
            vec!["anthropic-messages", "openai-responses", "openai-chat"],
            "this test proves nothing unless the gateway really serves all three"
        );

        for (harness, expected) in [
            (IntegrationId::ClaudeCode, "anthropic-messages"),
            (IntegrationId::Codex, "openai-responses"),
        ] {
            let adapter = adapter_for(harness).expect("a harness");
            let mut profile = profile_for(harness);
            profile.backend = BackendResource::GlasshouseGateway;

            let overlay = resolve_with_gateway(
                &profile,
                &native_cx(adapter, false, &FakeSecrets::empty()),
                Some(&gateway),
            )
            .unwrap_or_else(|err| panic!("{harness:?} did not resolve: {err}"));

            let note = overlay
                .mechanisms()
                .iter()
                .find(|note| note.category == "glasshouse gateway")
                .unwrap_or_else(|| panic!("{harness:?} recorded no gateway mechanism"));
            assert!(
                note.detail.contains(expected),
                "{harness:?} was given the wrong protocol: {}",
                note.detail
            );
        }
    }

    /// The target table and the protocol list are two halves of one fact,
    /// and nothing else checks that they agree.
    ///
    /// [`ingress_targets`] is a `match` on [`WireProtocol`], so the compiler
    /// already refuses to let a protocol go unlisted. What it cannot check
    /// is that each entry is **non-empty** and **distinct** — a protocol
    /// whose targets were an empty slice would be declared served and would
    /// place no request at all, and two protocols sharing a prefix would
    /// make routing depend on declaration order.
    #[test]
    fn the_ingress_target_table_covers_every_protocol_the_gateway_serves() {
        let mut seen: Vec<&str> = Vec::new();
        for protocol in GATEWAY_INGRESS_PROTOCOLS {
            let targets = ingress_targets(*protocol);
            assert!(
                !targets.is_empty(),
                "{protocol} declares no request target, so nothing could ever be routed to it"
            );
            for target in targets {
                assert!(
                    target.starts_with('/'),
                    "{protocol}'s target {target:?} is not a path"
                );
                assert!(
                    !seen.contains(target),
                    "{target:?} is declared by two protocols, so routing would depend on the \
                     order they happen to be listed in"
                );
                seen.push(target);
            }
        }
        assert_eq!(GATEWAY_INGRESS_PROTOCOLS.len(), 3);
    }

    /// Phase 9G's line 1 for Claude Code, end to end at the resolution
    /// layer: a gateway-backed profile **resolves**, and the child is
    /// pointed at the local gateway with the gateway's own token.
    ///
    /// The two environment variables are asserted by name and by value
    /// because both are the capability. `ANTHROPIC_BASE_URL` pointing
    /// anywhere but this gateway would send the user's prompts somewhere
    /// nobody chose; `ANTHROPIC_AUTH_TOKEN` holding anything but the
    /// gateway's token would either fail authentication or — much worse —
    /// be the provider key this whole phase exists to keep out of the child.
    #[test]
    fn a_gateway_backed_claude_code_profile_resolves_into_the_local_gateway() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let gateway = running_gateway();
        let mut profile = profile_for(IntegrationId::ClaudeCode);
        profile.backend = BackendResource::GlasshouseGateway;

        let overlay = resolve_with_gateway(
            &profile,
            &native_cx(adapter, false, &FakeSecrets::empty()),
            Some(&gateway),
        )
        .expect("a gateway-backed Claude Code profile resolves once a gateway is running");

        let env: Vec<(String, String)> = overlay
            .env()
            .iter()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
            .collect();

        assert!(
            env.contains(&(
                "ANTHROPIC_BASE_URL".to_owned(),
                format!("http://{}", gateway.address())
            )),
            "the child was not pointed at this gateway: {env:?}"
        );
        assert!(
            env.contains(&(
                "ANTHROPIC_AUTH_TOKEN".to_owned(),
                gateway.token().expose().to_owned()
            )),
            "the child was not given this gateway's own token"
        );
        // ... and nothing resembling a provider credential went with it.
        assert!(
            !env.iter()
                .any(|(_, value)| value.contains(PLANTED_CREDENTIAL)),
            "a provider credential reached the child of a gateway-backed profile"
        );

        let mechanisms = overlay
            .mechanisms()
            .iter()
            .map(|note| format!("{}: {}", note.category, note.detail))
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(mechanisms.contains("glasshouse gateway"), "{mechanisms}");
        assert!(
            !mechanisms.contains(gateway.token().expose()),
            "a mechanism note carried the gateway token"
        );
    }

    /// A harness the *running* gateway cannot carry is refused rather than
    /// pointed at it anyway.
    ///
    /// This test used to hold because there was no OpenAI Responses ingress
    /// at all. There is one now, and it still holds — for the reason that
    /// actually matters. The ingress can serve Responses; **this** gateway
    /// does not, because the one provider behind it declares no Responses
    /// base URL. Codex declares `openai-responses` and nothing else, so the
    /// refusal comes before a child process exists, rather than as a `404`
    /// on the harness's first request.
    ///
    /// Lose this and `apply_gateway` starts refusing against
    /// [`GATEWAY_INGRESS_PROTOCOLS`] — what the ingress *could* serve — and
    /// a Codex session comes up pointed at a gateway with no route for it.
    #[test]
    fn a_harness_that_cannot_speak_the_ingress_protocol_is_refused() {
        let adapter = adapter_for(IntegrationId::Codex).expect("a harness");
        let gateway = running_gateway();
        let mut profile = profile_for(IntegrationId::Codex);
        profile.backend = BackendResource::GlasshouseGateway;

        let err = resolve_with_gateway(
            &profile,
            &native_cx(adapter, false, &FakeSecrets::empty()),
            Some(&gateway),
        )
        .expect_err("there is no OpenAI Responses ingress in this phase");
        assert!(
            matches!(err, Refusal::GatewayProtocolUnserved { .. }),
            "{err:?}"
        );
    }

    /// A profile that explicitly expects a protocol the ingress does not
    /// serve is refused too. An explicit ask is a constraint, never a hint —
    /// the same rule the direct-provider path applies.
    #[test]
    fn a_gateway_profile_expecting_another_protocol_is_refused() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let gateway = running_gateway();
        let mut profile = profile_for(IntegrationId::ClaudeCode);
        profile.backend = BackendResource::GlasshouseGateway;
        profile.expected_protocol = Some(WireProtocol::OpenAiChat);

        let err = resolve_with_gateway(
            &profile,
            &native_cx(adapter, false, &FakeSecrets::empty()),
            Some(&gateway),
        )
        .expect_err("Claude Code cannot be pointed at openai-chat at all");
        // Refused by the generic protocol check before the gateway arm is
        // reached — which is the right layer for it, and is asserted so that
        // moving the check does not silently change the message.
        assert!(matches!(err, Refusal::ProtocolMismatch { .. }), "{err:?}");
    }

    /// Which provider a gateway forwards to is a routing decision, and this
    /// phase makes exactly one of them: the single configured provider that
    /// serves the ingress protocol. Zero and several are both refusals that
    /// name what was found.
    #[test]
    fn the_gateway_upstream_is_the_one_provider_that_serves_the_ingress() {
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);

        let mut anthropic = provider_serving(
            "openrouter",
            WireProtocol::AnthropicMessages,
            "https://openrouter.ai/api",
        );
        anthropic.credential_env = vec![CREDENTIAL_VAR.to_owned()];

        let upstream = gateway_upstream(std::slice::from_ref(&anthropic), &secrets)
            .expect("exactly one provider serves the ingress");
        let rendered = format!("{upstream:?}");
        assert!(rendered.contains("openrouter"), "{rendered}");
        assert!(
            !rendered.contains(PLANTED_CREDENTIAL),
            "the upstream's own rendering carried the credential it holds"
        );

        // A provider serving only OpenAI Chat is a candidate now: the
        // ingress serves that protocol too, and this is the line that
        // changed when it started to. Before, it was the example of
        // "serves nothing the ingress offers".
        let mut chat_only =
            provider_serving("chat", WireProtocol::OpenAiChat, "https://a.example/v1");
        chat_only.credential_env = vec![CREDENTIAL_VAR.to_owned()];
        let chat_upstream = gateway_upstream(std::slice::from_ref(&chat_only), &secrets)
            .expect("a provider serving one ingress protocol backs the gateway");
        assert_eq!(chat_upstream.served_protocols(), vec!["openai-chat"]);

        // Nothing serving the ingress at all: a provider that serves none of
        // the three, and no provider whatsoever.
        let none = provider_serving(
            "unrelated",
            WireProtocol::OpenAiChat,
            // Serving the protocol without declaring where is not serving it.
            "",
        );
        assert!(matches!(
            gateway_upstream(std::slice::from_ref(&none), &secrets),
            Err(GatewayUpstreamRefusal::NoProviderServesTheIngress { .. })
        ));
        assert!(matches!(
            gateway_upstream(&[], &secrets),
            Err(GatewayUpstreamRefusal::NoProviderServesTheIngress { .. })
        ));

        // A provider that serves it but declares no base URL is not a
        // candidate: launching against `""` must never happen, which is the
        // same rule `apply_direct_provider` already applies.
        let mut no_url = provider_serving("no-url", WireProtocol::AnthropicMessages, "");
        no_url.credential_env = vec![CREDENTIAL_VAR.to_owned()];
        assert!(matches!(
            gateway_upstream(&[no_url], &secrets),
            Err(GatewayUpstreamRefusal::NoProviderServesTheIngress { .. })
        ));

        // Several providers is Phase 9H's assignment plus its failover
        // candidates, in configuration order — no longer the refusal Phase 9G
        // answered with. See `gateway_upstream`'s own documentation for why
        // choosing here is legitimate now and was not then.
        let mut second = provider_serving(
            "another-router",
            WireProtocol::AnthropicMessages,
            "https://another.example/api",
        );
        second.credential_env = vec![CREDENTIAL_VAR.to_owned()];
        let several = gateway_upstream(&[anthropic.clone(), second], &secrets)
            .expect("two protocol-compatible providers is an assignment, not a collision");
        assert_eq!(
            several.backends()[0].credential_id().provider(),
            "openrouter",
            "the first configured provider is the one assigned"
        );
        assert_eq!(
            several
                .backends()
                .iter()
                .map(|backend| backend.credential_id().provider().to_owned())
                .collect::<Vec<_>>(),
            vec!["openrouter", "another-router"],
            "the rest are where a real provider failure may move the session"
        );

        // And a provider whose credential variable holds nothing is refused
        // rather than launched without one — the gateway would otherwise
        // forward requests with an empty bearer token and the user would see
        // the provider's own 401.
        match gateway_upstream(&[anthropic], &FakeSecrets::empty()) {
            Err(GatewayUpstreamRefusal::CredentialUnavailable { variables, .. }) => {
                assert_eq!(variables, vec![CREDENTIAL_VAR.to_owned()]);
            }
            other => panic!("expected CredentialUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_gateway_candidate_set_names_the_requirement_and_declarations() {
        let declares_chat_without_a_destination =
            provider_serving("chat-only", WireProtocol::OpenAiChat, "");
        let declares_responses_without_a_destination =
            provider_serving("responses-without-url", WireProtocol::OpenAiResponses, "");

        let refusal = gateway_upstream(
            &[
                declares_chat_without_a_destination,
                declares_responses_without_a_destination,
            ],
            &FakeSecrets::empty(),
        )
        .expect_err("no provider can route any protocol the gateway requires");
        let rendered = refusal.to_string();

        let GatewayUpstreamRefusal::NoProviderServesTheIngress { protocols, served } = refusal
        else {
            panic!("expected a no-compatible-provider refusal");
        };
        assert!(protocols.contains(&WireProtocol::AnthropicMessages));
        assert!(
            served.contains("`chat-only` declares `openai-chat` (no base URL)"),
            "{served}"
        );
        assert!(
            served.contains("`responses-without-url` declares `openai-responses` (no base URL)"),
            "{served}"
        );
        assert!(rendered.contains("anthropic-messages"), "{rendered}");
        assert!(rendered.contains(&served), "{rendered}");
    }

    /// Every rendering of a gateway upstream refusal, checked against a
    /// planted value. These are printed on a launch path, which is exactly
    /// where a credential would be seen.
    #[test]
    fn no_gateway_upstream_refusal_carries_a_credential() {
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
        let mut anthropic = provider_serving(
            "openrouter",
            WireProtocol::AnthropicMessages,
            "https://openrouter.ai/api",
        );
        anthropic.credential_env = vec![CREDENTIAL_VAR.to_owned()];
        let mut unusable = provider_serving(
            "unusable",
            WireProtocol::AnthropicMessages,
            "not-an-absolute-url",
        );
        unusable.credential_env = vec![CREDENTIAL_VAR.to_owned()];

        let refusals = vec![
            gateway_upstream(&[], &secrets).unwrap_err(),
            gateway_upstream(&[anthropic], &FakeSecrets::empty()).unwrap_err(),
            gateway_upstream(&[unusable], &secrets).unwrap_err(),
        ];

        let mut seen = std::collections::BTreeSet::new();
        for refusal in &refusals {
            let display = refusal.to_string();
            let debug = format!("{refusal:?}");
            assert!(!display.contains(PLANTED_CREDENTIAL), "{display}");
            assert!(!debug.contains(PLANTED_CREDENTIAL), "{debug}");
            seen.insert(match refusal {
                GatewayUpstreamRefusal::NoProviderServesTheIngress { .. } => "none",
                GatewayUpstreamRefusal::CredentialUnavailable { .. } => "credential",
                GatewayUpstreamRefusal::Unusable(_) => "unusable",
            });
        }
        assert_eq!(seen.len(), 3, "every variant must be exercised: {seen:?}");
    }

    /// A direct-provider profile whose provider the caller could not look up
    /// is refused too — and it names the provider, so the user knows which
    /// configuration entry is missing.
    #[test]
    fn a_direct_provider_profile_with_no_configured_provider_is_refused() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let profile = direct_profile(IntegrationId::ClaudeCode, "not-configured");

        let err = resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty()))
            .expect_err("an unconfigured provider is refused");
        match &err {
            Refusal::ProviderNotConfigured {
                harness, provider, ..
            } => {
                assert_eq!(*harness, IntegrationId::ClaudeCode);
                assert_eq!(provider, "not-configured");
            }
            other => panic!("expected ProviderNotConfigured, got {other:?}"),
        }
        assert!(err.to_string().contains("not-configured"));
    }

    // --- 6. an overlay reaches only the child process ---------------------

    #[test]
    fn an_overlay_reaches_only_the_child_process() {
        use crate::Project;
        use crate::platform::exec;

        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("fake-harness");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
        }
        let executable = exec::resolve_explicit(&script).expect("resolve");

        let root = tmp.path().join("proj");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let project = Project::discover(&root, None, false).unwrap();

        // Constructed directly rather than through `resolve`, to exercise
        // `apply` in isolation with no provider and no secret store in the
        // way.
        //
        // Until Phase 9F this was the *only* way to reach `apply` with an
        // environment at all, because only `Native` resolved and it
        // contributes none. That is no longer true — a direct-provider
        // profile now populates `env` through `resolve`, which is what
        // `a_claude_code_profile_carries_the_providers_base_url_and_credential`
        // asserts. The two together are the chain, and this half is
        // deliberately kept unit-sized: it is about `apply` carrying an
        // environment operation onto a child, not about where one came from.
        let overlay = LaunchOverlay {
            args: vec![OsString::from("--overlay-flag")],
            env: vec![(
                OsString::from("GLASSHOUSE_TEST_OVERLAY_KEY"),
                OsString::from("unmistakable-secret-shaped-value"),
            )],
            mechanisms: vec![MechanismNote {
                category: "approval mode",
                detail: "automatic review (--overlay-flag)".to_owned(),
            }],
        };

        // Before consuming the overlay: its own safe rendering never carries
        // the value either.
        for note in overlay.mechanisms() {
            assert!(!note.detail.contains("unmistakable-secret-shaped-value"));
        }

        let launch = HarnessLaunch::new(executable, &project);
        let launch = overlay.apply(launch);

        // The overlay reached the launch: the env key (never the value) and
        // the arg count both show up in the launch's own redacted `Debug`.
        let rendered = format!("{launch:?}");
        assert!(
            rendered.contains("GLASSHOUSE_TEST_OVERLAY_KEY"),
            "{rendered}"
        );
        assert!(rendered.contains("\"set\""), "{rendered}");
        assert!(
            !rendered.contains("unmistakable-secret-shaped-value"),
            "the env value leaked into the launch's Debug: {rendered}"
        );
        assert!(rendered.contains("arg_count: 1"), "{rendered}");
    }

    // --- 7. the user's own arguments stay last -----------------------------

    #[test]
    fn the_user_s_own_arguments_stay_last() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let profile = profile_for(IntegrationId::ClaudeCode); // Default -> automatic review
        let overlay = resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty())).unwrap();
        assert!(
            !overlay.args().is_empty(),
            "Claude Code declares automatic review"
        );

        let adapter_args = vec![OsString::from("--session-id"), OsString::from("abc")];
        let user_args = [OsString::from("--resume"), OsString::from("xyz")];

        // The same order production code composes: adapter args, then the
        // overlay's own args, then the user's own `--` arguments.
        let mut composed = adapter_args.clone();
        composed.extend(overlay.args().iter().cloned());
        composed.extend(user_args.iter().cloned());

        assert_eq!(&composed[..adapter_args.len()], &adapter_args[..]);
        assert_eq!(
            &composed[adapter_args.len()..composed.len() - user_args.len()],
            overlay.args(),
            "the overlay's own arguments must sit strictly between the adapter's and the user's"
        );
        assert_eq!(
            &composed[composed.len() - user_args.len()..],
            &user_args[..],
            "the user's own arguments must be last, so they always win"
        );
    }

    // --- 11. no environment value is ever rendered -------------------------

    #[test]
    fn no_environment_value_is_ever_rendered() {
        // A model on a profile is validated but never turned into an
        // argument or an environment value in Phase 9A (see `resolve`'s
        // comment) — so even an unmistakably secret-shaped model name must
        // never surface anywhere resolution can render.
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let mut profile = profile_for(IntegrationId::ClaudeCode);
        profile.model = Some("sk-SUPER-SECRET-MODEL-VALUE-should-never-render".to_owned());

        let overlay = resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty()))
            .expect("Claude Code declares a model override");

        let args_rendered: Vec<String> = overlay
            .args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            args_rendered.iter().all(|a| !a.contains("SUPER-SECRET")),
            "{args_rendered:?}"
        );
        assert!(overlay.env().is_empty());
        for note in overlay.mechanisms() {
            assert!(!note.detail.contains("SUPER-SECRET"), "{}", note.detail);
        }
        let debug_rendered = format!("{overlay:?}");
        assert!(!debug_rendered.contains("SUPER-SECRET"), "{debug_rendered}");

        // And a refusal on an unrelated rule must not echo the model value
        // back either.
        let opencode = adapter_for(IntegrationId::OpenCode).expect("a harness");
        let mut refused_profile = profile_for(IntegrationId::OpenCode);
        refused_profile.expected_protocol = Some(WireProtocol::OpenAiChat);
        let err = resolve(
            &refused_profile,
            &native_cx(opencode, false, &FakeSecrets::empty()),
        )
        .expect_err("unsupported protocol");
        assert!(!err.to_string().contains("SECRET"));
    }

    // --- Phase 9F: direct provider launch profiles -----------------------

    /// The credential every test below plants in its store. Distinctive on
    /// purpose: a `!contains` assertion is only worth as much as the
    /// improbability of the needle appearing by accident.
    const PLANTED_CREDENTIAL: &str = "sk-glasshouse-planted-credential-must-never-render";
    const CREDENTIAL_VAR: &str = "GLASSHOUSE_TEST_PROVIDER_KEY";

    fn anthropic_provider() -> Provider {
        let mut provider = provider_serving(
            "my-gateway",
            WireProtocol::AnthropicMessages,
            "https://gateway.example/anthropic",
        );
        provider.credential_env = vec![CREDENTIAL_VAR.to_owned()];
        provider
    }

    fn responses_provider() -> Provider {
        let mut provider = provider_serving(
            "my-responses",
            WireProtocol::OpenAiResponses,
            "https://gateway.example/v1",
        );
        provider.credential_env = vec![CREDENTIAL_VAR.to_owned()];
        provider
    }

    fn direct_cx<'a>(
        adapter: &'a dyn HarnessAdapter,
        provider: &'a Provider,
        secrets: &'a dyn SecretStore,
    ) -> Resolution<'a> {
        Resolution {
            adapter,
            acknowledged_bypass: false,
            provider: Some(provider),
            secrets,
        }
    }

    /// Line 1/2/3/5: Claude Code is pointed at a compatible gateway with the
    /// provider's own base URL and the credential the store held, and neither
    /// touches anything but this one child's environment.
    #[test]
    fn a_claude_code_profile_carries_the_providers_base_url_and_credential() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let provider = anthropic_provider();
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
        let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);

        let overlay = resolve(&profile, &direct_cx(adapter, &provider, &secrets))
            .expect("an anthropic-compatible provider backs Claude Code");

        // Verbatim: Claude Code appends `/v1/messages` itself, so nothing
        // here may append or strip a path segment.
        assert_eq!(
            env_value(&overlay, "ANTHROPIC_BASE_URL"),
            Some(std::ffi::OsStr::new("https://gateway.example/anthropic"))
        );
        // Never `assert_eq!` on secret material: a failure prints both sides.
        let token = env_value(&overlay, "ANTHROPIC_AUTH_TOKEN").expect("the credential is placed");
        assert!(
            token == std::ffi::OsStr::new(PLANTED_CREDENTIAL),
            "ANTHROPIC_AUTH_TOKEN did not carry the value the store held"
        );
        // No arguments at all — the mechanism is purely the child's
        // environment, so nothing was written anywhere.
        assert!(
            !rendered_args(&overlay)
                .iter()
                .any(|arg| arg.starts_with("--settings")),
            "Claude Code's direct-provider mechanism must write no settings document"
        );
    }

    /// Line 4: a model is passed through when the profile names one, and
    /// `ANTHROPIC_MODEL` is *absent* — not empty — when it does not.
    #[test]
    fn a_claude_code_profile_carries_a_model_only_when_one_is_named() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let provider = anthropic_provider();
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);

        let without = direct_profile(IntegrationId::ClaudeCode, &provider.name);
        let overlay = resolve(&without, &direct_cx(adapter, &provider, &secrets)).unwrap();
        assert_eq!(
            env_value(&overlay, "ANTHROPIC_MODEL"),
            None,
            "a profile naming no model must leave ANTHROPIC_MODEL unset, not empty"
        );

        let mut with = without.clone();
        with.model = Some("provider/some-model-id".to_owned());
        let overlay = resolve(&with, &direct_cx(adapter, &provider, &secrets)).unwrap();
        assert_eq!(
            env_value(&overlay, "ANTHROPIC_MODEL"),
            Some(std::ffi::OsStr::new("provider/some-model-id"))
        );
    }

    /// Lines 6/7/8/10: Codex gets a whole custom provider out of `-c`
    /// overrides, in a fixed order, and **no file is written at all**.
    #[test]
    fn a_codex_profile_composes_its_provider_entirely_from_c_overrides() {
        let adapter = adapter_for(IntegrationId::Codex).expect("a harness");
        let provider = responses_provider();
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
        let mut profile = direct_profile(IntegrationId::Codex, &provider.name);
        profile.model = Some("some-responses-model".to_owned());

        let overlay = resolve(&profile, &direct_cx(adapter, &provider, &secrets))
            .expect("an openai-responses provider backs Codex");

        let args = rendered_args(&overlay);
        let expected = [
            "-c",
            "model_provider=my-responses",
            "-c",
            "model_providers.my-responses.name=my-responses",
            "-c",
            "model_providers.my-responses.base_url=https://gateway.example/v1",
            "-c",
            "model_providers.my-responses.wire_api=responses",
            "-c",
            &format!("model_providers.my-responses.env_key={CREDENTIAL_VAR}"),
            "-c",
            "model=some-responses-model",
        ]
        .map(str::to_owned);
        assert_eq!(
            &args[..expected.len()],
            &expected[..],
            "the six -c overrides must be composed in a fixed order"
        );

        // `env_key` names a variable of the child process, and the overlay
        // sets exactly that variable — a name agreeing with a destination is
        // the whole mechanism.
        let placed = env_value(&overlay, CREDENTIAL_VAR).expect("the credential is placed");
        assert!(
            placed == std::ffi::OsStr::new(PLANTED_CREDENTIAL),
            "{CREDENTIAL_VAR} did not carry the value the store held"
        );
        // Nothing that looks like a path to a generated configuration.
        assert!(
            !args.iter().any(|arg| arg.contains("config.toml")),
            "Codex's mechanism must name no configuration file: {args:?}"
        );
    }

    /// Codex 0.149.1 removed `wire_api = "chat"`, so a provider serving only
    /// `openai-chat` cannot back Codex. Refused — never a configuration Codex
    /// would reject after the process had already started.
    #[test]
    fn a_codex_profile_backed_by_an_openai_chat_provider_is_refused() {
        let adapter = adapter_for(IntegrationId::Codex).expect("a harness");
        let mut provider = provider_serving(
            "chat-only",
            WireProtocol::OpenAiChat,
            "https://gateway.example/v1",
        );
        provider.credential_env = vec![CREDENTIAL_VAR.to_owned()];
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
        let profile = direct_profile(IntegrationId::Codex, &provider.name);

        let err = resolve(&profile, &direct_cx(adapter, &provider, &secrets))
            .expect_err("openai-chat cannot back Codex 0.149.1");
        match &err {
            Refusal::ProviderProtocolUnsupported {
                harness, provider, ..
            } => {
                assert_eq!(*harness, IntegrationId::Codex);
                assert_eq!(provider, "chat-only");
            }
            other => panic!("expected ProviderProtocolUnsupported, got {other:?}"),
        }
        let message = err.to_string();
        assert!(
            message.contains(WireProtocol::OpenAiChat.slug()),
            "the message must name what the provider serves: {message}"
        );
        assert!(
            message.contains(WireProtocol::OpenAiResponses.slug()),
            "the message must name what Codex needs: {message}"
        );
        assert!(message.contains("Codex"), "{message}");
    }

    /// The real, shipped NVIDIA template — not a synthetic stand-in — is the
    /// honest consequence of declaring `openai-chat` only: it cannot back
    /// Codex, exactly like the synthetic case just above.
    #[test]
    fn a_codex_profile_backed_by_the_real_nvidia_template_is_refused_on_protocol_grounds() {
        let adapter = adapter_for(IntegrationId::Codex).expect("a harness");
        let provider = crate::provider::template("nvidia").expect("nvidia is a built-in template");
        let secrets = FakeSecrets::empty();
        let profile = direct_profile(IntegrationId::Codex, &provider.name);

        let err = resolve(&profile, &direct_cx(adapter, &provider, &secrets))
            .expect_err("NVIDIA declares openai-chat only, which cannot back Codex 0.149.1");
        assert!(matches!(err, Refusal::ProviderProtocolUnsupported { .. }));
    }

    /// Line 423's consumer: configured headers reach Claude Code as one
    /// `ANTHROPIC_CUSTOM_HEADERS` variable, `Name: value` per line.
    #[test]
    fn claude_code_receives_configured_headers_as_a_custom_headers_variable() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let mut provider = anthropic_provider();
        provider.headers = vec![
            ("X-Glasshouse-One".to_owned(), "value-one".to_owned()),
            ("X-Glasshouse-Two".to_owned(), "value-two".to_owned()),
        ];
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
        let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);

        let overlay = resolve(&profile, &direct_cx(adapter, &provider, &secrets)).unwrap();
        let headers = env_value(&overlay, "ANTHROPIC_CUSTOM_HEADERS")
            .expect("configured headers must reach the child")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            headers,
            "X-Glasshouse-One: value-one\nX-Glasshouse-Two: value-two"
        );
    }

    /// The same headers reach Codex as one `-c
    /// model_providers.<id>.http_headers=…` inline-TOML-table override.
    #[test]
    fn codex_receives_configured_headers_as_an_http_headers_override() {
        let adapter = adapter_for(IntegrationId::Codex).expect("a harness");
        let mut provider = responses_provider();
        provider.headers = vec![("X-Glasshouse-One".to_owned(), "value-one".to_owned())];
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
        let profile = direct_profile(IntegrationId::Codex, &provider.name);

        let overlay = resolve(&profile, &direct_cx(adapter, &provider, &secrets)).unwrap();
        let args = rendered_args(&overlay);
        assert!(
            args.iter().any(|arg| arg
                == "model_providers.my-responses.http_headers={ \"X-Glasshouse-One\" = \"value-one\" }"),
            "the header override never reached the argument list: {args:?}"
        );
    }

    /// No headers configured, no header mechanism at all — on either
    /// harness. An always-present but empty header line would be a subtler
    /// version of the same invention this whole line refuses elsewhere.
    #[test]
    fn no_headers_configured_means_no_header_mechanism_on_either_harness() {
        let claude = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let provider = anthropic_provider();
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
        let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);
        let overlay = resolve(&profile, &direct_cx(claude, &provider, &secrets)).unwrap();
        assert!(env_value(&overlay, "ANTHROPIC_CUSTOM_HEADERS").is_none());

        let codex = adapter_for(IntegrationId::Codex).expect("a harness");
        let responses = responses_provider();
        let profile = direct_profile(IntegrationId::Codex, &responses.name);
        let overlay = resolve(&profile, &direct_cx(codex, &responses, &secrets)).unwrap();
        assert!(
            !rendered_args(&overlay)
                .iter()
                .any(|arg| arg.contains("http_headers")),
        );
    }

    /// A provider name is interpolated into a dotted TOML path, so it is
    /// refused — before any argument is composed — rather than sanitised.
    #[test]
    fn an_unsafe_provider_name_is_refused_before_any_argument_is_composed() {
        let adapter = adapter_for(IntegrationId::Codex).expect("a harness");
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);

        for (name, offending) in [
            ("bad.name", '.'),
            ("bad;name", ';'),
            ("bad\"name", '"'),
            ("bad$name", '$'),
            ("bad name", ' '),
        ] {
            let mut provider =
                provider_serving(name, WireProtocol::OpenAiResponses, "https://a.example/v1");
            provider.credential_env = vec![CREDENTIAL_VAR.to_owned()];
            let profile = direct_profile(IntegrationId::Codex, name);

            let err = resolve(&profile, &direct_cx(adapter, &provider, &secrets))
                .expect_err("an unsafe provider name must be refused");
            match &err {
                Refusal::UnsafeProviderName {
                    provider: refused,
                    offending: found,
                    ..
                } => {
                    assert_eq!(refused, name);
                    assert_eq!(*found, offending, "the offending character must be named");
                }
                other => panic!("expected UnsafeProviderName for `{name}`, got {other:?}"),
            }
            let message = err.to_string();
            assert!(
                message.contains(offending),
                "the message must name `{offending}`: {message}"
            );
        }
    }

    /// Line 11, and the reason this task is red-risk. A declared credential
    /// with no value is a refusal, **not** a launch that lets the harness
    /// reach for the user's own paid account.
    #[test]
    fn a_credential_that_cannot_be_resolved_is_refused_and_produces_no_overlay() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let provider = anthropic_provider();
        let secrets = FakeSecrets::empty();
        let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);

        let result = resolve(&profile, &direct_cx(adapter, &provider, &secrets));
        assert!(
            result.is_err(),
            "an unresolvable credential must produce no overlay at all"
        );
        let err = result.unwrap_err();
        match &err {
            Refusal::CredentialUnavailable {
                harness, variables, ..
            } => {
                assert_eq!(*harness, IntegrationId::ClaudeCode);
                assert_eq!(variables, &vec![CREDENTIAL_VAR.to_owned()]);
            }
            other => panic!("expected CredentialUnavailable, got {other:?}"),
        }
        let message = err.to_string();
        assert!(
            message.contains(CREDENTIAL_VAR),
            "the message must name the variable: {message}"
        );
    }

    /// Several declared variables are a pool: the first that resolves wins,
    /// and only an empty pool refuses.
    #[test]
    fn the_first_credential_variable_that_resolves_is_the_one_used() {
        let adapter = adapter_for(IntegrationId::Codex).expect("a harness");
        let mut provider = responses_provider();
        provider.credential_env = vec![
            "GLASSHOUSE_TEST_KEY_PRIMARY".to_owned(),
            "GLASSHOUSE_TEST_KEY_BACKUP".to_owned(),
        ];
        let profile = direct_profile(IntegrationId::Codex, &provider.name);

        // Only the second one has a value: it is used rather than refused.
        let secrets = FakeSecrets::holding("GLASSHOUSE_TEST_KEY_BACKUP", PLANTED_CREDENTIAL);
        let overlay = resolve(&profile, &direct_cx(adapter, &provider, &secrets)).unwrap();
        assert!(rendered_args(&overlay).iter().any(|arg| arg
            == "model_providers.my-responses.env_key=GLASSHOUSE_TEST_KEY_BACKUP"));
        assert!(env_value(&overlay, "GLASSHOUSE_TEST_KEY_BACKUP").is_some());
        assert!(env_value(&overlay, "GLASSHOUSE_TEST_KEY_PRIMARY").is_none());

        // Both set: the first declared wins, deterministically.
        let secrets = FakeSecrets(vec![
            (
                "GLASSHOUSE_TEST_KEY_PRIMARY".to_owned(),
                "primary".to_owned(),
            ),
            ("GLASSHOUSE_TEST_KEY_BACKUP".to_owned(), "backup".to_owned()),
        ]);
        let overlay = resolve(&profile, &direct_cx(adapter, &provider, &secrets)).unwrap();
        assert!(env_value(&overlay, "GLASSHOUSE_TEST_KEY_PRIMARY").is_some());
        assert!(env_value(&overlay, "GLASSHOUSE_TEST_KEY_BACKUP").is_none());
    }

    /// The two generic templates ship an empty base URL on purpose. Launching
    /// a harness against `""` must never happen.
    #[test]
    fn a_provider_with_no_base_url_for_the_chosen_protocol_is_refused() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let provider =
            provider_serving("anthropic-compatible", WireProtocol::AnthropicMessages, "");
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
        let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);

        let err = resolve(&profile, &direct_cx(adapter, &provider, &secrets))
            .expect_err("an empty base URL is refused");
        match &err {
            Refusal::ProviderBaseUrlMissing {
                provider, protocol, ..
            } => {
                assert_eq!(provider, "anthropic-compatible");
                assert_eq!(*protocol, WireProtocol::AnthropicMessages);
            }
            other => panic!("expected ProviderBaseUrlMissing, got {other:?}"),
        }

        // And the real shipped template is exactly that shape, so this is not
        // a hypothetical: `anthropic-compatible` cannot launch until the user
        // supplies a URL.
        let template = crate::provider::template("anthropic-compatible").unwrap();
        assert!(
            template
                .serves(WireProtocol::AnthropicMessages)
                .unwrap()
                .base_url
                .is_empty()
        );
    }

    /// A harness that declares no direct-provider mechanism is refused,
    /// naming the harness — never launched natively instead.
    #[test]
    fn a_harness_with_no_direct_provider_mechanism_is_refused() {
        // The other five adapters inherit the `None` default *and* declare
        // `protocols: Unverified`, so they are refused one step earlier —
        // at the protocol intersection, which is still a refusal naming the
        // harness and still starts nothing.
        let adapter = adapter_for(IntegrationId::OpenCode).expect("a harness");
        let provider = anthropic_provider();
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
        let profile = direct_profile(IntegrationId::OpenCode, &provider.name);

        let err = resolve(&profile, &direct_cx(adapter, &provider, &secrets))
            .expect_err("OpenCode declares no direct-provider mechanism");
        assert!(
            err.to_string().contains("OpenCode"),
            "the refusal must name the harness: {err}"
        );

        // And the `NoDirectProviderMechanism` rule itself, on a harness that
        // *does* declare a matching protocol but no mechanism — the state
        // every future adapter starts in.
        let double = NoDirectProviderMechanism;
        let mut profile = direct_profile(IntegrationId::Pi, &provider.name);
        profile.name = "gateway".to_owned();
        let err = resolve(&profile, &direct_cx(&double, &provider, &secrets))
            .expect_err("a declared protocol without a mechanism is still refused");
        match &err {
            Refusal::NoDirectProviderMechanism {
                harness, protocol, ..
            } => {
                assert_eq!(*harness, IntegrationId::Pi);
                assert_eq!(*protocol, WireProtocol::AnthropicMessages);
            }
            other => panic!("expected NoDirectProviderMechanism, got {other:?}"),
        }
        assert!(err.to_string().contains("Pi"), "{err}");
    }

    /// A harness that *can* be pointed at a backend but declares nowhere to
    /// put the credential — the one shape that would silently launch a
    /// gateway-backed session the gateway itself would then refuse.
    #[derive(Debug)]
    struct TokenUnplaceable;

    impl HarnessAdapter for TokenUnplaceable {
        fn id(&self) -> IntegrationId {
            IntegrationId::Pi
        }
        fn executable_candidates(&self) -> &'static [&'static str] {
            &["pretend"]
        }
        fn start(&self) -> crate::harness::Invocation {
            crate::harness::Invocation::bare()
        }
        fn resume(&self, _native_session: &str) -> Option<crate::harness::Invocation> {
            None
        }
        fn describe(&self) -> crate::harness::HarnessDescription {
            NoDirectProviderMechanism.describe()
        }
        fn direct_provider_launch(
            &self,
            _request: &DirectProviderRequest<'_>,
        ) -> Option<crate::harness::DirectProviderPlan> {
            Some(crate::harness::DirectProviderPlan {
                args: Vec::new(),
                env: Vec::new(),
                credential: None,
                mechanism: "a test double that forgets the credential".to_owned(),
            })
        }
    }

    /// A harness declaring a protocol it can serve, and no way at all to be
    /// pointed at a provider — the default every adapter inherits.
    #[derive(Debug)]
    struct NoDirectProviderMechanism;

    impl HarnessAdapter for NoDirectProviderMechanism {
        fn id(&self) -> IntegrationId {
            IntegrationId::Pi
        }
        fn executable_candidates(&self) -> &'static [&'static str] {
            &["pretend"]
        }
        fn start(&self) -> crate::harness::Invocation {
            crate::harness::Invocation::bare()
        }
        fn resume(&self, _native_session: &str) -> Option<crate::harness::Invocation> {
            None
        }
        fn describe(&self) -> crate::harness::HarnessDescription {
            crate::harness::HarnessDescription {
                vendor: crate::harness::Declared::Unverified,
                hooks: crate::harness::Declared::Unverified,
                session_ids: crate::harness::Declared::Unverified,
                capabilities: crate::harness::Capabilities::UNVERIFIED,
                backends: crate::harness::Backends {
                    protocols: Declared::verified(
                        &[WireProtocol::AnthropicMessages],
                        "a test double, declaring exactly one protocol",
                    ),
                    model_override: Declared::Unverified,
                    selection: Declared::Unverified,
                },
                approvals: crate::harness::ApprovalModes::UNVERIFIED,
                communication_style: crate::harness::Declared::Unverified,
            }
        }
    }

    /// An explicitly expected protocol is a constraint, never a hint: the
    /// provider must serve *that* one.
    #[test]
    fn an_expected_protocol_the_provider_does_not_serve_is_refused() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let provider = anthropic_provider();
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
        let mut profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);

        // Claude Code can serve it, so the harness-side check passes; the
        // provider cannot, so this is the provider-side refusal.
        profile.expected_protocol = Some(WireProtocol::AnthropicMessages);
        resolve(&profile, &direct_cx(adapter, &provider, &secrets))
            .expect("both sides serve anthropic-messages");

        let chat_only = {
            let mut p = provider_serving(
                "my-gateway",
                WireProtocol::OpenAiChat,
                "https://gateway.example/v1",
            );
            p.credential_env = vec![CREDENTIAL_VAR.to_owned()];
            p
        };
        let err = resolve(&profile, &direct_cx(adapter, &chat_only, &secrets))
            .expect_err("the provider does not serve the expected protocol");
        assert!(matches!(err, Refusal::ProviderProtocolUnsupported { .. }));
    }

    /// A credential variable name is interpolated into a `-c` value too, so
    /// it is checked the same way — and the check names the problem without
    /// naming any value.
    #[test]
    fn an_unusable_credential_variable_name_is_refused() {
        let adapter = adapter_for(IntegrationId::Codex).expect("a harness");
        let mut provider = responses_provider();
        provider.credential_env = vec!["BAD-VAR NAME".to_owned()];
        let secrets = FakeSecrets::holding("BAD-VAR NAME", PLANTED_CREDENTIAL);
        let profile = direct_profile(IntegrationId::Codex, &provider.name);

        let err = resolve(&profile, &direct_cx(adapter, &provider, &secrets))
            .expect_err("an unusable variable name is refused");
        match &err {
            Refusal::UnsafeCredentialVariable { variable, .. } => {
                assert_eq!(variable, "BAD-VAR NAME");
            }
            other => panic!("expected UnsafeCredentialVariable, got {other:?}"),
        }
        assert!(!err.to_string().contains(PLANTED_CREDENTIAL), "{err}");
    }

    /// **The credential never leaks.** Not from a successful overlay's
    /// `Debug`, not from its mechanism notes, and not from any refusal a real
    /// resolution can produce while a store holds a value.
    #[test]
    fn a_resolved_credential_never_reaches_a_rendering() {
        let claude = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let codex = adapter_for(IntegrationId::Codex).expect("a harness");
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);

        // 1. A successful resolution, rendered every way this type allows.
        for (adapter, provider) in [
            (claude, anthropic_provider()),
            (codex, responses_provider()),
        ] {
            let mut profile = direct_profile(adapter.id(), &provider.name);
            profile.model = Some("a-model".to_owned());
            let overlay = resolve(&profile, &direct_cx(adapter, &provider, &secrets)).unwrap();

            let debug = format!("{overlay:?}");
            assert!(
                !debug.contains(PLANTED_CREDENTIAL),
                "the credential reached LaunchOverlay's Debug"
            );
            for note in overlay.mechanisms() {
                assert!(
                    !note.detail.contains(PLANTED_CREDENTIAL),
                    "the credential reached a mechanism note"
                );
            }
            for arg in rendered_args(&overlay) {
                assert!(
                    !arg.contains(PLANTED_CREDENTIAL),
                    "the credential reached an argument"
                );
            }
            // It *is* in exactly one place, and that place is the child's
            // environment — proven by key, never by printing the value.
            assert!(
                overlay
                    .env()
                    .iter()
                    .any(|(_, value)| value == std::ffi::OsStr::new(PLANTED_CREDENTIAL)),
                "the credential must reach the child environment"
            );

            // And onward through `apply`, whose own `Debug` is redacted too.
            let debug = format!("{:?}", overlay.mechanisms());
            assert!(!debug.contains(PLANTED_CREDENTIAL));
        }

        // 2. Every refusal a resolution can produce, while the store holds a
        //    value, rendered both ways.
        let empty = FakeSecrets::empty();
        let mut unsafe_name = provider_serving(
            "bad.name",
            WireProtocol::AnthropicMessages,
            "https://a.example",
        );
        unsafe_name.credential_env = vec![CREDENTIAL_VAR.to_owned()];
        let mut no_url = provider_serving("no-url", WireProtocol::AnthropicMessages, "");
        no_url.credential_env = vec![CREDENTIAL_VAR.to_owned()];
        let mut bad_var = provider_serving(
            "bad-var",
            WireProtocol::AnthropicMessages,
            "https://a.example",
        );
        bad_var.credential_env = vec!["9NOPE".to_owned()];

        let scenarios: Vec<(&str, Refusal)> = vec![
            ("harness executable unavailable", {
                let p = direct_profile(IntegrationId::ClaudeCode, "my-gateway");
                resolve_checked(
                    &p,
                    &direct_cx(claude, &anthropic_provider(), &secrets),
                    None,
                    &crate::harness::ExecutablePresence::NotFound,
                )
                .unwrap_err()
            }),
            ("gateway not running", {
                let mut p = profile_for(IntegrationId::ClaudeCode);
                p.backend = BackendResource::GlasshouseGateway;
                resolve(&p, &native_cx(claude, false, &secrets)).unwrap_err()
            }),
            ("gateway protocol unserved", {
                let gateway = running_gateway();
                let mut p = profile_for(IntegrationId::Codex);
                p.backend = BackendResource::GlasshouseGateway;
                resolve_with_gateway(&p, &native_cx(codex, false, &secrets), Some(&gateway))
                    .unwrap_err()
            }),
            ("gateway token unplaceable", {
                let gateway = running_gateway();
                let double = TokenUnplaceable;
                let mut p = profile_for(IntegrationId::Pi);
                p.backend = BackendResource::GlasshouseGateway;
                resolve_with_gateway(&p, &native_cx(&double, false, &secrets), Some(&gateway))
                    .unwrap_err()
            }),
            ("unconfigured provider", {
                let p = direct_profile(IntegrationId::ClaudeCode, "nope");
                resolve(&p, &native_cx(claude, false, &secrets)).unwrap_err()
            }),
            ("unsafe provider name", {
                let p = direct_profile(IntegrationId::ClaudeCode, "bad.name");
                resolve(&p, &direct_cx(claude, &unsafe_name, &secrets)).unwrap_err()
            }),
            ("unsafe credential variable", {
                let p = direct_profile(IntegrationId::ClaudeCode, "bad-var");
                resolve(&p, &direct_cx(claude, &bad_var, &secrets)).unwrap_err()
            }),
            ("protocol unsupported", {
                let p = direct_profile(IntegrationId::Codex, "my-gateway");
                resolve(&p, &direct_cx(codex, &anthropic_provider(), &secrets)).unwrap_err()
            }),
            ("base url missing", {
                let p = direct_profile(IntegrationId::ClaudeCode, "no-url");
                resolve(&p, &direct_cx(claude, &no_url, &secrets)).unwrap_err()
            }),
            ("no mechanism", {
                let p = direct_profile(IntegrationId::Pi, "my-gateway");
                resolve(
                    &p,
                    &direct_cx(&NoDirectProviderMechanism, &anthropic_provider(), &secrets),
                )
                .unwrap_err()
            }),
            ("credential unavailable", {
                let p = direct_profile(IntegrationId::ClaudeCode, "my-gateway");
                resolve(&p, &direct_cx(claude, &anthropic_provider(), &empty)).unwrap_err()
            }),
            ("no automatic review", {
                let opencode = adapter_for(IntegrationId::OpenCode).expect("a harness");
                let mut p = profile_for(IntegrationId::OpenCode);
                p.approval = ApprovalSelection::AutomaticReview;
                resolve(&p, &native_cx(opencode, false, &secrets)).unwrap_err()
            }),
            ("bypass not acknowledged", {
                let mut p = profile_for(IntegrationId::ClaudeCode);
                p.approval = ApprovalSelection::Bypass;
                resolve(&p, &native_cx(claude, false, &secrets)).unwrap_err()
            }),
            ("no bypass", {
                let pi = adapter_for(IntegrationId::Pi).expect("a harness");
                let mut p = profile_for(IntegrationId::Pi);
                p.approval = ApprovalSelection::Bypass;
                resolve(&p, &native_cx(pi, true, &secrets)).unwrap_err()
            }),
            ("no model override", {
                // The double declares no model-override mechanism either.
                let mut p = profile_for(IntegrationId::Pi);
                p.model = Some("m".to_owned());
                resolve(&p, &native_cx(&NoDirectProviderMechanism, false, &secrets)).unwrap_err()
            }),
            ("protocol mismatch", {
                let mut p = profile_for(IntegrationId::ClaudeCode);
                p.expected_protocol = Some(WireProtocol::OpenAiResponses);
                resolve(&p, &native_cx(claude, false, &secrets)).unwrap_err()
            }),
            ("automatic review needs a native backend", {
                let mut p = direct_profile(IntegrationId::ClaudeCode, "my-gateway");
                p.approval = ApprovalSelection::AutomaticReview;
                resolve(&p, &direct_cx(claude, &anthropic_provider(), &secrets)).unwrap_err()
            }),
        ];

        for (label, refusal) in &scenarios {
            let display = refusal.to_string();
            let debug = format!("{refusal:?}");
            assert!(
                !display.contains(PLANTED_CREDENTIAL),
                "`{label}`'s Display carried the credential"
            );
            assert!(
                !debug.contains(PLANTED_CREDENTIAL),
                "`{label}`'s Debug carried the credential"
            );
        }

        // Exhaustive by construction: adding a `Refusal` variant without
        // covering it here stops compiling, rather than quietly leaving a
        // rendering nobody checked.
        let mut seen = std::collections::BTreeSet::new();
        for (_, refusal) in &scenarios {
            seen.insert(match refusal {
                Refusal::GatewayNotRunning { .. } => "GatewayNotRunning",
                Refusal::GatewayProtocolUnserved { .. } => "GatewayProtocolUnserved",
                Refusal::GatewayTokenUnplaceable { .. } => "GatewayTokenUnplaceable",
                Refusal::ProviderNotConfigured { .. } => "ProviderNotConfigured",
                Refusal::ProviderProtocolUnsupported { .. } => "ProviderProtocolUnsupported",
                Refusal::ProviderBaseUrlMissing { .. } => "ProviderBaseUrlMissing",
                Refusal::UnsafeProviderName { .. } => "UnsafeProviderName",
                Refusal::UnsafeCredentialVariable { .. } => "UnsafeCredentialVariable",
                Refusal::NoDirectProviderMechanism { .. } => "NoDirectProviderMechanism",
                Refusal::CredentialUnavailable { .. } => "CredentialUnavailable",
                Refusal::NoAutomaticReview { .. } => "NoAutomaticReview",
                Refusal::BypassNotAcknowledged { .. } => "BypassNotAcknowledged",
                Refusal::NoBypass { .. } => "NoBypass",
                Refusal::NoModelOverride { .. } => "NoModelOverride",
                Refusal::ProtocolMismatch { .. } => "ProtocolMismatch",
                Refusal::AutomaticReviewNeedsNativeBackend { .. } => {
                    "AutomaticReviewNeedsNativeBackend"
                }
                Refusal::HarnessExecutableUnavailable { .. } => "HarnessExecutableUnavailable",
            });
        }
        assert_eq!(
            seen.len(),
            17,
            "every Refusal variant must be exercised here: {seen:?}"
        );
    }

    /// Amendment 1. A gateway-backed Claude Code session must not come up
    /// carrying `--permission-mode auto`: the mode's classifier is a
    /// server-side model call the backend may not serve, and auto mode fails
    /// closed with tools blocked. The contrast is the behaviour — the same
    /// profile on `Native` still selects it.
    #[test]
    fn a_defaulted_profile_selects_automatic_review_only_on_a_native_backend() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let provider = anthropic_provider();
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);

        let direct = direct_profile(IntegrationId::ClaudeCode, &provider.name);
        assert_eq!(direct.approval, ApprovalSelection::Default);
        let overlay = resolve(&direct, &direct_cx(adapter, &provider, &secrets)).unwrap();
        let args = rendered_args(&overlay);
        assert!(
            !args.iter().any(|arg| arg == "--permission-mode"),
            "a gateway-backed session must carry no --permission-mode: {args:?}"
        );
        assert!(
            !args.iter().any(|arg| arg == "auto"),
            "nor its value: {args:?}"
        );
        assert!(
            overlay
                .mechanisms()
                .iter()
                .any(|note| note.detail.contains("automatic review withheld")),
            "the decision must be recorded: {:?}",
            overlay.mechanisms()
        );

        // The other half: `Native` behaviour does not change by one byte.
        let native = profile_for(IntegrationId::ClaudeCode);
        assert_eq!(native.approval, ApprovalSelection::Default);
        let overlay = resolve(&native, &native_cx(adapter, false, &secrets)).unwrap();
        let args = rendered_args(&overlay);
        assert_eq!(
            args,
            vec!["--permission-mode".to_owned(), "auto".to_owned()],
            "a native-backed session must still select automatic review"
        );
    }

    /// Amendment 1, the explicit half: a default that falls back is not a
    /// request that is refused.
    #[test]
    fn an_explicit_automatic_review_request_on_a_gateway_backed_profile_is_refused() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let provider = anthropic_provider();
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
        let mut profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);
        profile.approval = ApprovalSelection::AutomaticReview;

        let err = resolve(&profile, &direct_cx(adapter, &provider, &secrets))
            .expect_err("automatic review is refused on a gateway-backed profile");
        let backend = match &err {
            Refusal::AutomaticReviewNeedsNativeBackend {
                profile: name,
                harness,
                backend,
            } => {
                assert_eq!(name, "gateway");
                assert_eq!(*harness, IntegrationId::ClaudeCode);
                *backend
            }
            other => panic!("expected AutomaticReviewNeedsNativeBackend, got {other:?}"),
        };
        let message = err.to_string();
        assert!(message.contains("Claude Code"), "{message}");
        assert!(
            message.contains(backend),
            "the message must name the backend: {message}"
        );
        assert_eq!(backend, "a direct provider");

        // A bypass is unchanged: still refused until acknowledged, still
        // resolved once it is.
        let mut bypass = direct_profile(IntegrationId::ClaudeCode, &provider.name);
        bypass.approval = ApprovalSelection::Bypass;
        let err = resolve(&bypass, &direct_cx(adapter, &provider, &secrets))
            .expect_err("an unacknowledged bypass is still refused");
        assert!(matches!(err, Refusal::BypassNotAcknowledged { .. }));

        let acknowledged = Resolution {
            adapter,
            acknowledged_bypass: true,
            provider: Some(&provider),
            secrets: &secrets,
        };
        let overlay = resolve(&bypass, &acknowledged).expect("an acknowledged bypass resolves");
        assert!(
            rendered_args(&overlay)
                .iter()
                .any(|arg| arg == "--dangerously-skip-permissions")
        );
    }

    // --- resolution rule coverage not named above --------------------------

    #[test]
    fn a_harness_without_a_model_override_refuses_a_model_request() {
        // Antigravity declares only a command-line model override; pick a
        // harness that declares none at all — none of the seven do today, so
        // this uses a double to exercise the rule.
        #[derive(Debug)]
        struct NoModelOverride;
        impl HarnessAdapter for NoModelOverride {
            fn id(&self) -> IntegrationId {
                IntegrationId::Pi
            }
            fn executable_candidates(&self) -> &'static [&'static str] {
                &["pretend"]
            }
            fn start(&self) -> crate::harness::Invocation {
                crate::harness::Invocation::bare()
            }
            fn resume(&self, _native_session: &str) -> Option<crate::harness::Invocation> {
                None
            }
            fn describe(&self) -> crate::harness::HarnessDescription {
                crate::harness::HarnessDescription {
                    vendor: crate::harness::Declared::Unverified,
                    hooks: crate::harness::Declared::Unverified,
                    session_ids: crate::harness::Declared::Unverified,
                    capabilities: crate::harness::Capabilities::UNVERIFIED,
                    backends: crate::harness::Backends::UNVERIFIED,
                    approvals: crate::harness::ApprovalModes::UNVERIFIED,
                    communication_style: crate::harness::Declared::Unverified,
                }
            }
        }

        let adapter = NoModelOverride;
        let mut profile = profile_for(IntegrationId::Pi);
        profile.model = Some("some-model".to_owned());

        let err = resolve(&profile, &native_cx(&adapter, false, &FakeSecrets::empty()))
            .expect_err("no model override declared");
        assert!(matches!(err, Refusal::NoModelOverride { .. }));
    }

    #[test]
    fn a_bypass_selection_on_a_harness_with_no_bypass_mode_is_refused() {
        // Pi's whole `ApprovalModes` is unverified, so it declares neither
        // automatic review nor a bypass. Asking for bypass must not panic or
        // silently produce an empty overlay — it must refuse.
        let adapter = adapter_for(IntegrationId::Pi).expect("a harness");
        let mut profile = profile_for(IntegrationId::Pi);
        profile.approval = ApprovalSelection::Bypass;

        let err = resolve(&profile, &native_cx(adapter, true, &FakeSecrets::empty()))
            .expect_err("Pi declares no bypass mode");
        assert!(matches!(err, Refusal::NoBypass { .. }));
    }

    #[test]
    fn a_protocol_a_harness_cannot_serve_is_refused() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let mut profile = profile_for(IntegrationId::ClaudeCode);
        profile.expected_protocol = Some(WireProtocol::OpenAiResponses);

        let err = resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty()))
            .expect_err("Claude Code only speaks Anthropic Messages");
        match &err {
            Refusal::ProtocolMismatch {
                harness, protocol, ..
            } => {
                assert_eq!(*harness, IntegrationId::ClaudeCode);
                assert_eq!(*protocol, WireProtocol::OpenAiResponses);
            }
            other => panic!("expected ProtocolMismatch, got {other:?}"),
        }
    }

    #[test]
    fn a_protocol_a_harness_can_serve_is_accepted() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let mut profile = profile_for(IntegrationId::ClaudeCode);
        profile.expected_protocol = Some(WireProtocol::AnthropicMessages);

        resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty()))
            .expect("Claude Code speaks Anthropic Messages natively");
    }

    #[test]
    fn an_unverified_protocol_declaration_cannot_serve_anything() {
        // Cursor's protocols are `Unverified`. "Nobody checked" must not be
        // treated as "yes" for a protocol match.
        let adapter = adapter_for(IntegrationId::Cursor).expect("a harness");
        let mut profile = profile_for(IntegrationId::Cursor);
        profile.expected_protocol = Some(WireProtocol::AnthropicMessages);

        let err = resolve(&profile, &native_cx(adapter, false, &FakeSecrets::empty()))
            .expect_err("unverified protocols cannot serve anything");
        assert!(matches!(err, Refusal::ProtocolMismatch { .. }));
    }

    #[test]
    fn backend_slugs_never_carry_a_credential_shaped_field_name() {
        assert_eq!(BackendResource::Native.slug(), "native");
        assert_eq!(
            BackendResource::DirectProvider {
                provider: "openrouter".to_owned()
            }
            .slug(),
            "direct-provider:openrouter"
        );
        assert_eq!(
            BackendResource::GlasshouseGateway.slug(),
            "glasshouse-gateway"
        );
    }

    #[test]
    fn profile_class_matches_the_backend_kind() {
        let mut profile = LaunchProfile::native(IntegrationId::ClaudeCode);
        assert_eq!(profile.class(), ProfileClass::NativeSubscription);
        profile.backend = BackendResource::DirectProvider {
            provider: "openrouter".to_owned(),
        };
        assert_eq!(profile.class(), ProfileClass::DirectProvider);
        profile.backend = BackendResource::GlasshouseGateway;
        assert_eq!(profile.class(), ProfileClass::GlasshouseGateway);
    }

    // --- Phase 9F line 466: the executable precondition -------------------

    use crate::harness::ExecutablePresence;

    /// Acceptance test 1: a direct-provider profile naming a harness whose
    /// executable is not installed is refused, names the harness and the
    /// candidates tried, and starts nothing (there is no overlay to apply).
    #[test]
    fn a_direct_provider_profile_is_refused_when_the_executable_is_not_found() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let provider = anthropic_provider();
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
        let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);

        let err = resolve_checked(
            &profile,
            &direct_cx(adapter, &provider, &secrets),
            None,
            &ExecutablePresence::NotFound,
        )
        .expect_err("an absent executable must refuse a direct-provider profile");

        match &err {
            Refusal::HarnessExecutableUnavailable {
                harness, detail, ..
            } => {
                assert_eq!(*harness, IntegrationId::ClaudeCode);
                assert!(detail.contains("candidates tried"), "{detail}");
                for candidate in IntegrationId::ClaudeCode.executable_candidates() {
                    assert!(detail.contains(candidate), "{detail}");
                }
            }
            other => panic!("expected HarnessExecutableUnavailable, got {other:?}"),
        }
        let message = err.to_string();
        assert!(message.contains("Claude Code"), "{message}");
        assert!(message.contains("candidates tried"), "{message}");
    }

    /// Acceptance test 2: the same profile, with the executable present, is
    /// not refused for that reason — and resolves exactly as plain
    /// `resolve` would.
    #[test]
    fn the_same_profile_is_not_refused_when_the_executable_is_usable() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let provider = anthropic_provider();
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
        let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);
        let cx = direct_cx(adapter, &provider, &secrets);

        let checked = resolve_checked(&profile, &cx, None, &ExecutablePresence::Usable)
            .expect("a usable executable must not be refused");
        let unchecked =
            resolve(&profile, &cx).expect("the same profile resolves without the check too");
        assert_eq!(
            env_value(&checked, "ANTHROPIC_BASE_URL"),
            env_value(&unchecked, "ANTHROPIC_BASE_URL")
        );
        assert_eq!(rendered_args(&checked), rendered_args(&unchecked));
    }

    /// A found-but-unusable executable (a Windows-interop-only `PATH` hit,
    /// for example) is refused too, and the refusal carries the resolver's
    /// own reason rather than "candidates tried".
    #[test]
    fn an_unusable_executable_is_refused_with_its_own_reason() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let provider = anthropic_provider();
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
        let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);

        let err = resolve_checked(
            &profile,
            &direct_cx(adapter, &provider, &secrets),
            None,
            &ExecutablePresence::Unusable {
                reason: "found only in the Windows side of PATH".to_owned(),
            },
        )
        .expect_err("an unusable executable must be refused");
        match &err {
            Refusal::HarnessExecutableUnavailable { detail, .. } => {
                assert_eq!(detail, "found only in the Windows side of PATH");
            }
            other => panic!("expected HarnessExecutableUnavailable, got {other:?}"),
        }
    }

    /// Acceptance test 3: a `Native` profile is unaffected by line 466's
    /// check, byte for byte — an absent executable changes nothing about
    /// it, because the check is never even consulted for one.
    #[test]
    fn a_native_profile_is_unaffected_by_the_executable_check() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let secrets = FakeSecrets::empty();
        let profile = profile_for(IntegrationId::ClaudeCode);
        let cx = native_cx(adapter, false, &secrets);

        let via_checked = resolve_checked(&profile, &cx, None, &ExecutablePresence::NotFound)
            .expect("a Native profile must resolve even when the check would refuse");
        let via_plain = resolve(&profile, &cx).expect("plain resolve must agree");

        assert_eq!(rendered_args(&via_checked), rendered_args(&via_plain));
        assert!(via_checked.env().is_empty() && via_plain.env().is_empty());
        assert_eq!(via_checked.mechanisms().len(), via_plain.mechanisms().len());
    }

    /// Acceptance test 6 (line 466's half): the check never reroutes to a
    /// different backend — a refusal is the only effect it can have. Proven
    /// by construction: `resolve_checked` either returns exactly what
    /// `resolve_with_gateway` would, or refuses; there is no third path that
    /// substitutes a different backend.
    #[test]
    fn the_executable_check_never_changes_which_backend_would_be_selected() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let provider = anthropic_provider();
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
        let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);
        let cx = direct_cx(adapter, &provider, &secrets);

        for presence in [
            ExecutablePresence::Usable,
            ExecutablePresence::NotFound,
            ExecutablePresence::Unusable {
                reason: "x".to_owned(),
            },
        ] {
            let is_usable = presence.is_usable();
            match resolve_checked(&profile, &cx, None, &presence) {
                Ok(overlay) => {
                    assert!(
                        is_usable,
                        "an unusable presence must never produce an overlay"
                    );
                    // Identical to what plain resolution against the same
                    // provider produces — no different backend was chosen.
                    let plain = resolve(&profile, &cx).unwrap();
                    assert_eq!(rendered_args(&overlay), rendered_args(&plain));
                }
                Err(Refusal::HarnessExecutableUnavailable { .. }) => {
                    assert!(!is_usable);
                }
                Err(other) => panic!("only the executable refusal may appear here: {other}"),
            }
        }
    }

    /// Acceptance test 7 (line 466's half): no credential leaks through the
    /// new refusal's `Display` or `Debug`.
    #[test]
    fn the_executable_refusal_never_carries_a_credential() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let provider = anthropic_provider();
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
        let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);

        let err = resolve_checked(
            &profile,
            &direct_cx(adapter, &provider, &secrets),
            None,
            &ExecutablePresence::NotFound,
        )
        .unwrap_err();
        assert!(!err.to_string().contains(PLANTED_CREDENTIAL));
        assert!(!format!("{err:?}").contains(PLANTED_CREDENTIAL));
    }

    /// A gateway-backed profile is covered by line 466 too, not only a
    /// direct-provider one.
    #[test]
    fn a_gateway_backed_profile_is_also_refused_when_the_executable_is_not_found() {
        let claude = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let secrets = FakeSecrets::empty();
        let gateway = running_gateway();
        let mut profile = profile_for(IntegrationId::ClaudeCode);
        profile.backend = BackendResource::GlasshouseGateway;

        let err = resolve_checked(
            &profile,
            &native_cx(claude, false, &secrets),
            Some(&gateway),
            &ExecutablePresence::NotFound,
        )
        .expect_err("a gateway-backed profile must be refused too");
        assert!(matches!(err, Refusal::HarnessExecutableUnavailable { .. }));
    }

    // --- Phase 9F line 465: the capability check ---------------------------

    /// Acceptance test 5: a provider for which no cheap check is available —
    /// here, a gateway-backed profile, which has no fixed upstream
    /// combination — reports that no check was made, and nothing about
    /// resolving the profile itself changes because of it.
    #[test]
    fn a_gateway_backed_profile_has_no_capability_check_available() {
        let claude = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let secrets = FakeSecrets::empty();
        let mut profile = profile_for(IntegrationId::ClaudeCode);
        profile.backend = BackendResource::GlasshouseGateway;
        let cx = native_cx(claude, false, &secrets);

        match capability_probe(&profile, &cx) {
            CapabilityProbe::Unavailable { reason } => assert!(!reason.is_empty()),
            CapabilityProbe::Available(_) => panic!("a gateway-backed profile has no check yet"),
        }

        // The launch itself proceeds regardless — a gateway-backed profile
        // resolves (once a gateway is running) whether or not a capability
        // check was ever considered.
        let gateway = running_gateway();
        resolve_with_gateway(&profile, &cx, Some(&gateway))
            .expect("the absent capability check must not block the launch");
    }

    /// A `Native` profile has no capability check available either — there
    /// is no protocol, base URL or credential this crate holds for it.
    #[test]
    fn a_native_profile_has_no_capability_check_available() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let secrets = FakeSecrets::empty();
        let profile = profile_for(IntegrationId::ClaudeCode);
        let cx = native_cx(adapter, false, &secrets);

        match capability_probe(&profile, &cx) {
            CapabilityProbe::Unavailable { reason } => assert!(!reason.is_empty()),
            CapabilityProbe::Available(_) => panic!("a native profile has no check available"),
        }
    }

    /// A resolvable direct-provider profile always has a check available,
    /// even when the provider has no established model-list endpoint: the
    /// base URL itself is still a valid target.
    #[test]
    fn a_resolvable_direct_provider_profile_always_has_a_check_available() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let provider = anthropic_provider();
        assert!(
            !provider.model_list_endpoint.is_known_present(),
            "this test wants the base-URL-only path"
        );
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
        let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);
        let cx = direct_cx(adapter, &provider, &secrets);

        let request = match capability_probe(&profile, &cx) {
            CapabilityProbe::Available(request) => request,
            CapabilityProbe::Unavailable { reason } => {
                panic!("a resolvable provider must have a check available: {reason}")
            }
        };
        assert_eq!(request.provider(), provider.name);
        assert_eq!(request.protocol(), WireProtocol::AnthropicMessages);
        assert_eq!(request.url(), "https://gateway.example/anthropic");
    }

    /// A direct-provider profile this crate cannot resolve (here: an
    /// unconfigured provider) has no capability check available either —
    /// the same "unavailable, not a failure" answer, for a different reason.
    #[test]
    fn an_unresolvable_direct_provider_profile_has_no_check_available() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let secrets = FakeSecrets::empty();
        let profile = direct_profile(IntegrationId::ClaudeCode, "not-configured");
        // `cx.provider` is `None`: the caller could not find "not-configured"
        // in configuration, exactly as `resolve` would see it too.
        let cx = native_cx(adapter, false, &secrets);

        match capability_probe(&profile, &cx) {
            CapabilityProbe::Unavailable { reason } => assert!(!reason.is_empty()),
            CapabilityProbe::Available(_) => {
                panic!("an unconfigured provider has nothing to probe")
            }
        }
    }

    /// Acceptance test 4 (formatting half): a `401` renders as
    /// reachable-but-rejected, distinctly from a host that never answered —
    /// the two must never read the same.
    #[test]
    fn describe_probe_outcome_distinguishes_rejected_from_unreachable() {
        use crate::provider::discovery::ProbeOutcome;

        let rejected = describe_probe_outcome(&ProbeOutcome::Rejected { status: 401 });
        let unreachable = describe_probe_outcome(&ProbeOutcome::Unreachable {
            reason: "the connection was refused".to_owned(),
        });
        assert_ne!(rejected, unreachable);
        assert!(rejected.contains("401"));
        assert!(rejected.contains("reachable"), "{rejected}");
        assert!(unreachable.contains("never answered"), "{unreachable}");

        let reached = describe_probe_outcome(&ProbeOutcome::Reached { status: 200 });
        assert_ne!(reached, rejected);
    }

    /// End to end, over a real loopback socket: [`capability_probe`] builds
    /// the request, and [`crate::provider::discovery::connectivity`] — the
    /// same function a real caller would run off-thread — actually sends it.
    /// Three real servers, three real distinctions: reached, reachable but
    /// rejected, and never answered at all.
    #[test]
    fn a_capability_probe_composes_with_a_real_connectivity_check() {
        use crate::provider::discovery::{ProbeOutcome, ProbeTimeouts, connectivity};
        use crate::provider::fixture::FixtureProvider;

        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
        let quick = ProbeTimeouts {
            connect: std::time::Duration::from_millis(500),
            response: std::time::Duration::from_millis(400),
            total: std::time::Duration::from_millis(900),
        };

        // A provider that answers.
        let ok_fixture = FixtureProvider::answering("HTTP/1.1 200 OK", "", "{}");
        let ok_provider = provider_serving(
            "answers-ok",
            WireProtocol::AnthropicMessages,
            &ok_fixture.base_url(),
        );
        let ok_profile = direct_profile(IntegrationId::ClaudeCode, &ok_provider.name);
        let request =
            match capability_probe(&ok_profile, &direct_cx(adapter, &ok_provider, &secrets)) {
                CapabilityProbe::Available(request) => request,
                CapabilityProbe::Unavailable { reason } => panic!("expected a request: {reason}"),
            };
        let outcome = connectivity(&request, quick);
        assert_eq!(outcome, ProbeOutcome::Reached { status: 200 });
        assert!(describe_probe_outcome(&outcome).contains("reached"));

        // A provider that answers, but rejects the credential.
        let rejecting_fixture = FixtureProvider::answering("HTTP/1.1 401 Unauthorized", "", "{}");
        let rejecting_provider = provider_serving(
            "answers-401",
            WireProtocol::AnthropicMessages,
            &rejecting_fixture.base_url(),
        );
        let rejecting_profile = direct_profile(IntegrationId::ClaudeCode, &rejecting_provider.name);
        let request = match capability_probe(
            &rejecting_profile,
            &direct_cx(adapter, &rejecting_provider, &secrets),
        ) {
            CapabilityProbe::Available(request) => request,
            CapabilityProbe::Unavailable { reason } => panic!("expected a request: {reason}"),
        };
        let outcome = connectivity(&request, quick);
        assert_eq!(outcome, ProbeOutcome::Rejected { status: 401 });
        let described = describe_probe_outcome(&outcome);
        assert!(described.contains("rejected"), "{described}");

        // A provider that is not there at all — nothing listening.
        let port = {
            let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .expect("loopback is bindable");
            listener
                .local_addr()
                .expect("a bound listener has an address")
                .port()
        };
        let absent_provider = provider_serving(
            "unreachable",
            WireProtocol::AnthropicMessages,
            &format!("http://127.0.0.1:{port}"),
        );
        let absent_profile = direct_profile(IntegrationId::ClaudeCode, &absent_provider.name);
        let request = match capability_probe(
            &absent_profile,
            &direct_cx(adapter, &absent_provider, &secrets),
        ) {
            CapabilityProbe::Available(request) => request,
            CapabilityProbe::Unavailable { reason } => panic!("expected a request: {reason}"),
        };
        let outcome = connectivity(&request, quick);
        assert!(!outcome.answered(), "nothing was listening: {outcome:?}");

        // **Which** not-answered outcome a closed port produces is the
        // platform's choice, not Glasshouse's, and asserting one of them cost
        // this repository a red Windows run. A Unix stack answers a connection
        // to a closed loopback port with an immediate refusal, so the probe
        // reports `Unreachable`; Windows drops the SYN instead, so the probe
        // waits out its own bound and reports `TimedOut`. Both are honest and
        // both are correct — the product's distinction between them is worth
        // keeping, so the test asserts the property it actually cares about
        // rather than the platform's spelling of it.
        assert!(
            matches!(
                outcome,
                ProbeOutcome::TimedOut { .. } | ProbeOutcome::Unreachable { .. }
            ),
            "a closed port must be timed-out or unreachable, never a response: {outcome:?}"
        );
        let described = describe_probe_outcome(&outcome);

        // Reached, rejected and not-answered never collapse into the same
        // sentence, whichever not-answered outcome this platform produced.
        let reached_desc = describe_probe_outcome(&ProbeOutcome::Reached { status: 200 });
        let rejected_desc = describe_probe_outcome(&ProbeOutcome::Rejected { status: 401 });
        assert_ne!(reached_desc, described);
        assert_ne!(rejected_desc, described);

        // And both spellings are checked on **every** platform, not just the
        // one whose stack happens to produce them: the assertion above can
        // only ever see one of the two, which is precisely how the Windows
        // spelling reached CI unexamined. Practice §18 applied to a runtime
        // difference rather than a `cfg`.
        for not_answered in [
            ProbeOutcome::TimedOut { waited_ms: 509 },
            ProbeOutcome::Unreachable {
                reason: "connection refused".to_owned(),
            },
        ] {
            let desc = describe_probe_outcome(&not_answered);
            assert!(!not_answered.answered(), "{not_answered:?}");
            assert_ne!(reached_desc, desc);
            assert_ne!(rejected_desc, desc);
        }
    }

    /// Acceptance test 6 (line 465's half): nothing about a capability
    /// probe's *outcome* can reach `resolve` at all — `capability_probe`
    /// and `describe_probe_outcome` are read-only functions of a
    /// [`ProbeRequest`]/[`ProbeOutcome`][crate::provider::discovery::ProbeOutcome]
    /// that `resolve` never takes as input, so a failed check has no
    /// mechanism by which it could reroute a launch to a different backend.
    #[test]
    fn a_capability_probe_cannot_influence_which_backend_resolve_selects() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let provider = anthropic_provider();
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
        let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);
        let cx = direct_cx(adapter, &provider, &secrets);

        let before = resolve(&profile, &cx).unwrap();
        let _ = capability_probe(&profile, &cx);
        let after = resolve(&profile, &cx).unwrap();
        assert_eq!(rendered_args(&before), rendered_args(&after));
        assert_eq!(
            env_value(&before, "ANTHROPIC_BASE_URL"),
            env_value(&after, "ANTHROPIC_BASE_URL")
        );
    }

    /// Acceptance test 7 (line 465's half): the credential a capability
    /// probe resolves reaches only the `ProbeRequest`'s private field —
    /// never this module's own rendering of it.
    #[test]
    fn a_capability_probes_credential_never_reaches_this_modules_own_renderings() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let provider = anthropic_provider();
        let secrets = FakeSecrets::holding(CREDENTIAL_VAR, PLANTED_CREDENTIAL);
        let profile = direct_profile(IntegrationId::ClaudeCode, &provider.name);
        let cx = direct_cx(adapter, &provider, &secrets);

        let request = match capability_probe(&profile, &cx) {
            CapabilityProbe::Available(request) => request,
            CapabilityProbe::Unavailable { reason } => panic!("expected a request: {reason}"),
        };
        let debug = format!("{request:?}");
        assert!(!debug.contains(PLANTED_CREDENTIAL), "{debug}");
        assert!(debug.contains(crate::secret::REDACTED), "{debug}");
        assert!(!request.url().contains(PLANTED_CREDENTIAL));
    }
}
