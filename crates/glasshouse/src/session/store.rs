//! Glasshouse's own record of the sessions in one project.
//!
//! This is deliberately *not* a view over a harness's session files. Claude
//! Code, Codex, and the rest each keep their own history in their own format
//! in their own directory, and Glasshouse neither parses nor owns those files.
//! What it keeps here is the metadata it needs to list, resume, and reason
//! about sessions: which harness, when it started, when it was last active,
//! what role it plays, where it is presented, and what state it is in. The
//! harness's own identifier is recorded when it is known, as a nullable
//! reference — so a session survives in this table whether or not the harness
//! kept anything, and clearing a harness's history never silently deletes
//! Glasshouse's record of what happened.
//!
//! # Project isolation
//!
//! Every row carries the project identifier, and it is enforced in two places
//! on purpose:
//!
//! - **Structurally**, by SQLite triggers created in migration 2, which abort
//!   any insert or update whose `project_id` is not the identifier bound in
//!   `project_metadata`. No query in this module — or any future one — has to
//!   remember to filter, because a foreign row cannot be written at all.
//! - **At the resume boundary**, by [`SessionStore::open_for_resume`], which
//!   compares the stored identifier against the active project before handing
//!   back anything a caller could act on.
//!
//! The second check is not redundant with the first. The trigger governs what
//! this database will accept from now on; the resume check governs what
//! Glasshouse will *act on*, including rows that predate a guard, arrived
//! through a restored backup, or were written by a build whose triggers
//! differed. A resume is the one operation that takes a stored identity and
//! turns it back into a running process, so it verifies rather than assumes.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, Row};

use crate::database::PROJECT_ID_KEY;
use crate::profile::response::{
    AnswerFormat, Audience, Dimension, EvidenceDetail, Narration, ResponseProfile, Verbosity,
};
use crate::routing::AssignedModel;

use super::supervision::{self, ProcessIdentity, Supervision, SupervisionRefusal};

/// A Glasshouse session identifier.
///
/// Distinct from any harness's native identifier, which is recorded
/// separately: Glasshouse names its own sessions so that a session remains
/// identifiable before a harness has produced an identifier, and after the
/// harness's own history is gone.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(String);

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
/// enforced by [`SessionStore::rename`]'s SQL naming one column; this type is
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
const MAX_SESSION_NAME: usize = 64;

/// The longest purpose a session may be tagged with.
const MAX_SESSION_PURPOSE: usize = 32;

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
fn encode_response_profile(profile: &ResponseProfile) -> String {
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
fn decode_response_profile(value: &str) -> Option<ResponseProfile> {
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
fn encode_assigned_model(model: &AssignedModel) -> String {
    match model {
        AssignedModel::Named(id) => format!("named:{id}"),
        AssignedModel::HarnessDefault => "harness-default".to_owned(),
    }
}

fn decode_assigned_model(value: &str) -> Option<AssignedModel> {
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

            fn from_str(value: &str) -> Option<Self> {
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
    /// Written only by [`SessionStore::record_observed_compaction`], from the
    /// one production site that can tell a compaction is coming.
    pub observed_compactions: Option<i64>,
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

/// The four prompt-cache states map line 1162 requires — *"at least hot,
/// warm, cold, or unknown"*.
///
/// Never constructed directly outside this module: the only way to obtain one
/// is through [`AdvisoryCacheState`], which is line 1163's requirement made
/// structural rather than written in a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheState {
    /// A provider-side cached prefix is likely to still exist.
    Hot,
    /// One may exist. No provider in scope guarantees it this far out.
    Warm,
    /// Every published cache lifetime this project knows of has passed.
    Cold,
    /// The question could not be answered from what is recorded.
    Unknown,
}

impl CacheState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Warm => "warm",
            Self::Cold => "cold",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for CacheState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// A prompt-cache state Glasshouse **estimated** — map line 1163's *"treat
/// cache-state estimates as advisory when the provider does not expose
/// authoritative cache telemetry."*
///
/// # Why this is a wrapper and not a comment on [`CacheState`]
///
/// The line is a requirement about how the value may be *used*, and a comment
/// is not a mechanism. This type's field is private and its only constructors
/// are [`AdvisoryCacheState::estimate`] and [`AdvisoryCacheState::unknown`],
/// so no code outside this module can produce an `AdvisoryCacheState::Hot`
/// from an authority it claims to have. There is no authoritative counterpart
/// type, and there is no `From<CacheState>`: every cache state in this crate
/// arrives wrapped in the word "advisory", in every signature that carries
/// one. That is the whole of line 1163.
///
/// # What the estimate is made of, and what it is not
///
/// Elapsed time since the session's last recorded activity, and nothing else.
/// Glasshouse observes neither a provider cache's presence nor its lifetime —
/// [`crate::routing::session::prompt_cache_state`] says so in its own
/// evidence string, and `crate::config::pairing`'s warm-session window says
/// provider caches "expire in minutes". So this is a decay curve over a
/// published TTL, not a reading, and it is labelled as one.
///
/// **It is deliberately not a function of resumability** — map line 1161,
/// *"independently from session resumability."* Resumability is
/// [`SessionRecord::disposition`], which is decided by `lifecycle` and
/// whether a native identifier was recorded; neither is an input here. A
/// closed session with no native identifier that was active a moment ago is
/// [`CacheState::Hot`] and not resumable at all, and a resumable session idle
/// since yesterday is [`CacheState::Cold`]. The independence is structural,
/// because the inputs do not overlap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdvisoryCacheState(CacheState);

/// How long a provider-side cached prefix is likely to survive, in seconds.
///
/// Five minutes is the shortest published default among the providers in
/// scope, and the one `crate::config::pairing`'s own note is about when it
/// says such caches "expire in minutes". Inside it, a cached prefix plausibly
/// still exists.
const HOT_PROMPT_CACHE_SECONDS: i64 = 5 * 60;

/// How long one might survive, in seconds.
///
/// One hour is the longest extended cache lifetime any provider in scope
/// offers, and it is offered as an option rather than a default. Between
/// [`HOT_PROMPT_CACHE_SECONDS`] and this, "warm" is the honest word: not the
/// default lifetime, not past every lifetime.
///
/// **Both numbers are reasoning, not measurement**, exactly like the warm
/// session window they sit beside. The measurement that would change them is
/// a provider that reports a cache hit; none does, which is the reason this
/// whole type is advisory.
const WARM_PROMPT_CACHE_SECONDS: i64 = 60 * 60;

impl AdvisoryCacheState {
    /// Estimate from how long a session has been idle.
    ///
    /// `now` before `last_activity_at` yields [`CacheState::Unknown`] rather
    /// than a clamp to zero. A clock that steps backwards is real — migration
    /// 14's own doc comment is about exactly that case — and reporting a
    /// session as `Hot` because the clock moved would be inventing the one
    /// answer this type is least entitled to give.
    pub fn estimate(now: i64, last_activity_at: i64) -> Self {
        let Some(idle_seconds) = now.checked_sub(last_activity_at) else {
            return Self(CacheState::Unknown);
        };
        if idle_seconds < 0 {
            return Self(CacheState::Unknown);
        }
        Self(if idle_seconds <= HOT_PROMPT_CACHE_SECONDS {
            CacheState::Hot
        } else if idle_seconds <= WARM_PROMPT_CACHE_SECONDS {
            CacheState::Warm
        } else {
            CacheState::Cold
        })
    }

    /// An estimate that declines to guess.
    pub fn unknown() -> Self {
        Self(CacheState::Unknown)
    }

    /// The estimated state, which is all this type has ever held.
    pub fn state(self) -> CacheState {
        self.0
    }
}

impl fmt::Display for AdvisoryCacheState {
    /// Prints the word "estimated" beside the state, so that a value reaching
    /// a user through a listing carries line 1163 with it rather than relying
    /// on the reader knowing the type.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (estimated)", self.0)
    }
}

/// Whether a session has a portable checkpoint that still describes where it
/// is — map line 1164.
///
/// # "Recent" is measured against the session, not against the clock
///
/// A wall-clock window would need a threshold nobody could defend: a
/// checkpoint five minutes old is stale if the session did an hour of work in
/// between, and one from yesterday is current if the session has not moved
/// since. So the comparison is `checkpoints.created_at` against the session's
/// own `last_activity_at`, and the answer is a fact about the data rather
/// than a tuning knob.
///
/// Both columns are whole seconds, so a checkpoint written in the same second
/// as the last recorded activity counts as [`CheckpointRecency::Current`] —
/// the tie goes to the checkpoint, because within one second the checkpoint
/// is at least as new as the activity and reporting it stale would be the
/// answer that costs a user a checkpoint they have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointRecency {
    /// Nothing has been recorded as happening in this session since this
    /// checkpoint was written. Seconds since the Unix epoch.
    Current(i64),
    /// A checkpoint exists and the session has recorded activity after it.
    Stale(i64),
    /// No checkpoint has ever been stored for this session.
    Never,
}

impl CheckpointRecency {
    /// Line 1164's question in one word.
    pub fn is_current(self) -> bool {
        matches!(self, Self::Current(_))
    }

    /// When the newest checkpoint was written, if there is one.
    pub fn stored_at(self) -> Option<i64> {
        match self {
            Self::Current(at) | Self::Stale(at) => Some(at),
            Self::Never => None,
        }
    }

    /// A bare word, with no timestamp. `Never` prints as `"never"`, not a
    /// date and not `"stale"` — the two readings that would make "no
    /// checkpoint exists" indistinguishable from "one exists and is old".
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current(_) => "current",
            Self::Stale(_) => "stale",
            Self::Never => "never",
        }
    }
}

impl fmt::Display for CheckpointRecency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// A lightweight flag for whether a session is still working on the task it
/// started — map line 1165.
///
/// # What it counts, and why that is the honest signal available
///
/// Completed task boundaries this session has crossed, read from its own
/// `turn_ended` rows in the project event log. `main`'s hook path treats
/// `TurnEnded { Completed }` as *the* moment a harness says a task finished —
/// it is what triggers memory extraction and an automatic checkpoint — so the
/// count is Glasshouse's own record of the boundaries it acted on, not a new
/// interpretation of anything.
///
/// # What it deliberately is not
///
/// It says nothing about what the tasks **were**. Phase 36's affinity score
/// wants same-task work; `crate::routing::session::session_affinity` records
/// that no producer for task *identity* exists in this build, and this flag
/// does not become one — two consecutive turns on one feature are
/// indistinguishable here from two on unrelated ones. Comparing tasks would
/// mean storing what the task is, and a session record must never hold
/// transcript content. What this does give a router is the difference between
/// a session whose whole context is one piece of work and a session carrying
/// seventeen finished ones, which is a real distinction it could not draw at
/// all before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskContinuity {
    /// The event log holds nothing at all for this session, so nothing has
    /// been observed about its turns — a harness that reports no events, or a
    /// session that has not run yet. Never confused with `OneTask`: a session
    /// nobody watched is not a session seen doing one thing.
    Unknown,
    /// Work has been observed and no completed task boundary among it.
    /// Everything this session holds belongs to the one piece of work it
    /// started.
    OneTask,
    /// How many completed task boundaries have been observed. At one or more,
    /// the task the session began is finished, and its context spans more
    /// than whatever it is doing now.
    BoundariesCrossed(i64),
}

impl fmt::Display for TaskContinuity {
    /// `Unknown` prints as `"unknown"`, never as `"one task"` — a harness
    /// that has reported nothing is not a session seen doing one thing, and
    /// this rendering must not read as a signal either way. See this type's
    /// own doc comment.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => f.pad("unknown"),
            Self::OneTask => f.pad("one task"),
            Self::BoundariesCrossed(1) => f.pad("1 task completed"),
            Self::BoundariesCrossed(n) => write!(f, "{n} tasks completed"),
        }
    }
}

/// What Glasshouse can say about one session's context — Phase 30, read
/// together so that a caller cannot assemble half of it.
///
/// # Line 1158 is absent from this struct on purpose
///
/// *"Track an estimated context-size value for a session when the harness
/// exposes enough information"* — no harness exposes it. The hook path is the
/// only channel a harness reports through, it carries an event name and
/// nothing else, and its payload is drained into `io::sink()` unread by
/// `main`'s own hook handler. The one place in this schema with token counts,
/// `routing_observations`, has them permanently NULL: its module documentation
/// states they are "not supplied", because the only producer is the gateway
/// and the gateway never parses a response body. `HarnessTelemetry`, the
/// harness-side telemetry seam, carries a plan name and nothing more.
///
/// A field here would therefore have to be estimated from something that is
/// not a context size — message counts, elapsed turns — and a future router
/// would read it as telemetry. There is no field, and this paragraph is the
/// record of why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionContext {
    pub session: SessionId,
    /// Line 1159. `None` is *"nobody was counting"*, never zero — see
    /// [`SessionRecord::observed_compactions`].
    pub observed_compactions: Option<i64>,
    /// Line 1160, and it is `sessions.last_activity_at` itself rather than a
    /// second column meaning almost the same thing. Seconds since the Unix
    /// epoch.
    ///
    /// The single `UPDATE` that moves a session's lifecycle stamps it, and
    /// `main`'s hook handler is what calls that on every translated harness
    /// event — so `UserPromptSubmit` (a request) and `Stop` (a turn ending)
    /// both move it, which is exactly the pair the line names.
    pub last_activity_at: i64,
    /// Lines 1161, 1162 and 1163.
    pub prompt_cache: AdvisoryCacheState,
    /// Line 1164.
    pub checkpoint: CheckpointRecency,
    /// Line 1165.
    pub task_continuity: TaskContinuity,
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
}

/// Format 32 hex characters as an RFC 4122 version-4 UUID.
///
/// Six of the 128 bits are overwritten — four for the version, two for the
/// variant — which is what makes the result *valid* rather than merely
/// UUID-shaped, and leaves 122 random bits. A strict validator rejects an
/// 8-4-4-4-12 string whose version nibble is not `4`, and Glasshouse cannot
/// tell in advance which harnesses validate strictly.
///
/// Panics if `hex` is not exactly 32 hex characters; its only caller is the
/// SQL above, which cannot produce anything else.
fn uuid_v4_from_hex(hex: &str) -> String {
    assert_eq!(hex.len(), 32, "a 16-byte blob is 32 hex characters");
    let mut chars: Vec<char> = hex.chars().collect();
    // Version 4.
    chars[12] = '4';
    // Variant: the top two bits are `10`, so the nibble is one of 8, 9, a, b.
    chars[16] = match chars[16] {
        '0' | '4' | '8' | 'c' => '8',
        '1' | '5' | '9' | 'd' => '9',
        '2' | '6' | 'a' | 'e' => 'a',
        _ => 'b',
    };
    let s: String = chars.into_iter().collect();
    format!(
        "{}-{}-{}-{}-{}",
        &s[0..8],
        &s[8..12],
        &s[12..16],
        &s[16..20],
        &s[20..32]
    )
}

/// Everything a resume needs, once the record has been proven to belong here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumableSession {
    pub id: SessionId,
    pub harness: String,
    /// Never `None`: a record without one is refused as not resumable.
    pub native_session_id: String,
}

/// Whether a lifecycle change counts as the session having done something.
///
/// A marker rather than a `bool`, because the two call sites that differ are
/// three lines apart and `false` at a call site says nothing about what it
/// means. See [`SessionStore::write_lifecycle_locked`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Activity {
    Yes,
    No,
}

/// Whether this write is Glasshouse resuming a session, and may therefore
/// move a finished record back to a live state.
///
/// # The asymmetry this type exists to express
///
/// *"A finished session stays finished"* was written for one hazard, and it is
/// a real one: hook processes are separate processes, and a slow one can
/// deliver its event after the harness it belongs to has exited. Applying it
/// would resurrect a stopped session in the records.
///
/// A genuine resume is not that case, and until this marker existed the two
/// were indistinguishable — with the consequence that
/// `main.rs::resume_session`'s own *"this session is running again"* write was
/// silently declined along with the zombies, leaving a demonstrably live
/// session reading `stopped` and every hook it went on to send discarded. That
/// was observed against a live Codex, with the resume twenty-nine seconds
/// after the process exit that preceded it.
///
/// **A resume is an act Glasshouse performs; a late hook is an event that
/// merely arrives.** So the authority is a value only the resume boundary can
/// supply, rather than a property of the event or a loosening of
/// [`SessionLifecycle::is_live`] — which is unchanged, and which other callers
/// depend on. [`SessionStore::begin_resume`] is the only constructor of
/// [`Revival::Authorized`] in the crate, and it is unreachable from the hook
/// path: `glasshouse hook` never opens a resume boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Revival {
    /// The default, and what every other caller passes: a finished session
    /// stays finished.
    Forbidden,
    /// Glasshouse is resuming this session itself.
    Authorized,
}

/// What this project last recorded about a session's process.
///
/// Read separately from [`SessionRecord`] — see
/// [`SessionStore::supervision_of`] for why the two are not one type.
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

/// Failures a caller has to distinguish.
#[derive(Debug, thiserror::Error)]
pub enum SessionStoreError {
    #[error("no session `{id}` in this project")]
    NotFound { id: SessionId },
    #[error(
        "session `{id}` belongs to project `{actual}`, not to the active \
         project `{expected}`; refusing to resume another project's session"
    )]
    ForeignProject {
        id: SessionId,
        expected: String,
        actual: String,
    },
    #[error(
        "session `{id}` cannot be resumed because it is {disposition}; only a \
         stopped session with a recorded native session identifier can be \
         continued"
    )]
    NotResumable {
        id: SessionId,
        disposition: &'static str,
    },
    #[error(
        "`{prefix}` matches {} sessions ({}); use more of the identifier",
        .matches.len(),
        .matches.iter().map(SessionId::as_str).collect::<Vec<_>>().join(", ")
    )]
    AmbiguousPrefix {
        prefix: String,
        matches: Vec<SessionId>,
    },
    #[error("`{prefix}` is not a session identifier; identifiers are hexadecimal")]
    MalformedId { prefix: String },
    #[error(transparent)]
    Supervision(#[from] SupervisionRefusal),
    #[error("session `{id}` stored an unrecognized {column} value `{value}`")]
    UnknownValue {
        id: SessionId,
        column: &'static str,
        value: String,
    },
    #[error(
        "`{harness}` is {what}, not a harness; an interactive Glasshouse \
         session is always owned by a real harness, so there is no session \
         to record for one"
    )]
    NotAHarness { harness: String, what: &'static str },
    #[error(
        "`{harness}` is not a harness this build knows; a direct provider or \
         a gateway is a backend a harness talks to, never the owner of a \
         session"
    )]
    UnknownHarness { harness: String },
    #[error("session `{id}` is {lifecycle}, and a live session cannot be closed; stop it first")]
    StillLive {
        id: SessionId,
        lifecycle: SessionLifecycle,
    },
    #[error(transparent)]
    Label(#[from] LabelError),
    #[error("the project database has no project identifier bound")]
    UnboundDatabase,
    #[error("could not {action} in the project database")]
    Sql {
        action: &'static str,
        #[source]
        source: rusqlite::Error,
    },
}

/// Reads the wall clock, in seconds since the Unix epoch.
///
/// Injected rather than called directly so tests can assert on exact
/// timestamps instead of sleeping or accepting a range. Shared ownership
/// rather than a bare `fn` pointer because a useful test clock has to
/// *advance*, which means capturing state.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Seconds since the Unix epoch.
///
/// Saturates rather than panicking on a clock set before 1970: a nonsensical
/// timestamp on one row is a far smaller problem than refusing to record a
/// session at all.
///
/// `pub(crate)` so that everything stamping a project-scoped record reads the
/// same clock: [`crate::checkpoint`] shares it rather than growing a second
/// one that could disagree with this one about what "now" is.
pub(crate) fn system_clock() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

const ALL_COLUMNS: &str = "id, project_id, harness, native_session_id, role, \
                           lifecycle, presentation, created_at, last_activity_at, \
                           launch_profile, backend_resource, model, pairing_class, \
                           protocol, response_profile, response_mechanism, \
                           display_name, purpose, source_session_id, \
                           observed_compactions, presentation_ref";

/// An open project database plus the sessions inside it.
///
/// [`SessionStore`] borrows its connection so that one connection can back
/// several kinds of store as later phases add them. Callers that just want the
/// sessions — the CLI, and eventually the TUI — want something that owns the
/// connection instead, and this is it.
///
/// Opening goes through the crate's own `database::open` like everything
/// else, so the
/// symlink refusal, the read-only refusal, the project-identity check and the
/// migrations all still apply, and the path still comes from the runtime
/// rather than from a caller.
pub struct ProjectSessions {
    conn: Connection,
    project_id: String,
    clock: Clock,
    /// Where this project keeps a session's own files, so a quarantine can
    /// name what is still held. Carried rather than recomputed because
    /// `SessionStore` has no `Runtime` and must not grow one — it is a
    /// database-facing type, and the whole point of the split is that the
    /// paths come from the runtime.
    sessions_root: PathBuf,
}

