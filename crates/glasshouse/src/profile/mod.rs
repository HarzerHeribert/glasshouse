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
//! # Why this module never imports `crate::database`
//!
//! A launch profile is configuration, not project memory. It is read from
//! [`crate::config`], resolved here, and applied to one child process; none
//! of that touches the project's SQLite database, and it must not start to.
//! Only a *reference* to which profile a session ran under belongs in the
//! database — see `session/store.rs` — and a reference is not a definition.

use std::ffi::OsString;
use std::fmt;

use crate::harness::{ApprovalKind, ApprovalMode, HarnessAdapter, WireProtocol};
use crate::integrations::IntegrationId;
use crate::launch::HarnessLaunch;

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
        "launch profile `{profile}` for {} needs {backend}, but provider-backed launch \
         profiles need provider configuration, which is Phase 9C/9D",
        .harness.display_name(),
    )]
    BackendUnavailable {
        profile: String,
        harness: IntegrationId,
        backend: &'static str,
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
}

/// Resolve `profile` against `adapter`, producing the overlay for exactly
/// one child process — or refusing, which starts nothing.
///
/// `acknowledged_bypass` is the caller's answer to "has this harness's
/// blanket-bypass risk been shown to and accepted by the user", read from
/// user-level configuration only — see
/// [`crate::config::EffectiveConfig::bypass_acknowledged`].
pub fn resolve(
    profile: &LaunchProfile,
    adapter: &dyn HarnessAdapter,
    acknowledged_bypass: bool,
) -> Result<LaunchOverlay, Refusal> {
    if !matches!(profile.backend, BackendResource::Native) {
        return Err(Refusal::BackendUnavailable {
            profile: profile.name.clone(),
            harness: profile.harness,
            backend: profile.backend.kind_description(),
        });
    }

    if profile.model.is_some() {
        let can_override_model = adapter.describe().backends.model_override.value().is_some();
        if !can_override_model {
            return Err(Refusal::NoModelOverride {
                profile: profile.name.clone(),
                harness: profile.harness,
            });
        }
        // Phase 9A only validates that the harness *could* honour a
        // requested model; turning that model into an argument or an
        // environment variable needs a real provider identity, which is
        // Phase 9F. Doing it "generically" here — picking one of a harness's
        // several declared mechanisms without provider context — is exactly
        // the invention this module exists to refuse.
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

    match profile.approval {
        ApprovalSelection::Default => {
            // A harness with no automatic review gets no approval argument
            // at all here — never a silent bypass. `approval_args` already
            // answers `None` for a mode a harness lacks; there is nothing
            // else to try.
            if let Some(args) = adapter.approval_args(ApprovalKind::AutomaticReview) {
                append_approval(adapter, &mut overlay, &args, "automatic review");
            }
        }
        ApprovalSelection::AutomaticReview => {
            let Some(args) = adapter.approval_args(ApprovalKind::AutomaticReview) else {
                return Err(Refusal::NoAutomaticReview {
                    profile: profile.name.clone(),
                    harness: profile.harness,
                });
            };
            append_approval(adapter, &mut overlay, &args, "automatic review");
        }
        ApprovalSelection::Bypass => {
            let Some(args) = adapter.approval_args(ApprovalKind::Bypass) else {
                return Err(Refusal::NoBypass {
                    profile: profile.name.clone(),
                    harness: profile.harness,
                });
            };
            if !acknowledged_bypass {
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
    use crate::harness::adapter_for;

    fn profile_for(harness: IntegrationId) -> LaunchProfile {
        LaunchProfile::native(harness)
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

            let overlay = resolve(&profile, adapter, false)
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

        let err = resolve(&profile, adapter, false).expect_err("must be refused");
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

        let overlay = resolve(&profile, adapter, false).unwrap();
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

        let overlay_acknowledged = resolve(&profile, adapter, true).unwrap();
        assert!(overlay_acknowledged.args().is_empty());
    }

    // --- 4. bypass refused until acknowledged, per harness ----------------

    #[test]
    fn a_bypass_is_refused_until_it_is_acknowledged_for_that_harness() {
        let adapter = adapter_for(IntegrationId::Hermes).expect("a harness");
        let mut profile = profile_for(IntegrationId::Hermes);
        profile.approval = ApprovalSelection::Bypass;

        let err = resolve(&profile, adapter, false).expect_err("unacknowledged bypass is refused");
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

        let overlay = resolve(&profile, adapter, true).expect("acknowledged bypass resolves");
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
        let err = resolve(&other_profile, other_adapter, false)
            .expect_err("Hermes's acknowledgement must not carry over to Antigravity");
        assert!(matches!(err, Refusal::BypassNotAcknowledged { .. }));
    }

    // --- 5. a provider-backed profile names the phase that supplies it ---

    #[test]
    fn a_provider_backed_profile_is_refused_with_the_phase_that_supplies_it() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");

        for backend in [
            BackendResource::DirectProvider {
                provider: "openrouter".to_owned(),
            },
            BackendResource::GlasshouseGateway,
        ] {
            let mut profile = profile_for(IntegrationId::ClaudeCode);
            profile.backend = backend;

            let err = resolve(&profile, adapter, false).expect_err("provider backends are refused");
            match &err {
                Refusal::BackendUnavailable { harness, .. } => {
                    assert_eq!(*harness, IntegrationId::ClaudeCode);
                }
                other => panic!("expected BackendUnavailable, got {other:?}"),
            }
            let message = err.to_string();
            assert!(message.contains("9C"), "{message}");
            assert!(message.contains("9D"), "{message}");
            assert!(message.contains("Claude Code"), "{message}");
        }
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
        // `apply` in isolation: no shipped harness backend actually
        // populates `env` in Phase 9A (only `Native` ever resolves, and it
        // contributes none), so this is the only way to prove the mechanism
        // itself carries an environment operation onto the child.
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
        let overlay = resolve(&profile, adapter, false).unwrap();
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

        let overlay =
            resolve(&profile, adapter, false).expect("Claude Code declares a model override");

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
        let err = resolve(&refused_profile, opencode, false).expect_err("unsupported protocol");
        assert!(!err.to_string().contains("SECRET"));
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

        let err = resolve(&profile, &adapter, false).expect_err("no model override declared");
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

        let err = resolve(&profile, adapter, true).expect_err("Pi declares no bypass mode");
        assert!(matches!(err, Refusal::NoBypass { .. }));
    }

    #[test]
    fn a_protocol_a_harness_cannot_serve_is_refused() {
        let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
        let mut profile = profile_for(IntegrationId::ClaudeCode);
        profile.expected_protocol = Some(WireProtocol::OpenAiResponses);

        let err = resolve(&profile, adapter, false)
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

        resolve(&profile, adapter, false).expect("Claude Code speaks Anthropic Messages natively");
    }

    #[test]
    fn an_unverified_protocol_declaration_cannot_serve_anything() {
        // Cursor's protocols are `Unverified`. "Nobody checked" must not be
        // treated as "yes" for a protocol match.
        let adapter = adapter_for(IntegrationId::Cursor).expect("a harness");
        let mut profile = profile_for(IntegrationId::Cursor);
        profile.expected_protocol = Some(WireProtocol::AnthropicMessages);

        let err = resolve(&profile, adapter, false)
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
