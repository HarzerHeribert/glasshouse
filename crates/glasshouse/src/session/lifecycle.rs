//! Turning a harness's own lifecycle events into Glasshouse's.
//!
//! A harness reports what happened in its own vocabulary, and Glasshouse
//! records one of a handful of states that mean something to a session
//! overview. Claude Code and Codex happen to spell every shared event
//! identically, but that is a fact about the two installed binaries, not a
//! guarantee, so this module is deliberately the only place that knows
//! either vocabulary at all.
//!
//! An event this build has never heard of must leave the session exactly as
//! it was, because guessing a state from an unfamiliar name would show the
//! user a session that is idle when it is working, or working when it is
//! waiting for them.
//!
//! A slow hook process can deliver its event after the harness it belongs
//! to has exited; applying it would resurrect a stopped session in the
//! records, which is worse than losing a note about a session that has
//! already ended.
//!
//! History: design-decisions.md, "Trims: memory and session module docs", session/lifecycle.rs module doc.

use crate::events::{LifecycleEvent, RawObservation, TurnOutcome};
use crate::session::SessionLifecycle;

/// The Glasshouse lifecycle event a harness's own event means, or `None`
/// when the event says nothing Glasshouse models.
///
/// **This is the only place in the crate that knows a harness's vocabulary.**
/// Everything downstream sees [`LifecycleEvent`], which names no harness —
/// that is Phase 12's architectural requirement, and
/// `no_harness_is_named_in_the_core_event_stream` in [`crate::events`] is what
/// keeps a second translator from growing somewhere else.
///
/// It is also the only place that constructs
/// [`LifecycleEvent::TurnEnded`], the one event that carries a claim about
/// the work itself. Its input is a harness's own report and nothing else — no
/// exit status, no timer, no silence — so the inference the capability map
/// forbids has nowhere to be written. `turn_completion_is_minted_in_exactly_
/// one_place` fails if a second construction site appears.
///
/// Event names are the harness's own, exactly as its adapter declares them.
pub fn event_for(event: &str) -> Option<LifecycleEvent> {
    match event {
        // Codex only: the harness itself starting a session. Claude Code
        // 2.1.245 does not fire this event at all, so it never reaches this
        // function from that harness — see `session_start_is_not_among_the_
        // reported_events` in `harness/mod.rs`.
        //
        // Modelled as a turn starting rather than as
        // `LifecycleEvent::SessionStarted`: by the time a harness says this,
        // Glasshouse started the session itself and already recorded that.
        // Two events for one fact would put the same session start in the
        // stream twice with different timestamps.
        "SessionStart" => Some(LifecycleEvent::TurnStarted),
        // A prompt was submitted, so the session is working.
        "UserPromptSubmit" => Some(LifecycleEvent::TurnStarted),
        // The harness is asking the user to allow something and will not
        // proceed until they answer. Distinct from idle, and recorded only
        // because the harness said so.
        "PermissionRequest" => Some(LifecycleEvent::WaitingForUser),
        // The turn ended. `StopFailure` is a turn that ended badly, which is
        // a different fact from a session that died: both leave the session
        // alive and waiting for whatever comes next, and recording a failed
        // turn as a failed session would make a perfectly usable session look
        // dead in every listing.
        "Stop" => Some(LifecycleEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
        }),
        "StopFailure" => Some(LifecycleEvent::TurnEnded {
            outcome: TurnOutcome::Failed,
        }),
        // Codex only, and deliberately NOT translated. The operating system
        // reporting the child process exiting is the authority for a session
        // ending; a hook saying the same thing only adds a race against it,
        // one this separate hook process could lose by arriving late. Named
        // explicitly, rather than falling through to the wildcard below, so
        // the omission reads as a decision and not an oversight.
        "SessionEnd" => None,
        _ => None,
    }
}