impl fmt::Debug for ProjectSessions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectSessions")
            .field("project_id", &self.project_id)
            .finish_non_exhaustive()
    }
}

impl ProjectSessions {
    /// Open the active project's database and read its binding.
    pub fn open(runtime: &crate::Runtime) -> anyhow::Result<Self> {
        Self::open_with_clock(runtime, Arc::new(system_clock))
    }

    /// [`ProjectSessions::open`] with the clock replaced.
    ///
    /// # Supervision runs here — Phase 10A's second line
    ///
    /// *"Discover, on start, the sessions this project previously recorded
    /// whose processes are still running."* This is the door: `glasshouse
    /// launch`, `glasshouse resume`, `glasshouse sessions`, `glasshouse hook`
    /// and the interactive shell all open the project's sessions through
    /// here, and every one of them is a "start" in the sense the line means —
    /// a Glasshouse that is about to act on this project's sessions.
    ///
    /// Putting it in the shell alone would have missed the processes this
    /// phase exists because of: nobody was in the shell when they ran away.
    ///
    /// A failure to supervise is not a failure to open. Discovery reads the
    /// operating system, and an operating system that will not answer is a
    /// reason to say less, never a reason to refuse the user their session
    /// list.
    pub fn open_with_clock(runtime: &crate::Runtime, clock: Clock) -> anyhow::Result<Self> {
        let conn = crate::database::open(runtime)?;
        let project_id = SessionStore::with_clock(&conn, Arc::clone(&clock))?
            .project_id()
            .to_owned();
        let sessions = Self {
            conn,
            project_id,
            clock,
            sessions_root: runtime.session_dir(""),
        };
        sessions.supervise();
        Ok(sessions)
    }

    /// Reconcile every recorded live session against the machine, and tell the
    /// user about anything they have to decide.
    ///
    /// Phase 10A's eighth line — *"surface a quarantined session to the user
    /// with what is known about it and what it still holds"* — is the
    /// `eprintln!`. Standard error rather than standard output, because a
    /// script reading `glasshouse sessions` must keep getting the session
    /// list and nothing else; and it is written before any interface claims
    /// the terminal, because this runs at open.
    fn supervise(&self) {
        let store = self.store();
        let identity = supervision::ProcessIdentity::of_this_process();
        let now = (self.clock)();
        let report = match supervision::reconcile(&store, identity.as_ref(), now, &|id| {
            self.session_dir(id)
        }) {
            Ok(report) => report,
            Err(err) => {
                tracing::warn!(error = %err, "could not supervise this project's sessions");
                return;
            }
        };
        if let Some(described) = report.describe() {
            eprint!("{described}");
        }
        for session in report
            .adopted
            .iter()
            .chain(&report.quarantined)
            .chain(&report.lost)
            .chain(&report.never_ready)
        {
            tracing::info!(
                session = %session.id,
                harness = %session.harness,
                supervision = %session.supervision,
                reason = %session.reason,
                "supervision reached a conclusion about a recorded session"
            );
        }
    }

    /// Where Glasshouse keeps one session's own files.
    ///
    /// The same path [`crate::Runtime::session_dir`] produces; derived from
    /// the root captured at open so that this type needs no `Runtime` of its
    /// own.
    pub fn session_dir(&self, id: &SessionId) -> PathBuf {
        self.sessions_root.join(id.as_str())
    }

    /// The sessions in this project.
    pub fn store(&self) -> SessionStore<'_> {
        SessionStore {
            conn: &self.conn,
            project_id: self.project_id.clone(),
            clock: Arc::clone(&self.clock),
            sessions_root: self.sessions_root.clone(),
        }
    }
}

/// Session records for one project.
///
/// Borrows the connection rather than owning it, so the caller keeps control
/// of the database's lifetime and a single connection can back several stores
/// of different kinds as later phases add them.
pub struct SessionStore<'a> {
    conn: &'a Connection,
    project_id: String,
    clock: Clock,
    /// See [`ProjectSessions::sessions_root`]. Empty for a store opened
    /// straight over a connection, which is how the unit tests build one; a
    /// refusal then names the session directory relatively, which is still
    /// true and still useful.
    sessions_root: PathBuf,
}

impl fmt::Debug for SessionStore<'_> {
    /// Hand-written because [`Clock`] is a trait object with no `Debug`.
    /// Prints the project identifier — a hash of the canonical root, not a
    /// secret — and nothing about the connection.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionStore")
            .field("project_id", &self.project_id)
            .finish_non_exhaustive()
    }
}

impl<'a> SessionStore<'a> {
    /// Open the store over a connection produced by `database::open`.
    ///
    /// The project identifier is read from the database's own binding rather
    /// than accepted as an argument. That keeps the store honest about which
    /// project it is serving even if a caller is confused, and it means the
    /// identifier the store writes is by construction the identifier the
    /// triggers compare against.
    pub fn new(conn: &'a Connection) -> Result<Self, SessionStoreError> {
        Self::with_clock(conn, Arc::new(system_clock))
    }

    /// [`SessionStore::new`] with the clock replaced.
    pub fn with_clock(conn: &'a Connection, clock: Clock) -> Result<Self, SessionStoreError> {
        let project_id: Option<String> = conn
            .query_row(
                "SELECT value FROM project_metadata WHERE key = ?1",
                [PROJECT_ID_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| SessionStoreError::Sql {
                action: "read the project identifier",
                source,
            })?;

        Ok(Self {
            project_id: project_id.ok_or(SessionStoreError::UnboundDatabase)?,
            conn,
            clock,
            sessions_root: PathBuf::from("sessions"),
        })
    }

    /// The project every record in this store belongs to.
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// Where Glasshouse keeps one session's own files — see
    /// [`ProjectSessions::session_dir`], which is where the root comes from.
    pub fn session_dir(&self, id: &SessionId) -> PathBuf {
        self.sessions_root.join(id.as_str())
    }

    /// Start tracking a session.
    ///
    /// The identifier is generated by SQLite's own CSPRNG, which avoids a
    /// dependency and — more usefully — avoids the collision risk of anything
    /// derived from the clock, since sessions can be spawned in a burst.
    pub fn create(&self, new: NewSession) -> Result<SessionRecord, SessionStoreError> {
        let now = (self.clock)();
        // Line 646, and it is enforced here because this is the only door.
        // Refusing before an identifier is minted means a refused session
        // leaves nothing behind at all.
        require_owning_harness(&new.harness)?;

        // Phase 10A, first line. Recorded here because `create` is the only
        // door a session record comes through, so no future caller can start
        // a session Glasshouse would later be unable to identify.
        //
        // `None` is a real answer — a platform that will not name its
        // processes gets a session with no identity, and supervision then
        // refuses to conclude anything about it rather than guessing. That is
        // strictly better than a placeholder, which would match every other
        // placeholder on every other machine.
        let identity = supervision::ProcessIdentity::of_this_process();

        // Phase 10A, seventh line. A replacement must not be started while a
        // process nobody can account for still holds the same resources, and
        // the resource a *new* record can collide with is the harness's own
        // conversation. Checked before an identifier is minted, so a refused
        // session leaves nothing behind at all — `require_owning_harness`'s
        // argument, one line up, applied to the other refusal.
        if let Some(native) = new.native_session_id.as_deref() {
            self.refuse_if_quarantined_holds(&new.harness, native)?;
        }

        let id = SessionId(self.generate_id()?);

        let record = SessionRecord {
            id,
            project_id: self.project_id.clone(),
            harness: new.harness,
            native_session_id: new.native_session_id,
            role: new.role,
            lifecycle: SessionLifecycle::Starting,
            presentation: new.presentation,
            created_at: now,
            last_activity_at: now,
            launch_profile: new.launch_profile,
            backend_resource: new.backend_resource,
            model: new.model,
            pairing_class: new.pairing_class,
            protocol: new.protocol,
            response_profile: new.response_profile,
            response_mechanism: new.response_mechanism,
            // Two labels a person applies afterwards, never at creation: a
            // session Glasshouse named itself would be a name nobody chose.
            display_name: None,
            purpose: None,
            source_session_id: new.source_session_id,
            // `Some(0)`, never `None`. This build is counting from here on,
            // and a session it started that has compacted nothing has a
            // *measured* zero — which is the fact migration 16's nullable
            // column exists to keep apart from "nobody was counting". A
            // `None` written here would make the two indistinguishable for
            // every session Glasshouse ever starts, and the column would then
            // be carrying no information at all.
            observed_compactions: Some(0),
            presentation_ref: new.presentation_ref,
        };

        self.conn
            .execute(
                "INSERT INTO sessions (id, project_id, harness, native_session_id, \
                 role, lifecycle, presentation, created_at, last_activity_at, \
                 launch_profile, backend_resource, model, pairing_class, protocol, \
                 response_profile, response_mechanism, process_id, \
                 process_started_at, process_host, supervision, source_session_id, \
                 observed_compactions, presentation_ref) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, \
                 ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
                rusqlite::params![
                    record.id.as_str(),
                    &record.project_id,
                    &record.harness,
                    &record.native_session_id,
                    record.role.as_str(),
                    record.lifecycle.as_str(),
                    record.presentation.as_str(),
                    record.created_at,
                    record.last_activity_at,
                    &record.launch_profile,
                    &record.backend_resource,
                    record.model.as_ref().map(encode_assigned_model),
                    record.pairing_class.map(SessionPairingClass::as_str),
                    record.protocol.map(SessionProtocol::as_str),
                    record
                        .response_profile
                        .as_ref()
                        .map(encode_response_profile),
                    record.response_mechanism.map(ResponseMechanism::as_str),
                    identity.as_ref().map(|identity| identity.pid),
                    identity.as_ref().map(|identity| identity.started_at_ms),
                    identity.as_ref().map(|identity| identity.host.as_str()),
                    // This Glasshouse started it and this Glasshouse is
                    // responsible for it. `owned` is the only conclusion
                    // `create` may reach; every other word in the vocabulary
                    // is something `supervision::reconcile` observed later,
                    // and writing one here would record an observation nobody
                    // made.
                    identity
                        .as_ref()
                        .map(|_| Supervision::Owned)
                        .map(Supervision::as_str),
                    record.source_session_id.as_ref().map(SessionId::as_str),
                    record.observed_compactions,
                    &record.presentation_ref,
                ],
            )
            .map_err(|source| SessionStoreError::Sql {
                action: "record a new session",
                source,
            })?;

        Ok(record)
    }

    /// Mint an identifier for a harness that lets Glasshouse choose one.
    ///
    /// Formatted as an RFC 4122 version-4 UUID because that is what the
    /// harnesses which accept an assigned identifier demand — Claude Code
    /// refuses anything else outright ("Invalid session ID. Must be a valid
    /// UUID"). The randomness is SQLite's, the same source this store already
    /// uses for its own identifiers, so no second generator has to be trusted.
    ///
    /// Deliberately *not* derived from the Glasshouse session identifier.
    /// The two identifier spaces are independent by design — see
    /// [`SessionId`] — and a session's own name must stay meaningful after
    /// the harness's history is gone.
    pub fn new_native_session_id(&self) -> Result<String, SessionStoreError> {
        let hex: String = self
            .conn
            .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
            .map_err(|source| SessionStoreError::Sql {
                action: "generate a native session identifier",
                source,
            })?;
        Ok(uuid_v4_from_hex(&hex))
    }

    fn generate_id(&self) -> Result<String, SessionStoreError> {
        self.conn
            .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
            .map_err(|source| SessionStoreError::Sql {
                action: "generate a session identifier",
                source,
            })
    }

    /// Resolve a whole identifier, or the leading part of one, to exactly one
    /// session.
    ///
    /// A prefix is not a convenience here, it is a requirement: `glasshouse
    /// sessions` prints only the first twelve characters of an identifier, so
    /// the short form is the *only* one a user can copy from the screen. A
    /// resume command that demanded all thirty-two would be unusable with the
    /// identifiers Glasshouse itself shows.
    ///
    /// Ambiguity is refused rather than resolved — resuming the wrong session
    /// is worse than being asked to type four more characters — and the error
    /// names every candidate so the next attempt can succeed.
    ///
    /// Matching is done with `substr`, not `LIKE`: a `%` or `_` typed by the
    /// user would be a wildcard under `LIKE`, and `%` alone would silently
    /// match every session in the project.
    pub fn resolve_id(&self, prefix: &str) -> Result<SessionId, SessionStoreError> {
        let prefix = prefix.trim();
        if prefix.is_empty() || !prefix.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(SessionStoreError::MalformedId {
                prefix: prefix.to_owned(),
            });
        }
        let prefix = prefix.to_ascii_lowercase();

        let mut statement = self
            .conn
            .prepare("SELECT id FROM sessions WHERE substr(id, 1, ?2) = ?1 ORDER BY id")
            .map_err(|source| SessionStoreError::Sql {
                action: "prepare the session lookup",
                source,
            })?;
        let matches: Vec<SessionId> = statement
            .query_map(
                rusqlite::params![&prefix, i64::try_from(prefix.len()).unwrap_or(i64::MAX)],
                |row| row.get::<_, String>(0).map(SessionId),
            )
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            .map_err(|source| SessionStoreError::Sql {
                action: "look a session up by identifier",
                source,
            })?;

        match matches.as_slice() {
            [] => Err(SessionStoreError::NotFound {
                id: SessionId(prefix),
            }),
            [only] => Ok(only.clone()),
            _ => Err(SessionStoreError::AmbiguousPrefix { prefix, matches }),
        }
    }

    /// Look one session up. `Ok(None)` means it is simply not here.
    pub fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, SessionStoreError> {
        self.conn
            .query_row(
                &format!("SELECT {ALL_COLUMNS} FROM sessions WHERE id = ?1"),
                [id.as_str()],
                |row| Ok(read_record(row)),
            )
            .optional()
            .map_err(|source| SessionStoreError::Sql {
                action: "look a session up",
                source,
            })?
            .transpose()
    }

    /// Every session in the project, most recently active first.
    pub fn list(&self) -> Result<Vec<SessionRecord>, SessionStoreError> {
        let mut statement = self
            .conn
            .prepare(&format!(
                "SELECT {ALL_COLUMNS} FROM sessions ORDER BY last_activity_at DESC, id ASC"
            ))
            .map_err(|source| SessionStoreError::Sql {
                action: "prepare the session list",
                source,
            })?;

        let rows = statement
            .query_map([], |row| Ok(read_record(row)))
            .map_err(|source| SessionStoreError::Sql {
                action: "list sessions",
                source,
            })?;

        let mut records = Vec::new();
        for row in rows {
            let record = row.map_err(|source| SessionStoreError::Sql {
                action: "read a session row",
                source,
            })?;
            records.push(record?);
        }
        Ok(records)
    }

    /// Move a session to a new lifecycle state, which also counts as activity.
    ///
    /// # This is the single ordered path — Phase 10A's twelfth line
    ///
    /// Every lifecycle change in the shipped binary arrives here: the launch
    /// path's `note_lifecycle`, the shell's exit handling and its failed-start
    /// handling, and `glasshouse hook` when a harness reports something. They
    /// are **separate operating-system processes**, so nothing in Rust's type
    /// system orders them, and until this method took a transaction they raced
    /// in the classic read-check-write shape:
    ///
    /// 1. a hook process reads `running` and decides `idle`;
    /// 2. the launch process observes the harness exit and writes `stopped`;
    /// 3. the hook process writes `idle`.
    ///
    /// The result is `idle` — a live state for a session whose process is
    /// gone. Neither writer asked for that, which is exactly the interleaving
    /// the line forbids.
    ///
    /// `BEGIN IMMEDIATE` takes SQLite's write lock **before** the read, so the
    /// read and the write are one indivisible step and the second writer's
    /// check runs against what the first writer actually left. The order is
    /// then decided by the lock rather than by which process happened to read
    /// first, and the losing writer sees the winner's state and declines.
    ///
    /// # What it declines
    ///
    /// One rule, and it is [`super::lifecycle::may_apply`]'s: **a session that
    /// has finished may not be moved back to a live state.** It refuses
    /// nothing the shipped binary legitimately does — every real transition is
    /// from a live state — so this is not a new policy, it is the existing
    /// policy moved to where two processes cannot step over it. A declined
    /// change returns the record as it stands rather than an error: the caller
    /// asked for something that is no longer true, which is not its fault and
    /// not a failure.
    pub fn set_lifecycle(
        &self,
        id: &SessionId,
        lifecycle: SessionLifecycle,
    ) -> Result<SessionRecord, SessionStoreError> {
        let action = "update a session's lifecycle";
        self.in_a_write_transaction(action, || {
            self.write_lifecycle_locked(id, lifecycle, Activity::Yes, Revival::Forbidden, action)
        })?;
        self.get(id)?
            .ok_or(SessionStoreError::NotFound { id: id.clone() })
    }

    /// **The only statement in this crate that moves a session's lifecycle.**
    ///
    /// That is what "a single ordered path" means at the level a reader can
    /// check: not that one function is polite about it, but that there is one
    /// `UPDATE` and everything else has to come through it.
    /// `one_statement_moves_a_sessions_lifecycle` fails if a second appears,
    /// because a second writer is a second order and two orders are no order.
    ///
    /// Callers must already hold a write transaction — see
    /// [`SessionStore::in_a_write_transaction`], which is what makes the read
    /// below and the write after it one indivisible step.
    ///
    /// # What it declines
    ///
    /// One rule, and it is [`super::lifecycle::may_apply`]'s: **a session that
    /// has finished may not be moved back to a live state.** It refuses
    /// nothing the shipped binary legitimately does — every real transition is
    /// from a live state — so this is not a new policy, it is the existing
    /// policy moved to where two processes cannot step over it. A declined
    /// change leaves the record as it stands rather than erroring: the caller
    /// asked for something that is no longer true, which is not its fault.
    fn write_lifecycle_locked(
        &self,
        id: &SessionId,
        next: SessionLifecycle,
        activity: Activity,
        revival: Revival,
        action: &'static str,
    ) -> Result<(), SessionStoreError> {
        let current = self.read_lifecycle_locked(id, action)?;

        // A finished session stays finished — unless Glasshouse is the one
        // reopening it. See [`Revival`] for why that is a value the caller
        // supplies rather than something inferred from `next`: every caller
        // but [`SessionStore::begin_resume`] passes `Forbidden`, and the hook
        // path cannot reach the one that does not.
        if revival == Revival::Forbidden && !current.is_live() && next.is_live() {
            return Ok(());
        }

        // Whether the change counts as activity is decided *inside the one
        // statement*, rather than by having two statements. Closing a record
        // is something a person did to it, not something the session did —
        // see `SessionStore::close` — and stamping it would push a finished
        // session back to the top of a list ordered by when it last ran.
        self.conn
            .execute(
                "UPDATE sessions SET lifecycle = ?2, \
                 last_activity_at = CASE WHEN ?4 THEN ?3 ELSE last_activity_at END \
                 WHERE id = ?1",
                rusqlite::params![
                    id.as_str(),
                    next.as_str(),
                    (self.clock)(),
                    activity == Activity::Yes
                ],
            )
            .map_err(|source| SessionStoreError::Sql { action, source })?;
        Ok(())
    }

    /// A session's current lifecycle, read inside the caller's write
    /// transaction so that what it decides cannot be stale by the time it
    /// writes.
    fn read_lifecycle_locked(
        &self,
        id: &SessionId,
        action: &'static str,
    ) -> Result<SessionLifecycle, SessionStoreError> {
        let current: Option<String> = self
            .conn
            .query_row(
                "SELECT lifecycle FROM sessions WHERE id = ?1",
                [id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| SessionStoreError::Sql { action, source })?;
        let Some(current) = current else {
            return Err(SessionStoreError::NotFound { id: id.clone() });
        };
        SessionLifecycle::from_str(&current).ok_or(SessionStoreError::UnknownValue {
            id: id.clone(),
            column: "lifecycle",
            value: current,
        })
    }

    /// Run `body` with SQLite's write lock already held, and end the
    /// transaction on every path out.
    ///
    /// `IMMEDIATE`, not the default `DEFERRED`. A deferred transaction takes
    /// only a read lock until its first write and then has to *upgrade*; if
    /// another connection has committed in between, SQLite refuses the upgrade
    /// rather than waiting, and `busy_timeout` does not help because there is
    /// nothing to wait for — the read is already stale. Taking the write lock
    /// up front turns that failure into a wait, which is what makes several
    /// `glasshouse` processes writing to one session's record safe rather than
    /// merely lucky.
    fn in_a_write_transaction<T>(
        &self,
        action: &'static str,
        body: impl FnOnce() -> Result<T, SessionStoreError>,
    ) -> Result<T, SessionStoreError> {
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|source| SessionStoreError::Sql { action, source })?;
        let outcome = body();
        let ended = if outcome.is_ok() {
            self.conn.execute_batch("COMMIT")
        } else {
            self.conn.execute_batch("ROLLBACK")
        };
        let value = outcome?;
        ended.map_err(|source| SessionStoreError::Sql { action, source })?;
        Ok(value)
    }

