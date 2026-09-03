//! The launch-profile abstraction and its resolution into a per-launch
//! overlay.
//!
//! Three things live here, and they are deliberately not the same type:
//!
//! - A [`LaunchProfile`] is **inert configuration** — a name, a harness, a
//!   backend resource, an optional model, an optional expected protocol, an
//!   approval selection, and an optional named response preset. Nothing
//!   about it has touched a real adapter yet.
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

pub mod generated;
pub mod response;

pub use generated::EphemeralConfigs;
use std::ffi::OsString;
use std::fmt;

use crate::gateway::translate;
use crate::gateway::upstream::UpstreamBackend;
use crate::gateway::{Gateway, Route, Upstream};
use crate::harness::pairing::PairingOverrides;
use crate::harness::{
    ApprovalKind, ApprovalMode, ConfigFileNameProblem, CredentialPlacement, CredentialVarProblem,
    Declared, DirectProviderRequest, GeneratedConfigSite, HarnessAdapter, WireProtocol,
};
use crate::integrations::IntegrationId;
use crate::launch::HarnessLaunch;
use crate::provider::{ProtocolCompatibleProviders, Provider};
use crate::routing::{AssignedModel, Cost, CredentialId, ToolSemantics};
use crate::secret::{SecretRef, SecretStore};

/// The protocols the local gateway's ingress knows how to serve.
///
/// All four, and the list is here rather than in [`mod@crate::gateway`]
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
    // Phase 56 T3. Present for the *destination* half of this list's two
    // jobs: a protocol here is one `gateway_routes` will build a route for,
    // and without a route the pair table could name a Gemini provider that
    // nothing could ever forward to. No installed harness speaks it at the
    // ingress — every `gemini-generate-content -> …` row in the gateway's
    // pair table is refused by name — so its *ingress* half is a `404` that
    // names the missing adapter rather than a `404` that says nothing.
    WireProtocol::GeminiGenerateContent,
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
        // Two spellings, because Google's version segment is `v1beta` and
        // `VERSION_SEGMENT` — the `/v1` the gateway strips before matching —
        // does not cover it. `/models` catches the un-versioned and `/v1`
        // forms; the explicit `/v1beta/models` catches what every current
        // Gemini client actually sends. Read off Google's published
        // `POST https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent`,
        // the same standing as the OpenAI-Chat entry above.
        WireProtocol::GeminiGenerateContent => &["/models", "/v1beta/models"],
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
    /// Line 353's sixth axis: the named [`response::Preset`] this profile
    /// asks for, or `None` for a profile that says nothing about
    /// communication policy.
    ///
    /// A name, not a resolved [`response::ResponseProfile`] — the same reason
    /// [`LaunchProfile::backend`]'s `DirectProvider` variant carries a
    /// provider *name* rather than a looked-up [`crate::provider::Provider`]:
    /// resolving a preset name against `response::presets()` is cheap and
    /// total, so there is nothing to gain by asking the caller to resolve it
    /// before handing the profile over, and something to lose — a
    /// `LaunchProfile` that could hold an unresolvable preset would need a
    /// second refusal path this module does not otherwise have.
    ///
    /// Consulted by `main.rs::launch_session`, which folds it into the
    /// session's [`crate::config::response::ResponseRequest`] as the
    /// `PrecedenceLayer::Session` layer when the command line named no
    /// preset of its own — an explicit `--response-preset` always wins,
    /// because a person typing one on the command line is a stronger request
    /// than a profile's standing default. See that function's own comment
    /// for why this could not become a seventh [`response::PrecedenceLayer`]:
    /// the map's line 596 fixes the chain at exactly six named layers, and
    /// that box is already closed.
    pub response_preset: Option<String>,
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
            response_preset: None,
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
    configs: Vec<PendingConfig>,
    mechanisms: Vec<MechanismNote>,
}

/// One generated configuration document this overlay still has to have
/// written before its child process starts.
///
/// A *description*, not a write. [`resolve`] composes these and touches no
/// filesystem — `resolving_a_launch_profile_touches_no_files` is unchanged
/// and still enforces it — and [`LaunchOverlay::install`] is the only writer.
///
/// No `Debug` derive that could print `contents`: a document is composed from
/// a base URL, header values and variable *names*, none of which is a
/// credential today, but this is the one field in the overlay that carries a
/// whole harness configuration and it should not be rendered wholesale into
/// a log or a panic message.
#[derive(Clone, PartialEq, Eq)]
pub struct PendingConfig {
    file_name: &'static str,
    contents: String,
    placement: crate::harness::ConfigPathPlacement,
}