/// Translate a harness's event and preserve the raw observation for
/// troubleshooting.
///
/// The debug line is written whether or not the event is recognised, because
/// an unrecognised event is exactly the case someone will be debugging: a
/// harness gained an event between releases and Glasshouse silently ignores
/// it, which is correct behaviour and invisible without this.
///
/// Only the harness's name for the event travels. A hook payload also carries
/// the user's prompt and the model's last message; Glasshouse's handler
/// drains that stream without reading it, so there is nothing here to leak.
/// See `docs/product/design-decisions.md`.
pub fn observe(harness: &str, event: &str) -> Option<LifecycleEvent> {
    RawObservation::new(harness, event).preserve();
    event_for(event)
}

/// The Glasshouse state a harness event implies, or `None` when the event
/// says nothing about the session's state.
///
/// A thin reading of [`event_for`]: the translation happens once, in one
/// place, and this is the answer to the narrower question the session store
/// asks. Two independent translations of the same vocabulary would eventually
/// disagree.
///
/// Event names are the harness's own, exactly as its adapter declares them.
pub fn lifecycle_for(event: &str) -> Option<SessionLifecycle> {
    event_for(event)?.implied_state()
}

/// Whether `event` is a harness saying it is about to compact its own
/// context — Phase 21's *"allow memory extraction to run before or around
/// native prompt compaction."*
///
/// A compaction is **not a `SessionLifecycle` state**: a session that
/// compacts was running before and is running after, and there is no
/// `LifecycleEvent` for it, so this is a predicate a *trigger* can ask
/// rather than a new `event_for` variant.
///
/// What is recorded, since map line 1159, is a **count** on the session
/// row — migration 16's `sessions.observed_compactions`, written by
/// [`crate::session::SessionStore::record_observed_compaction`] — not an
/// event beside everything else that happened.
///
/// `PostCompact` is a real Codex event Glasshouse asks for, but extraction
/// reads **this project's event log**, which a harness compacting its own
/// context does not change; `PreCompact` is the "before" the line names,
/// named explicitly so the omission reads as a decision.
/// Event names are the harness's own, exactly as its adapter declares them.
/// History: design-decisions.md, "Trims: memory and session module docs", session/lifecycle.rs precedes_native_compaction.
pub fn precedes_native_compaction(event: &str) -> bool {
    matches!(event, "PreCompact")
}