    /// Everything supervision recorded about one session's process.
    ///
    /// # Why this is not five more fields on [`SessionRecord`]
    ///
    /// A `SessionRecord` is what a session *is*. Whether the process it was
    /// started in is still running is a fact about the machine right now, it
    /// changes without the record changing, and every caller that wants one
    /// wants it fresh. Folding it into the record would also have made a
    /// session's identity depend on a reading of the operating system, so two
    /// records of the same session taken a second apart would compare unequal.
    ///
    /// Returns [`SupervisionRecord::default`] — no identity, no conclusion —
    /// for a session recorded by a build that stored none. That is a real
    /// answer and callers treat it as one: nothing may be adopted, quarantined
    /// or declared stopped on the strength of an absent identity.
    pub fn supervision_of(&self, id: &SessionId) -> Result<SupervisionRecord, SessionStoreError> {
        let row = self
            .conn
            .query_row(
                "SELECT process_id, process_started_at, process_host, supervision, \
                 supervision_reason FROM sessions WHERE id = ?1",
                [id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|source| SessionStoreError::Sql {
                action: "read a session's supervision",
                source,
            })?;

        let Some((pid, started_at, host, supervision, reason)) = row else {
            return Err(SessionStoreError::NotFound { id: id.clone() });
        };

        // The three identity columns are read together or not at all. A pid
        // without a start time is not an identity — it is exactly what Phase
        // 10A's first line stops Glasshouse trusting — and a start time
        // without a host is a number about a machine that may not be this one.
        // A partially recorded identity therefore reads as no identity, and
        // supervision refuses to conclude anything rather than guessing at the
        // missing part.
        let identity = match (pid, started_at, host) {
            (Some(pid), Some(started_at_ms), Some(host)) => {
                u32::try_from(pid).ok().map(|pid| ProcessIdentity {
                    pid,
                    started_at_ms,
                    host,
                })
            }
            _ => None,
        };

        let supervision = match supervision {
            None => None,
            Some(word) => Some(Supervision::from_str(&word).ok_or_else(|| {
                SessionStoreError::UnknownValue {
                    id: id.clone(),
                    column: "supervision",
                    value: word,
                }
            })?),
        };

        Ok(SupervisionRecord {
            identity,
            supervision,
            reason,
        })
    }

    /// Record what supervision concluded about a session's process, and — when
    /// the conclusion implies one — the lifecycle state that follows from it.
    ///
    /// Both writes go through the same transaction as every other lifecycle
    /// change, for the reason [`SessionStore::set_lifecycle`] gives: a
    /// supervision pass in one `glasshouse` process runs beside a hook in
    /// another, and a conclusion drawn from a read that is already stale is
    /// worse than no conclusion.
    ///
    /// **This never ends anything.** `Lost` is written because the process was
    /// observed to be gone, and `Quarantined` deliberately leaves the
    /// lifecycle alone: a quarantined session is neither stopped nor healthy,
    /// and overwriting its state with either would erase the whole distinction
    /// this phase is about.
    pub fn record_supervision(
        &self,
        id: &SessionId,
        supervision: Supervision,
        reason: &str,
        lifecycle: Option<SessionLifecycle>,
    ) -> Result<SessionRecord, SessionStoreError> {
        let action = "record a supervision conclusion";
        self.in_a_write_transaction(action, || {
            let changed = self
                .conn
                .execute(
                    "UPDATE sessions SET supervision = ?2, supervision_reason = ?3 \
                     WHERE id = ?1",
                    rusqlite::params![id.as_str(), supervision.as_str(), reason],
                )
                .map_err(|source| SessionStoreError::Sql { action, source })?;
            if changed == 0 {
                return Err(SessionStoreError::NotFound { id: id.clone() });
            }
            // Through the same one statement every other lifecycle change goes
            // through, inside the same transaction as the conclusion that
            // implied it — so a supervision pass and a hook in another process
            // cannot each half-apply.
            match lifecycle {
                Some(lifecycle) => self.write_lifecycle_locked(
                    id,
                    lifecycle,
                    Activity::Yes,
                    Revival::Forbidden,
                    action,
                ),
                None => Ok(()),
            }
        })?;
        self.get(id)?
            .ok_or(SessionStoreError::NotFound { id: id.clone() })
    }

    /// Refuse a new session that would take a conversation a quarantined
    /// process still holds — Phase 10A's seventh line.
    ///
    /// Scoped to the harness as well as the identifier, because the unique
    /// index that already exists is scoped that way: two harnesses may
    /// coincidentally spell an identifier the same, and refusing across that
    /// coincidence would refuse a start for no reason.
    fn refuse_if_quarantined_holds(
        &self,
        harness: &str,
        native_session_id: &str,
    ) -> Result<(), SessionStoreError> {
        let held: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM sessions WHERE harness = ?1 AND native_session_id = ?2 \
                 AND supervision = 'quarantined'",
                rusqlite::params![harness, native_session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| SessionStoreError::Sql {
                action: "check whether a quarantined session holds this conversation",
                source,
            })?;
        match held {
            None => Ok(()),
            Some(id) => Err(SupervisionRefusal::Quarantined {
                id: SessionId(id),
                holds: format!("the {harness} conversation `{native_session_id}`"),
                reason: "a process Glasshouse cannot account for was still running when \
                         that session was last examined"
                    .to_owned(),
            }
            .into()),
        }
    }

    /// Record the harness's own identifier, once it is known.
    pub fn set_native_session_id(
        &self,
        id: &SessionId,
        native_session_id: &str,
    ) -> Result<SessionRecord, SessionStoreError> {
        self.update(
            id,
            "UPDATE sessions SET native_session_id = ?2, last_activity_at = ?3 WHERE id = ?1",
            rusqlite::params![id.as_str(), native_session_id, (self.clock)()],
            "record a native session identifier",
        )
    }

    /// Note that something happened in a session, without changing its state.
    pub fn touch(&self, id: &SessionId) -> Result<SessionRecord, SessionStoreError> {
        self.update(
            id,
            "UPDATE sessions SET last_activity_at = ?2 WHERE id = ?1",
            rusqlite::params![id.as_str(), (self.clock)()],
            "record session activity",
        )
    }

    /// Count one compaction a harness said it was about to perform — map
    /// line 1159.
    ///
    /// # Why this is a column and not an event
    ///
    /// `super::lifecycle::precedes_native_compaction` is the observation, and
    /// its own documentation explains why a compaction cannot join
    /// `LIFECYCLE_EVENT_KINDS`: that vocabulary is a SQL `CHECK`, SQLite
    /// cannot widen one in place, and the eleventh value already cost a full
    /// rebuild of the table `memories` references by `seq`. Migration 16 says
    /// the same thing from the schema's side. So the count lives on the
    /// session row, and the event log is left exactly as narrow as it was.
    ///
    /// # `COALESCE`, and what it costs
    ///
    /// A row recorded before migration 16 reads `NULL`, meaning *"nobody was
    /// counting"*. Its first observed compaction moves it to `1` rather than
    /// leaving it unknowable for ever, so from then on the number is a
    /// **lower bound** — compactions before the upgrade were observed by
    /// nothing and cannot be recovered. For a session this build created the
    /// count is exact, because `create` starts it at a measured `0`.
    ///
    /// # It is not activity
    ///
    /// `last_activity_at` is untouched, for `rename`'s reason turned around:
    /// a compaction is the harness reorganising what it holds, not the
    /// session doing work, and stamping it would move a session up a list
    /// ordered by when it last ran on the strength of housekeeping.
    pub fn record_observed_compaction(
        &self,
        id: &SessionId,
    ) -> Result<SessionRecord, SessionStoreError> {
        self.update(
            id,
            "UPDATE sessions \
             SET observed_compactions = COALESCE(observed_compactions, 0) + 1 \
             WHERE id = ?1",
            rusqlite::params![id.as_str()],
            "count an observed compaction",
        )
    }

    /// Everything Phase 30 can say about one session's context, as of now.
    ///
    /// `Ok(None)` for a session this project does not have, exactly as
    /// [`SessionStore::get`] answers.
    ///
    /// # Why one function and not five
    ///
    /// Four of Phase 30's lines are answered by facts that already existed —
    /// the session's own activity stamp, its checkpoints, and its turn
    /// events — and were unreadable together. A caller assembling them itself
    /// would have to know that "recent checkpoint" is a comparison against
    /// `last_activity_at` and that a cache state must never be derived from
    /// resumability; those are the rulings this phase is made of, and they
    /// belong in one place rather than in each caller. See
    /// [`SessionContext`], including its paragraph on the line that is
    /// **not** here.
    ///
    /// # It reads two sibling tables, and never writes them
    ///
    /// `checkpoints` and `lifecycle_events` are read by `project_id` and
    /// `session_id` together, so the project boundary
    /// [`SessionRecord::project_id`] draws is honoured by the query and not
    /// merely by the caller. Nothing here inserts, updates or deletes, and in
    /// `lifecycle_events`' case nothing could: migration 5's triggers
    /// `RAISE(ABORT)` on every write but an insert.
    ///
    /// # Nothing here is stored
    ///
    /// The cache estimate and the checkpoint verdict are computed at the
    /// moment they are asked for, on purpose. A stored `hot` is wrong the
    /// minute after it is written, and a stored copy of
    /// `checkpoints.created_at` would be a second source of truth for a
    /// column one table over — migration 15's objection to copying a token
    /// count, applied to this phase. Only [`SessionRecord::observed_compactions`]
    /// is durable, because a compaction leaves no trace anywhere else.
    pub fn context(&self, id: &SessionId) -> Result<Option<SessionContext>, SessionStoreError> {
        let Some(record) = self.get(id)? else {
            return Ok(None);
        };
        let now = (self.clock)();

        let newest_checkpoint: Option<i64> = self
            .conn
            .query_row(
                "SELECT MAX(created_at) FROM checkpoints \
                 WHERE project_id = ?1 AND session_id = ?2",
                rusqlite::params![&self.project_id, id.as_str()],
                |row| row.get(0),
            )
            .map_err(|source| SessionStoreError::Sql {
                action: "read a session's newest checkpoint",
                source,
            })?;

        // `MAX` over no rows is one row holding NULL, so the aggregate below
        // is read the same way: `COUNT(*)` is `0` and the conditional sum is
        // `0`, and the two together are what separates "no events at all"
        // from "events, no boundaries among them".
        let (observed_events, boundaries): (i64, i64) = self
            .conn
            .query_row(
                "SELECT COUNT(*), \
                        COALESCE(SUM(CASE WHEN kind = 'turn_ended' \
                                           AND turn_outcome = 'completed' \
                                          THEN 1 ELSE 0 END), 0) \
                   FROM lifecycle_events \
                  WHERE project_id = ?1 AND session_id = ?2",
                rusqlite::params![&self.project_id, id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|source| SessionStoreError::Sql {
                action: "count a session's observed task boundaries",
                source,
            })?;

        Ok(Some(SessionContext {
            session: record.id.clone(),
            observed_compactions: record.observed_compactions,
            last_activity_at: record.last_activity_at,
            prompt_cache: AdvisoryCacheState::estimate(now, record.last_activity_at),
            checkpoint: match newest_checkpoint {
                None => CheckpointRecency::Never,
                Some(at) if at >= record.last_activity_at => CheckpointRecency::Current(at),
                Some(at) => CheckpointRecency::Stale(at),
            },
            task_continuity: match (observed_events, boundaries) {
                (0, _) => TaskContinuity::Unknown,
                (_, 0) => TaskContinuity::OneTask,
                (_, crossed) => TaskContinuity::BoundariesCrossed(crossed),
            },
        }))
    }

    /// Give a session a name of the user's own — line 650.
    ///
    /// # The native session identifier is not among the columns named here
    ///
    /// That is the whole of line 650: *"allow the user to rename a session
    /// without changing its native session ID"*. The identifier is what a
    /// harness is asked to continue from, so a rename that touched it would
    /// silently break resume, and nothing about the failure would point back
    /// at the rename. One `SET`, one column, and
    /// `renaming_a_session_leaves_its_native_identifier_alone` reads the
    /// identifier back afterwards rather than merely checking no error was
    /// returned.
    ///
    /// # And `last_activity_at` is not among them either
    ///
    /// Naming a session is something the *user* did, not something the
    /// session did. Stamping it as activity would move a finished session
    /// back to the top of a list ordered by when it last ran, which is the
    /// one question that list exists to answer.
    pub fn rename(
        &self,
        id: &SessionId,
        name: &SessionName,
    ) -> Result<SessionRecord, SessionStoreError> {
        self.update(
            id,
            "UPDATE sessions SET display_name = ?2 WHERE id = ?1",
            rusqlite::params![id.as_str(), name.as_str()],
            "rename a session",
        )
    }

    /// Take a session's name away again, leaving it identified by nothing but
    /// its identifiers.
    pub fn clear_name(&self, id: &SessionId) -> Result<SessionRecord, SessionStoreError> {
        self.update(
            id,
            "UPDATE sessions SET display_name = NULL WHERE id = ?1",
            rusqlite::params![id.as_str()],
            "clear a session's name",
        )
    }

    /// Tag a session with a lightweight purpose — line 651.
    ///
    /// A separate column and a separate type from the display name, so that
    /// tagging cannot rename and renaming cannot tag. Like a rename, it does
    /// not count as session activity.
    pub fn set_purpose(
        &self,
        id: &SessionId,
        purpose: &SessionPurpose,
    ) -> Result<SessionRecord, SessionStoreError> {
        self.update(
            id,
            "UPDATE sessions SET purpose = ?2 WHERE id = ?1",
            rusqlite::params![id.as_str(), purpose.as_str()],
            "tag a session",
        )
    }

    /// Record where a session is presented now — line 760, for a session
    /// that was recorded before it reached the place it is shown: a
    /// continued session picked up inside an external pane.
    ///
    /// Two columns, written together, because they are one fact: a
    /// presentation that names a pane and a pane without a presentation
    /// would each be half an answer. Like a rename, it does not count as
    /// session activity. The reference is stored as given — see
    /// [`SessionRecord::presentation_ref`] for why this module never
    /// interprets it.
    pub fn set_presentation(
        &self,
        id: &SessionId,
        presentation: SessionPresentation,
        presentation_ref: Option<&str>,
    ) -> Result<SessionRecord, SessionStoreError> {
        self.update(
            id,
            "UPDATE sessions SET presentation = ?2, presentation_ref = ?3 WHERE id = ?1",
            rusqlite::params![id.as_str(), presentation.as_str(), presentation_ref],
            "record where a session is presented",
        )
    }

    /// Remove a session's purpose tag.
    pub fn clear_purpose(&self, id: &SessionId) -> Result<SessionRecord, SessionStoreError> {
        self.update(
            id,
            "UPDATE sessions SET purpose = NULL WHERE id = ?1",
            rusqlite::params![id.as_str()],
            "clear a session's purpose",
        )
    }

    /// Retire Glasshouse's record of a session — line 654.
    ///
    /// # What this deliberately does not do
    ///
    /// It writes one column. `native_session_id` is untouched, and so is
    /// every harness file on disk: this module has never parsed or owned
    /// those, and closing a Glasshouse record is not an occasion to start.
    /// Line 654 says the record may be closed *"without deleting the native
    /// provider history unless explicitly requested"*, and nothing here is a
    /// request. `closing_a_session_keeps_the_harnesss_own_history` proves the
    /// history is still there afterwards rather than proving no error came
    /// back.
    ///
    /// # A live session is refused
    ///
    /// Closing is filing a record away, and a record whose process is still
    /// running is not finished being written. Refusing names the state so the
    /// user knows to stop the session first, rather than leaving a `closed`
    /// row that a running harness keeps updating.
    ///
    /// # `last_activity_at` stays put, for [`SessionStore::rename`]'s reason
    ///
    /// When the session last did something is a fact about the session. When
    /// somebody filed it away is a different fact, and this column is not the
    /// place for it.
    pub fn close(&self, id: &SessionId) -> Result<SessionRecord, SessionStoreError> {
        // Through the same ordered path as every other lifecycle change —
        // Phase 10A's twelfth line. The liveness check and the write used to
        // be a read outside a transaction followed by a write, which is the
        // interleaving that line forbids: a hook process moving the session
        // back to `running` in between would leave a `closed` row that a live
        // harness kept updating. Reading under the write lock closes it.
        let action = "close a session record";
        self.in_a_write_transaction(action, || {
            let current = self.read_lifecycle_locked(id, action)?;
            if current.is_live() {
                return Err(SessionStoreError::StillLive {
                    id: id.clone(),
                    lifecycle: current,
                });
            }
            self.write_lifecycle_locked(
                id,
                SessionLifecycle::Closed,
                Activity::No,
                Revival::Forbidden,
                action,
            )
        })?;
        self.get(id)?
            .ok_or(SessionStoreError::NotFound { id: id.clone() })
    }

    fn update(
        &self,
        id: &SessionId,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
        action: &'static str,
    ) -> Result<SessionRecord, SessionStoreError> {
        let changed = self
            .conn
            .execute(sql, params)
            .map_err(|source| SessionStoreError::Sql { action, source })?;
        if changed == 0 {
            return Err(SessionStoreError::NotFound { id: id.clone() });
        }
        self.get(id)?
            .ok_or(SessionStoreError::NotFound { id: id.clone() })
    }

    /// Check that a session may be resumed here, and return what a resume
    /// needs.
    ///
    /// This is the enforcement point for the rule that one Glasshouse instance
    /// never continues another project's session. The stored project
    /// identifier is compared against the active one and a mismatch is an
    /// error, never a filtered-away row: the caller asked about a specific
    /// session and deserves to be told it belongs somewhere else, rather than
    /// being told it does not exist and left to wonder.
    ///
    /// The comparison is not made redundant by migration 2's triggers. Those
    /// decide what may be written; this decides what may be acted upon, and
    /// covers rows that arrived by any route the triggers did not police — a
    /// restored backup, a hand-edited file, a build whose schema predates the
    /// guard.
    pub fn open_for_resume(&self, id: &SessionId) -> Result<ResumableSession, SessionStoreError> {
        let record = self
            .get(id)?
            .ok_or_else(|| SessionStoreError::NotFound { id: id.clone() })?;

        if record.project_id != self.project_id {
            return Err(SessionStoreError::ForeignProject {
                id: id.clone(),
                expected: self.project_id.clone(),
                actual: record.project_id,
            });
        }

        // Phase 10A, lines five and seven, and they are asked *before* the
        // disposition question on purpose.
        //
        // A record whose process is verified still running is refused for
        // being still running, naming the process; a record held by something
        // Glasshouse cannot account for is refused for that, naming what is
        // held. Asking `disposition` first would answer both with *"still
        // running"* or *"closed"* — true of the record, useless about the
        // machine, and in the quarantine case actively misleading, because a
        // quarantined session is neither.
        supervision::guard_start(
            &record,
            &self.supervision_of(&record.id)?,
            supervision::ProcessIdentity::of_this_process().as_ref(),
            &|id| self.session_dir(id),
        )?;

        let disposition = record.disposition();
        if disposition != SessionDisposition::Resumable {
            return Err(SessionStoreError::NotResumable {
                id: id.clone(),
                disposition: match disposition {
                    SessionDisposition::Active => "still running",
                    SessionDisposition::Closed => "closed",
                    SessionDisposition::Failed => "failed",
                    SessionDisposition::Resumable => unreachable!("checked above"),
                },
            });
        }

        Ok(ResumableSession {
            id: record.id,
            harness: record.harness,
            native_session_id: record
                .native_session_id
                .expect("a resumable disposition requires a native session identifier"),
        })
    }

    /// Record that Glasshouse is resuming this session, moving it back to
    /// `Running`.
    ///
    /// # Why this is not `set_lifecycle`
    ///
    /// [`SessionStore::set_lifecycle`] declines to move a finished record back
    /// to a live state, and must keep declining: a hook process outliving its
    /// harness is exactly what that rule is for. But the resume path's own
    /// *"this session is running again"* write went through the same door and
    /// was refused by the same rule, so a session Glasshouse itself had just
    /// reopened kept reading `stopped` — and every hook the resumed harness
    /// then sent was discarded for arriving at a finished session.
    ///
    /// Observed against a live Codex over five compaction trials, with the
    /// resume recorded twenty-nine seconds after the process exit before it,
    /// so nothing about it was a race.
    ///
    /// The two cases are told apart by **who is acting**. A resume is
    /// something Glasshouse does, at a boundary it opened deliberately; a late
    /// hook is an event that merely arrives. So this is a separate operation
    /// carrying `Revival::Authorized`, rather than a widening of
    /// [`SessionLifecycle::is_live`] or of `lifecycle::may_apply` — and once
    /// this has run, a resumed session is live, so `may_apply` believes its
    /// harness again without knowing anything about resumes at all.
    ///
    /// # The disposition is checked again, under the write lock
    ///
    /// Not defence in depth for its own sake. [`SessionStore::open_for_resume`]
    /// reads outside a transaction, so between its answer and this write
    /// another process can close the record, quarantine it, or start it — the
    /// classic read-check-write window this module's
    /// `in_a_write_transaction` exists to shut. Re-asking
    /// [`SessionRecord::disposition`] with the write lock already held makes
    /// the check and the write one indivisible step, which is Phase 10A's
    /// requirement for every lifecycle change and is what makes this one safe
    /// to authorise at all.
    ///
    /// # `Stopped`, `Failed` and `Closed` are three different answers
    ///
    /// Only a **stopped** record with a native identifier is
    /// [`SessionDisposition::Resumable`], and only that one is revived here.
    /// A **failed** session ended badly and reports
    /// [`SessionDisposition::Failed`]; a **closed** one was retired by the
    /// user, and a stopped one with nothing to resume *to* is
    /// [`SessionDisposition::Closed`]. All three are refused, by the same
    /// classification `open_for_resume` refuses them by — one rule read twice
    /// rather than a second rule that could drift from the first.
    ///
    /// # The process identity is re-recorded here, and that is not a detail
    ///
    /// A resume happens in a **new operating-system process**. Making the
    /// record live again while it still named the `glasshouse` that created
    /// the session left every later invocation verifying a process id that
    /// had exited — so `supervision::reconcile` reached [`Verdict::Gone`],
    /// correctly, and wrote `stopped` back over the resume on the very next
    /// command. Observed twice out of two trials against a live Codex, where
    /// the command that undid the resume was the resumed session's own first
    /// hook.
    ///
    /// The two writes are one transaction on purpose. A resumed record is
    /// discoverable by supervision the instant its lifecycle goes live
    /// ([`supervision::discover`] filters on exactly that), so a live
    /// lifecycle and a stale identity must never both be readable, not even
    /// between two statements. Afterwards a resumed row is the same shape a
    /// created one is — live, with the identity of the Glasshouse responsible
    /// for it — and supervision reaches the same conclusions about it for the
    /// same reasons, which is the whole of the repair.
    ///
    /// Nothing about supervision is weakened. A resumed session whose process
    /// is genuinely gone is still found and still recorded `lost`; that is
    /// `a_resumed_session_whose_process_is_gone_is_still_lost` in
    /// `tests/session_supervision.rs`, reached against the identity this
    /// function wrote.
    ///
    /// `None` — a platform that will not name its processes — clears the
    /// columns rather than leaving the old values behind, for
    /// [`SessionStore::create`]'s reason: an unverifiable session is a real
    /// answer that supervision refuses to conclude anything from, and a stale
    /// identity is a wrong one it concludes a great deal from.
    ///
    /// [`Verdict::Gone`]: super::supervision::Verdict::Gone
    pub fn begin_resume(
        &self,
        resumable: &ResumableSession,
    ) -> Result<SessionRecord, SessionStoreError> {
        let id = &resumable.id;
        let action = "record a session resume";
        // Asked before the write lock is taken. It reads the operating system
        // about *this* process, whose answer no other writer can change, and
        // the lock is for ordering writers rather than for holding a syscall.
        let identity = supervision::ProcessIdentity::of_this_process();
        self.in_a_write_transaction(action, || {
            let record = self
                .get(id)?
                .ok_or_else(|| SessionStoreError::NotFound { id: id.clone() })?;
            let disposition = record.disposition();
            if disposition != SessionDisposition::Resumable {
                return Err(SessionStoreError::NotResumable {
                    id: id.clone(),
                    disposition: match disposition {
                        SessionDisposition::Active => "still running",
                        SessionDisposition::Closed => "closed",
                        SessionDisposition::Failed => "failed",
                        SessionDisposition::Resumable => unreachable!("checked above"),
                    },
                });
            }

            // The other half of what `open_for_resume` already decided,
            // re-asked here for the same reason the disposition is: it read
            // outside a transaction, and a quarantine recorded in between
            // would otherwise be *overwritten* by the identity write below —
            // turning a session Glasshouse may not touch into one it owns.
            // Only the quarantine arm can fire, because a resumable record is
            // stopped and the duplicate refusal is about a live one; it is
            // still asked through `guard_start` so that a caller cannot check
            // one refusal and forget the other, which is what that function
            // exists for.
            supervision::guard_start(
                &record,
                &self.supervision_of(id)?,
                identity.as_ref(),
                &|id| self.session_dir(id),
            )?;

            self.write_identity_locked(id, identity.as_ref(), action)?;
            self.write_lifecycle_locked(
                id,
                SessionLifecycle::Running,
                Activity::Yes,
                Revival::Authorized,
                action,
            )
        })?;
        self.get(id)?
            .ok_or(SessionStoreError::NotFound { id: id.clone() })
    }

    /// Record the process a session is running in, replacing whatever was
    /// recorded before it.
    ///
    /// The write [`SessionStore::create`] makes as part of its `INSERT`, as an
    /// `UPDATE`, so that the other way a session becomes live can make it too.
    /// Callers must already hold a write transaction — the identity and the
    /// lifecycle it belongs to are one change, and a reader that could see
    /// half of it is the defect this exists to close.
    ///
    /// `supervision` is set to [`Supervision::Owned`] beside the identity, and
    /// the reason cleared, for the reason `create` gives: this Glasshouse is
    /// responsible for this process, and it is the only conclusion a writer
    /// that is not [`super::supervision::reconcile`] may reach. Leaving the
    /// previous conclusion would leave a sentence like *"its process (65061)
    /// is no longer running"* printed beside a session that is running, about
    /// a process the row no longer names.
    ///
    /// A `None` identity clears all four columns rather than half of them —
    /// [`SessionStore::supervision_of`] reads the three identity columns
    /// together or not at all, and a partially cleared row would be read as an
    /// identity built from whichever parts survived.
    fn write_identity_locked(
        &self,
        id: &SessionId,
        identity: Option<&ProcessIdentity>,
        action: &'static str,
    ) -> Result<(), SessionStoreError> {
        let changed = self
            .conn
            .execute(
                "UPDATE sessions SET process_id = ?2, process_started_at = ?3, \
                 process_host = ?4, supervision = ?5, supervision_reason = NULL \
                 WHERE id = ?1",
                rusqlite::params![
                    id.as_str(),
                    identity.map(|identity| identity.pid),
                    identity.map(|identity| identity.started_at_ms),
                    identity.map(|identity| identity.host.as_str()),
                    identity.map(|_| Supervision::Owned.as_str()),
                ],
            )
            .map_err(|source| SessionStoreError::Sql { action, source })?;
        if changed == 0 {
            return Err(SessionStoreError::NotFound { id: id.clone() });
        }
        Ok(())
    }
}

/// Build a record from a row, turning an unrecognized enum string into a
/// typed error rather than a panic or a silent default.
fn read_record(row: &Row<'_>) -> Result<SessionRecord, SessionStoreError> {
    let id = SessionId(row.get_unwrap::<_, String>(0));

    fn decode<T>(
        id: &SessionId,
        column: &'static str,
        value: String,
        parsed: Option<T>,
    ) -> Result<T, SessionStoreError> {
        parsed.ok_or_else(|| SessionStoreError::UnknownValue {
            id: id.clone(),
            column,
            value,
        })
    }

    let role_text: String = row.get_unwrap(4);
    let lifecycle_text: String = row.get_unwrap(5);
    let presentation_text: String = row.get_unwrap(6);

    let role = decode(
        &id,
        "role",
        role_text.clone(),
        SessionRole::from_str(&role_text),
    )?;
    let lifecycle = decode(
        &id,
        "lifecycle",
        lifecycle_text.clone(),
        SessionLifecycle::from_str(&lifecycle_text),
    )?;
    let presentation = decode(
        &id,
        "presentation",
        presentation_text.clone(),
        SessionPresentation::from_str(&presentation_text),
    )?;

    // Each of these decodes through its own function and reports an
    // unrecognised value by name rather than defaulting. A row written by a
    // newer build is then a legible error naming the column and the value,
    // which is what a person needs; a silent default would report a session
    // as having run under something it did not.
    let model = optional(&id, "model", row.get_unwrap(11), decode_assigned_model)?;
    let pairing_class = optional(
        &id,
        "pairing_class",
        row.get_unwrap(12),
        SessionPairingClass::from_str,
    )?;
    let protocol = optional(
        &id,
        "protocol",
        row.get_unwrap(13),
        SessionProtocol::from_str,
    )?;
    let response_profile = optional(
        &id,
        "response_profile",
        row.get_unwrap(14),
        decode_response_profile,
    )?;
    let response_mechanism = optional(
        &id,
        "response_mechanism",
        row.get_unwrap(15),
        ResponseMechanism::from_str,
    )?;
    // The two labels are stored as the person typed them, so a stored value
    // that no longer parses — a bound tightened in a later release — is
    // reported rather than shown truncated.
    let display_name = optional(&id, "display_name", row.get_unwrap(16), |value| {
        SessionName::parse(value).ok()
    })?;
    let purpose = optional(&id, "purpose", row.get_unwrap(17), |value| {
        SessionPurpose::parse(value).ok()
    })?;
    // Never decoded, only wrapped: an identifier does not fail to parse the
    // way an enum's stored word can.
    let source_session_id: Option<String> = row.get_unwrap(18);
    let source_session_id = source_session_id.map(SessionId);
    // Never decoded either, and deliberately read as an `Option` rather than
    // with a fallback: NULL is a fact this column carries — see
    // [`SessionRecord::observed_compactions`] — and `unwrap_or(0)` here would
    // erase it at the one point every reader in the crate passes through.
    let observed_compactions: Option<i64> = row.get_unwrap(19);
    // Opaque, like `source_session_id`: a reference the presenting backend
    // understands, stored and returned as given. Validating its shape here
    // would teach this module what a backend's references look like, which
    // is the one thing line 762 says it must not learn.
    let presentation_ref: Option<String> = row.get_unwrap(20);

    Ok(SessionRecord {
        id,
        project_id: row.get_unwrap(1),
        harness: row.get_unwrap(2),
        native_session_id: row.get_unwrap(3),
        role,
        lifecycle,
        presentation,
        created_at: row.get_unwrap(7),
        last_activity_at: row.get_unwrap(8),
        launch_profile: row.get_unwrap(9),
        backend_resource: row.get_unwrap(10),
        model,
        pairing_class,
        protocol,
        response_profile,
        response_mechanism,
        display_name,
        purpose,
        source_session_id,
        observed_compactions,
        presentation_ref,
    })
}

/// Decode a nullable column, keeping NULL and "a value this build cannot read"
/// apart.
///
/// NULL is `Ok(None)` — the build that wrote the row recorded nothing. A
/// present value that does not decode is an error naming the column, never
/// `None`, because the two mean opposite things and a caller that saw `None`
/// for both would report a missing fact as a deliberate absence.
fn optional<T>(
    id: &SessionId,
    column: &'static str,
    stored: Option<String>,
    decode: impl FnOnce(&str) -> Option<T>,
) -> Result<Option<T>, SessionStoreError> {
    let Some(stored) = stored else {
        return Ok(None);
    };
    match decode(&stored) {
        Some(value) => Ok(Some(value)),
        None => Err(SessionStoreError::UnknownValue {
            id: id.clone(),
            column,
            value: stored,
        }),
    }
}

/// Refuse a session whose owner is not a real harness — line 646.
///
/// # The catalogue is asked, not held
///
/// The map's first fixed architectural requirement for this phase is that
/// *every interactive Glasshouse session is owned by a real harness*, and
/// line 646 names the failure it guards: a direct API provider or a gateway
/// appearing in this table as though it were one.
///
/// The question is answered by [`super::owning_harness`], one module up,
/// because Phase 6 line 294 keeps adapter knowledge out of the session store
/// and `harness::tests::the_session_model_depends_on_no_adapter` enforces it
/// by scanning this file. That separation is right on its own terms: this
/// module owns *what is recorded about a session* and has no business
/// holding a list of harnesses, which grows.
///
/// It is enforced **here** rather than at the caller because this is the only
/// door. A guard in `main.rs` would be a guard `shell::start_session` does
/// not have, and one any future caller could forget; a refusal in `create` is
/// one no caller can bypass — the §35 shape, applied before the fact instead
/// of after it.
fn require_owning_harness(harness: &str) -> Result<(), SessionStoreError> {
    super::owning_harness(harness)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cli, Runtime};
    use clap::Parser;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicI64, Ordering};

