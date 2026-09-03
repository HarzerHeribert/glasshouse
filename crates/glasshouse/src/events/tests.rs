//! Tests for the event bus, the lifecycle vocabulary and the project event log.
//!
//! Moved out of `mod.rs` by Phase 59 line 2049. Every `include_str!` here is
//! relative to this directory -- the one `mod.rs` sits in -- so each scan reads
//! exactly what it read before the move.

use super::*;
use crate::session::{SessionId, SessionLifecycle};

fn bus() -> EventBus {
    EventBus::with_history_and_clock(64, std::sync::Arc::new(|| 1_000))
}

fn session(name: &str) -> SessionId {
    SessionId::new(name)
}

/// The capability map's standing rule, as behaviour: *do not infer
/// successful task completion solely because a child process became
/// quiet.*
///
/// This session did everything a quiet, cleanly finished one does. Its
/// output ended and its process exited with code zero, which is what a
/// harness does when the user types `/quit` in the middle of a task. No
/// harness ever said a turn finished, so Glasshouse does not know that one
/// did — and `None` is the only honest answer.
#[test]
fn a_quiet_process_that_exited_cleanly_reports_no_task_outcome() {
    let bus = bus();
    let id = session("quiet");
    bus.publish(&id, LifecycleEvent::SessionStarted);
    bus.publish(&id, LifecycleEvent::TurnStarted);
    bus.publish(&id, LifecycleEvent::OutputEnded);
    bus.publish(
        &id,
        LifecycleEvent::ProcessExited {
            exit: ProcessExit {
                code: 0,
                signal: None,
            },
        },
    );

    assert_eq!(
        task_outcome(&bus.history_for(&id)),
        None,
        "a clean exit and no more output is not the harness saying it finished"
    );
}

/// The other half of the same rule: when the harness *does* say so, that
/// is exactly what is reported. A test that only proved `None` would pass
/// on a function that always returned `None`.
#[test]
fn only_a_harness_report_says_the_work_finished() {
    let bus = bus();
    let id = session("reported");
    bus.publish(&id, LifecycleEvent::TurnStarted);
    bus.publish(
        &id,
        LifecycleEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
        },
    );
    assert_eq!(
        task_outcome(&bus.history_for(&id)),
        Some(TurnOutcome::Completed)
    );

    // And a turn that ended badly is reported as having ended badly,
    // never as unknown and never as completed.
    let failed = session("failed-turn");
    bus.publish(
        &failed,
        LifecycleEvent::TurnEnded {
            outcome: TurnOutcome::Failed,
        },
    );
    assert_eq!(
        task_outcome(&bus.history_for(&failed)),
        Some(TurnOutcome::Failed)
    );
}

/// A turn in flight erases an older verdict. Asking "did the work finish?"
/// about a session that has since started something else must not answer
/// with the previous turn.
#[test]
fn a_turn_in_flight_is_not_the_previous_turns_verdict() {
    let bus = bus();
    let id = session("busy");
    bus.publish(
        &id,
        LifecycleEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
        },
    );
    bus.publish(&id, LifecycleEvent::TurnStarted);
    assert_eq!(task_outcome(&bus.history_for(&id)), None);
}

/// Waiting for the user is not idle, and neither is ever inferred from
/// the other. Only one event in the whole enum implies `Idle`; if a
/// second ever does, this fails.
#[test]
fn waiting_for_user_is_a_state_of_its_own() {
    assert_eq!(
        LifecycleEvent::WaitingForUser.implied_state(),
        Some(SessionLifecycle::WaitingForUser)
    );

    let every = [
        LifecycleEvent::SessionStarted,
        LifecycleEvent::SessionResumed,
        LifecycleEvent::TurnStarted,
        LifecycleEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
        },
        LifecycleEvent::WaitingForUser,
        LifecycleEvent::TextDelivered {
            origin: MessageOrigin::Machine,
            bytes: 1,
        },
        LifecycleEvent::InterruptDelivered {
            origin: MessageOrigin::UserKeystroke,
        },
        LifecycleEvent::ProcessExited {
            exit: ProcessExit {
                code: 0,
                signal: None,
            },
        },
        LifecycleEvent::OutputEnded,
        LifecycleEvent::GatewayUnhealthy {
            resource: "r".to_owned(),
            reason: GatewayFailure::TimedOut,
        },
        LifecycleEvent::GatewayBackendChanged {
            provider: "p".to_owned(),
            model: "m".to_owned(),
            cause: "c".to_owned(),
        },
    ];
    let idle: Vec<&LifecycleEvent> = every
        .iter()
        .filter(|event| event.implied_state() == Some(SessionLifecycle::Idle))
        .collect();
    assert_eq!(
        idle.len(),
        1,
        "exactly one event may mean idle, and it is a turn ending: {idle:?}"
    );
    assert!(matches!(idle[0], LifecycleEvent::TurnEnded { .. }));
}