/// Whether `current` may be moved to `next` by a harness event.
///
/// Only a live session can change state this way. A session that has stopped,
/// failed, or been closed is finished, and a hook arriving afterwards — from a
/// process that outlived its harness — must not bring it back.
///
/// A genuine resume needs nothing here: `SessionStore::begin_resume` is
/// something Glasshouse *does*, at a boundary it opened, and a hook is an
/// event that merely *arrives*. Widening this predicate to let a resumed
/// session's stale hooks through would mean a hook arguing for its own
/// authority, which is the one thing the rule exists to refuse. Once a
/// resume has been recorded the session is live, and a live session follows
/// its harness.
///
/// History: design-decisions.md, "Trims: memory and session module docs", session/lifecycle.rs may_apply.
pub fn may_apply(current: SessionLifecycle, next: SessionLifecycle) -> bool {
    current.is_live() && current != next
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_codes_events_translate_to_the_states_they_mean() {
        assert_eq!(
            lifecycle_for("UserPromptSubmit"),
            Some(SessionLifecycle::Running)
        );
        assert_eq!(
            lifecycle_for("PermissionRequest"),
            Some(SessionLifecycle::WaitingForUser)
        );
        assert_eq!(lifecycle_for("Stop"), Some(SessionLifecycle::Idle));
    }

    #[test]
    fn a_failed_turn_leaves_the_session_alive() {
        // `StopFailure` is a turn that ended badly, not a session that died.
        // Recording it as failed would make a perfectly usable session look
        // dead in every listing.
        assert_eq!(lifecycle_for("StopFailure"), Some(SessionLifecycle::Idle));
    }

    #[test]
    fn codex_events_translate_to_the_states_they_mean() {
        assert_eq!(
            lifecycle_for("SessionStart"),
            Some(SessionLifecycle::Running)
        );
        assert_eq!(
            lifecycle_for("UserPromptSubmit"),
            Some(SessionLifecycle::Running)
        );
        assert_eq!(
            lifecycle_for("PermissionRequest"),
            Some(SessionLifecycle::WaitingForUser)
        );
        assert_eq!(lifecycle_for("Stop"), Some(SessionLifecycle::Idle));
        // Deliberately unmapped — see `lifecycle_for`'s own comment on this
        // arm for why the operating system, not this hook, is the authority
        // for a session having ended.
        assert_eq!(lifecycle_for("SessionEnd"), None);
    }

    /// A compaction is observable and is not a state. Both halves matter:
    /// the first is what makes Phase 21's compaction trigger possible at
    /// all, and the second is why it needs no migration.
    #[test]
    fn a_compaction_is_observable_and_is_not_a_lifecycle_state() {
        assert!(precedes_native_compaction("PreCompact"));
        assert_eq!(lifecycle_for("PreCompact"), None);
        assert_eq!(event_for("PreCompact"), None);

        for other in ["PostCompact", "Stop", "UserPromptSubmit", "PreToolUse", ""] {
            assert!(
                !precedes_native_compaction(other),
                "`{other}` is not a harness saying it is about to compact"
            );
        }
    }

    #[test]
    fn an_unfamiliar_event_changes_nothing() {
        // Harnesses gain events between releases. Guessing a state from an
        // unfamiliar name would show a session as idle while it works, or
        // working while it waits for the user. `PreToolUse` and `PreCompact`
        // are real Codex events (see `harness/codex.rs: HOOK_EVENTS`) that
        // this translator still does not recognise, since neither says
        // anything about a *session's* state.
        for unknown in ["PreToolUse", "PreCompact", "Notification", "", "stop"] {
            assert_eq!(lifecycle_for(unknown), None, "{unknown}");
        }
    }

    #[test]
    fn a_finished_session_is_never_revived_by_a_late_hook() {
        // Hook processes outlive their harness. One arriving after the session
        // ended must not bring it back.
        for finished in [
            SessionLifecycle::Stopped,
            SessionLifecycle::Failed,
            SessionLifecycle::Closed,
        ] {
            for next in [
                SessionLifecycle::Running,
                SessionLifecycle::Idle,
                SessionLifecycle::WaitingForUser,
            ] {
                assert!(
                    !may_apply(finished, next),
                    "{finished:?} was revived as {next:?}"
                );
            }
        }
    }

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

    /// `main.rs`'s production code joined with every file Phase 59 moved out
    /// of it under `commands/`. Before that decomposition every subcommand's
    /// implementation lived in `main.rs` itself, so the two scans below read
    /// it alone; the move relocated the code they look for without changing
    /// it, so reading `main.rs` alone would now silently find nothing. Add a
    /// new file here whenever one is added under `commands/`.
    ///
    /// Each file's own `production_code` is taken **before** joining, not
    /// after: `main.rs` is first in the list and carries the
    /// `#[cfg(test)] mod tests` marker `production_code` cuts at, so joining
    /// the raw files first and stripping once would discard every
    /// `commands/*.rs` file after it — the same file, none of the code.
    fn main_rs_and_commands_source() -> String {
        [
            include_str!("../main.rs"),
            include_str!("../commands/status.rs"),
            include_str!("../commands/entitlements.rs"),
            include_str!("../commands/gateway.rs"),
            include_str!("../commands/setup.rs"),
            include_str!("../commands/response.rs"),
            include_str!("../commands/resources.rs"),
            include_str!("../commands/route.rs"),
            include_str!("../commands/routing_cost.rs"),
            include_str!("../commands/context_firewall.rs"),
            include_str!("../commands/sessions.rs"),
            include_str!("../commands/memory.rs"),
            include_str!("../commands/memory_extraction.rs"),
            include_str!("../commands/checkpoint.rs"),
            include_str!("../commands/hook.rs"),
            include_str!("../commands/shim.rs"),
            include_str!("../commands/assumptions.rs"),
            include_str!("../commands/launch.rs"),
            include_str!("../commands/resume.rs"),
            include_str!("../commands/routing_destinations.rs"),
            include_str!("../commands/routing_classification.rs"),
            include_str!("../commands/shared.rs"),
        ]
        .iter()
        .map(|source| production_code(source))
        .collect::<Vec<_>>()
        .join("\n")
    }

    /// "Keep terminal-text parsing only as a fallback for state that cannot be
    /// obtained structurally."
    ///
    /// Glasshouse has no such fallback, and this is what keeps it that way.
    /// Every state a session can be in comes from something structural: the
    /// operating system saying the process ended, or the harness itself
    /// reporting through a hook. Nothing reads the terminal and infers.
    ///
    /// The guard is the set of places allowed to move a session's state. A
    /// text-scanning state machine would have to write from somewhere, and
    /// anywhere new fails here.
    #[test]
    fn nothing_derives_session_state_from_terminal_output() {
        // The translator itself must not be able to see terminal output.
        let translator = production_code(include_str!("lifecycle.rs"));
        for forbidden in ["scrollback", "Scrollback", "screen", "vt100"] {
            assert!(
                !translator.contains(forbidden),
                "the lifecycle translator names `{forbidden}`, so state could be read out \
                 of terminal output"
            );
        }

        // And the only writers are the structural ones.
        let main_and_commands = main_rs_and_commands_source();
        let writers = [
            ("main.rs", main_and_commands.as_str()),
            ("shell/mod.rs", include_str!("../shell/mod.rs")),
            ("session/runtime.rs", include_str!("runtime.rs")),
            ("session/attach.rs", include_str!("attach.rs")),
            ("session/select/mod.rs", include_str!("select/mod.rs")),
        ];
        let allowed = ["main.rs", "shell/mod.rs"];
        for (name, source) in writers {
            let code = production_code(source);
            let writes = code.contains("set_lifecycle(");
            if writes {
                assert!(
                    allowed.contains(&name),
                    "{name} moves a session's state; only the launch and shell paths may, \
                     and only from structural events"
                );
            }
        }

        // The runtime, which is the one place that *does* see terminal output,
        // must never move a session's state.
        let runtime = production_code(include_str!("runtime.rs"));
        assert!(
            !runtime.contains("set_lifecycle("),
            "the runtime reads terminal output and must not also decide session state"
        );
    }

    /// The resume path must reopen its session through the store's resume
    /// boundary, and `may_apply` above is why that matters here rather than
    /// only in `session::store`.
    ///
    /// **§35's shape, and this defect's own history.** Every test that can
    /// reach a resume without spawning a harness executable calls
    /// `SessionStore::begin_resume` itself, so all of them would still pass
    /// against a binary whose resume path never called it — which is precisely
    /// the state this package found the tree in: `main.rs::resume_session`
    /// wrote a lifecycle the store silently declined, no test noticed, and the
    /// defect was found by running a real Codex instead.
    ///
    /// So the production call site is asserted directly. The slice is checked
    /// against a landmark first: a scan over the wrong span passes for the
    /// wrong reason, which this module has been caught by before.
    #[test]
    fn the_resume_path_reopens_its_session_through_the_stores_resume_boundary() {
        let main = main_rs_and_commands_source();

        let start = main
            .find("fn resume_session(")
            .expect("the CLI resume path must still be called `resume_session`");
        let body = &main[start..];
        let end = body
            .find("\n}\n")
            .expect("`resume_session` must end at a top-level closing brace");
        let body = &body[..end];

        // The landmark: this really is the function that crosses the resume
        // boundary, and not some other span that happens to be named alike.
        assert!(
            body.contains("open_for_resume("),
            "the slice scanned is not the resume path: it never opens a resume boundary"
        );

        assert!(
            body.contains("note_resume("),
            "the resume path does not reopen its session through the store's resume \
             boundary, so the record stays finished and every hook the resumed harness \
             sends is discarded"
        );
        assert!(
            main.contains("store.begin_resume("),
            "`note_resume` must reach `SessionStore::begin_resume`; nothing else in the \
             crate may move a finished session back to a live state"
        );
    }

    #[test]
    fn a_live_session_follows_its_harness() {
        assert!(may_apply(
            SessionLifecycle::Running,
            SessionLifecycle::WaitingForUser
        ));
        assert!(may_apply(
            SessionLifecycle::Starting,
            SessionLifecycle::Running
        ));
        // A state that is already current is not a change.
        assert!(!may_apply(
            SessionLifecycle::Running,
            SessionLifecycle::Running
        ));
    }
}
