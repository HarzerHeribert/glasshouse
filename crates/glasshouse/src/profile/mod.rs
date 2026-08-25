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

use crate::gateway::{Gateway, Upstream};
use crate::harness::{
    ApprovalKind, ApprovalMode, CredentialPlacement, CredentialVarProblem, DirectProviderRequest,
    HarnessAdapter, WireProtocol,
};
use crate::integrations::IntegrationId;
use crate::launch::HarnessLaunch;
use crate::provider::Provider;
use crate::secret::{SecretRef, SecretStore};

/// The protocols the local gateway's ingress actually serves.
///
/// One entry, and the list is here rather than in [`mod@crate::gateway`]
/// because that module is structurally forbidden from naming
/// [`crate::harness`] — see its own header — and a protocol enum lives
/// there. Phase 9G's map names an OpenAI Responses ingress and an OpenAI
/// Chat ingress as separate lines; when either is built, it is added here
/// and a Codex profile starts resolving. Until then a harness that cannot
/// speak Anthropic Messages is refused rather than pointed at an ingress
/// that would not understand it.
pub const GATEWAY_INGRESS_PROTOCOLS: &[WireProtocol] = &[WireProtocol::AnthropicMessages];

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
        "launch profile `{profile}` is backed by the local Glasshouse gateway, whose ingress \
         speaks {protocol}, but {} cannot be pointed at that protocol",
        .harness.display_name(),
    )]
    GatewayProtocolUnserved {
        profile: String,
        harness: IntegrationId,
        protocol: WireProtocol,
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

    // An explicit ask is a constraint, never a hint — the same rule
    // `choose_protocol` applies to a direct provider. A profile expecting a
    // protocol the ingress does not serve is refused rather than quietly
    // given the one that exists.
    let protocol = match profile.expected_protocol {
        Some(expected) => GATEWAY_INGRESS_PROTOCOLS
            .contains(&expected)
            .then_some(expected),
        None => harness_protocols
            .iter()
            .copied()
            .find(|protocol| GATEWAY_INGRESS_PROTOCOLS.contains(protocol)),
    };
    let Some(protocol) = protocol.filter(|protocol| harness_protocols.contains(protocol)) else {
        return Err(Refusal::GatewayProtocolUnserved {
            profile: profile.name.clone(),
            harness: profile.harness,
            // What the ingress serves, for a message that names the mismatch
            // from the side the user cannot change.
            protocol: GATEWAY_INGRESS_PROTOCOLS[0],
        });
    };

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

    Ok(())
}

