//! What every adapter declares about its harness's native communication
//! style, read through the public API rather than from inside the crate.
//!
//! `harness/mod.rs` already pins the whole table as a unit test. This file is
//! not that test again from outside: it holds the two properties a table
//! cannot hold, and that a table is in fact very good at hiding.
//!
//! 1. **A table cannot notice itself going empty.** If every adapter reverted
//!    to `Unverified` tomorrow, the in-crate table would be edited to match
//!    and would pass — the dimension would be dead and nothing would say so.
//!    [`at_least_one_harness_declares_a_communication_style`] fails in that
//!    world.
//! 2. **A table cannot read an evidence string.** `Declared::verified` takes
//!    any `&'static str`, so "because it obviously does" type-checks exactly
//!    as well as a `--help` line does. The module documentation says evidence
//!    must name a source "concrete enough to re-check", and
//!    [`every_verified_style_cites_something_a_reader_could_go_and_check`] is
//!    what makes that sentence enforceable instead of aspirational.

use glasshouse::harness::{Declared, StyleChange, all};
use glasshouse::integrations::IntegrationId;

/// Every adapter, with what it declares about its communication style.
fn declared_styles() -> Vec<(
    IntegrationId,
    Declared<glasshouse::harness::CommunicationStyle>,
)> {
    all()
        .map(|adapter| (adapter.id(), adapter.describe().communication_style))
        .collect()
}

/// The dimension is alive: something in this build answers the question.
///
/// This is the guard the in-crate table cannot be. `Unverified` is a correct
/// answer for an individual harness — several of them are, and deliberately
/// so — but it cannot be the answer for *all* of them without the whole
/// declaration having quietly stopped meaning anything.
#[test]
fn at_least_one_harness_declares_a_communication_style() {
    let verified: Vec<IntegrationId> = declared_styles()
        .into_iter()
        .filter(|(_, style)| style.is_verified())
        .map(|(id, _)| id)
        .collect();

    assert!(
        !verified.is_empty(),
        "no adapter declares a native communication-style mechanism any more. Either every \
         harness lost one at once, or a declaration was reverted to `Unverified` and the \
         in-crate table was edited to agree — that table cannot tell the two apart, which is \
         why this test is here"
    );
}

/// Both poles of the session-cost dimension are still occupied.
///
/// The point of [`StyleChange`] is that the two answers have different costs:
/// `NewSession` means applying a profile throws away a warm native session,
/// `InPlace` means it does not. A build in which every declaration had
/// collapsed onto one of them would keep compiling, keep passing a table
/// test, and have silently lost the distinction the field exists for.
///
/// Claude Code and Hermes are named because they are the two poles as read
/// from the installed binaries: Claude Code's output style rides in the
/// document passed with the launch-only `--settings`, and Hermes's
/// personality overlay is reassigned inside a running session.
#[test]
fn the_session_cost_of_changing_a_style_is_still_a_real_distinction() {
    let styles = declared_styles();

    let cost = |wanted: IntegrationId| {
        styles
            .iter()
            .find(|(id, _)| *id == wanted)
            .unwrap_or_else(|| panic!("{wanted:?} has no adapter"))
            .1
            .value()
            .map(|style| style.change)
    };

    assert_eq!(
        cost(IntegrationId::ClaudeCode),
        Some(StyleChange::NewSession),
        "Claude Code's output style is supplied in the document passed with `--settings`, which \
         is read when the session starts; recording it as an in-place change would have \
         Glasshouse apply a profile to a running session and silently fail to"
    );
    assert_eq!(
        cost(IntegrationId::Hermes),
        Some(StyleChange::InPlace),
        "Hermes reassigns its personality overlay inside the running session; recording it as \
         `NewSession` would throw away a warm conversation to apply a style it could have taken \
         where it stood"
    );
}