impl PendingConfig {
    /// What it will be called, inside whichever directory Glasshouse owns
    /// for the session it belongs to. Never a path: see
    /// [`LaunchOverlay::install`].
    pub fn file_name(&self) -> &'static str {
        self.file_name
    }

    /// The document itself. `pub(crate)` rather than `pub`: the only reader
    /// is [`mod@generated`], which writes it; nothing outside this crate has
    /// a reason to hold a whole harness configuration, and a public accessor
    /// is how one would end up in a log.
    pub(crate) fn contents(&self) -> &str {
        &self.contents
    }
}

impl fmt::Debug for PendingConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingConfig")
            .field("file_name", &self.file_name)
            .field("byte_count", &self.contents.len())
            .field("placement", &self.placement)
            .finish()
    }
}

impl LaunchOverlay {
    fn empty() -> Self {
        Self {
            args: Vec::new(),
            env: Vec::new(),
            configs: Vec::new(),
            mechanisms: Vec::new(),
        }
    }

    pub fn args(&self) -> &[OsString] {
        &self.args
    }

    pub fn env(&self) -> &[(OsString, OsString)] {
        &self.env
    }

    /// The generated configuration documents this overlay still needs
    /// written — see [`LaunchOverlay::install`].
    pub fn configs(&self) -> &[PendingConfig] {
        &self.configs
    }

    pub fn mechanisms(&self) -> &[MechanismNote] {
        &self.mechanisms
    }