/// Which configured provider the local gateway forwards to, with its
/// credential resolved.
///
/// # Why this refuses to choose
///
/// [`BackendResource::GlasshouseGateway`] names no provider — it cannot,
/// because which backend a gateway-backed session runs against is Phase 9H's
/// *sticky routing* decision and belongs to the session, not to the profile.
/// This phase therefore does the only thing that invents nothing: it takes
/// the single configured provider that serves the ingress protocol, and
/// refuses when there is no such provider or more than one. A gateway that
/// picked the alphabetically first of three routers would be making exactly
/// the routing decision the map defers.
///
/// The credential is resolved here, once, at start, and moved into the
/// [`Upstream`]. That is the second place in Glasshouse where a
/// [`crate::secret::Secret`] exists — the first being [`resolve`]'s
/// direct-provider path — and unlike that one it does not end at a child
/// process: it stays in this process for the gateway's lifetime, which is
/// the entire point of holding it here instead.
pub fn gateway_upstream(
    providers: &[Provider],
    secrets: &dyn SecretStore,
) -> Result<Upstream, GatewayUpstreamRefusal> {
    let protocol = GATEWAY_INGRESS_PROTOCOLS[0];
    let candidates: Vec<&Provider> = providers
        .iter()
        .filter(|provider| {
            provider
                .serves(protocol)
                .is_some_and(|support| !support.base_url.is_empty())
        })
        .collect();

    let provider = match candidates.as_slice() {
        [] => {
            return Err(GatewayUpstreamRefusal::NoProviderServesTheIngress { protocol });
        }
        [only] => *only,
        several => {
            return Err(GatewayUpstreamRefusal::SeveralProvidersServeTheIngress {
                protocol,
                candidates: several.iter().map(|p| p.name.clone()).collect(),
            });
        }
    };

    let support = provider
        .serves(protocol)
        .expect("the provider was selected because it serves this protocol");

    // A provider declaring several credential variables is a pool, and
    // choosing between them on cost or quota is a routing decision this
    // phase does not make: take the first that currently resolves, exactly
    // as `apply_direct_provider` does.
    let credential = provider
        .credential_env
        .iter()
        .find_map(|var| secrets.resolve(&SecretRef::Environment { var: var.clone() }))
        .ok_or_else(|| GatewayUpstreamRefusal::CredentialUnavailable {
            provider: provider.name.clone(),
            variables: provider.credential_env.clone(),
        })?;

    Ok(Upstream::new(
        provider.name.clone(),
        &support.base_url,
        credential,
    )?)
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
        "the local Glasshouse gateway forwards {protocol} requests, but no configured provider \
         serves {protocol} with a base URL; configure one before launching a gateway-backed \
         profile"
    )]
    NoProviderServesTheIngress { protocol: WireProtocol },

    #[error(
        "the local Glasshouse gateway would have to choose between {} to serve {protocol}, and \
         choosing a backend per session is sticky routing rather than something a launch \
         profile decides; configure exactly one provider for {protocol}",
        .candidates.join(", "),
    )]
    SeveralProvidersServeTheIngress {
        protocol: WireProtocol,
        /// Provider *names*, so the message can say which entries collided.
        candidates: Vec<String>,
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
    let support = provider
        .serves(protocol)
        .expect("the protocol was chosen because the provider serves it");

    // The two generic templates ship an empty base URL on purpose: the URL
    // is the user's to supply. Launching a harness against `""` must never
    // happen, so this is a refusal and not a default.
    if support.base_url.is_empty() {
        return Err(Refusal::ProviderBaseUrlMissing {
            profile: profile.name.clone(),
            provider: provider.name.clone(),
            protocol,
        });
    }

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
        base_url: &support.base_url,
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
/// declares that the provider also serves** — deterministic by the harness's
/// own declared order, which is the harness's own preference and not an
/// ordering invented here. With one, it is that protocol and no other: an
/// explicit request is a constraint, never a hint, and a provider that does
/// not serve it is refused rather than quietly given a neighbouring one.
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

    let chosen = match profile.expected_protocol {
        Some(expected) => provider.serves(expected).map(|_| expected),
        None => harness_protocols
            .iter()
            .copied()
            .find(|protocol| provider.serves(*protocol).is_some()),
    };

    chosen.ok_or_else(|| Refusal::ProviderProtocolUnsupported {
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
            let SecretRef::Environment { var } = reference;
            self.0
                .iter()
                .find(|(name, _)| name == var)
                .map(|(_, value)| crate::secret::Secret::mint_for_test(value))
        }

        fn is_present(&self, reference: &SecretRef) -> bool {
            let SecretRef::Environment { var } = reference;
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

    /// A running gateway, for the tests below. Its upstream never has to
    /// answer: resolution reads the gateway's address and token and opens no
    /// connection at all.
    fn running_gateway() -> crate::gateway::Gateway {
        let profiles = [{
            let mut profile = profile_for(IntegrationId::ClaudeCode);
            profile.backend = BackendResource::GlasshouseGateway;
            profile
        }];
        crate::gateway::start_if_required(&profiles, || {
            Ok(Upstream::new(
                "fixture".to_owned(),
                "https://provider.example/api",
                crate::secret::Secret::mint_for_test(PLANTED_CREDENTIAL),
            )?)
        })
        .expect("loopback is bindable")
        .expect("a gateway-backed profile asks for a gateway")
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

    /// The ingress speaks one protocol, and a harness that cannot be pointed
    /// at it is refused rather than pointed at it anyway. Codex declares
    /// `openai-responses`; there is no Responses ingress in this phase, and
    /// the map lists it as a separate line.
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

        // Nothing serving the ingress at all.
        let chat_only = provider_serving("chat", WireProtocol::OpenAiChat, "https://a.example/v1");
        assert!(matches!(
            gateway_upstream(std::slice::from_ref(&chat_only), &secrets),
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

        // Several, which is a refusal rather than a pick.
        let mut second = provider_serving(
            "another-router",
            WireProtocol::AnthropicMessages,
            "https://another.example/api",
        );
        second.credential_env = vec![CREDENTIAL_VAR.to_owned()];
        match gateway_upstream(&[anthropic.clone(), second], &secrets) {
            Err(GatewayUpstreamRefusal::SeveralProvidersServeTheIngress { candidates, .. }) => {
                assert_eq!(candidates, vec!["openrouter", "another-router"]);
            }
            other => panic!("expected a refusal naming both candidates, got {other:?}"),
        }

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
        let mut second = provider_serving(
            "another-router",
            WireProtocol::AnthropicMessages,
            "https://another.example/api",
        );
        second.credential_env = vec![CREDENTIAL_VAR.to_owned()];
        let mut unusable = provider_serving(
            "unusable",
            WireProtocol::AnthropicMessages,
            "not-an-absolute-url",
        );
        unusable.credential_env = vec![CREDENTIAL_VAR.to_owned()];

        let refusals = vec![
            gateway_upstream(&[], &secrets).unwrap_err(),
            gateway_upstream(&[anthropic.clone(), second], &secrets).unwrap_err(),
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
                GatewayUpstreamRefusal::SeveralProvidersServeTheIngress { .. } => "several",
                GatewayUpstreamRefusal::CredentialUnavailable { .. } => "credential",
                GatewayUpstreamRefusal::Unusable(_) => "unusable",
            });
        }
        assert_eq!(seen.len(), 4, "every variant must be exercised: {seen:?}");
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
            });
        }
        assert_eq!(
            seen.len(),
            16,
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
}
