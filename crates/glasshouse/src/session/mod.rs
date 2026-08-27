//! Running a real native harness as a Glasshouse session.
//!
//! A session is a real installed harness in a real pseudo-terminal, started
//! inside the active project root and never anywhere else. Opening one has
//! two halves, and this module holds both:
//!
//! - [`fn@select`] decides *which* harness and *which* executable, refusing
//!   ambiguity rather than guessing;
//! - [`fn@attach`] hands the terminal to it and stays out of the way.
//!
//! [`store`] holds the third: Glasshouse's own durable record of the sessions
//! in this project, kept independently of whatever session files the harness
//! writes for itself.
//!
//! Selecting and attaching both go through [`crate::launch::HarnessLaunch`],
//! the only sanctioned way to start a harness: it derives the child's working
//! directory from the active project and offers no way to override it.
//!
//! [`native_id`] is a fourth, smaller piece: for a harness that names its own
//! sessions rather than accepting one Glasshouse assigns, it finds that
//! identifier after the session ends and records it in [`store`].
//!
//! Three more pieces sit on top of those, and all three speak
//! [`crate::events`] rather than any harness's vocabulary:
//!
//! - [`lifecycle`] is the crate's **only** translator from a harness's own
//!   event names into Glasshouse's;
//! - [`api`] is the internal surface for driving and inspecting a live
//!   session — send, interrupt, query, list, read recent output — and the
//!   place a machine-originated message is distinguished from a keystroke;
//! - [`recovery`] decides what may happen to a task whose session died, and
//!   refuses rather than guesses when it cannot tell.
//!
//! [`supervision`] is the fourth, and it is about a different question from
//! all of them: not what a session *is* or what it *said*, but whether the
//! process it was started in is still there. It discovers what this project
//! recorded, verifies each process against the identity recorded for it,
//! adopts what it can verify, quarantines what is alive and unaccounted for,
//! and refuses to start a second session beside either. It never ends
//! anything.

pub mod api;
pub mod attach;
pub mod lifecycle;
pub mod native_id;
pub mod recovery;
pub mod runtime;
pub mod select;
pub mod store;
pub mod supervision;

pub use attach::attach;
pub use lifecycle::{event_for, lifecycle_for, may_apply, observe};
pub use runtime::{
    CrashReport, HEALTHY_AFTER, LiveSession, MAX_CONSECUTIVE_RESTARTS, RuntimeError, Scrollback,
    SessionRuntime, StartRefused,
};
pub use select::{ExecutableSource, HarnessSelection, SelectionError, select};
pub use store::{
    LabelError, NewSession, ProjectSessions, ResponseMechanism, ResumableSession,
    SessionDisposition, SessionId, SessionLifecycle, SessionName, SessionPairingClass,
    SessionPresentation, SessionProtocol, SessionPurpose, SessionRecord, SessionRole, SessionStore,
    SessionStoreError, SupervisionRecord,
};
pub use supervision::{
    ProcessIdentity, ProcessState, SupervisedSession, Supervision, SupervisionRefusal,
    SupervisionReport, Verdict,
};

// ------------------------------------------------------------------
// The boundary between what a session *records* and what the rest of
// Glasshouse *computes*.
//
// Phase 6 line 294 — "keep adapter-specific parsing isolated from the core
// Glasshouse session model", checked, and guarded by a source scan over
// `store.rs` in `harness::tests::the_session_model_depends_on_no_adapter` —
// means `store` may not name `crate::harness` at all. So the three
// conversions below live here, in the module root, where `native_id` and
// `select` already depend on `crate::harness` legitimately.
//
// The constraint turned out to describe a real distinction rather than
// merely forbidding an import. A stored vocabulary and a live one have
// different lifetimes: the schema's `CHECK` fixes the words a row may hold,
// and a row written by an older build has to stay readable when the live
// enum grows. Each function below is total and exhaustive over the *live*
// type, so adding a variant there is a compile error here — at the one place
// somebody has to decide what it means on disk.
// ------------------------------------------------------------------

use crate::harness::WireProtocol;
use crate::harness::pairing::PairingClass;
use crate::harness::response::AppliedMechanism;
use crate::integrations::{IntegrationId, IntegrationKind};

