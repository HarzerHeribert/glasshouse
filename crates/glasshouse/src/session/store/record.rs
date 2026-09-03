//! The stored session row and its vocabularies — Phase 59 split this out of
//! `session/store.rs` (`GH-DECOMP-SESSION-STORE`). [`super::SessionStore`]
//! and its SQL stay in the parent module; this file holds what a row *is*:
//! the identifiers, the enums the schema's `CHECK` constraints fix, the two
//! labels a person may attach, and the record types built from them.

use std::fmt;

use crate::profile::response::{
    AnswerFormat, Audience, Dimension, EvidenceDetail, Narration, ResponseProfile, Verbosity,
};
use crate::routing::AssignedModel;
use crate::session::supervision::{ProcessIdentity, Supervision};

/// A Glasshouse session identifier.
///
/// Distinct from any harness's native identifier, which is recorded
/// separately: Glasshouse names its own sessions so that a session remains
/// identifiable before a harness has produced an identifier, and after the
/// harness's own history is gone.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub(super) String);

impl SessionId {
    /// Wrap an identifier that already exists, such as one read back from the
    /// database or supplied on the command line.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

/// What a session is being used for.
///
/// The orchestrator role is a tag on an ordinary session, never a separate
/// kind of thing: an orchestrator is a real harness in a real terminal that
/// the user can enter, exactly like a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRole {
    Normal,
    Orchestrator,
    Worker,
}

/// Where a session's terminal is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPresentation {
    /// Inside Glasshouse's own TUI viewport.
    Embedded,
    /// Running with no visible viewport. Still a real session the user can
    /// bring to the front — not a hidden agent loop.
    Headless,
    /// Presented by something else, such as a cmux pane.
    External,
}

/// The state of the process behind a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLifecycle {
    /// Spawned, not yet known to be serving.
    Starting,
    /// Working on something.
    Running,
    /// Alive with nothing in flight.
    Idle,
    /// Alive and blocked on the user, which is different from idle and is only
    /// recorded when the harness says so rather than being guessed from
    /// silence.
    WaitingForUser,
    /// The process ended without an error worth flagging.
    Stopped,
    /// The process ended badly.
    Failed,
    /// The user retired the Glasshouse record. Closing does not touch the
    /// harness's own history.
    Closed,
}

/// The coarse categories a session list has to distinguish.
///
/// Derived from [`SessionLifecycle`] plus whether a native identifier was ever
/// recorded, rather than stored as its own column: two columns that can
/// disagree about the same fact eventually do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionDisposition {
    /// A live process.
    Active,
    /// No live process, but enough recorded to start the harness again where
    /// it left off.
    Resumable,
    /// Over, with nothing to go back to.
    Closed,
    /// Over because something went wrong.
    Failed,
}

/// The wire protocol a session's route speaks, as it was **recorded**.
///
/// # Why this is not `crate::harness::WireProtocol`
///
/// Phase 6 line 294 — checked, and guarded by a source scan in
/// `harness::tests::the_session_model_depends_on_no_adapter` — requires
/// adapter-specific parsing to stay isolated from the core session model, and
/// this module may not name `crate::harness` at all. That constraint turns
/// out to describe something true rather than merely forbidding an import: a
/// *stored* vocabulary and a *live* one have different lifetimes. A row
/// written last month has to stay readable when `WireProtocol` gains a
/// variant, and the schema's `CHECK` is what fixes the stored words. So the
/// two vocabularies are separate types, and `session::wire_protocol` is the
/// one total, exhaustive function between them — which makes a new
/// `WireProtocol` a compile error there, at the one place somebody has to
/// decide how it should be stored.
///
/// # `Unknown` is an answer and NULL is not
///
/// [`SessionProtocol::Unknown`] means Glasshouse established no wire protocol
/// for this session, which is what a launch profile naming none against a
/// harness declaring several produces. A NULL column means the build that
/// wrote the row recorded nothing here. Two facts, two representations,
/// because a single slot holding both is the collapse this phase's second
/// architectural requirement exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionProtocol {
    AnthropicMessages,
    OpenAiResponses,
    OpenAiChat,
    Unknown,
}

/// What the harness-and-model relationship was, as it was **recorded**.
///
/// The stored counterpart of `crate::harness::pairing::PairingClass`, kept
/// apart from it for [`SessionProtocol`]'s reason and converted by
/// `session::pairing_class`, which is exhaustive over the live enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPairingClass {
    VendorNative,
    VendorSupported,
    ProtocolNative,
    ProtocolCompatible,
    ProtocolTranslated,
    /// Nothing established a relationship. A recorded answer, not a gap.
    Unknown,
}

