//! `commands::assumptions` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use glasshouse::Runtime;
use glasshouse::guardrails::AssumptionStore;
use glasshouse::session::ProjectSessions;

/// `glasshouse assumptions [--session <id>] [--limit N]` — Phase 21K lines
/// 1048, 1049, 1051.
///
/// Every line is read from the ledger and rendered through
/// `guardrails::quote`, so what an agent stated reaches the terminal with
/// nothing in it that could act on the terminal. The session, when named,
/// is resolved through the session store's own prefix rule and that handle
/// is dropped before the ledger's is opened.
pub(crate) fn assumptions_report(
    runtime: &Runtime,
    session: Option<&str>,
    limit: usize,
) -> anyhow::Result<String> {
    use glasshouse::guardrails::{AssumptionState, TransitionKind, quote};
    use std::fmt::Write as _;

    let session = match session {
        Some(named) => {
            let sessions = ProjectSessions::open(runtime)?;
            let id = sessions.store().resolve_id(named)?;
            Some(id.as_str().to_owned())
        }
        None => None,
    };
    let ledger = AssumptionStore::open(runtime)?;
    let counts = ledger.counts(session.as_deref())?;
    let views = ledger.list(session.as_deref(), limit)?;

    let mut out = String::new();
    match &session {
        Some(id) => writeln!(out, "assumptions stated for session {id}")?,
        None => writeln!(out, "assumptions stated in this project")?,
    }
    let summary = counts
        .iter()
        .map(|(state, n)| format!("{state} {n}"))
        .collect::<Vec<_>>()
        .join(" · ");
    writeln!(out, "{summary}")?;
    if views.is_empty() {
        writeln!(
            out,
            "\nnone recorded — an agent states one through the control API's \
             record_assumption or the glasshouse_record_assumption tool; nothing is inferred"
        )?;
    }
    for view in &views {
        let record = &view.record;
        writeln!(out)?;
        writeln!(
            out,
            "{}  {:<14} {}/{}  {}{}",
            record.id.short(),
            view.state,
            record.uncertainty,
            record.evidence_source,
            crate::commands::shared::format_age(record.created_at),
            record
                .session_id
                .as_deref()
                .filter(|_| session.is_none())
                .map(|s| format!("  session {}", &s[..s.len().min(8)]))
                .unwrap_or_default()
        )?;
        writeln!(out, "    claim         {}", quote(&record.claim, 280))?;
        writeln!(out, "    evidence      {}", quote(&record.evidence, 200))?;
        writeln!(out, "    affects       {}", quote(&record.affected, 200))?;
        writeln!(
            out,
            "    verify        {}",
            quote(&record.verification, 200)
        )?;
        let latest = &view.latest;
        let mut trail = format!(
            "{} by {} {}",
            latest.state.map_or("-", AssumptionState::as_str),
            latest.origin,
            crate::commands::shared::format_age(latest.at)
        );
        if let Some(response) = latest.response {
            let _ = write!(trail, ", response {response}");
        }
        if let Some(note) = &latest.note {
            let _ = write!(trail, " — {}", quote(note, 200));
        }
        if view.transitions > 1 {
            let _ = write!(trail, " ({} transitions)", view.transitions);
        }
        writeln!(out, "    latest        {trail}")?;
    }

    if let Some(id) = &session {
        let events = ledger.session_events(id, None, 20)?;
        if !events.is_empty() {
            writeln!(out)?;
            writeln!(out, "gates, overrides and budgets for this session")?;
            for event in &events {
                let what = match event.kind {
                    TransitionKind::Gate => "gate",
                    TransitionKind::Override => "override",
                    TransitionKind::BudgetExceeded => "budget exceeded",
                    TransitionKind::Transition => "transition",
                };
                writeln!(
                    out,
                    "  {:<16} {:<32} {}{}",
                    what,
                    event.subject.as_deref().unwrap_or("-"),
                    crate::commands::shared::format_age(event.at),
                    event
                        .note
                        .as_deref()
                        .map(|note| format!("  — {}", quote(note, 120)))
                        .unwrap_or_default()
                )?;
            }
        }
    }
    Ok(out)
}