    /// Undo every migration above 13, for a rollback fixture that lands above
    /// version 5.
    ///
    /// A fixture that claims to be an older database has to undo **every**
    /// migration above the version it claims, not only the one it is about.
    /// Below version 5 that is free for `checkpoints` — the table did not
    /// exist yet, so the fixture drops it and migration 14 meets a fresh one.
    /// A fixture that lands on 5 or later keeps the table, and without this it
    /// fails the re-run with `duplicate column name: seq`.
    ///
    /// **The name was `UNDO_MIGRATION_FOURTEEN` and was wrong by two
    /// migrations**, which is how `database`'s twin constant explains its own
    /// name: this is one constant precisely so the next migration has one
    /// place to be added rather than three copies to miss, and a name saying
    /// "fourteen" invites a reader to think 15 and 16 are handled somewhere
    /// else. They are handled here.
    ///
    /// SQLite refuses to drop a column an index mentions, so migration 14's
    /// indexes go first and `checkpoints_by_session` is put back the way
    /// migration 5 left it. Migration 16's column is indexed by nothing, and a
    /// column-scoped `CHECK` goes with the column it is written on, so it is
    /// one statement. Migration 17's `memory_files` is one statement for
    /// migration 15's reason — dropping a table takes its index and its two
    /// triggers with it. Migration 18's column is one statement for
    /// migration 16's reason, and migration 19's two tables are two
    /// statements for migration 15's — each drop takes its indexes and
    /// triggers with it — and they go first, newest migration undone first.
    /// Migration 20's column is the newest of all, so it leads.
    const UNDO_MIGRATIONS_ABOVE_THIRTEEN: &str = "
        ALTER TABLE sessions DROP COLUMN presentation_ref;
        DROP TABLE assumption_transitions;
        DROP TABLE task_assumptions;
        ALTER TABLE routing_observations DROP COLUMN failure_class;

        DROP TABLE memory_files;

        ALTER TABLE sessions DROP COLUMN observed_compactions;
        DROP TABLE IF EXISTS evaluation_observations;
        DROP INDEX checkpoints_by_seq;
        DROP INDEX checkpoints_by_session;
        ALTER TABLE checkpoints DROP COLUMN seq;
        CREATE INDEX checkpoints_by_session
            ON checkpoints (session_id, created_at DESC);
    ";

    /// A bootstrapped project with an open connection to its database, which
    /// is what every caller of this module will have.
    struct Fixture {
        base: PathBuf,
        runtime: Runtime,
        conn: Connection,
    }

    impl Fixture {
        fn new(base: &Path, name: &str) -> Self {
            let root = base.join("workspace").join(name);
            std::fs::create_dir_all(root.join(".git")).unwrap();
            let root = std::fs::canonicalize(&root).unwrap();
            let runtime = bootstrap_at(base, &root);
            let conn = crate::database::open(&runtime).unwrap();
            Self {
                base: base.to_path_buf(),
                runtime,
                conn,
            }
        }

        fn store(&self) -> SessionStore<'_> {
            SessionStore::new(&self.conn).unwrap()
        }