/// Neither a process ending nor its output ending moves a session's state
/// through translation. The exit path decides that, with the status in
/// hand — see [`ProcessExit::session_state`].
#[test]
fn an_exit_and_a_silence_imply_nothing_about_session_state() {
    assert_eq!(LifecycleEvent::OutputEnded.implied_state(), None);
    for exit in [
        ProcessExit {
            code: 0,
            signal: None,
        },
        ProcessExit {
            code: 137,
            signal: Some("SIGKILL".to_owned()),
        },
    ] {
        assert_eq!(
            LifecycleEvent::ProcessExited { exit }.implied_state(),
            None,
            "a translated exit must not race the operating system"
        );
    }
}

#[test]
fn a_crash_and_a_departure_are_different_session_states() {
    let clean = ProcessExit {
        code: 0,
        signal: None,
    };
    assert!(!clean.is_crash());
    assert_eq!(clean.session_state(), SessionLifecycle::Stopped);

    for crashed in [
        ProcessExit {
            code: 3,
            signal: None,
        },
        ProcessExit {
            code: 0,
            signal: Some("SIGKILL".to_owned()),
        },
    ] {
        assert!(crashed.is_crash(), "{crashed:?}");
        assert_eq!(crashed.session_state(), SessionLifecycle::Failed);
    }
}

/// A backend resource going unhealthy touches the sessions that were on
/// it and nothing else. A session with no recorded backend resource — a
/// harness talking to its own vendor on the user's own subscription — is
/// not Glasshouse's gateway's business, and degrading it would take away
/// a session that is working.
#[test]
fn degrading_a_gateway_leaves_unrelated_native_subscriptions_alone() {
    let bus = bus();
    let on_gateway = record("a", Some("glasshouse-gateway"));
    let native = record("b", None);
    let other_gateway = record("c", Some("some-other-backend"));
    let records = vec![on_gateway.clone(), native.clone(), other_gateway.clone()];

    let degradation = degrade_resource(
        &bus,
        &records,
        "glasshouse-gateway",
        GatewayFailure::TimedOut,
    );

    assert_eq!(degradation.affected, vec![on_gateway.id.clone()]);
    assert!(bus.history_for(&native.id).is_empty(), "native untouched");
    assert!(bus.history_for(&other_gateway.id).is_empty());

    let events = bus.history_for(&on_gateway.id);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].event(),
        &LifecycleEvent::GatewayUnhealthy {
            resource: "glasshouse-gateway".to_owned(),
            reason: GatewayFailure::TimedOut,
        }
    );
}

/// A gateway failing is not a harness process failing. The harness is
/// still running and still steerable, so nothing here may mark it dead.
#[test]
fn a_gateway_failure_never_ends_a_session() {
    let bus = bus();
    let affected = record("a", Some("gw"));
    degrade_resource(
        &bus,
        std::slice::from_ref(&affected),
        "gw",
        GatewayFailure::Unreachable,
    );

    for recorded in bus.history_for(&affected.id) {
        assert_eq!(
            recorded.event().implied_state(),
            None,
            "a gateway failure must not move a live session's state"
        );
    }
    assert_eq!(task_outcome(&bus.history_for(&affected.id)), None);
}

fn record(id: &str, backend_resource: Option<&str>) -> crate::session::SessionRecord {
    crate::session::SessionRecord {
        id: SessionId::new(id),
        project_id: "project".to_owned(),
        harness: "claude-code".to_owned(),
        native_session_id: None,
        role: crate::session::SessionRole::Normal,
        lifecycle: SessionLifecycle::Running,
        presentation: crate::session::SessionPresentation::Embedded,
        created_at: 1,
        last_activity_at: 2,
        launch_profile: None,
        backend_resource: backend_resource.map(str::to_owned),
        model: None,
        pairing_class: None,
        protocol: None,
        response_profile: None,
        response_mechanism: None,
        display_name: None,
        purpose: None,
        source_session_id: None,
        observed_compactions: None,
        presentation_ref: None,
        last_seen_commit: None,
        entitlement: None,
    }
}