/// Which mechanism carried this session's response profile.
///
/// The category of [`crate::harness::response::AppliedMechanism`], and
/// deliberately *only* the
/// category. `harness::response` owns what a mechanism is; a session record
/// stores which of the three kinds answered, so that
/// `glasshouse sessions show` can say it after the fact. Storing the
/// mechanism's own free text here would put a second copy of one harness's
/// vocabulary in the project database, which is what line 603 forbids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseMechanism {
    /// The harness's own communication-style mechanism.
    Native,
    /// An instruction added alongside the harness's own system prompt.
    Additive,
    /// Nothing was applied.
    NotApplied,
}

/// A name a person gave a session.
///
/// A distinct type from [`SessionPurpose`] rather than a second `String`, so
/// that no call can hand a purpose to a rename or the other way round. Line
/// 650's rule — a rename never changes the native session identifier — is
/// enforced by [`super::SessionStore::rename`]'s SQL naming one column; this type is
/// what makes the *argument* unmistakable.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SessionName(String);

/// A lightweight purpose, such as `auth`, `tests`, or `research` — line 651.
///
/// Free text rather than an enumeration, because the map says "such as": a
/// fixed list would refuse the fourth thing a person actually does. Bounded
/// and single-line, because it is rendered in a fixed-width column beside
/// every other session.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SessionPurpose(String);

/// The longest name a session may be given.
pub(super) const MAX_SESSION_NAME: usize = 64;

/// The longest purpose a session may be tagged with.
pub(super) const MAX_SESSION_PURPOSE: usize = 32;

