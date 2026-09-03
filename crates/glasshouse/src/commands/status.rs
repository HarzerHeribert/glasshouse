//! `commands::status` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use glasshouse::Runtime;
use glasshouse::config::{self, EffectiveConfig, UserConfig};
use glasshouse::session::ProjectSessions;

/// A one-screen project and resource summary — capability map line 1779.
///
/// Composes what `doctor`, `sessions` and `resources` already compute —
/// [`Discovery::run`], [`ProjectSessions`], and
/// [`glasshouse::provider::registry::registry`] — into counts, rather than
/// re-deriving any of their own rendering. A reader who needs more than a
/// count already has the command that produces it.
pub(crate) fn status_report(runtime: &Runtime) -> anyhow::Result<String> {
    use std::fmt::Write as _;

    let project = runtime.project();
    let mut out = String::new();

    let _ = writeln!(out, "Glasshouse status");
    let _ = writeln!(out, "=================");
    let _ = writeln!(out);
    let _ = writeln!(out, "Project");
    let _ = writeln!(out, "  name: {}", project.name());
    let _ = writeln!(out, "  root: {}", project.display_root().display());
    let _ = writeln!(out, "  id:   {}", project.id());
    let _ = writeln!(out);

    let discovery = glasshouse::integrations::Discovery::run(project);
    let harnesses: Vec<_> = discovery.harnesses().collect();
    let usable = harnesses.iter().filter(|d| d.is_usable()).count();
    let problems: usize = harnesses.iter().map(|d| d.problems().len()).sum();
    let problem_note = if problems == 0 {
        String::new()
    } else {
        format!(
            " ({problems} problem{} — see `glasshouse doctor`)",
            if problems == 1 { "" } else { "s" }
        )
    };
    let _ = writeln!(
        out,
        "Harnesses    {usable}/{} usable{problem_note}",
        harnesses.len()
    );

    let sessions = ProjectSessions::open(runtime)?;
    let records = sessions.store().list()?;
    if records.is_empty() {
        let _ = writeln!(out, "Sessions     none recorded — see `glasshouse launch`");
    } else {
        let _ = writeln!(
            out,
            "Sessions     {} recorded, most recent {} ({}, {})",
            records.len(),
            crate::commands::shared::short_id(&records[0].id),
            crate::commands::shared::disposition_word(&records[0]),
            crate::commands::shared::format_age(records[0].last_activity_at)
        );
    }

    let resources = glasshouse::provider::registry::registry();
    let _ = writeln!(
        out,
        "Resources    {} tracked — see `glasshouse resources` for quota detail",
        resources.len()
    );

    // Map line 1963: every configured entitlement is its own resource, named
    // here one entry per account — never merged by vendor, kind or backing,
    // because two accounts of one vendor being two resources is what makes
    // the pool a pool. A user with no `[entitlements]` entries sees no line.
    //
    // Map line 1965: each entry then carries its four telemetry facets —
    // capacity band, time until reset, recent throttling, the models it can
    // serve — from the telemetry the provider actually exposes, `unknown`
    // spelled out where nothing exists, and every shared reading marked with
    // its scope. The sources are read here, once, and handed to the one
    // resolver, so two entitlements of one provider cannot be handed
    // different provider-wide readings.
    let user = UserConfig::load(runtime.paths())?;
    let project_config = config::load_project_config(runtime.project())?;
    let effective = EffectiveConfig::new(&user, project_config.as_ref());
    // One resolver, one set of sources — `entitlement_pool_with_telemetry`,
    // which `glasshouse entitlements` reads through as well so the two
    // commands cannot describe one account differently.
    match crate::commands::entitlements::entitlement_pool_with_telemetry(runtime, &effective) {
        Ok(entitlements) if entitlements.is_empty() => {}
        Ok(entitlements) => {
            let names: Vec<String> = entitlements
                .iter()
                .map(|entry| format!("`{}`", entry.name()))
                .collect();
            let _ = writeln!(
                out,
                "Entitlements {} configured — {}",
                entitlements.len(),
                names.join(", ")
            );
            let thresholds = effective.capacity_band_thresholds().value;
            // Map line 1836, the same replay `entitlements_report` renders —
            // see that function's own note on why this ledger is opened once
            // per pool rather than once per entry.
            let now_unix = glasshouse::provider::cache::now_unix_seconds();
            let evidence_ledger = glasshouse::routing::evidence::EvidenceLedger::open(runtime).ok();
            for entry in &entitlements {
                let replay = crate::commands::entitlements::headroom_replay_for(
                    evidence_ledger.as_ref(),
                    now_unix,
                    entry,
                );
                let _ = writeln!(
                    out,
                    "  `{}`  {}\n  {}",
                    entry.name(),
                    crate::commands::entitlements::entitlement_facets(entry, &thresholds),
                    crate::commands::entitlements::headroom_replay_facet(&replay)
                );
            }
        }
        Err(err) => {
            let _ = writeln!(out, "Entitlements not resolvable — {err}");
        }
    }

    // Map line 2006: mode and per-session aggregate savings, shown only
    // when the firewall is configured on — "with the firewall off, nothing
    // changes" (the guarantee every one of map lines 1980-2003 already
    // keeps) extends to this section too, so an `off` session's status
    // output stays exactly what it was before this package existed.
    let firewall_mode = effective.context_firewall_mode().value;
    if firewall_mode != glasshouse::config::firewall::FirewallMode::Off {
        let _ = writeln!(out);
        let _ = writeln!(out, "Context firewall  mode: {firewall_mode}");
        match context_firewall_savings_summary(runtime) {
            Some(summary) => {
                let _ = writeln!(out, "  {summary}");
            }
            None => {
                let _ = writeln!(out, "  no context-firewall activity recorded yet");
            }
        }
    }

    let _ = writeln!(out, "{}", last_routing_decision_line(runtime));

    Ok(out)
}