    /// Write this overlay's generated configuration documents into `site`,
    /// point the child at them, and return the guard that removes them again.
    ///
    /// # Why this is separate from [`resolve`] and from [`LaunchOverlay::apply`]
    ///
    /// Resolution happens **before** a session record exists, because a
    /// refusal must cost nothing — no row, no process. So at resolution time
    /// there is no session directory and no path to put in an environment
    /// variable. The adapter therefore declares a
    /// [`crate::harness::ConfigPathPlacement`] instead of a path, and this
    /// step fills it in once the caller knows where the session lives.
    ///
    /// `apply` cannot do it: it is infallible, and a write that failed there
    /// would have to be swallowed, leaving the child pointed at a document
    /// that does not exist.
    ///
    /// # Forgetting to call this fails loudly, by construction
    ///
    /// The mechanism note and the *selection* arguments — OpenCode's
    /// `--model <provider>/<model>` — are added during resolution; only the
    /// document and the variable naming it are added here. An overlay that
    /// was applied without being installed therefore starts a harness that
    /// has been told to use a provider it has never heard of, which OpenCode
    /// refuses outright ("Model not found: …") rather than silently falling
    /// back to the user's own paid account. That ordering is deliberate: the
    /// two halves are split so that the loud failure is the one that
    /// survives a mistake.
    ///
    /// # Ephemeral means ephemeral, and this is what makes it true
    ///
    /// The returned [`EphemeralConfigs`] removes every file it wrote when it
    /// drops, so a caller holding it across `session::attach` gets a document
    /// that exists for exactly the life of the child process. Dropping it
    /// early would delete a file the running harness may still re-read;
    /// dropping it late — or never — is the surprise file in somebody's
    /// state directory that the map's "temporary or Glasshouse-owned" line
    /// exists to prevent. It also registers a
    /// [`crate::shutdown::on_forced_exit`] cleanup, because the forced path
    /// calls [`std::process::exit`] and runs no destructor.
    ///
    /// An overlay with nothing to write returns a guard that owns nothing, so
    /// a caller never has to ask whether it has any.
    pub fn install(&mut self, site: GeneratedConfigSite<'_>) -> std::io::Result<EphemeralConfigs> {
        use crate::harness::ConfigPathPlacement;

        let mut paths = Vec::with_capacity(self.configs.len());
        for config in &self.configs {
            // The second of the two file-name checks, and the one that
            // matters: this is the step that turns a name into a path. An
            // adapter that got past `accept_generated_config` with a name
            // that could leave the site would be stopped here, before
            // anything is opened.
            let Some(path) = site.file(config.file_name) else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "`{}` is not a name a generated configuration may be written under",
                        config.file_name
                    ),
                ));
            };
            paths.push(path);
        }

        let installed = generated::write_all(&self.configs, &paths)?;

        for (config, path) in self.configs.iter().zip(&paths) {
            match config.placement {
                ConfigPathPlacement::Environment(var) => {
                    self.env
                        .push((OsString::from(var), path.clone().into_os_string()));
                }
                ConfigPathPlacement::Argument(flag) => {
                    self.args.push(OsString::from(flag));
                    self.args.push(path.clone().into_os_string());
                }
            }
        }
        Ok(installed)
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
        "launch profile `{profile}` is backed by the local Glasshouse gateway, but {} cannot be \
         served through a translated pairing: `{pair}` — {reason}",
        .harness.display_name(),
    )]
    GatewayTranslationRefused {
        profile: String,
        harness: IntegrationId,
        /// The pair table's own spelling, `from->to` — [`translate::Pair::slug`].
        pair: String,
        /// The table's own recorded reason the pair is refused.
        reason: &'static str,
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
        "launch profile `{profile}` asks {} to use the provider `{provider}`, but {} selects a \
         provider through the model it is given and this profile names none; add a `model` to \
         the profile",
        .harness.display_name(),
        .harness.display_name(),
    )]
    DirectProviderNeedsModel {
        profile: String,
        harness: IntegrationId,
        provider: String,
    },

    #[error(
        "refusing launch profile `{profile}`: {} asked for a generated configuration called \
         `{file_name}`, but {problem}",
        .harness.display_name(),
    )]
    UnsafeGeneratedConfigName {
        profile: String,
        harness: IntegrationId,
        file_name: String,
        problem: ConfigFileNameProblem,
    },

    #[error(
        "refusing launch profile `{profile}`: the provider `{provider}` declares a {field} \
         containing `{sequence}`, and {} substitutes that sequence inside the configuration \
         document Glasshouse generates for it — the value would be replaced by something else \
         before the harness ever read it. Remove it from your provider configuration",
        .harness.display_name(),
    )]
    UnsafeGeneratedConfigValue {
        profile: String,
        harness: IntegrationId,
        provider: String,
        /// Which configured value carried it — "base URL" or "header value".
        /// A name, never the value itself.
        field: &'static str,
        sequence: &'static str,
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

/// Phase 9J line 576: the native-pairing preference and corrections a
/// gateway-backed launch resolves once from configuration, then hands to the
/// gateway it points the child at — `apply_gateway` passes this to
/// [`crate::gateway::session::SessionRouting::set_pairing_preference`] beside
/// its own call to [`crate::gateway::session::SessionRouting::bind`].
///
/// A parameter on [`resolve_with_gateway`] rather than a field on
/// [`Resolution`], for the reason that function's own doc comment gives for
/// keeping `gateway` off `Resolution` too: this is a property of *this
/// gateway-backed call*, not of the profile or the adapter, and every
/// existing caller that resolves a [`Resolution`] by hand (`config`'s and
/// `onboarding`'s tests, `tests/pty_smoke.rs`, `tests/launch_overlay.rs`) can
/// go on doing so unchanged.
///
/// `preference_slug` is [`crate::config::pairing::PairingPreference::slug`]'s
/// own spelling, not that type itself: this module may not import
/// `crate::config` (see the module documentation), the same reason
/// [`gateway_upstream`]'s `free` closure answers a plain `bool` instead of a
/// `crate::config` type. [`SessionRouting::set_pairing_preference`] parses it
/// back and degrades an unrecognised spelling to
/// [`PairingPreference::Strong`], never refusing a launch over it.
///
/// [`PairingPreference::Strong`]: crate::config::pairing::PairingPreference::Strong
/// [`SessionRouting::set_pairing_preference`]: crate::gateway::session::SessionRouting::set_pairing_preference
#[derive(Debug, Clone)]
pub struct GatewayPairing {
    pub preference_slug: &'static str,
    pub overrides: PairingOverrides,
}

impl Default for GatewayPairing {
    /// The same out-of-the-box answer
    /// [`crate::config::EffectiveConfig::native_pairing_preference`]
    /// itself falls back to when nothing is configured — for a caller with no
    /// `EffectiveConfig` to resolve one from, which is every caller in this
    /// module's own test suite.
    fn default() -> Self {
        Self {
            preference_slug: "strong",
            overrides: PairingOverrides::default(),
        }
    }
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
    resolve_with_gateway(profile, cx, None, &GatewayPairing::default())
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
///
/// `pairing` is [`GatewayPairing::default`] for every caller that has no
/// [`crate::config::EffectiveConfig`] to resolve one from — the same
/// pre-Phase-9J-line-576 behaviour every caller other than `main.rs`'s own
/// two production sites gets today. Ignored entirely unless `gateway` is
/// `Some` and the profile's backend actually resolves through it.
pub fn resolve_with_gateway(
    profile: &LaunchProfile,
    cx: &Resolution<'_>,
    gateway: Option<&Gateway>,
    pairing: &GatewayPairing,
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
            apply_gateway(profile, gateway, adapter, pairing, &mut overlay)?;
        }
        // Phase 32 line 1184, and the gap Phase 32A's audit found: the other
        // two arms each record their resource kind, and this one recorded
        // nothing — so `ResourceKind::NativeSubscription` existed as a type
        // and was never constructed on any real launch. A distinction the
        // shipped binary never draws is not a distinction it makes.
        //
        // This adds a note and nothing else. It does not touch the harness's
        // argv, environment or configuration, which is what "Native behaviour
        // does not change" has always meant here.
        BackendResource::Native => {
            let kind = crate::provider::registry::ResourceKind::NativeSubscription {
                harness: profile.harness,
            };
            overlay.mechanisms.push(MechanismNote {
                category: "resource kind",
                detail: format!("{} — {}", kind.label(), kind.quota().as_str()),
            });
        }
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
///
/// Always resolves with [`GatewayPairing::default`] — this entry point has no
/// production caller today (only this module's own tests use it), so there is
/// no resolved `EffectiveConfig` value for it to thread through yet.
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
    resolve_with_gateway(profile, cx, gateway, &GatewayPairing::default())
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
///
/// 4. Phase 9J line 576: `pairing` is recorded on the gateway's routing
///    state beside the assignment `Gateway::routing().bind` just made, so a
///    later failover (`crate::gateway::session::SessionRouting::observe_exchange`)
///    scores candidates against what the user actually configured.
fn apply_gateway(
    profile: &LaunchProfile,
    gateway: &Gateway,
    adapter: &dyn HarnessAdapter,
    pairing: &GatewayPairing,
    overlay: &mut LaunchOverlay,
) -> Result<(), Refusal> {
    // A harness that selects its provider through the model needs one here
    // exactly as it does on the direct path — the gateway is a provider to
    // the child, not an exemption. Asked before anything is bound, so a
    // refusal binds no assignment.
    require_model_if_the_harness_selects_through_it(profile, adapter, GATEWAY_PROVIDER_NAME)?;
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
    // protocol the ingress does not serve natively is not refused on the
    // spot any more — the translated search below still has to be asked —
    // but it narrows that search to `expected` alone, exactly as it narrows
    // the native one here.
    let native = match profile.expected_protocol {
        Some(expected) => ingress_serves(&expected).then_some(expected),
        None => harness_protocols.iter().copied().find(ingress_serves),
    };
    let native = native.filter(|protocol| harness_protocols.contains(protocol));

    // No native route: ask the pair table for a harness protocol `P` (one of
    // `candidates`) with a `Supported` row to a served protocol `Q`. The
    // harness still speaks `P` at the ingress — `protocol` below feeds the
    // same `DirectProviderRequest` a native launch would build — and the
    // session binds to `Q`, the backend that actually answers.
    //
    // `translate::lookup` rather than `provider::translation_available`:
    // both sides here are already slugs (`WireProtocol::slug` and
    // `Gateway::served_protocols`'s own `&str`s, per the doc above), and
    // `translation_available` would only add a slug->`WireProtocol` round
    // trip this module has no other reason to grow.
    let candidates: Vec<WireProtocol> = match profile.expected_protocol {
        Some(expected) => vec![expected],
        None => harness_protocols.to_vec(),
    };
    let mut first_pair: Option<&'static translate::Pair> = None;
    let translated = 'search: {
        for candidate in candidates.iter().copied() {
            for &slug in &served {
                let Some(pair) = translate::lookup(candidate.slug(), slug) else {
                    continue;
                };
                if first_pair.is_none() {
                    first_pair = Some(pair);
                }
                if pair.is_supported() {
                    break 'search Some((candidate, slug.to_owned()));
                }
            }
        }
        None
    };

    let resolved = native
        .map(|protocol| (protocol, protocol.slug().to_owned()))
        .or(translated);
    let (protocol, served_protocol) = match resolved {
        Some(resolved) => resolved,
        None => {
            return Err(match first_pair {
                // The table was consulted and named a real, if refused, row
                // — refuse by that row's own name and reason rather than as
                // a bare "unserved".
                Some(pair) => Refusal::GatewayTranslationRefused {
                    profile: profile.name.clone(),
                    harness: profile.harness,
                    pair: pair.slug(),
                    reason: pair.refusal().unwrap_or("no reason recorded"),
                },
                // No row at all — the ingress serves nothing the table even
                // has an opinion on, so the message stays what it always
                // named: what the ingress serves, for a message that names
                // the mismatch from the side the user cannot change.
                None => Refusal::GatewayProtocolUnserved {
                    profile: profile.name.clone(),
                    harness: profile.harness,
                    protocols: served.iter().map(|slug| (*slug).to_owned()).collect(),
                },
            });
        }
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
        &served_protocol,
        model,
        gateway.upstream(),
    );

    // Phase 9J line 576, recorded beside the assignment above: the user's
    // configured native-pairing preference and corrections, so
    // `on_provider_failure` scores a later failover against them instead of
    // the out-of-the-box `PairingPreference::Strong` default. Unlike `bind`,
    // this never returns early — a preference is known whether or not the
    // protocol/backend lookup above found a route, and there is no honest
    // reason to drop it because of that.
    gateway
        .routing()
        .set_pairing_preference(pairing.preference_slug, pairing.overrides.clone());

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

    accept_generated_config(
        profile,
        GATEWAY_PROVIDER_NAME,
        &base_url,
        &[],
        &plan,
        overlay,
    )?;

    let Some(CredentialPlacement::Environment(destination)) = plan.credential else {
        return Err(Refusal::GatewayTokenUnplaceable {
            profile: profile.name.clone(),
            harness: profile.harness,
        });
    };

    // The base URL here is Glasshouse's own loopback address and there are no
    // provider headers at all, so neither can carry an interpolation
    // sequence. The check is made anyway, in the one place both paths go
    // through, so a future gateway address format cannot quietly acquire one.
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

    // Phase 32: the gateway is its own resource kind — a local router, not
    // a capacity of its own — see `crate::provider::registry::QuotaModel`
    // for why its quota is named as delegated rather than claimed as
    // metered, which would be wrong for a gateway currently bound to an
    // unmetered local upstream.
    let kind = crate::provider::registry::ResourceKind::GlasshouseGateway;
    overlay.mechanisms.push(MechanismNote {
        category: "resource kind",
        detail: format!("{} — {}", kind.label(), kind.quota().as_str()),
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
/// `free` answers, by provider name, whether that provider has at least one
/// model the user has marked free-tier — Phase 9I line 527's marking,
/// reaching this path per line 532. `crate::profile` may not import
/// `crate::config`, where that marking actually lives, so the caller passes
/// the answer in rather than this function looking it up; `main.rs`'s own
/// wrapper is where `ProviderConfig::free_models` and this closure meet. A
/// provider `free` was never asked about — because a caller has nothing to
/// mark, not because it is somehow known metered — answers `false`, which is
/// [`Cost::Metered`]'s own fail-closed default carried one level up.
pub fn gateway_upstream(
    providers: &[Provider],
    secrets: &dyn SecretStore,
    free: &dyn Fn(&str) -> bool,
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
                // Phase 9I line 527's marking, handed in by the caller — see
                // this function's own doc comment for why it arrives as a
                // closure rather than a lookup made here.
                if free(&provider.name) {
                    Cost::Free
                } else {
                    Cost::Metered
                },
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

    // Step 0 in the list above, and it comes before the protocol choice
    // because it is a fact about the harness rather than about this
    // provider: a harness that is pointed at a backend *through* its model
    // cannot be pointed at one without a model, whatever the provider serves.
    require_model_if_the_harness_selects_through_it(profile, cx.adapter, &provider.name)?;

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

    accept_generated_config(
        profile,
        &provider.name,
        base_url,
        &provider.headers,
        &plan,
        overlay,
    )?;

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

    // Phase 32: what kind of resource this actually is, so a session that
    // resolved to Ollama is never indistinguishable in the launch log from
    // one that resolved to a metered router — see
    // `crate::provider::registry`, whose classification this is.
    let kind = crate::provider::registry::ResourceKind::from_direct_provider(&provider.name);
    overlay.mechanisms.push(MechanismNote {
        category: "resource kind",
        detail: format!("{} — {}", kind.label(), kind.quota().as_str()),
    });

    Ok(())
}

/// The sequences a harness substitutes inside a configuration document
/// before parsing it.
///
/// OpenCode 1.18.22 replaces `{env:NAME}` from the child's environment and
/// `{file:PATH}` with a file's contents, anywhere in the document's text,
/// before it is parsed. That is the mechanism Glasshouse *uses* to keep a
/// credential out of a generated document — and it is therefore a mechanism a
/// configured value could smuggle itself into, which is why the two are
/// refused wherever they arrive from configuration.
///
/// The list is here rather than in the adapter because the rule is about the
/// class of mechanism, not about one harness: any harness that interpolates
/// its own configuration document has the same hazard, and a check written
/// once protects the adapters that have not been written yet — the same
/// reason [`crate::harness::unsafe_provider_name_char`] lives outside the
/// adapters.
const SUBSTITUTION_SEQUENCES: &[&str] = &["{env:", "{file:"];

/// Refuse before the adapter is asked, when this harness selects its
/// provider through a model and the profile names none.
///
/// Checked *before* [`crate::harness::HarnessAdapter::direct_provider_launch`]
/// so that a missing model is a message naming it. Asking the adapter first
/// would produce a `None`, which means "this harness declares no such
/// mechanism" — a different and, here, false statement.
fn require_model_if_the_harness_selects_through_it(
    profile: &LaunchProfile,
    adapter: &dyn HarnessAdapter,
    provider_name: &str,
) -> Result<(), Refusal> {
    if adapter.direct_provider_requires_model() && profile.model.is_none() {
        return Err(Refusal::DirectProviderNeedsModel {
            profile: profile.name.clone(),
            harness: profile.harness,
            provider: provider_name.to_owned(),
        });
    }
    Ok(())
}

/// Accept a plan's generated configuration document onto `overlay`, or
/// refuse it.
///
/// Two things are checked here, and each is a way the mechanism could stop
/// being "isolated, Glasshouse-owned, and carrying no secret":
///
/// 1. **What it is called.** An adapter never returns a path — only a name —
///    and a name that is not a plain file name is refused here rather than
///    joined onto the directory Glasshouse owns.
///    [`LaunchOverlay::install`] checks the same thing again at the moment it
///    joins, because that is the step that could actually write somewhere
///    else.
/// 2. **What the user's own configuration can smuggle into it.** See
///    [`SUBSTITUTION_SEQUENCES`].
///
/// Nothing is written, and no path is decided. The document goes onto the
/// overlay as a [`PendingConfig`]; [`LaunchOverlay::install`] is the only
/// writer, and the only thing that knows where the session's directory is.
fn accept_generated_config(
    profile: &LaunchProfile,
    provider_name: &str,
    base_url: &str,
    headers: &[(String, String)],
    plan: &crate::harness::DirectProviderPlan,
    overlay: &mut LaunchOverlay,
) -> Result<(), Refusal> {
    let Some(config) = &plan.config else {
        return Ok(());
    };

    for sequence in SUBSTITUTION_SEQUENCES {
        let carrier = if base_url.contains(sequence) {
            Some("base URL")
        } else if headers.iter().any(|(_, value)| value.contains(sequence)) {
            Some("header value")
        } else {
            None
        };
        if let Some(field) = carrier {
            return Err(Refusal::UnsafeGeneratedConfigValue {
                profile: profile.name.clone(),
                harness: profile.harness,
                provider: provider_name.to_owned(),
                field,
                sequence,
            });
        }
    }

    if let Some(problem) = crate::harness::unsafe_config_file_name(config.file_name) {
        return Err(Refusal::UnsafeGeneratedConfigName {
            profile: profile.name.clone(),
            harness: profile.harness,
            file_name: config.file_name.to_owned(),
            problem,
        });
    }

    // The file name and how the child will be told where it is. Not the
    // document, and not one byte of its contents — the same rule every other
    // mechanism note follows.
    overlay.mechanisms.push(MechanismNote {
        category: "generated configuration",
        detail: format!(
            "`{}` written in the directory Glasshouse owns for this session and removed with \
             it, named to the child by {}",
            config.file_name,
            match config.path_placement {
                crate::harness::ConfigPathPlacement::Environment(var) => var.to_owned(),
                crate::harness::ConfigPathPlacement::Argument(flag) => format!("`{flag}`"),
            }
        ),
    });
    overlay.configs.push(PendingConfig {
        file_name: config.file_name,
        contents: config.contents.clone(),
        placement: config.path_placement,
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

/// The timeout budget a pre-flight check runs under, and why it is
/// deliberately not [`crate::provider::discovery::ProbeTimeouts::default`].
///
/// Every existing caller of [`crate::provider::discovery::connectivity`] is
/// answering a question a keystroke just asked, and can afford the default's
/// 5/10/20 seconds because waiting *is* what the user asked for.
///
/// A pre-flight check is the opposite. Nobody asked for it, it sits between
/// `glasshouse launch` and the session, and its entire justification is the
/// qualifier capability map line 468 puts on the requirement itself: **when a
/// cheap capability check is available**. A launch that stalls twenty seconds
/// on an unreachable host has already cost more than the check could be
/// worth, so this budget is the worst delay a launch may pay — four seconds,
/// once, and only for a profile that has a check at all.
///
/// The numbers are not arbitrary. Every provider catalogue probed on
/// 2026-08-26 answered in well under a second — see
/// [`crate::provider::discovery::RESPONSE_TIMEOUT`]'s own doc — so two and a
/// half seconds is still an order of magnitude of headroom over every
/// measured healthy answer. A host that misses it is reported as "did not
/// answer", which, because a pre-flight check never refuses a launch, costs
/// the user a line of text rather than their session.
pub const PREFLIGHT_TIMEOUTS: crate::provider::discovery::ProbeTimeouts =
    crate::provider::discovery::ProbeTimeouts {
        connect: std::time::Duration::from_millis(1_500),
        response: std::time::Duration::from_millis(2_500),
        total: std::time::Duration::from_secs(4),
    };

/// What a pre-flight check found — capability map line 468.
///
/// # There is no `Refuse` variant, and that is the ruling
///
/// The map's verb is *verify*, and the obvious reading of "verify before
/// starting" is that a failed check refuses the launch. It was considered and
/// rejected, for reasons that are about this check specifically rather than
/// about caution in general:
///
/// 1. **No outcome of this check is unambiguous evidence that the combination
///    is wrong.** [`crate::provider::discovery::ProbeTarget::BaseUrl`] — the
///    target for every provider whose model-list endpoint nobody has
///    established, which is most of them — sends `GET <base>`, and a `404` or
///    `405` from a base URL that serves no `GET` is the *healthy* answer.
///    Refusing on that would refuse correct configurations.
/// 2. **Reachability is not correctness.** Three of the twenty-two provider
///    hosts probed for Phase 9D answer identically to a real path and a
///    nonsense one, so a negative result from a single request is a claim
///    about whether the host routed at all, never about whether this
///    credential would work. Turning that into a refusal is the mistake that
///    nearly deleted a correct URL from the provider table.
/// 3. **The failure it would prevent is cheaper than the failure it would
///    cause.** A wrong combination costs one harness startup and the
///    provider's own error — the status quo. A false refusal costs the user
///    the ability to start work at all, on a path (the network) that fails
///    independently of anything they configured.
/// 4. **[`resolve`] already owns refusal, and owns it better.** It refuses
///    from declarations — deterministically, offline, with a message naming
///    what was asked for. Putting a second, network-dependent authority
///    beside it would make whether a session may start depend on a remote
///    host's mood.
///
/// So this check *reports*, and the launch proceeds. The corollary is that it
/// needs no "start anyway" key: a refusal before start would need one, and the
/// reason it would need one — that the check can be wrong about a working
/// setup — is the same reason it does not refuse.
///
/// # What each variant means the caller should do
///
/// [`Preflight::NotChecked`] and [`Preflight::Answered`] are for the log.
/// [`Preflight::CredentialRejected`] and [`Preflight::Unreachable`] are the
/// two outcomes a user can act on before the harness takes the screen, and
/// [`Preflight::warning`] returns exactly those.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preflight {
    /// No check was available, which is line 468's own qualifier and **not a
    /// failure**: the launch proceeds exactly as it would if this function
    /// did not exist. See [`capability_probe`] for the three profiles that
    /// answer this way.
    NotChecked { reason: &'static str },
    /// The endpoint produced an HTTP response. Whether it was `200` or a
    /// `404` from a base URL that serves no `GET` is in the summary; either
    /// way the host is there and routing, which is the strongest claim one
    /// request can support.
    Answered { summary: String },
    /// The endpoint answered `401` or `403`. Reachable, and it refused this
    /// credential — the one outcome here that is both specific and
    /// actionable, and the reason the check is worth making at all.
    CredentialRejected { summary: String },
    /// Nothing answered: no route, no host, a refused connection, or the
    /// budget in [`PREFLIGHT_TIMEOUTS`] ran out.
    Unreachable { summary: String },
}

impl Preflight {
    /// One line for the launch log, on every path including the one where no
    /// check was possible — "there was nothing to check" is a fact a reader
    /// diagnosing a launch needs as much as a result.
    pub fn summary(&self) -> &str {
        match self {
            Self::NotChecked { reason } => reason,
            Self::Answered { summary }
            | Self::CredentialRejected { summary }
            | Self::Unreachable { summary } => summary,
        }
    }

    /// The line to put in front of the user before the harness starts, or
    /// `None` when there is nothing they can act on.
    ///
    /// `Answered` is deliberately not a warning. A `GET` to a base URL that
    /// serves none answers `404` for a perfectly good provider, and a channel
    /// that fires on every launch of a working profile is a channel users
    /// learn to ignore — which would cost them the two warnings that matter.
    pub fn warning(&self) -> Option<&str> {
        match self {
            Self::CredentialRejected { summary } | Self::Unreachable { summary } => Some(summary),
            Self::NotChecked { .. } | Self::Answered { .. } => None,
        }
    }
}

/// Run the pre-flight capability check for `profile` — capability map line
/// 468 — or report that there was none to run.
///
/// # This is the one function in this module that touches the network
///
/// [`resolve`] and [`resolve_with_gateway`] are pure functions of the values
/// in [`Resolution`], and stay that way: nothing here is called by either of
/// them, and **this runs after resolution, never before it**. That ordering
/// is not incidental — it is what makes
/// `a_capability_probe_cannot_influence_which_backend_resolve_selects` true
/// by construction on the production path rather than by inspection. A check
/// that ran first, and whose result reached `resolve`, would be a router; the
/// backend is chosen from the profile's declaration and nothing this function
/// learns can change it.
///
/// It costs nothing at all for a profile with no check available — no
/// request, no socket, no thread — which is every `Native` and every
/// gateway-backed profile, and therefore every launch that did not name a
/// direct provider. For one that does, it costs exactly one bounded HTTP
/// request; see [`PREFLIGHT_TIMEOUTS`] for the ceiling.
///
/// # The credential
///
/// The summary is built from the provider's name, the protocol slug, the URL
/// the probe requested and [`describe_probe_outcome`] — none of which is the
/// credential, and the last of which
/// [`crate::provider::discovery::ProbeOutcome::Unreachable`] deliberately
/// builds from a fixed set of phrases rather than an error's own words. It is
/// then passed through [`crate::secret::redact`] anyway, because a *base URL*
/// is user-supplied text that can carry anything and this string reaches both
/// the terminal and the log.
pub fn preflight(profile: &LaunchProfile, cx: &Resolution<'_>) -> Preflight {
    use crate::provider::discovery::{ProbeOutcome, connectivity};

    let request = match capability_probe(profile, cx) {
        CapabilityProbe::Unavailable { reason } => return Preflight::NotChecked { reason },
        CapabilityProbe::Available(request) => request,
    };
    let outcome = connectivity(&request, PREFLIGHT_TIMEOUTS);
    let summary = crate::secret::redact(&format!(
        "launch profile `{}`: provider `{}` at {} over `{}` {}",
        profile.name,
        request.provider(),
        request.url(),
        request.protocol(),
        describe_probe_outcome(&outcome),
    ));
    match outcome {
        ProbeOutcome::Reached { .. } | ProbeOutcome::Unexpected { .. } => {
            Preflight::Answered { summary }
        }
        ProbeOutcome::Rejected { .. } => Preflight::CredentialRejected { summary },
        ProbeOutcome::TimedOut { .. } | ProbeOutcome::Unreachable { .. } => {
            Preflight::Unreachable { summary }
        }
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
mod tests;