// ---------------------------------------------------------------
// Source guards
//
// Both scan by `str::lines`, which strips a carriage return for us, and
// both are exercised against a CRLF copy of their own input below. A
// multi-line literal search would find nothing on a checkout where Git
// converted line endings, and would do it silently — see
// `docs/product/design-decisions.md`, "A source-scanning guard reads by
// lines".
// ---------------------------------------------------------------

/// Production source of a module: everything before its
/// `#[cfg(test)] mod tests` block, with comment lines removed.
///
/// **Not** "everything before the first `#[cfg(test)]`", which is what
/// this helper did until a mutation caught it. `session/runtime.rs`
/// carries a `#[cfg(test)] const` two hundred lines in, so cutting at the
/// first attribute read a fifth of the file and silently exempted the
/// rest — including the exit path, which is exactly where a forbidden
/// inference would be written. A `TurnEnded` planted there survived the
/// scan. Anchoring on the attribute that actually introduces `mod tests`
/// is what makes the scan cover what it claims to cover.
///
/// Reads by `str::lines`, which strips a carriage return for us, so the
/// scan is blind to line endings by construction rather than by anyone
/// remembering — see `docs/product/design-decisions.md`, "A source-scanning
/// guard reads by lines".
fn production_code(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let end = lines
        .windows(2)
        .position(|pair| {
            pair[0].trim_end() == "#[cfg(test)]" && pair[1].trim_end().starts_with("mod tests")
        })
        .unwrap_or(lines.len());
    lines[..end]
        .iter()
        .filter(|line| !line.trim_start().starts_with("//"))
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

const EVENT_MODULES: [(&str, &str); 3] = [
    ("events/mod.rs", include_str!("mod.rs")),
    ("events/bus.rs", include_str!("bus.rs")),
    ("events/log.rs", include_str!("log.rs")),
];

/// "Adapters translate native observations into core events; consumers
/// must not create competing harness-specific lifecycle architectures."
///
/// The core stream is delivered to the orchestration layer without
/// coupling it to any harness, and this is what keeps that true: if the
/// word `codex` ever appears in production code here, some consumer can
/// branch on it, and the single normalized stream has quietly become two.
///
/// `crate::session::lifecycle` is the one module allowed to know a
/// harness's vocabulary, and it converts *into* these types.
#[test]
fn no_harness_is_named_in_the_core_event_stream() {
    for (name, source) in EVENT_MODULES {
        let code = production_code(source).to_ascii_lowercase();
        for harness in [
            "claude",
            "codex",
            "antigravity",
            "opencode",
            "cursor",
            "gemini",
        ] {
            assert!(
                !code.contains(harness),
                "{name} names `{harness}`; the core event stream must be \
                 harness-independent, and a consumer that can see a harness \
                 here will eventually branch on it"
            );
        }
    }
}

/// The one mechanism that makes the map's standing rule impossible to
/// break by accident rather than merely absent.
///
/// `LifecycleEvent::TurnEnded` is the only event carrying a claim about
/// the work itself, and it is constructed in exactly one production
/// function in the crate: the harness translator in
/// `crate::session::lifecycle`, whose entire input is an event name a
/// harness reported. A second construction site anywhere — in the exit
/// path, in a quiet-timer, in a consumer — fails this test, which is the
/// point: writing the forbidden inference requires deleting a test that
/// says why it is forbidden.
#[test]
fn turn_completion_is_minted_in_exactly_one_place() {
    let sites = mint_sites();
    assert!(
        !sites.is_empty(),
        "the scan found no construction at all, so it is proving nothing"
    );
    let files: std::collections::BTreeSet<&str> = sites.iter().map(|(file, _)| *file).collect();
    assert_eq!(
        files,
        ["session/lifecycle.rs"].into_iter().collect(),
        "`LifecycleEvent::TurnEnded` may be constructed only by the harness \
         translator, whose whole input is an event name a harness reported. \
         A second site is the forbidden inference growing a home: {sites:#?}"
    );
}

/// Every place a `LifecycleEvent::TurnEnded` value is *built*.
///
/// A match arm mentions the variant without building one, so it is
/// excluded by the position of the `=>`: in an arm the fat arrow comes
/// after the pattern, and in a construction any `=>` on the line comes
/// before the expression being built. The bare variant name in the enum's
/// own declaration carries no `LifecycleEvent::` path and is excluded by
/// that.
fn mint_sites() -> Vec<(&'static str, String)> {
    let mut sites = Vec::new();
    for (name, source) in SOURCES_THAT_MAY_NOT_MINT_A_TURN {
        for line in production_code(source).lines() {
            sites.extend(is_mint_site(line).then(|| (name, line.trim().to_owned())));
        }
    }
    sites
}

fn is_mint_site(line: &str) -> bool {
    let Some(at) = line.find("LifecycleEvent::TurnEnded") else {
        return false;
    };
    !line[at..].contains("=>")
}

/// Every production module that could plausibly reach for a turn verdict:
/// the event stream itself, the runtime that watches processes exit, the
/// API that drives sessions, and the recovery planner that reasons about
/// failed ones. The translator is in the list so the one legitimate site
/// is counted rather than exempted.
const SOURCES_THAT_MAY_NOT_MINT_A_TURN: [(&str, &str); 6] = [
    ("events/mod.rs", include_str!("mod.rs")),
    ("events/bus.rs", include_str!("bus.rs")),
    (
        "session/lifecycle.rs",
        include_str!("../session/lifecycle.rs"),
    ),
    ("session/runtime.rs", include_str!("../session/runtime.rs")),
    ("session/api/mod.rs", include_str!("../session/api/mod.rs")),
    (
        "session/recovery.rs",
        include_str!("../session/recovery.rs"),
    ),
];

/// The scan must actually read the file it names.
///
/// This is the assertion that would have caught the defect a surviving
/// mutation found: `production_code` used to cut at the *first*
/// `#[cfg(test)]`, and `session/runtime.rs` has one two hundred lines in,
/// so the guard above was silently reading a fifth of its target and
/// passing for that reason. A source scan whose reach nobody checked is
/// the third way this project has produced a test that passed for the
/// wrong reason.
#[test]
fn the_scan_reaches_the_end_of_every_file_it_claims_to_read() {
    // The last item in each file's production code. Named rather than
    // counted: a ratio would say nothing about a module that is mostly
    // tests, and it is *reach past an early `#[cfg(test)]`* that was
    // broken, which an anchor states directly.
    let anchors = [
        (
            "session/runtime.rs",
            include_str!("../session/runtime.rs"),
            "fn short(",
        ),
        ("events/bus.rs", include_str!("bus.rs"), "fn system_clock("),
        (
            "events/mod.rs",
            include_str!("mod.rs"),
            "pub fn task_outcome(",
        ),
        (
            "session/lifecycle.rs",
            include_str!("../session/lifecycle.rs"),
            "pub fn may_apply(",
        ),
    ];
    for (name, source, anchor) in anchors {
        assert!(
            source.contains(anchor),
            "{name}: the anchor `{anchor}` is gone, so this test has \
             stopped proving anything — pick a new one at the end of the \
             production code"
        );
        assert!(
            production_code(source).contains(anchor),
            "{name}: the scan stops before `{anchor}`, so the code between \
             there and wherever it stopped is exempt from a guard that \
             claims to cover the file"
        );
    }

    // And the scan must stop at the test module rather than run into it.
    for (name, source) in SOURCES_THAT_MAY_NOT_MINT_A_TURN {
        let code = production_code(source);
        assert!(
            !code.contains("mod tests {"),
            "{name}: the scan ran into the test module, so a construction \
             written in a test would be counted as production"
        );
    }
}

/// Both scans must be blind to line endings, and the only way to know
/// that is to run them against a CRLF copy. An LF checkout never
/// exercises the broken path, so without this the property is untested
/// exactly where it was needed.
#[test]
fn the_source_guards_are_blind_to_line_endings() {
    for (name, source) in SOURCES_THAT_MAY_NOT_MINT_A_TURN {
        // Build both sides from a normalised base, so the input does not
        // vary with how this file happened to be checked out.
        let lf = source.replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");

        let lf_code = production_code(&lf);
        let crlf_code = production_code(&crlf);
        assert_eq!(
            lf_code.lines().count(),
            crlf_code.lines().count(),
            "{name}: the scan reads a different number of lines under CRLF"
        );

        let count = |code: &str| code.lines().filter(|line| is_mint_site(line)).count();
        assert_eq!(
            count(&lf_code),
            count(&crlf_code),
            "{name}: the mint-site scan disagrees with itself under CRLF"
        );
    }
}