/// How a session's wire protocol is recorded.
///
/// `None` — the profile named no protocol and none could be established — is
/// recorded as [`SessionProtocol::Unknown`], which is an answer. It is not
/// the same as the column being NULL, which means nothing was recorded at
/// all.
pub fn session_protocol(protocol: Option<WireProtocol>) -> SessionProtocol {
    match protocol {
        Some(WireProtocol::AnthropicMessages) => SessionProtocol::AnthropicMessages,
        Some(WireProtocol::OpenAiResponses) => SessionProtocol::OpenAiResponses,
        Some(WireProtocol::OpenAiChat) => SessionProtocol::OpenAiChat,
        None => SessionProtocol::Unknown,
    }
}

/// The live protocol a recorded one names, or `None` when none was
/// established.
pub fn wire_protocol(protocol: SessionProtocol) -> Option<WireProtocol> {
    match protocol {
        SessionProtocol::AnthropicMessages => Some(WireProtocol::AnthropicMessages),
        SessionProtocol::OpenAiResponses => Some(WireProtocol::OpenAiResponses),
        SessionProtocol::OpenAiChat => Some(WireProtocol::OpenAiChat),
        SessionProtocol::Unknown => None,
    }
}

/// How a pairing class is recorded.
pub fn session_pairing_class(class: PairingClass) -> SessionPairingClass {
    match class {
        PairingClass::VendorNative => SessionPairingClass::VendorNative,
        PairingClass::VendorSupported => SessionPairingClass::VendorSupported,
        PairingClass::ProtocolNative => SessionPairingClass::ProtocolNative,
        PairingClass::ProtocolCompatible => SessionPairingClass::ProtocolCompatible,
        PairingClass::ProtocolTranslated => SessionPairingClass::ProtocolTranslated,
        PairingClass::Unknown => SessionPairingClass::Unknown,
    }
}

/// How the mechanism that carried a response profile is recorded.
///
/// The *category* only. `harness::response` owns what a mechanism is; a
/// session record stores which of the three kinds answered, so that
/// `glasshouse sessions show` can say it after the fact. Copying the
/// mechanism's own text into the project database would put a second copy of
/// one harness's vocabulary there, which is what line 603 forbids.
pub fn session_response_mechanism(mechanism: &AppliedMechanism) -> ResponseMechanism {
    match mechanism {
        AppliedMechanism::Native { .. } => ResponseMechanism::Native,
        AppliedMechanism::Additive { .. } => ResponseMechanism::Additive,
        AppliedMechanism::NotApplied { .. } => ResponseMechanism::NotApplied,
    }
}

/// Whether `harness` may own a Glasshouse session — line 646.
///
/// # What is being refused, and what cannot even be spelled
///
/// The map's requirement is that *every interactive Glasshouse session is
/// owned by a real harness*, and line 646 names the failure: a direct API
/// provider or a gateway represented as an interactive session in its own
/// right. A provider (`openai`, `anthropic`) and the Glasshouse gateway are
/// not integrations at all, so neither has a name this check could accept —
/// they fail as unknown. What *is* spellable and still wrong is one of the
/// integrations Glasshouse knows that is not a harness: `cmux` multiplexes
/// terminals, `ollama` and `llama.cpp` serve models. There is no session to
/// start in any of the three — they are exactly the three
/// [`crate::harness::adapter_for`] answers `None` for — so there is no
/// session record for one either.
///
/// # Not a `CHECK` in the schema
///
/// The list of harnesses lives in `crate::integrations` and grows. A copy of
/// it in a migration would need a migration every time a harness is added,
/// and would be a second place for it to be wrong. Migration 2 made the same
/// call for the `harness` column and gave the same reason.
pub fn owning_harness(harness: &str) -> Result<(), SessionStoreError> {
    let known = IntegrationId::ALL
        .iter()
        .copied()
        .find(|id| id.slug() == harness);
    match known {
        Some(id) if id.kind() == IntegrationKind::Harness => Ok(()),
        Some(id) => Err(SessionStoreError::NotAHarness {
            harness: harness.to_owned(),
            what: match id.kind() {
                IntegrationKind::Multiplexer => "a terminal multiplexer",
                IntegrationKind::LocalInference => "a local inference server",
                IntegrationKind::Harness => unreachable!("matched above"),
            },
        }),
        None => Err(SessionStoreError::UnknownHarness {
            harness: harness.to_owned(),
        }),
    }
}