/// `status`'s one-line summary of the newest routed launch — map line 1766:
/// the destination and its three largest contributions by absolute
/// magnitude, ties kept in recorded order, fewer printed when fewer exist.
/// *none recorded* for a project with no routed launch yet.
fn last_routing_decision_line(runtime: &Runtime) -> String {
    let row = glasshouse::evaluation::EvaluationObservations::open(runtime)
        .ok()
        .and_then(|ledger| ledger.latest_session_route().ok())
        .flatten();
    let Some(row) = row else {
        return "last routing decision: none recorded".to_owned();
    };

    let destination = row.subject.as_deref().unwrap_or("-");
    let mut contributions = row
        .detail
        .as_deref()
        .map(glasshouse::evaluation::route_contributions)
        .unwrap_or_default();
    contributions.sort_by(|a, b| b.magnitude.abs().total_cmp(&a.magnitude.abs()));
    let factors: Vec<String> = contributions
        .iter()
        .take(3)
        .map(|contribution| format!("{} {:+.3}", contribution.name, contribution.magnitude))
        .collect();
    let factors_part = if factors.is_empty() {
        String::new()
    } else {
        format!(" — {}", factors.join(", "))
    };
    let session: String = row
        .session_id
        .as_deref()
        .unwrap_or("-")
        .chars()
        .take(12)
        .collect();

    format!(
        "last routing decision: {destination}{factors_part} ({}, session {session})",
        crate::commands::shared::format_age(row.observed_at)
    )
}

/// Map line 2006's savings figure: an honest aggregate over every entry the
/// raw store currently holds, walked with [`glasshouse::firewall::RawStore::all_entries`]
/// rather than any evidence-ledger reader — the packet's own constraint
/// stands (map line 1987's ruling): the ledger's token columns are a
/// provider's own reported count, and this build's raw/forwarded figures
/// are `chars/4` estimates, so they are never written there. Chosen over a
/// bare request-count ("N of M reduced") because [`RawEntry::original_token_estimate`]
/// and [`RawEntry::forwarded_token_estimate`] are already persisted per
/// entry (map line 2005) and a token figure is closer to what "savings"
/// means than a request count alone — see this package's report for the
/// full reasoning. `None` when the store holds nothing yet, a different
/// fact from "0 saved".
fn context_firewall_savings_summary(runtime: &Runtime) -> Option<String> {
    let store = glasshouse::firewall::RawStore::open(runtime.state_dir().join("context-firewall"));
    let entries = store.all_entries().ok()?;
    if entries.is_empty() {
        return None;
    }

    let sessions: std::collections::HashSet<&str> = entries
        .iter()
        .map(|entry| entry.session_id.as_str())
        .collect();
    let mut original_of_estimated = 0u64;
    let mut forwarded_total = 0u64;
    let mut unestimated = 0usize;
    for entry in &entries {
        match entry.forwarded_token_estimate {
            Some(forwarded) => {
                original_of_estimated += entry.original_token_estimate;
                forwarded_total += forwarded;
            }
            // An entry recorded before map line 2005 carries no comparison
            // — counted toward "results", never folded into a savings
            // figure it never measured.
            None => unestimated += 1,
        }
    }
    let kept_local = original_of_estimated.saturating_sub(forwarded_total);
    let unestimated_note = if unestimated > 0 {
        format!(" ({unestimated} without a recorded estimate)")
    } else {
        String::new()
    };

    Some(format!(
        "{} session{}, {} result{} reduced, ~{kept_local} of ~{original_of_estimated} estimated \
         tokens kept local{unestimated_note}",
        sessions.len(),
        if sessions.len() == 1 { "" } else { "s" },
        entries.len(),
        if entries.len() == 1 { "" } else { "s" },
    ))
}