/// A declaration nobody could re-check is not a declaration.
///
/// The type cannot enforce this — `evidence` is a free string — so the suite
/// does. Every verified style must cite a **runnable command, a named file,
/// or a named function**: something a reader can go and execute or open. A
/// rationale ("because it supports output styles") satisfies the compiler and
/// tells the next reader nothing they can check.
///
/// The check is deliberately coarse. It cannot know whether the citation is
/// *true* — only a person re-running it can — but it can insist that there is
/// something there to re-run, which is the failure mode that actually
/// happens: a fact established once, cited loosely, and unfalsifiable a
/// release later.
#[test]
fn every_verified_style_cites_something_a_reader_could_go_and_check() {
    for (id, style) in declared_styles() {
        let Declared::Verified { value, evidence } = style else {
            continue;
        };

        assert!(
            !value.mechanism.trim().is_empty(),
            "{id:?} declares a communication style with no mechanism named"
        );

        // A backtick is how every adapter in this crate quotes the artifact
        // it read — `--settings`, `hermes config show`, `display.personality`.
        // Its absence means the evidence is prose about why, not a pointer to
        // what.
        assert!(
            evidence.contains('`'),
            "{id:?} cites its communication style as {evidence:?}, which names no command, file \
             or key to re-check. Evidence must point at the artifact it was read from"
        );

        // Long enough to identify a version and an artifact. The shortest
        // honest citation in this crate names a product, a version and a
        // command; nothing that short is a real one.
        assert!(
            evidence.len() > 40,
            "{id:?}'s communication-style evidence is only {} characters ({evidence:?}) — too \
             short to name the harness, its version, and the artifact read",
            evidence.len()
        );
    }
}

/// An unverified declaration answers nothing, and must not be readable as a
/// denial.
///
/// `Declared::Unverified` means "nothing available in this environment
/// established it. Not 'no', and never a guess." Four of these adapters are
/// `Unverified` for four different reasons — one because the binary is not
/// installed at all, three because what they *do* expose changes tool access
/// or reasoning effort and is therefore disqualified rather than absent. A
/// caller must not be able to tell any of that from the value, because the
/// value does not carry it; only the source does.
#[test]
fn an_unverified_style_yields_no_value_and_no_evidence() {
    for (id, style) in declared_styles() {
        if style.is_verified() {
            continue;
        }
        assert!(
            style.value().is_none(),
            "{id:?} is unverified but yielded a style value"
        );
        assert!(
            style.evidence().is_none(),
            "{id:?} is unverified but carries evidence — evidence for what?"
        );
    }
}

/// Line 290's consumer: the declaration has to reach a person.
///
/// The three tests above prove each adapter *declares* a style and that a
/// verified one cites something re-checkable. None of them proves anything
/// **reads** it — and for most of this line's life nothing did.
/// `communication_style` was written by all seven adapters and consumed, in
/// production, by nothing: `harness/mod.rs`'s own readers are behind
/// `#[cfg(test)]`, and every construction in `profile/` and `session/select.rs`
/// is a fixture. `glasshouse doctor` printed vendor, resume, session ids,
/// hooks, approvals, capabilities, protocols and model — and not this.
///
/// So this test is what makes the declaration observable rather than merely
/// present, which is the difference between a capability and a constant. It
/// runs the **shipped binary**, because that is the only thing that proves a
/// user can actually see it.
///
/// It deliberately does not assert a particular harness's mechanism text: that
/// would break every time an adapter learned something, which is the opposite
/// of what this dimension is for. It asserts the row exists for every adapter,
/// that both of line 290's clauses are rendered where a style is verified, and
/// that `unverified` is not silently spelled as "none" — those are different
/// claims and `Declared`'s whole design is that a reader can tell them apart.
#[test]
fn the_doctor_report_shows_each_harnesss_communication_style_and_its_session_cost() {
    let temp = tempfile::tempdir().expect("a temporary project");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_glasshouse"))
        .arg("doctor")
        .current_dir(temp.path())
        .output()
        .expect("the shipped binary runs");
    let report = String::from_utf8_lossy(&out.stdout);

    let rows: Vec<&str> = report
        .lines()
        .filter(|line| line.trim_start().starts_with("comm style:"))
        .collect();

    assert_eq!(
        rows.len(),
        all().count(),
        "every adapter's doctor entry must carry a communication-style row, so a \
         declaration nobody reads cannot masquerade as a capability; got:\n{report}"
    );

    for (id, declared) in declared_styles() {
        if let Declared::Verified { value, .. } = declared {
            assert!(
                rows.iter().any(|row| row.contains(value.mechanism)),
                "{id:?} declares the mechanism {:?} and the report does not name it",
                value.mechanism
            );
            let cost = match value.change {
                StyleChange::InPlace => "changeable in place",
                StyleChange::NewSession => "changing it needs a new session",
            };
            assert!(
                rows.iter().any(|row| row.contains(cost)),
                "line 290 asks for the session cost as well as the mechanism, and \
                 {id:?}'s report does not say {cost:?}"
            );
        }
    }

    assert!(
        rows.iter().any(|row| row.contains("unverified")),
        "an unverified style must read as `unverified` rather than as `none`: they are \
         different claims, and collapsing them tells a reader a harness has no mechanism \
         when in fact nothing has looked"
    );
}