        /// A store whose clock returns `start`, then `start + step` on each
        /// later call, so a test can assert exact timestamps.
        fn store_with_ticking_clock(&self, start: i64, step: i64) -> SessionStore<'_> {
            let next = AtomicI64::new(start);
            let clock: Clock = Arc::new(move || next.fetch_add(step, Ordering::SeqCst));
            SessionStore::with_clock(&self.conn, clock).unwrap()
        }

        /// Reopen the database the way a later launch would, proving what is
        /// on disk rather than what is in memory.
        fn reopen(&self) -> Connection {
            crate::database::open(&self.runtime).unwrap()
        }

        fn project_id(&self) -> &str {
            self.runtime.project().id().as_str()
        }

        /// A second project sharing this machine's data/config root.
        fn sibling(&self, name: &str) -> Runtime {
            let root = self.base.join("workspace").join(name);
            std::fs::create_dir_all(root.join(".git")).unwrap();
            let root = std::fs::canonicalize(&root).unwrap();
            bootstrap_at(&self.base, &root)
        }
    }

    fn bootstrap_at(base: &Path, root: &Path) -> Runtime {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        crate::bootstrap(&cli, root).unwrap()
    }

    /// Insert a row directly, bypassing [`SessionStore`] entirely.
    ///
    /// Used to plant a row belonging to another project, which is exactly what
    /// the schema's trigger exists to prevent — so the trigger is dropped for
    /// the insert and restored afterwards. That models the real threat the
    /// resume check answers: a row that reached the file by some route the
    /// trigger never saw, such as a restored backup or an older build.
    fn plant_foreign_row(conn: &Connection, id: &str, project_id: &str, native: Option<&str>) {
        conn.execute_batch("DROP TRIGGER sessions_reject_foreign_project_insert;")
            .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, project_id, harness, native_session_id, role, \
             lifecycle, presentation, created_at, last_activity_at) \
             VALUES (?1, ?2, 'claude-code', ?3, 'normal', 'stopped', 'embedded', 10, 20)",
            rusqlite::params![id, project_id, native],
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER sessions_reject_foreign_project_insert
             BEFORE INSERT ON sessions
             FOR EACH ROW
             WHEN NEW.project_id IS NOT (
                 SELECT value FROM project_metadata WHERE key = 'project_id'
             )
             BEGIN
                 SELECT RAISE(ABORT, 'session belongs to a different project');
             END;",
        )
        .unwrap();
    }

    // ---------------------------------------------------------------
    // Phase 1 line 90 — reject a cross-project resume.
    // ---------------------------------------------------------------

    /// The capability, stated as a contract: given a session record whose
    /// project identifier differs from the active project's, when a caller
    /// tries to resume it, Glasshouse refuses and names both projects, while
    /// leaving the record untouched.
    #[test]
    fn resuming_a_session_belonging_to_another_project_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let other = fixture.sibling("beta");
        let other_id = other.project().id().as_str();
        assert_ne!(
            other_id,
            fixture.project_id(),
            "fixture must use two projects"
        );

        plant_foreign_row(&fixture.conn, "planted", other_id, Some("native-1"));

        let store = fixture.store();
        let error = store
            .open_for_resume(&SessionId::new("planted"))
            .expect_err("a session from another project must never be resumable");

        match &error {
            SessionStoreError::ForeignProject {
                id,
                expected,
                actual,
            } => {
                assert_eq!(id.as_str(), "planted");
                assert_eq!(expected, fixture.project_id());
                assert_eq!(actual, other_id);
            }
            other => panic!("expected ForeignProject, got {other:?}"),
        }

        // Naming both projects is the point: "not found" would send the user
        // hunting for a session that is sitting right there.
        let message = error.to_string();
        assert!(
            message.contains(other_id),
            "message must name the owning project: {message}"
        );
        assert!(
            message.contains(fixture.project_id()),
            "message must name the active project: {message}"
        );

        // Refusing is not deleting. The record is still exactly as planted.
        let still_there: String = fixture
            .conn
            .query_row(
                "SELECT project_id FROM sessions WHERE id = 'planted'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(still_there, other_id);
    }

    /// The structural half: the database itself refuses to store a session
    /// belonging to another project, so no future query has to remember to
    /// filter by project.
    #[test]
    fn the_database_refuses_to_store_a_session_from_another_project() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let other = fixture.sibling("beta");

        let result = fixture.conn.execute(
            "INSERT INTO sessions (id, project_id, harness, role, lifecycle, \
             presentation, created_at, last_activity_at) \
             VALUES ('x', ?1, 'claude-code', 'normal', 'starting', 'embedded', 1, 1)",
            [other.project().id().as_str()],
        );

        let Err(error) = result else {
            panic!("the trigger must abort an insert for another project");
        };
        assert!(
            error.to_string().contains("different project"),
            "unexpected error: {error}"
        );
    }

    /// Same guard on the update path: a row cannot be *moved* to another
    /// project after the fact.
    #[test]
    fn a_stored_session_cannot_be_reassigned_to_another_project() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let other = fixture.sibling("beta");
        let record = fixture
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();

        let result = fixture.conn.execute(
            "UPDATE sessions SET project_id = ?2 WHERE id = ?1",
            rusqlite::params![record.id.as_str(), other.project().id().as_str()],
        );

        let Err(error) = result else {
            panic!("the trigger must abort a reassignment");
        };
        assert!(
            error.to_string().contains("different project"),
            "unexpected error: {error}"
        );
    }

    /// The guard fails closed: with no binding row to compare against, the
    /// trigger aborts rather than letting the write through. `<>` against a
    /// NULL subquery would have evaluated to NULL and allowed it.
    #[test]
    fn a_session_write_is_refused_when_the_project_binding_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let project_id = fixture.project_id().to_owned();

        fixture
            .conn
            .execute("DELETE FROM project_metadata WHERE key = 'project_id'", [])
            .unwrap();

        let result = fixture.conn.execute(
            "INSERT INTO sessions (id, project_id, harness, role, lifecycle, \
             presentation, created_at, last_activity_at) \
             VALUES ('x', ?1, 'claude-code', 'normal', 'starting', 'embedded', 1, 1)",
            [&project_id],
        );
        assert!(
            result.is_err(),
            "an unbound database must accept no session rows"
        );
    }

    /// The permitted case, so the refusals above are not simply "resume never
    /// works".
    #[test]
    fn a_stopped_session_of_this_project_can_be_resumed() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let record = store.create(NewSession::embedded("codex")).unwrap();
        store
            .set_native_session_id(&record.id, "thread-77")
            .unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Stopped)
            .unwrap();

        let resumable = store.open_for_resume(&record.id).unwrap();
        assert_eq!(
            resumable,
            ResumableSession {
                id: record.id,
                harness: "codex".to_owned(),
                native_session_id: "thread-77".to_owned(),
            }
        );
    }

    /// The defect this package repairs, at the layer that caused it.
    ///
    /// `set_lifecycle` is what `main.rs::resume_session` used to call, and it
    /// silently declines a finished record — so the resume left the session
    /// reading `stopped`, and the *caller got no error saying so*. Both halves
    /// are asserted: the old door still refuses, and the resume boundary's own
    /// door opens.
    #[test]
    fn a_resume_reopens_a_session_that_set_lifecycle_would_have_left_finished() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let record = store.create(NewSession::embedded("codex")).unwrap();
        store
            .set_native_session_id(&record.id, "thread-77")
            .unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Stopped)
            .unwrap();

        // The door a hook comes through, and the reason the defect was silent:
        // it returns the record as it stands rather than an error.
        let declined = store
            .set_lifecycle(&record.id, SessionLifecycle::Running)
            .expect("a declined lifecycle change is not a failure");
        assert_eq!(
            declined.lifecycle,
            SessionLifecycle::Stopped,
            "`set_lifecycle` must keep refusing to revive a finished session"
        );

        let resumable = store.open_for_resume(&record.id).unwrap();
        let resumed = store.begin_resume(&resumable).unwrap();
        assert_eq!(
            resumed.lifecycle,
            SessionLifecycle::Running,
            "the resume boundary must reopen the session it was given"
        );
    }

    /// A resume is not a licence that outlives the session it was granted for.
    /// Once the resumed process exits, the record is finished again and the
    /// next late hook is refused exactly as the first incarnation's was.
    #[test]
    fn a_resumed_session_that_stops_again_is_finished_again() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let record = store.create(NewSession::embedded("codex")).unwrap();
        store
            .set_native_session_id(&record.id, "thread-77")
            .unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Stopped)
            .unwrap();
        let resumable = store.open_for_resume(&record.id).unwrap();
        store.begin_resume(&resumable).unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Stopped)
            .unwrap();

        let declined = store
            .set_lifecycle(&record.id, SessionLifecycle::Running)
            .unwrap();
        assert_eq!(
            declined.lifecycle,
            SessionLifecycle::Stopped,
            "having once been resumed must not make a session revivable for ever"
        );
    }

    /// **The window `open_for_resume` cannot close on its own.** It reads
    /// outside a transaction, so its answer can be stale by the time the
    /// resume writes — and the write is what matters.
    ///
    /// Here the record is closed after a `ResumableSession` has been obtained,
    /// which is exactly what a `glasshouse sessions close` in another process
    /// does between the two steps. The resume must refuse rather than reopen a
    /// record the user retired.
    #[test]
    fn a_resume_refuses_a_record_that_stopped_being_resumable_after_it_was_opened() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let record = store.create(NewSession::embedded("codex")).unwrap();
        store
            .set_native_session_id(&record.id, "thread-77")
            .unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Stopped)
            .unwrap();
        let resumable = store.open_for_resume(&record.id).unwrap();

        store.close(&record.id).unwrap();

        let error = store
            .begin_resume(&resumable)
            .expect_err("a record closed since it was opened is no longer resumable");
        assert!(
            matches!(&error, SessionStoreError::NotResumable { disposition, .. } if *disposition == "closed"),
            "got {error:?}"
        );
        assert_eq!(
            store.get(&record.id).unwrap().unwrap().lifecycle,
            SessionLifecycle::Closed,
            "the refused resume must have written nothing"
        );
    }

    /// `Failed` and `Closed` are not `Stopped`, and neither is a stopped
    /// record with nothing to resume *to*. All three are refused by the resume
    /// boundary itself, so the refusal does not depend on every caller having
    /// remembered to ask `open_for_resume` first.
    #[test]
    fn only_a_stopped_session_with_something_to_resume_to_may_be_reopened() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        // A distinct identifier per case: `(harness, native_session_id)` is
        // unique, which is the constraint that stops two sessions claiming one
        // harness conversation.
        for (case, lifecycle, native_recorded, expected) in [
            (0, SessionLifecycle::Failed, true, "failed"),
            (1, SessionLifecycle::Closed, true, "closed"),
            (2, SessionLifecycle::Stopped, false, "closed"),
            (3, SessionLifecycle::Running, true, "still running"),
        ] {
            let native = format!("thread-{case}");
            let record = store.create(NewSession::embedded("codex")).unwrap();
            if native_recorded {
                store.set_native_session_id(&record.id, &native).unwrap();
            }
            store.set_lifecycle(&record.id, lifecycle).unwrap();

            // Built by hand rather than through `open_for_resume`, which
            // refuses all four: the claim is that the boundary refuses them
            // too, and a test that could not construct the input could not
            // make it.
            let resumable = ResumableSession {
                id: record.id.clone(),
                harness: "codex".to_owned(),
                native_session_id: native.clone(),
            };
            let error = store.begin_resume(&resumable).unwrap_err();
            assert!(
                matches!(&error, SessionStoreError::NotResumable { disposition, .. } if *disposition == expected),
                "{lifecycle:?} with a recorded identifier={native_recorded} got {error:?}"
            );
            assert_eq!(
                store.get(&record.id).unwrap().unwrap().lifecycle,
                lifecycle,
                "a refused resume must leave {lifecycle:?} exactly as it was"
            );
        }
    }

    #[test]
    fn resuming_an_unknown_session_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let error = fixture
            .store()
            .open_for_resume(&SessionId::new("nope"))
            .expect_err("an unknown session cannot be resumed");
        assert!(
            matches!(error, SessionStoreError::NotFound { .. }),
            "got {error:?}"
        );
    }

    #[test]
    fn a_live_session_is_not_resumable() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        store.set_native_session_id(&record.id, "native-1").unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Running)
            .unwrap();

        let error = store
            .open_for_resume(&record.id)
            .expect_err("a running session is not resumable");
        assert!(
            matches!(&error, SessionStoreError::NotResumable { disposition, .. } if *disposition == "still running"),
            "got {error:?}"
        );
    }

    /// Without a native identifier there is nothing to resume *to*, so
    /// offering a resume would produce a blank session wearing an old name.
    #[test]
    fn a_stopped_session_with_no_native_identifier_is_not_resumable() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Stopped)
            .unwrap();

        let error = store
            .open_for_resume(&record.id)
            .expect_err("nothing to resume to");
        assert!(
            matches!(error, SessionStoreError::NotResumable { .. }),
            "got {error:?}"
        );
    }

    // ---------------------------------------------------------------
    // Phase 2 line 183 — metadata independent of native session files.
    // ---------------------------------------------------------------

    /// The record is Glasshouse's own: it is complete before the harness has
    /// produced any identifier, it survives a reopen, and nothing about it is
    /// read from a harness's files.
    #[test]
    fn a_session_is_recorded_and_survives_a_reopen_with_no_harness_involved() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");

        let created = fixture
            .store_with_ticking_clock(1_700_000_000, 0)
            .create(
                NewSession::embedded("claude-code")
                    .with_role(SessionRole::Orchestrator)
                    .with_presentation(SessionPresentation::External),
            )
            .unwrap();
        assert!(
            created.native_session_id.is_none(),
            "no harness has spoken yet"
        );

        // A different connection to the same file, as a later launch makes.
        let reopened = fixture.reopen();
        let store = SessionStore::new(&reopened).unwrap();
        let read_back = store
            .get(&created.id)
            .unwrap()
            .expect("the record is on disk");
        assert_eq!(read_back, created);
    }

    // ---------------------------------------------------------------
    // Phase 2 line 184 — Glasshouse ID <-> native harness ID mapping.
    // ---------------------------------------------------------------

    #[test]
    fn a_native_session_identifier_can_be_attached_later_and_read_back() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        let updated = store.set_native_session_id(&record.id, "sess-abc").unwrap();

        assert_eq!(updated.native_session_id.as_deref(), Some("sess-abc"));
        assert_eq!(
            updated.id, record.id,
            "the Glasshouse identifier never changes"
        );
        assert_eq!(
            store
                .get(&record.id)
                .unwrap()
                .unwrap()
                .native_session_id
                .as_deref(),
            Some("sess-abc")
        );
    }

    /// A mapping, not an annotation: one native session cannot be claimed by
    /// two Glasshouse sessions, or a resume would not know which to continue.
    #[test]
    fn one_native_session_cannot_map_to_two_glasshouse_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let first = store.create(NewSession::embedded("claude-code")).unwrap();
        let second = store.create(NewSession::embedded("claude-code")).unwrap();
        store.set_native_session_id(&first.id, "shared").unwrap();

        let error = store
            .set_native_session_id(&second.id, "shared")
            .expect_err("the same native session must not be claimed twice");
        assert!(
            matches!(error, SessionStoreError::Sql { .. }),
            "got {error:?}"
        );
    }

    /// Scoped per harness, so two harnesses that happen to use the same
    /// identifier format do not collide.
    #[test]
    fn two_harnesses_may_use_the_same_native_identifier() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let claude = store.create(NewSession::embedded("claude-code")).unwrap();
        let codex = store.create(NewSession::embedded("codex")).unwrap();
        store.set_native_session_id(&claude.id, "1").unwrap();
        store.set_native_session_id(&codex.id, "1").unwrap();

        assert_eq!(store.list().unwrap().len(), 2);
    }

    /// Sessions awaiting a native identifier must coexist freely.
    ///
    /// SQLite's unique indexes treat NULLs as distinct, so this holds today
    /// without help from the index's `WHERE` clause. The test earns its place
    /// by pinning the behaviour against the obvious future refactor: making
    /// the column `NOT NULL DEFAULT ''` would make every unidentified session
    /// collide with the next one.
    #[test]
    fn many_sessions_may_have_no_native_identifier_at_once() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        for _ in 0..3 {
            store.create(NewSession::embedded("claude-code")).unwrap();
        }
        assert_eq!(store.list().unwrap().len(), 3);
    }

    // ---------------------------------------------------------------
    // Phase 2 line 185 — harness, times, role, lifecycle, project id.
    // ---------------------------------------------------------------

    /// Every field the capability names, asserted by value rather than by
    /// "it round-trips".
    #[test]
    fn every_required_field_is_persisted() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");

        let record = fixture
            .store_with_ticking_clock(1_600_000_000, 0)
            .create(NewSession::embedded("codex").with_role(SessionRole::Worker))
            .unwrap();

        assert_eq!(record.harness, "codex");
        assert_eq!(record.role, SessionRole::Worker);
        assert_eq!(record.lifecycle, SessionLifecycle::Starting);
        assert_eq!(record.project_id, fixture.project_id());
        assert_eq!(record.created_at, 1_600_000_000);
        assert_eq!(record.last_activity_at, 1_600_000_000);
        assert!(!record.id.as_str().is_empty());
    }

    #[test]
    fn every_role_and_lifecycle_value_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        for role in [
            SessionRole::Normal,
            SessionRole::Orchestrator,
            SessionRole::Worker,
        ] {
            let record = store
                .create(NewSession::embedded("claude-code").with_role(role))
                .unwrap();
            assert_eq!(store.get(&record.id).unwrap().unwrap().role, role);

            for lifecycle in [
                SessionLifecycle::Starting,
                SessionLifecycle::Running,
                SessionLifecycle::Idle,
                SessionLifecycle::WaitingForUser,
                SessionLifecycle::Stopped,
                SessionLifecycle::Failed,
                SessionLifecycle::Closed,
            ] {
                store.set_lifecycle(&record.id, lifecycle).unwrap();
                assert_eq!(store.get(&record.id).unwrap().unwrap().lifecycle, lifecycle);
            }
        }
    }

    /// Activity time is what a session list sorts and ages by, so it has to
    /// move independently of creation time.
    #[test]
    fn activity_time_advances_while_creation_time_stays_put() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store_with_ticking_clock(1_000, 10);

        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        assert_eq!(record.created_at, 1_000);

        let touched = store.touch(&record.id).unwrap();
        assert_eq!(touched.created_at, 1_000, "creation time is immutable");
        assert_eq!(touched.last_activity_at, 1_010);

        let moved = store
            .set_lifecycle(&record.id, SessionLifecycle::Running)
            .unwrap();
        assert_eq!(
            moved.last_activity_at, 1_020,
            "a state change counts as activity"
        );
        assert_eq!(moved.created_at, 1_000);
    }

    #[test]
    fn sessions_are_listed_most_recently_active_first() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store_with_ticking_clock(500, 10);

        let first = store.create(NewSession::embedded("claude-code")).unwrap();
        let second = store.create(NewSession::embedded("codex")).unwrap();
        store.touch(&first.id).unwrap();

        let listed: Vec<_> = store.list().unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(listed, vec![first.id, second.id]);
    }

    #[test]
    fn touching_an_unknown_session_reports_it_missing_rather_than_inventing_one() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let error = fixture
            .store()
            .touch(&SessionId::new("ghost"))
            .expect_err("no such session");
        assert!(
            matches!(error, SessionStoreError::NotFound { .. }),
            "got {error:?}"
        );
        assert_eq!(
            fixture.store().list().unwrap().len(),
            0,
            "nothing was created"
        );
    }

    // ---------------------------------------------------------------
    // Phase 17 line 760 — an external session's pane, as opaque metadata.

    /// The reference survives a round trip exactly as given, an embedded
    /// session records none, and the store never interprets the string:
    /// a value no backend would accept is stored and returned all the same,
    /// because deciding what a reference means is the presenting
    /// integration's job (line 762), not this module's.
    #[test]
    fn an_external_sessions_presentation_ref_round_trips_and_is_never_interpreted() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let external = store
            .create(
                NewSession::embedded("claude-code")
                    .with_presentation(SessionPresentation::External)
                    .with_presentation_ref(Some("workspace:349".to_owned())),
            )
            .unwrap();
        assert_eq!(
            store
                .get(&external.id)
                .unwrap()
                .unwrap()
                .presentation_ref
                .as_deref(),
            Some("workspace:349")
        );

        let embedded = store.create(NewSession::embedded("claude-code")).unwrap();
        assert_eq!(
            store.get(&embedded.id).unwrap().unwrap().presentation_ref,
            None,
            "a session with no pane records no pane"
        );

        let opaque = store
            .create(
                NewSession::embedded("claude-code")
                    .with_presentation(SessionPresentation::External)
                    .with_presentation_ref(Some("not-a-cmux-ref".to_owned())),
            )
            .unwrap();
        assert_eq!(
            store
                .get(&opaque.id)
                .unwrap()
                .unwrap()
                .presentation_ref
                .as_deref(),
            Some("not-a-cmux-ref"),
            "the store stores; it does not decide what a reference looks like"
        );
    }

    /// A session recorded somewhere else and then continued inside a pane
    /// has its presentation and its pane rewritten together, and its
    /// activity clock untouched.
    #[test]
    fn a_continued_session_can_be_moved_into_a_pane_afterwards() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        assert_eq!(record.presentation, SessionPresentation::Embedded);
        assert_eq!(record.presentation_ref, None);

        let moved = store
            .set_presentation(
                &record.id,
                SessionPresentation::External,
                Some("workspace:349"),
            )
            .unwrap();
        assert_eq!(moved.presentation, SessionPresentation::External);
        assert_eq!(moved.presentation_ref.as_deref(), Some("workspace:349"));
        assert_eq!(
            moved.last_activity_at, record.last_activity_at,
            "moving a session is not session activity"
        );
        let read_back = store.get(&record.id).unwrap().unwrap();
        assert_eq!(read_back.presentation, SessionPresentation::External);
        assert_eq!(read_back.presentation_ref.as_deref(), Some("workspace:349"));
    }

    // Phase 2 line 186 — presentation mode.
    // ---------------------------------------------------------------

    #[test]
    fn every_presentation_mode_is_persisted() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        for presentation in [
            SessionPresentation::Embedded,
            SessionPresentation::Headless,
            SessionPresentation::External,
        ] {
            let record = store
                .create(NewSession::embedded("claude-code").with_presentation(presentation))
                .unwrap();
            assert_eq!(
                store.get(&record.id).unwrap().unwrap().presentation,
                presentation,
                "presentation must survive a round trip"
            );
        }
    }

    // ---------------------------------------------------------------
    // Phase 2 line 187 — active / resumable / closed / failed.
    // ---------------------------------------------------------------

    #[test]
    fn the_four_dispositions_are_distinguishable_from_stored_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let make = |lifecycle: SessionLifecycle, native: Option<&str>| {
            let record = store.create(NewSession::embedded("claude-code")).unwrap();
            if let Some(native) = native {
                store.set_native_session_id(&record.id, native).unwrap();
            }
            store.set_lifecycle(&record.id, lifecycle).unwrap()
        };

        assert_eq!(
            make(SessionLifecycle::Starting, None).disposition(),
            SessionDisposition::Active
        );
        assert_eq!(
            make(SessionLifecycle::Running, None).disposition(),
            SessionDisposition::Active
        );
        assert_eq!(
            make(SessionLifecycle::Idle, None).disposition(),
            SessionDisposition::Active
        );
        assert_eq!(
            make(SessionLifecycle::WaitingForUser, None).disposition(),
            SessionDisposition::Active
        );
        assert_eq!(
            make(SessionLifecycle::Stopped, Some("n1")).disposition(),
            SessionDisposition::Resumable
        );
        assert_eq!(
            make(SessionLifecycle::Stopped, None).disposition(),
            SessionDisposition::Closed,
            "stopped with nothing to resume to is over, not resumable"
        );
        assert_eq!(
            make(SessionLifecycle::Closed, None).disposition(),
            SessionDisposition::Closed
        );
        assert_eq!(
            make(SessionLifecycle::Failed, Some("n2")).disposition(),
            SessionDisposition::Failed,
            "a failure stays visible as a failure even with a native id"
        );
    }

    // ---------------------------------------------------------------
    // Phase 2 line 188 — no provider credentials in the project database.
    // ---------------------------------------------------------------

    /// The whole schema, locked to an explicit list.
    ///
    /// Fuzzy name matching would be worse than useless here: `project_metadata`
    /// legitimately has a column called `key`, and a credential column could
    /// just as easily be called `value`. Pinning the exact schema instead means
    /// any new column fails this test until someone updates the list, and that
    /// is the moment to ask what the new column can hold.
    ///
    /// **What this test can and cannot prove.** It proves no column exists
    /// whose *purpose* is to hold a credential, and that adding one is a
    /// deliberate act somebody has to write down here. It does not prove a
    /// credential can never be stored: `memories.subject` and `memories.body`
    /// are free text, and free text can hold anything.
    ///
    /// That gap is real and is not closed by widening this list. It is closed
    /// on the **producer** side — Phase 21's memory extractor must never be
    /// fed, and must never emit, credential material, and that is an explicit
    /// acceptance condition of Phase 21 rather than something inherited by
    /// assumption. Recorded when migration 4 added the memory tables and the
    /// worker adding them declined to certify otherwise.
    ///
    /// **Migration 6's twelve new columns, and the answer this test exists to
    /// force.** Two of them are integers: `source_event_first` and
    /// `source_event_last` are positions in `lifecycle_events.seq`, and an
    /// `INTEGER` column cannot hold a credential — there is no question to
    /// ask about those two.
    ///
    /// The other ten **can**. `rationale`, `problem`, `assumptions`,
    /// `scale_assumptions`, `security_assumptions`,
    /// `compatibility_assumptions`, `operational_assumptions`, `evidence`
    /// and `source_excerpt` are free text a producer chooses, exactly like
    /// `subject` and `body`, and `source_excerpt` is the sharpest of the ten
    /// because it is *verbatim session text* rather than a model's
    /// paraphrase — a decision quoted from a session that discussed
    /// configuring a provider is precisely where a key would appear.
    /// (`project_phase` is the eleventh and the one exception: migration 6
    /// gives it a `CHECK` over five fixed words, so it is not free text.)
    ///
    /// So the answer for migration 6 is the same as migration 4's and it is
    /// written down rather than inherited: **this test does not certify
    /// them.** The control is on the producer side, and it covers the new
    /// fields *without being extended*, which is the property worth having:
    /// `memory::extract::schema::judge` screens each emitted element whole,
    /// over its serialized text, **before reading any field of it**, so a
    /// field the contract gained yesterday is screened today. That ordering
    /// is why the coverage is automatic, and it is a Phase 21 acceptance
    /// condition rather than a convention.
    ///
    /// **Migration 5's twenty new columns, judged one at a time.** Nineteen
    /// hold a value drawn from a fixed set or from Glasshouse's own machinery
    /// — a kind, an origin, an exit code, a signal name, a backend resource
    /// slug, an integration slug, a harness event name from an adapter's own
    /// constant list — and none of them is free text a caller chooses.
    /// `checkpoints.document` is the twentieth and it **is** free text, for
    /// the same reason `memories.body` is: a person writes a handoff. The same
    /// limit therefore applies to it and is recorded here rather than glossed
    /// — it is closed on the producer side, by whoever authors a checkpoint,
    /// and this test does not and cannot certify it.
    #[test]
    fn the_project_database_schema_has_nowhere_to_put_a_credential() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");

        let mut statement = fixture
            .conn
            .prepare(
                "SELECT m.name, p.name FROM sqlite_master m \
                 JOIN pragma_table_info(m.name) p \
                 WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite_%' \
                 ORDER BY m.name, p.cid",
            )
            .unwrap();
        let columns: Vec<String> = statement
            .query_map([], |row| {
                Ok(format!(
                    "{}.{}",
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();

        assert_eq!(
            columns,
            vec![
                // Migration 19 (carried from `assumption-guardrails`). The
                // premises an agent states, their evidence class, scope and
                // verification, and the transitions between six states —
                // free text a caller supplied, sanitized by that package's
                // writer, and vocabularies that live in Rust. That package's
                // own review of these columns is the authority; they are
                // listed here so this tree's ladder (1..20) is the schema
                // this test sees.
                "assumption_transitions.seq",
                "assumption_transitions.project_id",
                "assumption_transitions.assumption_id",
                "assumption_transitions.session_id",
                "assumption_transitions.at",
                "assumption_transitions.kind",
                "assumption_transitions.state",
                "assumption_transitions.origin",
                "assumption_transitions.subject",
                "assumption_transitions.response",
                "assumption_transitions.note",
                "checkpoints.id",
                "checkpoints.project_id",
                "checkpoints.session_id",
                "checkpoints.created_at",
                "checkpoints.reason",
                "checkpoints.document",
                // Migration 14. A counter of how many checkpoints this project
                // had written before this one — an integer with no free text
                // anywhere near it.
                "checkpoints.seq",
                // Migration 15. Every one confirmed unable to hold a provider
                // credential: `seq`/`observed_at`/`routing_seq` are integers,
                // `project_id` is the project hash every table already
                // carries, `kind`/`outcome` come from an exhaustive Rust match
                // at the single writer, `subject` from a two-value scope enum,
                // `session_id`/`memory_id` are identifiers, and `feature`,
                // `arm` and `detail` have no production writer at all.
                "evaluation_observations.seq",
                "evaluation_observations.project_id",
                "evaluation_observations.observed_at",
                "evaluation_observations.kind",
                "evaluation_observations.outcome",
                "evaluation_observations.subject",
                "evaluation_observations.session_id",
                "evaluation_observations.feature",
                "evaluation_observations.arm",
                "evaluation_observations.memory_id",
                "evaluation_observations.routing_seq",
                "evaluation_observations.detail",
                "lifecycle_events.seq",
                "lifecycle_events.project_id",
                "lifecycle_events.session_id",
                "lifecycle_events.at",
                "lifecycle_events.kind",
                "lifecycle_events.turn_outcome",
                "lifecycle_events.origin",
                "lifecycle_events.bytes",
                "lifecycle_events.exit_code",
                "lifecycle_events.exit_signal",
                "lifecycle_events.resource",
                "lifecycle_events.gateway_reason",
                "lifecycle_events.gateway_provider",
                "lifecycle_events.gateway_model",
                "lifecycle_events.gateway_cause",
                "lifecycle_events.observed_harness",
                "lifecycle_events.observed_event",
                "memories.id",
                "memories.project_id",
                "memories.kind",
                "memories.authority",
                "memories.status",
                "memories.subject",
                "memories.body",
                "memories.source_session_id",
                "memories.source_commit",
                "memories.superseded_by",
                "memories.created_at",
                "memories.updated_at",
                "memories.source_event_first",
                "memories.source_event_last",
                "memories.rationale",
                "memories.project_phase",
                "memories.problem",
                "memories.assumptions",
                "memories.scale_assumptions",
                "memories.security_assumptions",
                "memories.compatibility_assumptions",
                "memories.operational_assumptions",
                "memories.evidence",
                "memories.source_excerpt",
                // Migration 10. `review_reason` is one of six fixed words (a
                // `CHECK` enum); `review_marked_at` and `last_validated_at` are
                // Unix timestamps — none of the three can hold a credential.
                // `validity_conditions` and `invalidation_conditions` are free
                // text a producer writes, exactly like `rationale` and the rest
                // of migration 6's provenance columns beside them, and this test
                // does not and cannot certify them for the same reason it does
                // not certify those: the control is on the producer side, where
                // `memory::extract::chunk` scrubs and `schema::judge` screens.
                "memories.validity_conditions",
                "memories.invalidation_conditions",
                "memories.review_reason",
                "memories.review_marked_at",
                "memories.last_validated_at",
                "memories.superseded_reason",
                "memories_fts.subject",
                "memories_fts.body",
                "memories_fts.rationale",
                "memories_fts_config.k",
                "memories_fts_config.v",
                "memories_fts_data.id",
                "memories_fts_data.block",
                "memories_fts_docsize.id",
                "memories_fts_docsize.sz",
                "memories_fts_idx.segid",
                "memories_fts_idx.term",
                "memories_fts_idx.pgno",
                // Migration 17. `seq` and `observed_at` are integers,
                // `project_id` is the project hash every table carries,
                // `memory_id` is an identifier, and `provenance` comes from
                // an exhaustive Rust match at the single writer. `path` is
                // the one to argue about, and it is argued: it is never free
                // text a caller chooses — the only writer is
                // `MemoryStore::record_observed_files`, whose paths come from
                // the git index by way of
                // `checkpoint::git::WorkingTreeStatus::detect`, and
                // `memory::normalize_observed_path` refuses anything that is
                // not a repo-relative path before it can reach the column. A
                // credential is not a tracked file name.
                "memory_files.seq",
                "memory_files.project_id",
                "memory_files.memory_id",
                "memory_files.path",
                "memory_files.provenance",
                "memory_files.observed_at",
                "project_metadata.key",
                "project_metadata.value",
                // Migration 11: `routing_observations` (Phase 33A). `seq`,
                // `observed_at`, the timestamps, the counters and the
                // fixed-vocabulary columns cannot hold a credential; the
                // free-text ones (`route`, `quota_context`, `harness`,
                // `purpose`) are names and slugs a producer inside this crate
                // constructs, never text copied from a provider response body
                // — the gateway that writes this table is structurally unable
                // to read a response body at all (see `routing::evidence`).
                "routing_observations.seq",
                "routing_observations.project_id",
                "routing_observations.observed_at",
                "routing_observations.provider",
                "routing_observations.model",
                "routing_observations.route",
                "routing_observations.quota_context",
                "routing_observations.harness",
                "routing_observations.purpose",
                "routing_observations.dispatched_at",
                "routing_observations.first_byte_at",
                "routing_observations.first_token_at",
                "routing_observations.first_tool_call_at",
                "routing_observations.completed_at",
                "routing_observations.input_tokens",
                "routing_observations.output_tokens",
                "routing_observations.cached_input_tokens",
                "routing_observations.cost_micro_usd",
                "routing_observations.cost_confidence",
                "routing_observations.tool_rounds",
                "routing_observations.retries",
                "routing_observations.repairs",
                "routing_observations.failovers",
                "routing_observations.outcome",
                "routing_observations.context_state",
                "routing_observations.failure_class",
                "schema_migrations.version",
                "sessions.id",
                "sessions.project_id",
                "sessions.harness",
                "sessions.native_session_id",
                "sessions.role",
                "sessions.lifecycle",
                "sessions.presentation",
                "sessions.created_at",
                "sessions.last_activity_at",
                "sessions.launch_profile",
                "sessions.backend_resource",
                // Migration 8. Every one of these is a name, a slug or a
                // label a person typed: a model id, a pairing class, a wire
                // protocol, five response axes, a mechanism category, a
                // session name and a purpose. None of them is a place a
                // credential could be put, and there is still no column that
                // could hold one.
                "sessions.model",
                "sessions.pairing_class",
                "sessions.protocol",
                "sessions.response_profile",
                "sessions.response_mechanism",
                "sessions.display_name",
                "sessions.purpose",
                // Migration 9. A process id, a kernel start time, a host
                // name, one of four fixed supervision words, and a sentence
                // Glasshouse composes itself in `session::supervision`. None
                // of the five is ever written from anything a user typed or a
                // provider returned, and the two that are free-form are
                // `process_host` — the machine's own name — and
                // `supervision_reason`, whose every producer is a `format!`
                // in this crate over a process id and a timestamp.
                "sessions.process_id",
                "sessions.process_started_at",
                "sessions.process_host",
                "sessions.supervision",
                "sessions.supervision_reason",
                // Migration 12. A Glasshouse-generated session identifier —
                // the same one every other `sessions.id` column already
                // holds — never anything a user typed or a provider
                // returned.
                "sessions.source_session_id",
                // Migration 16. A count of compactions Glasshouse observed:
                // an integer this crate increments by one, constrained
                // non-negative by the schema, and never given a value from
                // outside the process. There is no string here for anything
                // to be typed into.
                "sessions.observed_compactions",
                // Migration 20. A cmux workspace reference of the shape
                // `workspace:<n>`, written only from what cmux itself
                // printed or from a reference a person typed on the command
                // line, and validated to that shape before it is ever handed
                // back. A pane number is not a place a credential could be
                // put.
                "sessions.presentation_ref",
                // Migration 19: the six fields an agent states about a
                // premise and their bookkeeping. Free text, sanitized by the
                // writer and bounded; no column is named for, or shaped
                // like, a credential.
                "task_assumptions.id",
                "task_assumptions.project_id",
                "task_assumptions.session_id",
                "task_assumptions.created_at",
                "task_assumptions.origin",
                "task_assumptions.claim",
                "task_assumptions.evidence",
                "task_assumptions.evidence_source",
                "task_assumptions.uncertainty",
                "task_assumptions.affected",
                "task_assumptions.verification",
            ],
            "the project database schema changed; confirm the new column cannot \
             hold a provider credential before updating this list"
        );
    }

    // ---------------------------------------------------------------
    // Phase 9A — a launch profile is a reference here, never a definition.
    // ---------------------------------------------------------------

    /// The database schema has exactly a reference column for the profile a
    /// session ran under, and no table defining what a profile *is* —
    /// profiles are configuration, resolved in `crate::config`/
    /// `crate::profile`, never project memory.
    #[test]
    fn no_launch_profile_definition_is_stored_in_the_project_database() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");

        let mut statement = fixture
            .conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
                 ORDER BY name",
            )
            .unwrap();
        let tables: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            tables,
            vec![
                // Migration 19 (carried from `assumption-guardrails`): a
                // ledger of stated premises, not a profile definition.
                "assumption_transitions",
                "checkpoints",
                "evaluation_observations",
                "lifecycle_events",
                "memories",
                "memories_fts",
                "memories_fts_config",
                "memories_fts_data",
                "memories_fts_docsize",
                "memories_fts_idx",
                "memory_files",
                "project_metadata",
                "routing_observations",
                "schema_migrations",
                "sessions",
                "task_assumptions",
            ],
            "no table defining launch profiles may exist in the project database"
        );

        let record = fixture
            .store()
            .create(
                NewSession::embedded("claude-code")
                    .with_launch_profile(Some("native".to_owned()))
                    .with_backend_resource(Some("native".to_owned())),
            )
            .unwrap();
        assert_eq!(record.launch_profile.as_deref(), Some("native"));
        assert_eq!(record.backend_resource.as_deref(), Some("native"));

        let read_back = fixture.store().get(&record.id).unwrap().unwrap();
        assert_eq!(read_back.launch_profile.as_deref(), Some("native"));
        assert_eq!(read_back.backend_resource.as_deref(), Some("native"));
    }

    /// Building a session without naming a profile leaves both columns NULL
    /// rather than inventing a value — the same "None means not recorded"
    /// rule the rest of this table already follows for `native_session_id`.
    #[test]
    fn a_session_with_no_recorded_profile_leaves_both_columns_null() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let record = fixture
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();
        assert_eq!(record.launch_profile, None);
        assert_eq!(record.backend_resource, None);
    }

    /// An existing version-2 database gains the two launch-profile columns on
    /// the next launch, with every existing session's data intact and both
    /// new columns `NULL` — a session recorded before this migration ran is a
    /// different fact from one that ran the Native profile, so NULL must
    /// stay NULL rather than default to `"native"`.
    #[test]
    fn upgrading_a_version_2_database_preserves_every_existing_session() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();

        let record = store
            .create(NewSession::embedded("claude-code").with_role(SessionRole::Worker))
            .unwrap();
        store.set_native_session_id(&record.id, "native-1").unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Stopped)
            .unwrap();

        // Roll the database back to what version 2 left behind: drop what
        // migrations 3 and 4 added, and forget that they ran.
        //
        // `DELETE ... WHERE version = 3` is what this said while 3 was the
        // highest migration, and it stopped working the moment 4 existed. The
        // runner resumes from `MAX(version)`, so deleting only row 3 leaves a
        // *hole* — max is still 4, nothing re-applies, and the test failed
        // later and confusingly with "no such column: launch_profile". Roll
        // back a contiguous range, or do not roll back at all.
        //
        // Everything a later migration created has to go with the rows that
        // record it, or the re-run fails on `table … already exists` instead —
        // which is the same trap wearing the opposite coat, and is exactly how
        // migration 5 announced itself here.
        fixture
            .conn
            .execute_batch(
                "ALTER TABLE sessions DROP COLUMN presentation_ref;
                 ALTER TABLE sessions DROP COLUMN observed_compactions;
                 ALTER TABLE sessions DROP COLUMN launch_profile;
                 ALTER TABLE sessions DROP COLUMN backend_resource;
                 ALTER TABLE sessions DROP COLUMN model;
                 ALTER TABLE sessions DROP COLUMN pairing_class;
                 ALTER TABLE sessions DROP COLUMN protocol;
                 ALTER TABLE sessions DROP COLUMN response_profile;
                 ALTER TABLE sessions DROP COLUMN response_mechanism;
                 ALTER TABLE sessions DROP COLUMN display_name;
                 ALTER TABLE sessions DROP COLUMN purpose;
                 ALTER TABLE sessions DROP COLUMN process_id;
                 ALTER TABLE sessions DROP COLUMN process_started_at;
                 ALTER TABLE sessions DROP COLUMN process_host;
                 ALTER TABLE sessions DROP COLUMN supervision;
                 ALTER TABLE sessions DROP COLUMN supervision_reason;
                 ALTER TABLE sessions DROP COLUMN source_session_id;
                 DROP TABLE IF EXISTS memories_fts;
                 DROP TABLE IF EXISTS memories;
                 DROP TABLE IF EXISTS lifecycle_events;
                 DROP TABLE IF EXISTS checkpoints;
                 DROP TABLE IF EXISTS routing_observations;
                 DROP TABLE IF EXISTS evaluation_observations;
                 DROP TABLE IF EXISTS memory_files;
                 DROP TABLE IF EXISTS assumption_transitions;
                 DROP TABLE IF EXISTS task_assumptions;
                 DELETE FROM schema_migrations WHERE version >= 3;",
            )
            .unwrap();

        let reopened = fixture.reopen();
        let version: i64 = reopened
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            version, 20,
            "the launch must have applied migrations 3 through 20"
        );

        let migrated_store = SessionStore::new(&reopened).unwrap();
        let migrated = migrated_store
            .get(&record.id)
            .unwrap()
            .expect("the pre-migration session must survive");
        assert_eq!(migrated.id, record.id);
        assert_eq!(migrated.harness, "claude-code");
        assert_eq!(migrated.role, SessionRole::Worker);
        assert_eq!(migrated.native_session_id.as_deref(), Some("native-1"));
        assert_eq!(migrated.lifecycle, SessionLifecycle::Stopped);
        assert_eq!(migrated.created_at, record.created_at);
        assert_eq!(
            migrated.launch_profile, None,
            "a pre-migration session has no recorded profile — never a guessed default"
        );
        assert_eq!(migrated.backend_resource, None);
    }

    /// `project_metadata` is a key/value table, which is the one place a
    /// credential could be smuggled in without a schema change. Its keys are
    /// pinned too.
    #[test]
    fn project_metadata_holds_only_the_project_identifier() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        fixture
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();

        let mut statement = fixture
            .conn
            .prepare("SELECT key FROM project_metadata ORDER BY key")
            .unwrap();
        let keys: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(keys, vec!["project_id"]);
    }

    // ---------------------------------------------------------------
    // Storage-layer integrity.
    // ---------------------------------------------------------------

    /// The `CHECK` constraints are the reason readers can trust the enum
    /// columns, so verify they actually reject nonsense.
    #[test]
    fn the_schema_rejects_enum_values_it_does_not_define() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let project_id = fixture.project_id().to_owned();

        for (column, bad) in [
            ("role", "admin"),
            ("lifecycle", "probably_fine"),
            ("presentation", "invisible"),
        ] {
            let mut values = std::collections::HashMap::from([
                ("role", "normal"),
                ("lifecycle", "starting"),
                ("presentation", "embedded"),
            ]);
            values.insert(column, bad);

            let result = fixture.conn.execute(
                "INSERT INTO sessions (id, project_id, harness, role, lifecycle, \
                 presentation, created_at, last_activity_at) \
                 VALUES (?1, ?2, 'claude-code', ?3, ?4, ?5, 1, 1)",
                rusqlite::params![
                    format!("bad-{column}"),
                    &project_id,
                    values["role"],
                    values["lifecycle"],
                    values["presentation"],
                ],
            );
            assert!(result.is_err(), "`{column}` must reject `{bad}`");
        }
    }

    /// A value that somehow got past the constraint must surface as a typed
    /// error naming the column, never a panic or a silent default.
    #[test]
    fn an_unrecognized_stored_enum_value_is_reported_rather_than_guessed() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let record = fixture
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();

        // Rebuild the table without its CHECK constraints to model a database
        // written by a future build that knows a lifecycle this one does not.
        fixture
            .conn
            .execute_batch(
                "PRAGMA writable_schema = ON;
                 UPDATE sqlite_master
                    SET sql = replace(sql, \"CHECK (lifecycle IN ('starting', 'running', 'idle',\
\n                                 'waiting_for_user', 'stopped', 'failed',\
\n                                 'closed'))\", '')
                  WHERE type = 'table' AND name = 'sessions';
                 PRAGMA writable_schema = OFF;",
            )
            .unwrap();
        let reopened = fixture.reopen();
        reopened
            .execute(
                "UPDATE sessions SET lifecycle = 'hibernating' WHERE id = ?1",
                [record.id.as_str()],
            )
            .unwrap();

        let store = SessionStore::new(&reopened).unwrap();
        let error = store
            .get(&record.id)
            .expect_err("an unknown lifecycle must not be guessed");
        match error {
            SessionStoreError::UnknownValue { column, value, .. } => {
                assert_eq!(column, "lifecycle");
                assert_eq!(value, "hibernating");
            }
            other => panic!("expected UnknownValue, got {other:?}"),
        }
    }

    /// Identifiers come from SQLite's CSPRNG rather than the clock, because
    /// sessions get spawned in bursts.
    #[test]
    fn generated_session_identifiers_are_unique_within_a_burst() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        // A frozen clock: any identifier derived from time would collide.
        let store = fixture.store_with_ticking_clock(42, 0);

        let ids: std::collections::HashSet<_> = (0..64)
            .map(|_| {
                store
                    .create(NewSession::embedded("claude-code"))
                    .unwrap()
                    .id
            })
            .collect();
        assert_eq!(ids.len(), 64, "identifiers must not collide");
    }

    /// An existing version-1 database gains the sessions table on the next
    /// launch without losing its project binding.
    #[test]
    fn a_version_one_database_migrates_forward_keeping_its_binding() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let project_id = fixture.project_id().to_owned();

        // Wind the database back to what version 1 left behind.
        //
        // The deleted range must stay contiguous to the newest migration: the
        // runner resumes from `MAX(version)`, so leaving a higher row behind
        // makes it believe there is nothing to do. See the sibling test.
        fixture
            .conn
            .execute_batch(
                "DROP TRIGGER sessions_reject_foreign_project_insert;
                 DROP TRIGGER sessions_reject_foreign_project_update;
                 DROP TABLE sessions;
                 DROP TABLE IF EXISTS memories_fts;
                 DROP TABLE IF EXISTS memories;
                 DROP TABLE IF EXISTS lifecycle_events;
                 DROP TABLE IF EXISTS checkpoints;
                 DROP TABLE IF EXISTS routing_observations;
                 DROP TABLE IF EXISTS evaluation_observations;
                 DROP TABLE IF EXISTS memory_files;
                 DROP TABLE IF EXISTS assumption_transitions;
                 DROP TABLE IF EXISTS task_assumptions;
                 DELETE FROM schema_migrations WHERE version >= 2;",
            )
            .unwrap();
        drop(fixture.reopen());

        let reopened = fixture.reopen();
        let version: i64 = reopened
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            version, 20,
            "the launch must have applied migrations 2 through 20"
        );

        let store = SessionStore::new(&reopened).unwrap();
        assert_eq!(store.project_id(), project_id, "the binding survived");
        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        assert_eq!(record.project_id, project_id);
    }

    /// Two projects on one machine keep entirely separate session lists —
    /// separate files, not a shared file with a filter.
    #[test]
    fn two_projects_have_independent_session_lists() {
        let tmp = tempfile::tempdir().unwrap();
        let alpha = Fixture::new(tmp.path(), "alpha");
        let beta = Fixture::new(tmp.path(), "beta");

        alpha
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();
        alpha.store().create(NewSession::embedded("codex")).unwrap();
        beta.store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();

        assert_ne!(alpha.runtime.database_path(), beta.runtime.database_path());
        assert_eq!(alpha.store().list().unwrap().len(), 2);
        assert_eq!(beta.store().list().unwrap().len(), 1);
    }

    /// The store refuses to work against a database with no project bound,
    /// rather than defaulting to something and writing rows nobody can place.
    #[test]
    fn the_store_refuses_an_unbound_database() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        fixture
            .conn
            .execute("DELETE FROM project_metadata WHERE key = 'project_id'", [])
            .unwrap();

        let error = SessionStore::new(&fixture.conn).expect_err("an unbound database is unusable");
        assert!(
            matches!(error, SessionStoreError::UnboundDatabase),
            "got {error:?}"
        );
    }

    /// The injected clock is the one every test above uses, so the real one
    /// needs its own check that it returns sane epoch seconds rather than,
    /// say, nanoseconds or zero.
    #[test]
    fn the_default_clock_returns_plausible_epoch_seconds() {
        let first = system_clock();
        let second = system_clock();
        assert!(
            second >= first,
            "the wall clock must not run backwards mid-test"
        );
        assert!(
            first > 1_600_000_000,
            "the clock must return seconds since the epoch"
        );
        assert!(
            first < 32_000_000_000,
            "seconds, not milliseconds or nanoseconds"
        );
    }

    // --- resolving an identifier ----------------------------------------

    #[test]
    fn a_whole_identifier_resolves_to_its_session() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        assert_eq!(store.resolve_id(record.id.as_str()).unwrap(), record.id);
    }

    #[test]
    fn the_short_form_the_listing_prints_is_enough_to_resolve() {
        // `glasshouse sessions` prints twelve characters and nothing else, so
        // twelve characters have to be usable. If they were not, the only
        // identifier a user can see would be the one they cannot use.
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        let record = store.create(NewSession::embedded("claude-code")).unwrap();

        let short: String = record.id.as_str().chars().take(12).collect();
        assert_eq!(store.resolve_id(&short).unwrap(), record.id);
    }

    #[test]
    fn an_ambiguous_prefix_is_refused_and_names_its_candidates() {
        // Resuming the wrong session is worse than being asked to type more.
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        let first = store.create(NewSession::embedded("claude-code")).unwrap();
        let second = store.create(NewSession::embedded("codex")).unwrap();

        // Every identifier shares the empty prefix; the shortest prefix both
        // share is found by comparison so the test does not depend on the
        // random values.
        let shared: String = first
            .id
            .as_str()
            .chars()
            .zip(second.id.as_str().chars())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a)
            .collect();
        let ambiguous = shared;
        if ambiguous.is_empty() {
            // Two identifiers with no shared prefix: use a one-character one
            // that both cannot share, and assert the exact-match path instead.
            assert_eq!(store.resolve_id(first.id.as_str()).unwrap(), first.id);
            return;
        }

        match store.resolve_id(&ambiguous) {
            Err(SessionStoreError::AmbiguousPrefix { matches, .. }) => {
                assert!(matches.contains(&first.id));
                assert!(matches.contains(&second.id));
            }
            other => panic!("expected an ambiguous prefix, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_identifier_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        store.create(NewSession::embedded("claude-code")).unwrap();
        assert!(matches!(
            store.resolve_id("ffffffffffffffffffffffffffffffff"),
            Err(SessionStoreError::NotFound { .. })
        ));
    }

    #[test]
    fn a_wildcard_cannot_be_smuggled_into_the_lookup() {
        // Identifiers are matched with `substr`, not `LIKE`. Under `LIKE`, a
        // bare `%` would match every session in the project, and resuming
        // "whichever one came first" is exactly the wrong answer.
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        store.create(NewSession::embedded("claude-code")).unwrap();

        for hostile in ["%", "_", "%%", "a%", "' OR 1=1 --"] {
            assert!(
                matches!(
                    store.resolve_id(hostile),
                    Err(SessionStoreError::MalformedId { .. })
                ),
                "`{hostile}` was not refused"
            );
        }
    }

    #[test]
    fn an_empty_identifier_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        assert!(matches!(
            store.resolve_id("   "),
            Err(SessionStoreError::MalformedId { .. })
        ));
    }

    // --- assigned native identifiers -------------------------------------

    #[test]
    fn a_minted_native_identifier_is_a_valid_version_4_uuid() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        for _ in 0..64 {
            let id = store.new_native_session_id().unwrap();
            assert_eq!(id.len(), 36, "{id}");
            let groups: Vec<&str> = id.split('-').collect();
            assert_eq!(
                groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
                vec![8, 4, 4, 4, 12],
                "{id}"
            );
            assert!(
                id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
                "{id}"
            );
            // The two things a strict validator checks beyond the shape.
            assert_eq!(groups[2].chars().next(), Some('4'), "version nibble: {id}");
            assert!(
                matches!(groups[3].chars().next(), Some('8' | '9' | 'a' | 'b')),
                "variant nibble: {id}"
            );
        }
    }

    #[test]
    fn minted_native_identifiers_do_not_repeat() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..256 {
            assert!(
                seen.insert(store.new_native_session_id().unwrap()),
                "a minted identifier repeated"
            );
        }
    }

    #[test]
    fn the_uuid_formatter_only_overwrites_the_version_and_variant() {
        // Every other nibble survives, so the identifier keeps 122 bits of
        // the randomness it was given rather than being quietly reshaped.
        let hex = "0123456789abcdef0123456789abcdef";
        let uuid = uuid_v4_from_hex(hex);
        assert_eq!(uuid, "01234567-89ab-4def-8123-456789abcdef");

        let plain: String = uuid.chars().filter(|c| *c != '-').collect();
        let differences = hex
            .chars()
            .zip(plain.chars())
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        assert_eq!(differences, vec![12, 16], "only these two nibbles may move");
    }

    #[test]
    fn a_session_can_be_recorded_with_its_native_identifier_from_the_start() {
        // The point of assignment: the record carries the identifier before
        // the harness has produced any output at all, so a session that dies
        // during startup is still resumable rather than anonymous.
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        let native = store.new_native_session_id().unwrap();
        let record = store
            .create(
                NewSession::embedded("claude-code").with_native_session_id(Some(native.clone())),
            )
            .unwrap();
        assert_eq!(record.native_session_id.as_deref(), Some(native.as_str()));

        let read_back = store.get(&record.id).unwrap().expect("the session");
        assert_eq!(read_back.native_session_id, Some(native));
    }

    // ---------------------------------------------------------------
    // Phase 10A — one ordered path for lifecycle changes.
    // ---------------------------------------------------------------

    /// *"Apply session lifecycle changes through a single ordered path."*
    ///
    /// The ordering is worth nothing if there are two paths, and a second one
    /// is the natural thing to add: supervision needs to move a session to
    /// `stopped` when it finds the process gone, and writing its own `UPDATE`
    /// beside the conclusion it had just drawn was in fact the first way this
    /// was written. It passed every behavioural test and left two writers with
    /// two orders.
    ///
    /// So the structure is the assertion. Reads by lines, so it is blind to
    /// line endings by construction — see `docs/product/design-decisions.md`.
    #[test]
    fn one_statement_moves_a_sessions_lifecycle() {
        let source = include_str!("store.rs");
        let lines: Vec<&str> = source.lines().collect();
        let end = lines
            .windows(2)
            .position(|pair| {
                pair[0].trim_end() == "#[cfg(test)]" && pair[1].trim_end().starts_with("mod tests")
            })
            .unwrap_or(lines.len());
        let code: String = lines[..end]
            .iter()
            .filter(|line| !line.trim_start().starts_with("//"))
            .flat_map(|line| line.chars().filter(|c| !c.is_whitespace()))
            .collect();

        let writes = code.matches("UPDATEsessionsSETlifecycle").count();
        assert_eq!(
            writes, 1,
            "{writes} statements move a session's lifecycle; there must be exactly \
             one, and every caller must reach it through `write_lifecycle_locked` \
             inside `in_a_write_transaction`"
        );

        let locked = code
            .find("fnwrite_lifecycle_locked(")
            .expect("the one write site must still be called `write_lifecycle_locked`");
        let write = code
            .find("UPDATEsessionsSETlifecycle")
            .expect("checked above");
        assert!(
            write > locked,
            "the lifecycle write must be inside `write_lifecycle_locked`, where the \
             read that decides it is also taken"
        );

        // And the transaction it must be run inside is `IMMEDIATE`. A deferred
        // one reads without the write lock and then has to upgrade, which
        // SQLite refuses outright once another connection has committed.
        assert!(
            code.contains(r#"execute_batch("BEGINIMMEDIATE")"#),
            "the write transaction must take SQLite's write lock up front"
        );
        assert!(
            !code.contains(r#"execute_batch("BEGINDEFERRED")"#)
                && !code.contains(r#"execute_batch("BEGIN")"#),
            "a deferred write transaction cannot order two writers"
        );
    }

    mod phase_10 {
        //! Phase 10 — the unified session model, at the storage layer.
        //!
        //! The production surfaces live in `main.rs` and are exercised against the
        //! shipped binary in `tests/session_model.rs`. What is here is what only
        //! the store can answer: that a session records nine separate facts in
        //! nine separate places, that the two labels a person owns cannot reach
        //! the identifier a resume depends on, and that migration 8 leaves a
        //! version-7 database's rows exactly as it found them.

        use super::*;

        /// The seven kinds of thing a session records, all different, all read
        /// back apart.
        ///
        /// Every value below is distinct from every other, on purpose: a build
        /// that filled the pairing class in from the launch profile — or the
        /// model from the backend resource, or either label from the other —
        /// would put the same string in two columns, and this fails on it. That
        /// is the failure line 645 and the phase's second architectural
        /// requirement exist to prevent, and it is checked *after a reopen*, so
        /// what is proved is what is on disk rather than what was in memory.
        #[test]
        fn a_session_records_seven_facts_and_no_two_of_them_share_a_column() {
            let tmp = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(tmp.path(), "alpha");
            let store = fixture.store();

            let profile = ResponseProfile::new(
                Verbosity::Terse,
                Audience::Executive,
                Narration::Silent,
                EvidenceDetail::Audit,
                AnswerFormat::Bullets,
            );
            let record = store
                .create(
                    NewSession::embedded("claude-code")
                        .with_launch_profile(Some("a-launch-profile".to_owned()))
                        .with_backend_resource(Some("a-backend-resource".to_owned()))
                        .with_model(Some(AssignedModel::named("a-model")))
                        .with_pairing_class(Some(SessionPairingClass::ProtocolCompatible))
                        .with_protocol(Some(SessionProtocol::OpenAiResponses))
                        .with_response_profile(Some(profile))
                        .with_response_mechanism(Some(ResponseMechanism::Additive)),
                )
                .unwrap();

            let reopened = fixture.reopen();
            let stored = SessionStore::new(&reopened)
                .unwrap()
                .get(&record.id)
                .unwrap()
                .expect("the session survived the reopen");

            assert_eq!(stored.harness, "claude-code");
            assert_eq!(stored.launch_profile.as_deref(), Some("a-launch-profile"));
            assert_eq!(
                stored.backend_resource.as_deref(),
                Some("a-backend-resource")
            );
            assert_eq!(stored.model, Some(AssignedModel::named("a-model")));
            assert_eq!(
                stored.pairing_class,
                Some(SessionPairingClass::ProtocolCompatible)
            );
            assert_eq!(stored.protocol, Some(SessionProtocol::OpenAiResponses));
            assert_eq!(stored.response_profile, Some(profile));
            assert_eq!(stored.response_mechanism, Some(ResponseMechanism::Additive));

            // And the columns themselves hold seven different strings. Reading
            // the row rather than the record, because a record built from one
            // column read twice would satisfy every assertion above.
            let raw: Vec<Option<String>> = reopened
                .query_row(
                    "SELECT harness, launch_profile, backend_resource, model, pairing_class, \
                     protocol, response_profile, response_mechanism FROM sessions WHERE id = ?1",
                    [record.id.as_str()],
                    |row| {
                        Ok((0..8)
                            .map(|i| row.get::<_, Option<String>>(i).unwrap())
                            .collect())
                    },
                )
                .unwrap();
            let mut seen = std::collections::BTreeSet::new();
            for value in &raw {
                let value = value.as_deref().expect("every column was written");
                assert!(
                    seen.insert(value.to_owned()),
                    "two session columns hold `{value}`; the seven facts line 645 \
                     keeps apart have started sharing a slot:\n{raw:?}"
                );
            }
        }

        /// Line 646, at the only door there is.
        ///
        /// A provider and the gateway are not integrations at all, so they cannot
        /// even be named; `cmux`, `ollama` and `llama.cpp` are integrations and
        /// still not harnesses. None of the five may own a session, and the
        /// refusal happens before an identifier is minted, so nothing is left
        /// behind.
        #[test]
        fn only_a_real_harness_may_own_a_session() {
            let tmp = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(tmp.path(), "alpha");
            let store = fixture.store();

            for owner in ["cmux", "ollama", "llama-cpp"] {
                let err = store.create(NewSession::embedded(owner)).unwrap_err();
                assert!(
                    matches!(err, SessionStoreError::NotAHarness { .. }),
                    "`{owner}` is not a harness and must be refused as one, got: {err}"
                );
            }
            for backend in [
                "openai",
                "anthropic",
                "openrouter",
                "glasshouse-gateway",
                "",
            ] {
                let err = store.create(NewSession::embedded(backend)).unwrap_err();
                assert!(
                    matches!(err, SessionStoreError::UnknownHarness { .. }),
                    "`{backend}` is a backend, never a session owner, got: {err}"
                );
            }
            assert!(
                store.list().unwrap().is_empty(),
                "a refused session must leave no row behind"
            );

            // And every real harness is still accepted, so the guard is a filter
            // rather than a wall.
            for harness in ["claude-code", "codex", "opencode", "cursor", "pi", "hermes"] {
                store
                    .create(NewSession::embedded(harness))
                    .unwrap_or_else(|err| panic!("`{harness}` is a harness: {err}"));
            }
        }

        /// Line 650. The rename writes one column; the identifier a resume
        /// depends on is read back afterwards and is the one it was before.
        #[test]
        fn renaming_a_session_leaves_its_native_identifier_alone() {
            let tmp = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(tmp.path(), "alpha");
            let store = fixture.store();

            let record = store
                .create(
                    NewSession::embedded("claude-code").with_native_session_id(Some(
                        "d4c3b2a1-0000-4000-8000-000000000001".to_owned(),
                    )),
                )
                .unwrap();

            let renamed = store
                .rename(
                    &record.id,
                    &SessionName::parse("  the auth probe  ").unwrap(),
                )
                .unwrap();

            assert_eq!(
                renamed.display_name.as_ref().map(SessionName::as_str),
                Some("the auth probe"),
                "surrounding whitespace is trimmed rather than stored"
            );
            assert_eq!(
                renamed.native_session_id.as_deref(),
                Some("d4c3b2a1-0000-4000-8000-000000000001"),
                "a rename must not touch the identifier a resume continues from"
            );
            assert_eq!(renamed.id, record.id, "nor the Glasshouse identifier");

            // And it is still resumable afterwards, which is the consequence that
            // would actually bite a user.
            store
                .set_lifecycle(&record.id, SessionLifecycle::Stopped)
                .unwrap();
            let resumable = store.open_for_resume(&record.id).unwrap();
            assert_eq!(
                resumable.native_session_id,
                "d4c3b2a1-0000-4000-8000-000000000001"
            );
        }

        /// Renaming and tagging are things the *user* did, not things the session
        /// did, so neither counts as activity.
        ///
        /// The listing is ordered by `last_activity_at`. If a rename stamped it,
        /// naming an old session would jump it to the top of a list whose whole
        /// job is to say what ran most recently.
        #[test]
        fn naming_or_tagging_a_session_is_not_session_activity() {
            let tmp = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(tmp.path(), "alpha");
            let store = fixture.store_with_ticking_clock(1_000, 100);

            let record = store.create(NewSession::embedded("claude-code")).unwrap();
            let created_activity = record.last_activity_at;

            let renamed = store
                .rename(&record.id, &SessionName::parse("old work").unwrap())
                .unwrap();
            assert_eq!(renamed.last_activity_at, created_activity);

            let tagged = store
                .set_purpose(&record.id, &SessionPurpose::parse("research").unwrap())
                .unwrap();
            assert_eq!(tagged.last_activity_at, created_activity);

            store
                .set_lifecycle(&record.id, SessionLifecycle::Stopped)
                .unwrap();
            let closed = store.close(&record.id).unwrap();
            assert_eq!(
                closed.last_activity_at,
                store.get(&record.id).unwrap().unwrap().last_activity_at,
            );
            assert_ne!(
                closed.last_activity_at, created_activity,
                "the state change before it *was* activity, and did move the clock"
            );
        }

        /// Line 651. A name and a purpose are two columns and two types, and
        /// setting one leaves the other exactly as it was.
        #[test]
        fn a_name_and_a_purpose_are_two_different_things() {
            let tmp = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(tmp.path(), "alpha");
            let store = fixture.store();

            let record = store.create(NewSession::embedded("codex")).unwrap();
            store
                .rename(&record.id, &SessionName::parse("nightly").unwrap())
                .unwrap();
            let tagged = store
                .set_purpose(&record.id, &SessionPurpose::parse("tests").unwrap())
                .unwrap();

            assert_eq!(
                tagged.display_name.as_ref().map(SessionName::as_str),
                Some("nightly")
            );
            assert_eq!(
                tagged.purpose.as_ref().map(SessionPurpose::as_str),
                Some("tests")
            );

            let cleared = store.clear_purpose(&record.id).unwrap();
            assert_eq!(cleared.purpose, None);
            assert_eq!(
                cleared.display_name.as_ref().map(SessionName::as_str),
                Some("nightly"),
                "clearing a purpose must not clear a name"
            );

            let unnamed = store.clear_name(&record.id).unwrap();
            assert_eq!(unnamed.display_name, None);
            assert_eq!(unnamed.purpose, None);
        }

        /// A label a person typed is refused rather than repaired.
        #[test]
        fn an_unusable_label_is_refused_by_name() {
            assert!(SessionName::parse("   ").is_err());
            assert!(SessionPurpose::parse("").is_err());
            assert!(SessionName::parse("two\nlines").is_err());
            assert!(SessionPurpose::parse("a\tb").is_err());
            assert!(SessionName::parse(&"x".repeat(MAX_SESSION_NAME)).is_ok());
            assert!(SessionName::parse(&"x".repeat(MAX_SESSION_NAME + 1)).is_err());
            assert!(SessionPurpose::parse(&"x".repeat(MAX_SESSION_PURPOSE)).is_ok());
            assert!(SessionPurpose::parse(&"x".repeat(MAX_SESSION_PURPOSE + 1)).is_err());
            // Counted in characters, not bytes: a name of thirty-two emoji is
            // thirty-two characters and would be a hundred and twenty-eight bytes.
            assert!(SessionPurpose::parse(&"é".repeat(MAX_SESSION_PURPOSE)).is_ok());
        }

        /// Line 654. Closing writes one column and leaves the pointer to the
        /// harness's own history exactly where it was.
        #[test]
        fn closing_a_session_keeps_the_pointer_to_its_native_history() {
            let tmp = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(tmp.path(), "alpha");
            let store = fixture.store();

            let record = store
                .create(
                    NewSession::embedded("claude-code").with_native_session_id(Some(
                        "11112222-3333-4444-8555-666677778888".to_owned(),
                    )),
                )
                .unwrap();
            store
                .set_lifecycle(&record.id, SessionLifecycle::Stopped)
                .unwrap();

            let closed = store.close(&record.id).unwrap();
            assert_eq!(closed.lifecycle, SessionLifecycle::Closed);
            assert_eq!(
                closed.native_session_id.as_deref(),
                Some("11112222-3333-4444-8555-666677778888"),
                "closing a Glasshouse record must not throw away the name of the \
                 harness history it points at"
            );

            // Still there after a reopen, so what survived is the file rather
            // than a value the closing call happened to return.
            let reopened = fixture.reopen();
            let after = SessionStore::new(&reopened)
                .unwrap()
                .get(&record.id)
                .unwrap()
                .expect("a closed record is retired, never deleted");
            assert_eq!(
                after.native_session_id.as_deref(),
                Some("11112222-3333-4444-8555-666677778888")
            );
            assert_eq!(after.harness, "claude-code");
        }

        /// A record whose process is still running is not finished being written.
        #[test]
        fn a_live_session_cannot_be_closed() {
            let tmp = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(tmp.path(), "alpha");
            let store = fixture.store();

            let record = store.create(NewSession::embedded("claude-code")).unwrap();
            for live in [
                SessionLifecycle::Starting,
                SessionLifecycle::Running,
                SessionLifecycle::Idle,
                SessionLifecycle::WaitingForUser,
            ] {
                store.set_lifecycle(&record.id, live).unwrap();
                let err = store.close(&record.id).unwrap_err();
                assert!(
                    matches!(err, SessionStoreError::StillLive { .. }),
                    "a {live} session must be stopped before its record is closed, got: {err}"
                );
                assert_eq!(store.get(&record.id).unwrap().unwrap().lifecycle, live);
            }
        }

        /// Line 653. A stopped session with something to resume to is a different
        /// row from a live one and from a closed one, and closing moves it out of
        /// the resumable group without deleting it.
        #[test]
        fn a_resumable_session_stays_visible_and_apart_from_the_live_ones() {
            let tmp = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(tmp.path(), "alpha");
            let store = fixture.store();

            let live = store.create(NewSession::embedded("claude-code")).unwrap();
            store
                .set_lifecycle(&live.id, SessionLifecycle::Running)
                .unwrap();

            let stopped = store
                .create(
                    NewSession::embedded("codex")
                        .with_native_session_id(Some("codex-native-1".to_owned())),
                )
                .unwrap();
            store
                .set_lifecycle(&stopped.id, SessionLifecycle::Stopped)
                .unwrap();

            let listed = store.list().unwrap();
            assert_eq!(listed.len(), 2, "both are visible in one listing");
            let by_id = |id: &SessionId| {
                listed
                    .iter()
                    .find(|record| &record.id == id)
                    .unwrap()
                    .disposition()
            };
            assert_eq!(by_id(&live.id), SessionDisposition::Active);
            assert_eq!(by_id(&stopped.id), SessionDisposition::Resumable);

            store.close(&stopped.id).unwrap();
            let after = store.list().unwrap();
            assert_eq!(after.len(), 2, "a closed session is retired, not removed");
            assert_eq!(
                after
                    .iter()
                    .find(|record| record.id == stopped.id)
                    .unwrap()
                    .disposition(),
                SessionDisposition::Closed
            );
        }

        /// Every value the store can write is one the schema accepts.
        ///
        /// The `CHECK` constraints in migration 8 are second copies of three
        /// vocabularies. This is what keeps the copies honest: each variant is
        /// written through a real insert, so a slug that drifted from the schema
        /// fails here rather than on a background writer where nobody is looking.
        #[test]
        fn every_stored_vocabulary_is_one_the_schema_accepts() {
            let tmp = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(tmp.path(), "alpha");
            let store = fixture.store();

            let classes = [
                SessionPairingClass::VendorNative,
                SessionPairingClass::VendorSupported,
                SessionPairingClass::ProtocolNative,
                SessionPairingClass::ProtocolCompatible,
                SessionPairingClass::ProtocolTranslated,
                SessionPairingClass::Unknown,
            ];
            let protocols = [
                SessionProtocol::AnthropicMessages,
                SessionProtocol::OpenAiResponses,
                SessionProtocol::OpenAiChat,
                SessionProtocol::Unknown,
            ];
            let mechanisms = [
                ResponseMechanism::Native,
                ResponseMechanism::Additive,
                ResponseMechanism::NotApplied,
            ];

            for (i, class) in classes.iter().enumerate() {
                let protocol = protocols[i % protocols.len()];
                let mechanism = mechanisms[i % mechanisms.len()];
                let record = store
                    .create(
                        NewSession::embedded("claude-code")
                            .with_pairing_class(Some(*class))
                            .with_protocol(Some(protocol))
                            .with_response_mechanism(Some(mechanism))
                            .with_model(Some(AssignedModel::HarnessDefault)),
                    )
                    .unwrap_or_else(|err| panic!("the schema rejected {class}/{protocol}: {err}"));
                let read = store.get(&record.id).unwrap().unwrap();
                assert_eq!(read.pairing_class, Some(*class));
                assert_eq!(read.protocol, Some(protocol));
                assert_eq!(read.response_mechanism, Some(mechanism));
            }
            // The two the loop above could not reach by index.
            for protocol in protocols {
                for mechanism in mechanisms {
                    store
                        .create(
                            NewSession::embedded("codex")
                                .with_protocol(Some(protocol))
                                .with_response_mechanism(Some(mechanism)),
                        )
                        .unwrap_or_else(|err| panic!("the schema rejected {protocol}: {err}"));
                }
            }
        }

        /// "Glasshouse assigned no model" and "this build recorded no model" are
        /// two facts, and the column keeps them apart.
        #[test]
        fn a_harness_default_is_not_the_same_stored_fact_as_nothing_recorded() {
            let tmp = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(tmp.path(), "alpha");
            let store = fixture.store();

            let defaulted = store
                .create(
                    NewSession::embedded("claude-code")
                        .with_model(Some(AssignedModel::HarnessDefault)),
                )
                .unwrap();
            let unrecorded = store.create(NewSession::embedded("claude-code")).unwrap();
            let named = store
                .create(
                    NewSession::embedded("claude-code")
                        .with_model(Some(AssignedModel::named("harness-default"))),
                )
                .unwrap();

            assert_eq!(
                store.get(&defaulted.id).unwrap().unwrap().model,
                Some(AssignedModel::HarnessDefault)
            );
            assert_eq!(store.get(&unrecorded.id).unwrap().unwrap().model, None);
            // A model whose id is literally the sentinel word still reads back as
            // a named model, which is what the `named:` prefix is for.
            assert_eq!(
                store.get(&named.id).unwrap().unwrap().model,
                Some(AssignedModel::named("harness-default"))
            );
        }

        /// All 324 combinations of the five axes survive the round trip, so no
        /// axis is dropped or defaulted on the way through the column.
        #[test]
        fn every_response_profile_round_trips_through_one_column() {
            for verbosity in Verbosity::ALL {
                for audience in Audience::ALL {
                    for narration in Narration::ALL {
                        for evidence in EvidenceDetail::ALL {
                            for format in AnswerFormat::ALL {
                                let profile = ResponseProfile::new(
                                    *verbosity, *audience, *narration, *evidence, *format,
                                );
                                let encoded = encode_response_profile(&profile);
                                assert_eq!(
                                    decode_response_profile(&encoded),
                                    Some(profile),
                                    "`{encoded}` did not come back as the profile it was"
                                );
                            }
                        }
                    }
                }
            }
            // A partial encoding is refused rather than completed from defaults:
            // a profile a session never ran under, reported as though it had, is
            // worse than an error naming the column.
            assert_eq!(
                decode_response_profile("verbosity=terse,audience=plain"),
                None
            );
            assert_eq!(decode_response_profile("verbosity=nonsense"), None);
            assert_eq!(decode_response_profile(""), None);
        }

        /// A value this build cannot read is reported by column and value, never
        /// silently turned into `None` — which would say "nothing was recorded"
        /// about a row that recorded something.
        #[test]
        fn a_stored_value_this_build_cannot_read_is_reported_rather_than_erased() {
            let tmp = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(tmp.path(), "alpha");
            let store = fixture.store();
            let record = store.create(NewSession::embedded("claude-code")).unwrap();

            // Written straight into the column, as a newer build's value would
            // arrive: the schema's `CHECK` is what stops this build writing one.
            fixture
                .conn
                .execute(
                    "UPDATE sessions SET response_profile = 'verbosity=galactic' WHERE id = ?1",
                    [record.id.as_str()],
                )
                .unwrap();

            let err = store.get(&record.id).unwrap_err();
            match err {
                SessionStoreError::UnknownValue { column, value, .. } => {
                    assert_eq!(column, "response_profile");
                    assert_eq!(value, "verbosity=galactic");
                }
                other => panic!("expected the column and the value to be named, got: {other}"),
            }
        }

        /// Migration 8 applies to a database created by the previous schema, and
        /// every existing row survives it unchanged.
        ///
        /// The rollback is contiguous to the newest migration for the reason
        /// `upgrading_a_version_2_database_preserves_every_existing_session`
        /// records: the runner resumes from `MAX(version)`, so leaving a higher
        /// row behind makes it believe there is nothing to do.
        #[test]
        fn upgrading_a_version_7_database_preserves_every_existing_session() {
            let tmp = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(tmp.path(), "alpha");
            let store = fixture.store();

            let record = store
                .create(
                    NewSession::embedded("codex")
                        .with_role(SessionRole::Orchestrator)
                        .with_presentation(SessionPresentation::Headless)
                        .with_native_session_id(Some("codex-native-7".to_owned()))
                        .with_launch_profile(Some("nightly".to_owned()))
                        .with_backend_resource(Some("direct:openai".to_owned())),
                )
                .unwrap();
            store
                .set_lifecycle(&record.id, SessionLifecycle::Stopped)
                .unwrap();
            let before = store.get(&record.id).unwrap().unwrap();

            fixture
                .conn
                .execute_batch(&format!(
                    // Migration 10's columns go first, for the same reason
                    // migration 8's sessions columns are dropped below: this
                    // rollback lands on version 7, and `memories` must not
                    // still carry columns a later migration added.
                    "{UNDO_MIGRATIONS_ABOVE_THIRTEEN}
                     ALTER TABLE memories DROP COLUMN superseded_reason;
                     ALTER TABLE memories DROP COLUMN validity_conditions;
                     ALTER TABLE memories DROP COLUMN invalidation_conditions;
                     ALTER TABLE memories DROP COLUMN review_reason;
                     ALTER TABLE memories DROP COLUMN review_marked_at;
                     ALTER TABLE memories DROP COLUMN last_validated_at;
                     ALTER TABLE sessions DROP COLUMN model;
                     ALTER TABLE sessions DROP COLUMN pairing_class;
                     ALTER TABLE sessions DROP COLUMN protocol;
                     ALTER TABLE sessions DROP COLUMN response_profile;
                     ALTER TABLE sessions DROP COLUMN response_mechanism;
                     ALTER TABLE sessions DROP COLUMN display_name;
                     ALTER TABLE sessions DROP COLUMN purpose;
                     ALTER TABLE sessions DROP COLUMN process_id;
                     ALTER TABLE sessions DROP COLUMN process_started_at;
                     ALTER TABLE sessions DROP COLUMN process_host;
                     ALTER TABLE sessions DROP COLUMN supervision;
                     ALTER TABLE sessions DROP COLUMN supervision_reason;
                     ALTER TABLE sessions DROP COLUMN source_session_id;
                     DROP TABLE IF EXISTS routing_observations;
                 DROP TABLE IF EXISTS evaluation_observations;
                 DROP TABLE IF EXISTS memory_files;
                 DELETE FROM schema_migrations WHERE version >= 8;"
                ))
                .unwrap();

            let reopened = fixture.reopen();
            let version: i64 = reopened
                .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(
                version, 20,
                "the launch must have applied migrations 8 through 20"
            );

            let after = SessionStore::new(&reopened)
                .unwrap()
                .get(&record.id)
                .unwrap()
                .expect("the pre-migration session must survive");

            // Everything migration 7 knew about, byte for byte.
            assert_eq!(after.id, before.id);
            assert_eq!(after.project_id, before.project_id);
            assert_eq!(after.harness, before.harness);
            assert_eq!(after.native_session_id, before.native_session_id);
            assert_eq!(after.role, before.role);
            assert_eq!(after.lifecycle, before.lifecycle);
            assert_eq!(after.presentation, before.presentation);
            assert_eq!(after.created_at, before.created_at);
            assert_eq!(after.last_activity_at, before.last_activity_at);
            assert_eq!(after.launch_profile, before.launch_profile);
            assert_eq!(after.backend_resource, before.backend_resource);

            // And the seven new columns are NULL rather than guessed at: a
            // session recorded before migration 8 ran under a response profile
            // Glasshouse never wrote down, which is a different fact from having
            // run the default one.
            assert_eq!(after.model, None);
            assert_eq!(after.pairing_class, None);
            assert_eq!(after.protocol, None);
            assert_eq!(after.response_profile, None);
            assert_eq!(after.response_mechanism, None);
            assert_eq!(after.display_name, None);
            assert_eq!(after.purpose, None);

            // The upgraded database is fully usable: the old row can still be
            // renamed and tagged, and a new session records all seven.
            let migrated_store = SessionStore::new(&reopened).unwrap();
            let renamed = migrated_store
                .rename(&record.id, &SessionName::parse("survivor").unwrap())
                .unwrap();
            assert_eq!(
                renamed.display_name.as_ref().map(SessionName::as_str),
                Some("survivor")
            );
            assert_eq!(renamed.native_session_id, before.native_session_id);
        }
    }

    mod phase_40 {
        //! Phase 40 line 1646 — which session, if any, a session was
        //! bootstrapped from.
        //!
        //! `main.rs::resolve_bootstrap_prompt` and `launch_session` are
        //! exercised against the shipped binary in `tests/handoff_lines.rs`
        //! (the positive case, once per harness pair, and the negative case
        //! of an ordinary launch). What is here is what only the store can
        //! answer: that the column round-trips on its own, and that a
        //! database written before this migration still reads back —
        //! `upgrading_a_version_7_database_preserves_every_existing_session`'s
        //! own reasoning, one migration later.
        use super::*;

        #[test]
        fn a_recorded_source_session_round_trips() {
            let tmp = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(tmp.path(), "alpha");
            let store = fixture.store();

            let source = store.create(NewSession::embedded("claude-code")).unwrap();
            let target = store
                .create(NewSession::embedded("codex").with_source_session(Some(source.id.clone())))
                .unwrap();
            assert_eq!(target.source_session_id, Some(source.id.clone()));

            let read_back = store.get(&target.id).unwrap().unwrap();
            assert_eq!(read_back.source_session_id, Some(source.id));
        }

        /// The negative case, and it matters as much as the positive one: a
        /// session started without naming a checkpoint must record no
        /// source, never an invented one.
        #[test]
        fn a_session_not_started_from_a_checkpoint_has_no_source() {
            let tmp = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(tmp.path(), "alpha");
            let record = fixture
                .store()
                .create(NewSession::embedded("claude-code"))
                .unwrap();
            assert_eq!(record.source_session_id, None);
        }

        /// Migration 12 applies to a database created by the previous
        /// schema, and every existing row survives it unchanged — the same
        /// contiguous rollback `upgrading_a_version_7_database_preserves_
        /// every_existing_session` uses, one migration later.
        #[test]
        fn upgrading_a_version_11_database_preserves_every_existing_session() {
            let tmp = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(tmp.path(), "alpha");
            let store = fixture.store();

            let record = store
                .create(
                    NewSession::embedded("claude-code")
                        .with_role(SessionRole::Worker)
                        .with_native_session_id(Some("native-pre-12".to_owned())),
                )
                .unwrap();
            let before = store.get(&record.id).unwrap().unwrap();

            fixture
                .conn
                .execute_batch(&format!(
                    "{UNDO_MIGRATIONS_ABOVE_THIRTEEN}
                     ALTER TABLE sessions DROP COLUMN source_session_id;
                     ALTER TABLE memories DROP COLUMN superseded_reason;
                     DELETE FROM schema_migrations WHERE version >= 12;"
                ))
                .unwrap();

            let reopened = fixture.reopen();
            let version: i64 = reopened
                .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(
                version, 20,
                "the reopen must have applied migrations 12 through 20"
            );

            let after = SessionStore::new(&reopened)
                .unwrap()
                .get(&record.id)
                .unwrap()
                .expect("the pre-migration session must survive");

            assert_eq!(after.id, before.id);
            assert_eq!(after.harness, before.harness);
            assert_eq!(after.native_session_id, before.native_session_id);
            assert_eq!(after.role, before.role);
            assert_eq!(after.lifecycle, before.lifecycle);

            // A session recorded before migration 12 ran has no recorded
            // source — a different fact from having been started fresh by a
            // build that could name one, but the column cannot and must not
            // distinguish them.
            assert_eq!(after.source_session_id, None);
        }
    }
}

#[cfg(test)]
mod display_tests {
    use super::*;

    /// A `Display` impl that writes straight to the formatter ignores width,
    /// which turns any aligned listing into ragged columns. Cheap to get
    /// wrong, invisible in a round-trip test, so it gets its own check.
    #[test]
    fn stored_values_honour_format_width_so_listings_align() {
        assert_eq!(format!("[{:<10}]", SessionRole::Normal), "[normal    ]");
        assert_eq!(
            format!("[{:<10}]", SessionRole::Orchestrator),
            "[orchestrator]"
        );
        assert_eq!(
            format!("[{:<10}]", SessionPresentation::Embedded),
            "[embedded  ]"
        );
        assert_eq!(
            format!("[{:<20}]", SessionLifecycle::WaitingForUser),
            "[waiting_for_user    ]"
        );
        assert_eq!(format!("[{:<6}]", SessionId::new("ab")), "[ab    ]");
    }
}