macro_rules! session_label {
    ($ty:ident, $what:literal, $max:ident) => {
        impl $ty {
            /// Parse a label a person typed.
            ///
            /// Surrounding whitespace is trimmed, because a trailing space is
            /// never what anyone meant and a stored label that differs from
            /// the one on screen by an invisible character is worse than a
            /// refusal. Everything else is refused rather than repaired:
            /// silently truncating or stripping would store something the
            /// user did not ask for.
            pub fn parse(value: &str) -> Result<Self, LabelError> {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    return Err(LabelError::Empty { what: $what });
                }
                if trimmed.chars().count() > $max {
                    return Err(LabelError::TooLong {
                        what: $what,
                        max: $max,
                        found: trimmed.chars().count(),
                    });
                }
                if trimmed.chars().any(char::is_control) {
                    return Err(LabelError::Control { what: $what });
                }
                Ok(Self(trimmed.to_owned()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.pad(&self.0)
            }
        }
    };
}

session_label!(SessionName, "a session name", MAX_SESSION_NAME);
session_label!(SessionPurpose, "a session purpose", MAX_SESSION_PURPOSE);

/// Why a label a person typed was refused.
#[derive(Debug, thiserror::Error)]
pub enum LabelError {
    #[error("{what} cannot be empty")]
    Empty { what: &'static str },
    #[error("{what} is at most {max} characters; that one is {found}")]
    TooLong {
        what: &'static str,
        max: usize,
        found: usize,
    },
    #[error("{what} cannot contain control characters")]
    Control { what: &'static str },
}

/// The five axes of a response profile, encoded for one column.
///
/// `verbosity=<v>,audience=<a>,narration=<n>,evidence=<e>,format=<f>`, built
/// from [`ResponseProfile::axes`] so the five names and the five slugs come
/// from `profile::response` rather than from a second list here.
pub(super) fn encode_response_profile(profile: &ResponseProfile) -> String {
    profile
        .axes()
        .iter()
        .map(|(dimension, value)| format!("{}={value}", dimension.slug()))
        .collect::<Vec<_>>()
        .join(",")
}

/// The reverse of [`encode_response_profile`], or `None` for anything this
/// build cannot read back exactly.
///
/// All five axes are required and every one has to parse. A profile decoded
/// from four axes and a default would be a profile the session never ran
/// under, reported as though it had.
pub(super) fn decode_response_profile(value: &str) -> Option<ResponseProfile> {
    let mut verbosity = None;
    let mut audience = None;
    let mut narration = None;
    let mut evidence = None;
    let mut format = None;

    for field in value.split(',') {
        let (name, slug) = field.split_once('=')?;
        match name {
            _ if name == Dimension::Verbosity.slug() => verbosity = Verbosity::from_slug(slug),
            _ if name == Dimension::Audience.slug() => audience = Audience::from_slug(slug),
            _ if name == Dimension::Narration.slug() => narration = Narration::from_slug(slug),
            _ if name == Dimension::Evidence.slug() => evidence = EvidenceDetail::from_slug(slug),
            _ if name == Dimension::Format.slug() => format = AnswerFormat::from_slug(slug),
            _ => return None,
        }
    }

    Some(ResponseProfile::new(
        verbosity?, audience?, narration?, evidence?, format?,
    ))
}

/// The model Glasshouse assigned, encoded for one column.
///
/// `harness-default` or `named:<id>`. The prefix is what keeps the two apart
/// however a model is named — a bare id column would have had one empty slot
/// for "the harness chose" and another for "this build recorded nothing", and
/// the two are different facts.
pub(super) fn encode_assigned_model(model: &AssignedModel) -> String {
    match model {
        AssignedModel::Named(id) => format!("named:{id}"),
        AssignedModel::HarnessDefault => "harness-default".to_owned(),
    }
}

pub(super) fn decode_assigned_model(value: &str) -> Option<AssignedModel> {
    if value == "harness-default" {
        return Some(AssignedModel::HarnessDefault);
    }
    let id = value.strip_prefix("named:")?;
    if id.is_empty() {
        return None;
    }
    Some(AssignedModel::named(id))
}

macro_rules! sql_enum {
    ($ty:ty { $($variant:ident => $text:literal),+ $(,)? }) => {
        impl $ty {
            /// The value stored in SQLite. The schema's `CHECK` constraint
            /// lists exactly these strings, so adding a variant here without
            /// a migration makes writes fail loudly rather than silently
            /// storing something readers cannot interpret.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }

            pub(super) fn from_str(value: &str) -> Option<Self> {
                match value {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }

        impl fmt::Display for $ty {
            /// `pad`, not `write_str`: a `Display` that writes straight to the
            /// formatter silently ignores width and alignment, so
            /// `{:<12}` in a table would produce ragged columns.
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.pad(self.as_str())
            }
        }
    };
}

sql_enum!(SessionRole {
    Normal => "normal",
    Orchestrator => "orchestrator",
    Worker => "worker",
});

sql_enum!(SessionPresentation {
    Embedded => "embedded",
    Headless => "headless",
    External => "external",
});

sql_enum!(SessionProtocol {
    AnthropicMessages => "anthropic-messages",
    OpenAiResponses => "openai-responses",
    OpenAiChat => "openai-chat",
    Unknown => "unknown",
});

sql_enum!(SessionPairingClass {
    VendorNative => "vendor-native",
    VendorSupported => "vendor-supported",
    ProtocolNative => "protocol-native",
    ProtocolCompatible => "protocol-compatible",
    ProtocolTranslated => "protocol-translated",
    Unknown => "unknown",
});

sql_enum!(ResponseMechanism {
    Native => "native",
    Additive => "additive",
    NotApplied => "none",
});

sql_enum!(SessionLifecycle {
    Starting => "starting",
    Running => "running",
    Idle => "idle",
    WaitingForUser => "waiting_for_user",
    Stopped => "stopped",
    Failed => "failed",
    Closed => "closed",
});

impl SessionLifecycle {
    /// True while a process is expected to exist.
    ///
    /// A full `match` rather than `matches!`, which imposes no exhaustiveness:
    /// a new variant must be classified here instead of defaulting to "not
    /// live".
    pub fn is_live(self) -> bool {
        match self {
            Self::Starting | Self::Running | Self::Idle | Self::WaitingForUser => true,
            Self::Stopped | Self::Failed | Self::Closed => false,
        }
    }
}

/// One stored session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub id: SessionId,
    /// The project this session belongs to. Always the active project for any
    /// record this module hands out.
    pub project_id: String,
    /// The harness that operates the session, as an
    /// [`crate::integrations::IntegrationId`] string.
    pub harness: String,
    /// The harness's own identifier, once one is known.
    pub native_session_id: Option<String>,
    pub role: SessionRole,
    pub lifecycle: SessionLifecycle,
    pub presentation: SessionPresentation,
    /// Where an [`SessionPresentation::External`] session is presented, as
    /// an opaque reference the presenting backend understands — line 760.
    /// `None` means no pane was recorded: either the session is not
    /// external, or it was recorded before this column existed, and this
    /// module does not distinguish the two. Stored and returned, never
    /// interpreted here: what the string means is the presenting
    /// integration's business, and this module names no integration (line
    /// 762).
    pub presentation_ref: Option<String>,
    /// Seconds since the Unix epoch.
    pub created_at: i64,
    /// Seconds since the Unix epoch.
    pub last_activity_at: i64,
    /// The launch profile this session ran under, by name. `None` means a
    /// session recorded before this column existed — a different fact from a
    /// session that ran the Native profile, which is recorded as
    /// `Some("native")`. A reference only: profiles themselves are
    /// configuration (see [`crate::profile`] and [`crate::config`]), never
    /// project memory.
    pub launch_profile: Option<String>,
    /// The resolved backend resource's [`crate::profile::BackendResource::slug`],
    /// recorded for the same reason and with the same `None` meaning as
    /// `launch_profile`.
    pub backend_resource: Option<String>,
    /// The model Glasshouse assigned this session, if any — see
    /// [`AssignedModel`]. `None` means the build that recorded this session
    /// stored nothing here, which is a different fact from
    /// `Some(AssignedModel::HarnessDefault)`, where Glasshouse deliberately
    /// named none and the harness chose.
    pub model: Option<AssignedModel>,
    /// What the harness-and-model relationship *is* — see
    /// [`crate::harness::pairing`]. Never derived from `harness`,
    /// `launch_profile` or `model`: it is
    /// [`crate::harness::pairing::classify`]'s answer, recorded.
    /// `Some(PairingClass::Unknown)` is a recorded answer; `None` is not one.
    pub pairing_class: Option<SessionPairingClass>,
    /// The wire protocol this session's route speaks. See
    /// [`SessionProtocol`] for why `Unknown` and `None` are not the same
    /// thing.
    pub protocol: Option<SessionProtocol>,
    /// The response profile this session was started with — its five axes,
    /// read back as the same type [`crate::profile::response`] resolves.
    /// Communication policy only; it says nothing about which model ran or
    /// what the session was allowed to do.
    pub response_profile: Option<ResponseProfile>,
    /// Which mechanism actually carried that profile to the harness.
    pub response_mechanism: Option<ResponseMechanism>,
    /// A name a person gave this session. Never the native session
    /// identifier and never derived from one — line 650.
    pub display_name: Option<SessionName>,
    /// A lightweight purpose a person tagged this session with — line 651.
    pub purpose: Option<SessionPurpose>,
    /// The session this one was bootstrapped from, if it was started with
    /// `--from-checkpoint` — Phase 40 line 1646. `None` means either a session
    /// recorded before this column existed, or a session that was never
    /// started from a checkpoint at all; both are the same fact, "this
    /// session has no recorded source," and the column does not distinguish
    /// them.
    pub source_session_id: Option<SessionId>,
    /// How many times a harness has told Glasshouse it was about to compact
    /// this session's context — map line 1159, *"when known"*.
    ///
    /// `None` means the build that recorded this session was not counting,
    /// which is a different fact from `Some(0)`, *"counted, and no compaction
    /// was observed"*. A router that could not tell those apart would read a
    /// session whose history is unknown as a session with a clean history —
    /// the same confident wrong answer [`SessionRecord::launch_profile`]'s
    /// `None` exists to prevent. Every session this build creates starts at
    /// `Some(0)`; a row from before migration 16 stays `None` until its first
    /// observed compaction, after which its count is a **lower bound**,
    /// because nothing observed the compactions that came before the upgrade.
    ///
    /// Written only by [`super::SessionStore::record_observed_compaction`], from the
    /// one production site that can tell a compaction is coming.
    pub observed_compactions: Option<i64>,
    /// Where HEAD stood the last time Glasshouse looked at this session —
    /// map line 1149's half of *"after a successful Git commit"*.
    ///
    /// The full object name, as `crate::checkpoint::git::GitPosition` read
    /// it. `None` means **nobody has looked yet**, which is a different fact
    /// from "HEAD has not moved" and is why the first look records a position
    /// without treating it as a code-change boundary: a boundary is a
    /// *change*, and there is nothing to have changed from.
    ///
    /// Per session on purpose. Two sessions working in one project each have
    /// their own idea of what they have already seen, and a project-wide
    /// column would let whichever session's turn ended first consume the
    /// boundary for both.
    ///
    /// Written only by [`super::SessionStore::record_seen_commit`].
    pub last_seen_commit: Option<String>,
    /// Which configured `[entitlements.<name>]` account served this session,
    /// by **name** — map line 1972's *"what it served"*, made answerable of
    /// the durable record.
    ///
    /// [`Self::backend_resource`] cannot answer this and no widening of it
    /// could: it holds a
    /// [`crate::profile::BackendResource::slug`], whose whole vocabulary is
    /// `native`, `direct-provider:<provider>` and `glasshouse-gateway` — a
    /// **kind** of resource. Two Claude accounts of one vendor, which is
    /// precisely Phase 56A's unit of capacity, both slug to `native`. This
    /// column names the **instance**.
    ///
    /// `None` means the serving account was never established — a session
    /// recorded before this column existed, a launch under a resource no
    /// `[entitlements]` entry describes, or a gateway-backed profile whose
    /// upstream is assigned after launch. All three are the same fact,
    /// *"nothing recorded"*, and the column does not distinguish them; it is
    /// [`Self::launch_profile`]'s `None` and it is never a name.
    ///
    /// A name, never a credential: an entitlement authenticates through a
    /// [`crate::secret::SecretRef`] resolved at the moment of use, and this
    /// struct's own doc says why nothing else may travel here.
    ///
    /// Written only by [`super::SessionStore::create`], from
    /// [`NewSession::entitlement`].
    pub entitlement: Option<String>,
}

impl SessionRecord {
    /// Which of the four categories a session list has to separate.
    ///
    /// A stopped session counts as resumable only when a native identifier was
    /// recorded, because that identifier is the entire mechanism by which a
    /// harness is asked to continue rather than start fresh. Without one there
    /// is nothing to resume *to*, so it is reported as closed instead — better
    /// than offering the user a resume that could only ever produce a blank
    /// session wearing an old session's name.
    pub fn disposition(&self) -> SessionDisposition {
        // Every variant is listed and there is no `_` arm, so adding a
        // lifecycle state is a compile error here rather than a silent
        // classification. An earlier version led with `lifecycle if
        // lifecycle.is_live()`; a guarded arm does not count towards
        // exhaustiveness, so it needed a wildcard, and a new variant would
        // have quietly become `Active`.
        match self.lifecycle {
            SessionLifecycle::Starting
            | SessionLifecycle::Running
            | SessionLifecycle::Idle
            | SessionLifecycle::WaitingForUser => SessionDisposition::Active,
            SessionLifecycle::Failed => SessionDisposition::Failed,
            SessionLifecycle::Stopped if self.native_session_id.is_some() => {
                SessionDisposition::Resumable
            }
            SessionLifecycle::Stopped | SessionLifecycle::Closed => SessionDisposition::Closed,
        }
    }
}

/// What a caller supplies to start tracking a session.
///
/// There is no field for a credential, a token, or a provider key, and there
/// is no column for one either. Provider secrets belong in the operating
/// system's own secret storage; the project database is checked into nothing
/// and backed up casually, so it must never become a place a secret can end
/// up by accident.
#[derive(Debug, Clone)]
pub struct NewSession {
    pub harness: String,
    pub role: SessionRole,
    pub presentation: SessionPresentation,
    /// See [`SessionRecord::presentation_ref`]. Only meaningful with
    /// [`SessionPresentation::External`]; stored as given either way.
    pub presentation_ref: Option<String>,
    /// Usually `None`: most harnesses only reveal an identifier once they are
    /// running.
    pub native_session_id: Option<String>,
    /// The launch profile this session is starting under, by name. See
    /// [`SessionRecord::launch_profile`] for what `None` means.
    pub launch_profile: Option<String>,
    /// The resolved backend resource, as
    /// [`crate::profile::BackendResource::slug`]. See
    /// [`SessionRecord::backend_resource`] for what `None` means.
    pub backend_resource: Option<String>,
    /// The model Glasshouse assigned. See [`SessionRecord::model`].
    pub model: Option<AssignedModel>,
    /// The pairing class this session's harness and model fall into.
    pub pairing_class: Option<SessionPairingClass>,
    /// The wire protocol its route speaks.
    pub protocol: Option<SessionProtocol>,
    /// The response profile it starts under.
    pub response_profile: Option<ResponseProfile>,
    /// Which mechanism carried that profile.
    pub response_mechanism: Option<ResponseMechanism>,
    /// The session this one is being bootstrapped from, if this launch is a
    /// `--from-checkpoint` handoff. See [`SessionRecord::source_session_id`].
    pub source_session_id: Option<SessionId>,
    /// The entitlement that will serve this session, by name. See
    /// [`SessionRecord::entitlement`] for what `None` means and for why a
    /// name is the only thing that may go here.
    pub entitlement: Option<String>,
}

impl NewSession {
    /// A normal embedded session, which is what starting a harness from the
    /// TUI produces.
    pub fn embedded(harness: impl Into<String>) -> Self {
        Self {
            harness: harness.into(),
            role: SessionRole::Normal,
            presentation: SessionPresentation::Embedded,
            presentation_ref: None,
            native_session_id: None,
            launch_profile: None,
            backend_resource: None,
            model: None,
            pairing_class: None,
            protocol: None,
            response_profile: None,
            response_mechanism: None,
            source_session_id: None,
            entitlement: None,
        }
    }

    pub fn with_role(mut self, role: SessionRole) -> Self {
        self.role = role;
        self
    }

    pub fn with_presentation(mut self, presentation: SessionPresentation) -> Self {
        self.presentation = presentation;
        self
    }

    /// Record where an external session is presented. See
    /// [`SessionRecord::presentation_ref`].
    pub fn with_presentation_ref(mut self, presentation_ref: Option<String>) -> Self {
        self.presentation_ref = presentation_ref;
        self
    }

    /// Record the harness's native identifier from the start.
    ///
    /// Only for a harness that lets Glasshouse *assign* one: the identifier
    /// is then known before the process exists, so a session that dies during
    /// startup still has one, and nothing has to be discovered afterwards.
    pub fn with_native_session_id(mut self, native: Option<String>) -> Self {
        self.native_session_id = native;
        self
    }

    /// Record which launch profile this session is starting under.
    pub fn with_launch_profile(mut self, launch_profile: Option<String>) -> Self {
        self.launch_profile = launch_profile;
        self
    }

    /// Record the resolved backend resource this session is starting with.
    pub fn with_backend_resource(mut self, backend_resource: Option<String>) -> Self {
        self.backend_resource = backend_resource;
        self
    }

    /// Record the model Glasshouse assigned.
    ///
    /// One setter per fact, and each takes exactly the type of the fact it
    /// sets — the shape [`crate::profile::response::ResponseProfile`]'s five
    /// axes already use, and for the same reason. There is deliberately no
    /// constructor that fills several of these from one value: the phase's
    /// second architectural requirement says they stay separately
    /// represented, and a `with_pairing(...)` that set the class, the model
    /// and the protocol together would be the collapse wearing a builder's
    /// clothes.
    pub fn with_model(mut self, model: Option<AssignedModel>) -> Self {
        self.model = model;
        self
    }

    /// Record the pairing class. Never derived from anything else here.
    pub fn with_pairing_class(mut self, pairing_class: Option<SessionPairingClass>) -> Self {
        self.pairing_class = pairing_class;
        self
    }

    /// Record the wire protocol this session's route speaks.
    pub fn with_protocol(mut self, protocol: Option<SessionProtocol>) -> Self {
        self.protocol = protocol;
        self
    }

    /// Record the response profile this session starts under.
    pub fn with_response_profile(mut self, response_profile: Option<ResponseProfile>) -> Self {
        self.response_profile = response_profile;
        self
    }

    /// Record which mechanism carried that profile.
    pub fn with_response_mechanism(
        mut self,
        response_mechanism: Option<ResponseMechanism>,
    ) -> Self {
        self.response_mechanism = response_mechanism;
        self
    }

    /// Record the session this one was bootstrapped from, if this launch is
    /// a `--from-checkpoint` handoff. See
    /// [`SessionRecord::source_session_id`].
    pub fn with_source_session(mut self, source_session_id: Option<SessionId>) -> Self {
        self.source_session_id = source_session_id;
        self
    }

    /// Record which entitlement will serve this session, by name. See
    /// [`SessionRecord::entitlement`].
    pub fn with_entitlement(mut self, entitlement: Option<String>) -> Self {
        self.entitlement = entitlement;
        self
    }
}

/// Everything a resume needs, once the record has been proven to belong here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumableSession {
    pub id: SessionId,
    pub harness: String,
    /// Never `None`: a record without one is refused as not resumable.
    pub native_session_id: String,
}

/// What this project last recorded about a session's process.
///
/// Read separately from [`SessionRecord`] — see
/// [`super::SessionStore::supervision_of`] for why the two are not one type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SupervisionRecord {
    /// The process the session was started in, or `None` when the build that
    /// wrote the row recorded none.
    pub identity: Option<ProcessIdentity>,
    /// What supervision last concluded about that process.
    pub supervision: Option<Supervision>,
    /// Why it concluded that, in a sentence meant for a person.
    pub reason: Option<String>,
}
