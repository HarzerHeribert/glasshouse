//! Running a real native harness as a Glasshouse session.
//!
//! A session is a real installed harness in a real pseudo-terminal, started
//! inside the active project root and never anywhere else. [`fn@select`]
//! decides which harness and executable; [`fn@attach`] hands it the
//! terminal. Both go through [`crate::launch::HarnessLaunch`], the only
//! sanctioned way to start a harness. [`store`] is Glasshouse's own durable
//! record of this project's sessions, independent of any harness's own
//! files. [`native_id`] finds a self-naming harness's own session identifier
//! after the session ends.
//!
//! [`lifecycle`], [`api`] and [`recovery`] sit on top and speak only
//! [`crate::events`], never a harness's own vocabulary. [`supervision`] is
//! the odd one out: not what a session *is* or *said*, but whether its
//! process is still there — it adopts what it can verify, quarantines what
//! it cannot, and never ends anything.
// History: design-decisions.md, "Trims: session module docs, second packet", session/mod.rs module doc.

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
    AdvisoryCacheState, CacheState, CheckpointRecency, FileClaim, LabelError, NewSession,
    ProjectSessions, ResponseMechanism, ResumableSession, STALE_CLAIM_AFTER, SessionContext,
    SessionDisposition, SessionId, SessionLifecycle, SessionName, SessionPairingClass,
    SessionPresentation, SessionProtocol, SessionPurpose, SessionRecord, SessionRole, SessionStore,
    SessionStoreError, SupervisionRecord, TASK_PROGRESS_EXPIRES_AFTER, TaskContinuity,
    TaskProgressDeclaration,
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
        // Phase 56 T3, and a recorded LIMIT rather than a mapping.
        //
        // The stored vocabulary is fixed by migration 8's
        // `CHECK (protocol IN (…))`, and widening a `CHECK` in SQLite means
        // rebuilding the table — which that migration's own comment refuses
        // without a migration written for it. So a session served through a
        // Gemini-serving entitlement records `unknown`, which is a recorded
        // answer ("nothing was established") and not the truth here.
        //
        // Reachable today: a `DirectProvider` profile on a provider that
        // declares only `gemini-generate-content` takes that as its route
        // protocol (`config::pairing`). Nothing downstream reads the column
        // to make a decision — `glasshouse sessions show` renders it — so
        // the cost is a wrong word in a listing rather than a wrong route.
        // Successor: the migration that adds `gemini-generate-content` to
        // the column's vocabulary, with `SessionProtocol`'s own variant.
        Some(WireProtocol::GeminiGenerateContent) => SessionProtocol::Unknown,
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

/// Whether `harness` may own a Glasshouse session — line 646: every
/// interactive session must be owned by a real harness.
///
/// A provider (`openai`, `anthropic`) or the Glasshouse gateway is not an
/// integration at all and fails as unknown. `cmux` (a multiplexer), `ollama`
/// and `llama.cpp` (local inference) are known integrations but not
/// harnesses, and fail as such — the three [`crate::harness::adapter_for`]
/// answers `None` for.
///
/// Not a schema `CHECK`: the harness list lives in `crate::integrations` and
/// grows, and a copy in a migration would need one every time it does —
/// migration 2 made the same call for the `harness` column.
// History: design-decisions.md, "Trims: session module docs, second packet", session/mod.rs `owning_harness`.
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

#[cfg(test)]
mod role_is_inert_tests {
    /// Phase 14's second box, structurally: *"keep an orchestrator session
    /// otherwise identical to a normal native harness session."*
    ///
    /// This is an **absence** claim, and `orchestrator-role`'s audit closed it
    /// by reading every `SessionRole` site in the crate and finding none in the
    /// lifecycle path. That reading was correct — re-verified by the integrator
    /// — but an audit protects nothing against the next edit. `routing::
    /// interactive` already carries this project's pattern for exactly this
    /// shape of claim (`the_assignment_is_not_a_session_of_its_own`), and this
    /// is that pattern applied to the file the map's line is about.
    ///
    /// The list below is the map's own allowance read narrowly: a role may
    /// change how an answer *reads* (`config::response`), how a session is
    /// *displayed* (`shell/**`), and — since map line 2414 — *who a
    /// coordination notice names as its recipient*
    /// (`commands::hook::notify_orchestrator_of_conflict`, which reads
    /// `SessionRole` to find the one live orchestrator to tell about a file
    /// conflict; nothing about the session's own lifecycle changes). Nothing
    /// else. If a future edit makes the launch path branch on a role, this
    /// fails and the box reopens — which is the whole point of writing it
    /// down as a test rather than a paragraph.
    const LIFECYCLE_FILES: [(&str, &str); 5] = [
        ("session/runtime.rs", include_str!("runtime.rs")),
        ("session/lifecycle.rs", include_str!("lifecycle.rs")),
        ("session/attach.rs", include_str!("attach.rs")),
        ("session/select/mod.rs", include_str!("select/mod.rs")),
        ("session/native_id/mod.rs", include_str!("native_id/mod.rs")),
    ];

    /// Production half only, comments stripped — `routing::interactive`'s own
    /// helper, copied rather than shared because these two modules have no
    /// dependency on each other and a test helper is not worth creating one.
    ///
    /// **Scans by `str::lines` on purpose (§14).** `include_str!` reads the
    /// file as checked out, so on a runner whose Git converts line endings the
    /// source carries `\r\n`; `lines()` strips the carriage return, while any
    /// search for a multi-line literal would silently find nothing and the
    /// guard would pass by failing to look.
    fn production_code(source: &str) -> String {
        source
            .split("#[cfg(test)]")
            .next()
            .expect("split always yields at least one part")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_sessions_role_never_reaches_its_lifecycle() {
        for (name, source) in LIFECYCLE_FILES {
            let code = production_code(source);
            assert!(
                !code.contains("SessionRole"),
                "{name} names `SessionRole`: an orchestrator session has stopped being \
                 identical to a normal one, which is Phase 14's second box. A role may \
                 change how an answer reads (`config::response`), how a session is \
                 displayed (`shell/**`), and who a coordination notice names as its \
                 recipient (`commands::hook`) — nothing else."
            );
        }
    }

    /// §14's second rule: an LF checkout never exercises the CRLF path, so the
    /// scan is tested against a CRLF copy of its own input. Without this the
    /// guard is untested precisely where it was once needed — a source scan
    /// took Windows CI red on this project in August for this exact reason.
    #[test]
    fn the_scan_reads_the_same_source_with_windows_line_endings() {
        for (name, source) in LIFECYCLE_FILES {
            let lf = production_code(source);
            let crlf = production_code(&source.replace('\n', "\r\n"));
            assert_eq!(
                lf.contains("SessionRole"),
                crlf.contains("SessionRole"),
                "{name}: the scan disagrees with itself across line endings, so it would \
                 pass on a CRLF checkout by failing to look"
            );
        }
    }
}
