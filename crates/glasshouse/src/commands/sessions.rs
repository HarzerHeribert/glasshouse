//! `commands::sessions` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use glasshouse::Runtime;
use glasshouse::config::UserConfig;
use glasshouse::guardrails::AssumptionStore;
use glasshouse::integrations::cmux;
use glasshouse::session::{
    ProjectSessions, SessionDisposition, SessionName, SessionProtocol, SessionPurpose,
    SessionRecord, SessionStore,
};

/// `glasshouse sessions focus` — Phase 17 line 759. One `workspace select`
/// through the integration, for a session that has a pane; a session that
/// has none, or a cmux that is not available, is reported rather than
/// guessed around.
pub(crate) fn focus_session(runtime: &Runtime, session: &str) -> anyhow::Result<String> {
    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    let id = store.resolve_id(session)?;
    let record = store
        .get(&id)?
        .ok_or_else(|| anyhow::anyhow!("session `{id}` is not in this project"))?;
    let Some(reference) = record.presentation_ref.as_deref() else {
        anyhow::bail!(
            "session `{id}` has no external pane to focus; it is presented {}",
            record.presentation
        );
    };
    match cmux::detect() {
        cmux::Availability::Absent(reason) => anyhow::bail!(
            "session `{id}` is presented in cmux {reference}, but cmux is not available \
             from here ({reason})"
        ),
        cmux::Availability::Available(control) => {
            let pane = cmux::focus(reference, &control)?;
            Ok(format!(
                "glasshouse: focused cmux {pane} for session {id}\n"
            ))
        }
    }
}

/// The `PRESENTED` cell: the presentation word, followed by the pane when
/// one is recorded — `external workspace:349`. The word alone for every
/// other session, exactly as before the pane existed.
fn presented_cell(record: &SessionRecord) -> String {
    match record.presentation_ref.as_deref() {
        Some(reference) => format!("{} {reference}", record.presentation),
        None => record.presentation.to_string(),
    }
}

pub(crate) fn session_report(runtime: &Runtime) -> anyhow::Result<String> {
    use std::fmt::Write as _;

    let sessions = ProjectSessions::open(runtime)?;
    let records = sessions.store().list()?;

    if records.is_empty() {
        return Ok(format!(
            "No sessions recorded for {}.\nStart one with `glasshouse launch`.\n",
            runtime.project().name()
        ));
    }

    // The one column whose width depends on the data: `external
    // workspace:<n>` is wider than any presentation word, and a listing with
    // no pane in it is laid out exactly as it was before panes existed.
    let presented_width = records
        .iter()
        .map(|record| presented_cell(record).len())
        .chain(std::iter::once(PRESENTED_WIDTH))
        .max()
        .unwrap_or(PRESENTED_WIDTH);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}",
        session_row(
            "SESSION",
            "NAME",
            "PURPOSE",
            "HARNESS",
            "PROFILE",
            "STATE",
            "ROLE",
            "PRESENTED",
            "LAST ACTIVITY",
            presented_width,
        )
    );
    for record in &records {
        let _ = writeln!(
            out,
            "{}",
            session_row(
                &crate::commands::shared::short_id(&record.id),
                // A name and a purpose are the user's, and most sessions have
                // neither. A dash rather than a blank: an empty cell in a
                // fixed-width table reads as a rendering fault.
                record
                    .display_name
                    .as_ref()
                    .map_or("-", |name| name.as_str()),
                record
                    .purpose
                    .as_ref()
                    .map_or("-", |purpose| purpose.as_str()),
                &record.harness,
                // A dash, not the word "native": a session recorded before
                // Phase 9A ran under no profile at all, and that is a
                // different fact from having run the Native profile — see
                // `SessionRecord::launch_profile`'s doc.
                record.launch_profile.as_deref().unwrap_or("-"),
                crate::commands::shared::disposition_word(record),
                &record.role.to_string(),
                &presented_cell(record),
                &crate::commands::shared::format_age(record.last_activity_at),
                presented_width,
            )
        );
    }
    if let Some(claims) = claims_block(&sessions.store())? {
        // Line 2398, and its *"when they are relevant to parallel work"*
        // half: nothing is printed when nothing is claimed, so a project that
        // does not use claims sees the listing it always saw.
        let _ = write!(out, "\n{claims}");
    }
    Ok(out)
}

/// The `CLAIMED BY` block for the session overview — map line 2398 — or
/// `None` when this project has no active claim.
///
/// Ordered by path, so the sessions claiming one file stand next to each
/// other. That adjacency is the whole of what is surfaced: it is not a
/// conflict verdict, not a warning, and not a recommendation, all of which
/// belong to a later package.
fn claims_block(store: &SessionStore<'_>) -> anyhow::Result<Option<String>> {
    use std::fmt::Write as _;

    let claims = store.active_claims()?;
    if claims.is_empty() {
        return Ok(None);
    }

    let path_width = claims
        .iter()
        .map(|claim| claim.path.len())
        .chain(std::iter::once("PATH".len()))
        .max()
        .unwrap_or("PATH".len());

    // `active_claims` is `ORDER BY path`, so a path more than one session
    // holds is already adjacent here — this only counts rows per path, so a
    // row on such a path can say so. Map line 2410, same words as the hook
    // (`commands::hook::edit_intent_conflict`): `OverlapKind::describe` is
    // the one place the vocabulary lives.
    let mut per_path: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for claim in &claims {
        *per_path.entry(claim.path.as_str()).or_insert(0) += 1;
    }

    let mut out = String::new();
    let _ = writeln!(out, "{:<12}  {:<path_width$}  SINCE", "CLAIMED BY", "PATH");
    for claim in &claims {
        let _ = write!(
            out,
            "{:<12}  {:<path_width$}  {}",
            crate::commands::shared::short_id(&claim.session_id),
            claim.path,
            crate::commands::shared::format_age(claim.claimed_at),
        );
        if per_path.get(claim.path.as_str()).copied().unwrap_or(0) > 1 {
            let _ = write!(
                out,
                "  ({})",
                glasshouse::firewall::adapter::OverlapKind::DirectFile.describe(),
            );
        }
        let _ = writeln!(out);
    }
    Ok(Some(out))
}

/// The declarations block for the session overview and `glasshouse
/// task-progress --list`, or `None` when nothing is declared.
fn task_progress_block(store: &SessionStore<'_>) -> anyhow::Result<Option<String>> {
    use std::fmt::Write as _;

    let declared = store.active_task_progress()?;
    if declared.is_empty() {
        return Ok(None);
    }

    // Display only, and the same wall-clock seconds `format_age` reads a
    // line below — the row's own clock is the store's, and this introduces
    // no second one for it.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0);

    let mut out = String::new();
    let _ = writeln!(out, "{:<12}  {:<12}  EXPIRES IN", "TASK NEARLY", "DECLARED");
    for declaration in &declared {
        let remaining = declaration.expires_at - now;
        let _ = writeln!(
            out,
            "{:<12}  {:<12}  {}",
            crate::commands::shared::short_id(&declaration.session_id),
            crate::commands::shared::format_age(declaration.declared_at),
            format_remaining(remaining),
        );
    }
    Ok(Some(out))
}

/// `EXPIRES IN`, in the coarsest unit that still says something useful.
///
/// A declaration people can see expiring is the point: the horizon is what
/// stops a statement outliving the task it described, so an overview that
/// showed only *"declared"* would hide the half of the design that keeps it
/// honest.
fn format_remaining(seconds: i64) -> String {
    match seconds {
        s if s <= 0 => "expired".to_owned(),
        s if s < 60 => format!("{s}s"),
        s => format!("{}m", s / 60),
    }
}

/// `glasshouse task-progress` — the producer of
/// `provider::quota::ReserveDecisionInputs::task_nearly_complete`, capability
/// map lines 1294 and 1610.
///
/// # Why a person types this
///
/// The field this writes is the **first** branch the reserve policy takes,
/// outranking every other signal including the user's own override. Nothing
/// in this build can observe task progress — a turn boundary is not a task
/// boundary — and every available proxy reports "almost complete" for work
/// that has merely been running a while, which is precisely the long-running
/// work a protected reserve exists to keep serving. So a value Glasshouse
/// invented would invert the policy rather than approximate it, and the only
/// honest source is somebody saying so on purpose about one named session.
///
/// A seam, not a feature: everything it decides is decided in
/// `session::store::progress`, and a caller that wants the declaration reads
/// that store directly rather than this verb.
pub(crate) fn task_progress_command(
    runtime: &Runtime,
    session: Option<&str>,
    withdraw: bool,
    list: bool,
) -> anyhow::Result<String> {
    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();

    if list {
        return Ok(match task_progress_block(&store)? {
            Some(block) => block,
            None => format!(
                "No task declared nearly complete in {}.\n",
                runtime.project().name()
            ),
        });
    }

    // `clap` requires `--session` unless `--list`; stated here rather than
    // assumed, because an argument definition is not a proof.
    let Some(session) = session else {
        anyhow::bail!(
            "`glasshouse task-progress` needs `--session <id>`; `--list` needs no session"
        );
    };
    let id = store.resolve_id(session)?;
    let short = crate::commands::shared::short_id(&id);

    if withdraw {
        return Ok(if store.withdraw_task_progress(&id)? {
            format!("glasshouse: session {short} withdrew its task-progress declaration\n")
        } else {
            format!("glasshouse: session {short} had declared no task progress\n")
        });
    }

    let declared = store.declare_task_nearly_complete(&id)?;
    let minutes = (declared.expires_at - declared.renewed_at) / 60;
    Ok(format!(
        "glasshouse: session {short}'s current task is declared nearly complete; a crossed \
         quota reserve alone will not move this work for the next {minutes}m\n"
    ))
}

/// The sessions that currently declare their task nearly complete, for the
/// routers — capability map lines 1294 and 1610.
///
/// **Best-effort on purpose.** A project database that cannot be opened
/// yields an empty set, which is *nothing declared*, which is byte-identical
/// to the behaviour every routing path had before this line had a producer.
/// The alternative — failing a routing decision because a declaration could
/// not be read — would let an unreadable database deny work that has nothing
/// to do with task progress.
pub(crate) fn declared_task_progress_sessions(
    runtime: &Runtime,
) -> std::collections::BTreeSet<String> {
    let Ok(sessions) = ProjectSessions::open(runtime) else {
        return std::collections::BTreeSet::new();
    };
    match sessions.store().sessions_declaring_task_nearly_complete() {
        Ok(declared) => declared,
        Err(err) => {
            tracing::debug!(error = %err, "could not read declared task progress");
            std::collections::BTreeSet::new()
        }
    }
}

/// `glasshouse claim` — the deliberate entry point for map line 2392 while
/// Glasshouse cannot yet observe edit intent for itself.
///
/// A seam, not a feature: everything it decides is decided in
/// `session::store::claims`, and the next package calls that store directly
/// rather than this verb.
pub(crate) fn claim_command(
    runtime: &Runtime,
    path: Option<&str>,
    session: Option<&str>,
    release: bool,
    list: bool,
) -> anyhow::Result<String> {
    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();

    if list {
        return Ok(match claims_block(&store)? {
            Some(block) => block,
            None => format!("No file claims in {}.\n", runtime.project().name()),
        });
    }

    // `clap` requires `--session` unless `--list`, and requires a path unless
    // `--list` or `--release`; both are stated here rather than assumed,
    // because an argument definition is not a proof.
    let Some(session) = session else {
        anyhow::bail!("`glasshouse claim` needs `--session <id>`; `--list` needs no session");
    };
    let id = store.resolve_id(session)?;
    let short = crate::commands::shared::short_id(&id);

    match (release, path) {
        (true, None) => {
            let released = store.release_claims_of(&id)?;
            Ok(format!(
                "glasshouse: released {released} file {} held by session {short}\n",
                if released == 1 { "claim" } else { "claims" }
            ))
        }
        (true, Some(raw)) => {
            let path = claimed_path(runtime, raw)?;
            if store.release_claim(&id, &path)? {
                Ok(format!(
                    "glasshouse: session {short} released its claim on {path}\n"
                ))
            } else {
                Ok(format!(
                    "glasshouse: session {short} held no claim on {path}\n"
                ))
            }
        }
        (false, Some(raw)) => {
            let path = claimed_path(runtime, raw)?;
            let claim = store.claim_file(&id, &path)?;
            Ok(format!(
                "glasshouse: session {short} claims {} (held since {})\n",
                claim.path,
                crate::commands::shared::format_age(claim.claimed_at),
            ))
        }
        (false, None) => {
            anyhow::bail!("`glasshouse claim` needs a path, `--release`, or `--list`")
        }
    }
}

/// The repo-relative spelling of a path a person typed, or a refusal naming
/// the project it is not inside.
///
/// A relative path is resolved against the working directory, which is what
/// a path typed at a shell means — unlike a tool input, which
/// [`crate::commands::context_firewall::project_relative_path`] treats as
/// already root-relative. Everything after that is that function's, so a
/// claimed path and a remembered path are the same string for the same file.
fn claimed_path(runtime: &Runtime, raw: &str) -> anyhow::Result<String> {
    let root = runtime.project().root();
    let given = std::path::Path::new(raw);
    let absolute = if given.is_absolute() {
        given.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(given))
            .unwrap_or_else(|_| root.join(given))
    };

    crate::commands::context_firewall::project_relative_path(root, &absolute.display().to_string())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "`{raw}` is not a file inside {}; a file claim is project-scoped \
                 and names a path in this project",
                root.display()
            )
        })
}

/// One line of the session listing, header included.
///
/// The header and the rows go through the same function so their columns
/// cannot drift apart — the usual way a hand-aligned table stops lining up is
/// someone widening a column in one of the two format strings.
/// The `PRESENTED` column's width when no row needs more: the header's own
/// length, which every presentation word fits inside.
pub(crate) const PRESENTED_WIDTH: usize = "PRESENTED".len();

#[allow(clippy::too_many_arguments)]
pub(crate) fn session_row(
    session: &str,
    name: &str,
    purpose: &str,
    harness: &str,
    profile: &str,
    state: &str,
    role: &str,
    presented: &str,
    activity: &str,
    presented_width: usize,
) -> String {
    // Widths fit the longest value each column can hold: `resumable`,
    // `orchestrator`. `presented` is the one column sized by the listing —
    // see `session_report` — because `external workspace:<n>` is wider than
    // any presentation word and a listing without one should not pay for
    // it. `name` and `purpose` are the two the user controls, and they are
    // truncated by the format rather than bounded here — the store already
    // refuses anything longer than 64 and 32.
    format!(
        "{session:<12}  {name:<16}  {purpose:<10}  {harness:<14}  {profile:<12}  {state:<9}  \
         {role:<12}  {presented:<presented_width$}  {activity}"
    )
}

/// Everything one session recorded, one fact per line.
///
/// # Seven answers, not one
///
/// The harness, the launch profile, the backend resource, the model, the
/// pairing class, the wire protocol and the response profile each get their
/// own line, printed from their own column, with no line derived from
/// another. That is the phase's second fixed architectural requirement made
/// visible: a reader can see that Glasshouse holds them apart, and a build
/// that started filling one in from another would show it here.
///
/// # A dash is not a value
///
/// `-` means *this build recorded nothing here*, which is what a session
/// started before these columns existed leaves behind. It is deliberately
/// different from `unknown` and from `the harness's own default`, both of
/// which are answers Glasshouse recorded on purpose.
pub(crate) fn session_detail(
    runtime: &Runtime,
    session: &str,
    debug: bool,
) -> anyhow::Result<String> {
    use std::fmt::Write as _;

    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    let id = store.resolve_id(session)?;
    let record = store
        .get(&id)?
        .ok_or_else(|| anyhow::anyhow!("session `{id}` is not in this project"))?;

    let mut out = String::new();
    let mut line = |label: &str, value: &str| {
        let _ = writeln!(out, "{label:<19}{value}");
    };

    line("session", record.id.as_str());
    line(
        "name",
        record.display_name.as_ref().map_or("-", |n| n.as_str()),
    );
    line(
        "purpose",
        record.purpose.as_ref().map_or("-", |p| p.as_str()),
    );
    line("project", &record.project_id);
    line("harness", &record.harness);
    line(
        "native session",
        record.native_session_id.as_deref().unwrap_or("-"),
    );
    line("state", crate::commands::shared::disposition_word(&record));
    line("lifecycle", record.lifecycle.as_str());
    line("role", record.role.as_str());
    line("presented", record.presentation.as_str());
    line(
        "presentation ref",
        record.presentation_ref.as_deref().unwrap_or("-"),
    );
    line(
        "launch profile",
        record.launch_profile.as_deref().unwrap_or("-"),
    );
    line(
        "backend resource",
        record.backend_resource.as_deref().unwrap_or("-"),
    );
    line("model", record.model.as_ref().map_or("-", |m| m.label()));
    line(
        "pairing class",
        record
            .pairing_class
            .map_or("-", glasshouse::session::SessionPairingClass::as_str),
    );
    line(
        "protocol",
        record.protocol.map_or("-", SessionProtocol::as_str),
    );
    line("response profile", &response_profile_line(&record));
    line(
        "response mechanism",
        record
            .response_mechanism
            .map_or("-", glasshouse::session::ResponseMechanism::as_str),
    );
    line(
        "created",
        &crate::commands::shared::format_age(record.created_at),
    );
    line(
        "last activity",
        &crate::commands::shared::format_age(record.last_activity_at),
    );

    // Phase 30, lines 1159 and 1161-1165. `store.context` is the sole
    // producer of these facts (`session/store.rs::SessionStore::context`);
    // before this call it had no caller outside that module's own tests, so
    // every value it computes was correct and unreachable. A read failure
    // collapses to the same "-" the fields above use for nothing recorded,
    // exactly like `context()`'s own `Ok(None)` case — a session detail
    // report must finish even when this extra context cannot be read.
    let context = store.context(&id).ok().flatten();
    line(
        "compactions",
        &context
            .as_ref()
            .and_then(|c| c.observed_compactions)
            .map_or_else(|| "-".to_string(), |n| n.to_string()),
    );
    line(
        "prompt cache",
        &context
            .as_ref()
            .map_or_else(|| "-".to_string(), |c| c.prompt_cache.to_string()),
    );
    line(
        "checkpoint",
        &context
            .as_ref()
            .map_or_else(|| "-".to_string(), |c| c.checkpoint.to_string()),
    );
    line(
        "task continuity",
        &context
            .as_ref()
            .map_or_else(|| "-".to_string(), |c| c.task_continuity.to_string()),
    );

    // Phase 21K lines 1048 and 1049: the session's open premises and its
    // last gate, on a handle opened after the session store's is gone
    // (practice §65), and bounded so the normal view is not flooded.
    drop(store);
    drop(sessions);
    out.push_str(&assumption_section(runtime, &id));
    out.push_str(&routing_rationale_section(runtime, &id));
    if debug {
        out.push_str(&prompt_cache_debug_section(runtime, &id));
    }
    Ok(out)
}

/// `sessions show`'s `routing rationale` block, map line 1757 — the
/// session's newest [`glasshouse::evaluation::EvaluationKind::SessionRouteDecided`]
/// row, one line per contribution, in recorded order.
///
/// `-` for a session with no row — started before this build recorded one,
/// or spawned through the machine door, which is not routed — matching
/// every other field [`session_detail`] prints for nothing recorded. An
/// explanation with no contributions still has a row, so the heading prints
/// and no contribution line follows: the decision happened even when
/// nothing weighed in.
fn routing_rationale_section(runtime: &Runtime, id: &glasshouse::session::SessionId) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let mut line = |label: &str, value: &str| {
        let _ = writeln!(out, "{label:<19}{value}");
    };

    let row = glasshouse::evaluation::EvaluationObservations::open(runtime)
        .ok()
        .and_then(|ledger| ledger.session_route_for(id.as_str()).ok())
        .flatten();
    let Some(row) = row else {
        line("routing rationale", "-");
        return out;
    };

    line("routing rationale", row.subject.as_deref().unwrap_or("-"));
    let contributions = row
        .detail
        .as_deref()
        .map(glasshouse::evaluation::route_contributions)
        .unwrap_or_default();
    let width = contributions
        .iter()
        .map(|contribution| contribution.name.len())
        .max()
        .unwrap_or(0);
    for contribution in &contributions {
        line(
            "",
            &format!(
                "  {:<width$}  {:+.3}  {}",
                contribution.name, contribution.magnitude, contribution.evidence
            ),
        );
    }
    out
}

/// `sessions show <id> --debug`'s cache-temperature view, map line 1760.
///
/// Two readings, kept apart rather than blended into one number: (a) the
/// router's own `prompt-cache state` contribution
/// ([`glasshouse::routing::session::prompt_cache_state`]) from this
/// session's newest recorded rationale — the estimate the router made
/// *before* any of this session's exchanges happened — and (b) the
/// cached-input share this project's ledger actually holds over this
/// session's own translated exchanges, from
/// [`glasshouse::routing::evidence::EvidenceLedger::cached_share_for_session`].
///
/// The trailing sentence is fixed and always printed: this build observes
/// neither a provider cache's presence nor its lifetime (see
/// `prompt_cache_state`'s own doc comment), so (a) is an estimate and (b) is
/// what providers reported on exchanges that came after it, never a
/// measurement of the same cache the estimate describes.
fn prompt_cache_debug_section(runtime: &Runtime, id: &glasshouse::session::SessionId) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    out.push_str("\nprompt-cache estimate (1760):\n");

    let estimate = glasshouse::evaluation::EvaluationObservations::open(runtime)
        .ok()
        .and_then(|ledger| ledger.session_route_for(id.as_str()).ok())
        .flatten()
        .and_then(|row| {
            row.detail
                .as_deref()
                .map(glasshouse::evaluation::route_contributions)
        })
        .and_then(|contributions| {
            contributions
                .into_iter()
                .find(|contribution| contribution.name == "prompt-cache state")
        });
    match estimate {
        Some(contribution) => {
            let _ = writeln!(
                out,
                "  the router's estimate at launch: {:+.3}  {}",
                contribution.magnitude, contribution.evidence
            );
        }
        None => out.push_str("  no routing rationale recorded for this session\n"),
    }

    let share = glasshouse::routing::evidence::EvidenceLedger::open(runtime)
        .ok()
        .and_then(|ledger| ledger.cached_share_for_session(id.as_str()).ok())
        .flatten();
    match share {
        Some(share) => {
            let percent = share
                .cache_read_ratio()
                .map(|ratio| ratio * 100.0)
                .unwrap_or(0.0);
            let _ = writeln!(
                out,
                "  cached-input share {percent:.0}% over {} translated exchange(s) ({} cached \
                 of {} input tokens)",
                share.sample_count, share.cached_input_tokens, share.input_tokens
            );
        }
        None => out.push_str(
            "  no translated exchange has reported cached-input tokens for this session\n",
        ),
    }

    out.push_str(
        "  the share above is what providers reported on this session's own exchanges; the \
         estimate above was made before any of them — an estimate and its evidence, never a \
         measurement of the provider's cache\n",
    );
    out
}

/// How many open premises `glasshouse sessions show` lists before it says
/// how many more there are — line 1048's *"without flooding"*.
const SHOWN_OPEN_PREMISES: usize = 3;

/// The `sessions show` lines for a session's assumptions: a count line, at
/// most [`SHOWN_OPEN_PREMISES`] open premises, the last gate and the
/// override in force. A ledger that cannot be read collapses to `-`, like
/// every other field above it.
fn assumption_section(runtime: &Runtime, id: &glasshouse::session::SessionId) -> String {
    use glasshouse::guardrails::{AssumptionState, TransitionKind, quote};
    use std::fmt::Write as _;

    let mut out = String::new();
    let mut line = |label: &str, value: &str| {
        let _ = writeln!(out, "{label:<19}{value}");
    };

    let Ok(ledger) = AssumptionStore::open(runtime) else {
        line("assumptions", "-");
        return out;
    };
    let session = id.as_str();
    let (Ok(counts), Ok(open)) = (
        ledger.counts(Some(session)),
        ledger.open_for_session(session),
    ) else {
        line("assumptions", "-");
        return out;
    };
    let count_of = |state: AssumptionState| {
        counts
            .iter()
            .find(|(s, _)| *s == state)
            .map_or(0, |(_, n)| *n)
    };
    let total: i64 = counts.iter().map(|(_, n)| n).sum();
    if total == 0 {
        line("assumptions", "none stated");
    } else {
        line(
            "assumptions",
            &format!(
                "{} open · {} supported · {} refuted · {} waived",
                open.len(),
                count_of(AssumptionState::Supported),
                count_of(AssumptionState::Refuted),
                count_of(AssumptionState::WaivedByUser)
            ),
        );
    }
    for view in open.iter().take(SHOWN_OPEN_PREMISES) {
        line(
            "  open premise",
            &format!(
                "[{}] {} ({})",
                view.state,
                quote(&view.record.claim, 96),
                view.record.id.short()
            ),
        );
    }
    if open.len() > SHOWN_OPEN_PREMISES {
        line(
            "",
            &format!(
                "… and {} more; `glasshouse assumptions --session {}`",
                open.len() - SHOWN_OPEN_PREMISES,
                crate::commands::shared::short_id(id)
            ),
        );
    }
    if let Ok(gates) = ledger.session_events(session, Some(TransitionKind::Gate), 1)
        && let Some(gate) = gates.first()
    {
        line(
            "  last gate",
            &format!(
                "{} — {}",
                gate.subject.as_deref().unwrap_or("?"),
                crate::commands::shared::format_age(gate.at)
            ),
        );
    }
    if let Ok(Some((kind, row))) = ledger.latest_override(session) {
        line(
            "  guardrail",
            &format!(
                "{kind} (recorded by {}, {})",
                row.origin,
                crate::commands::shared::format_age(row.at)
            ),
        );
    }
    out
}

/// A session's five response axes on one line, or `-` when none was recorded.
///
/// Rendered from `ResponseProfile::axes`, so the five names and the five
/// values come from `profile::response` rather than from a second list here.
fn response_profile_line(record: &SessionRecord) -> String {
    match &record.response_profile {
        Some(profile) => profile
            .axes()
            .iter()
            .map(|(dimension, value)| format!("{}={value}", dimension.slug()))
            .collect::<Vec<_>>()
            .join("  "),
        None => "-".to_owned(),
    }
}

/// Give a session a name, or take its name away — line 650.
///
/// The report says the native session identifier afterwards, and says it is
/// unchanged. That is the capability's own promise, and a promise a user
/// cannot see is one they have to take on trust.
pub(crate) fn rename_session(
    runtime: &Runtime,
    session: &str,
    name: Option<&str>,
    clear: bool,
) -> anyhow::Result<String> {
    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    let id = store.resolve_id(session)?;
    let before = store
        .get(&id)?
        .ok_or_else(|| anyhow::anyhow!("session `{id}` is not in this project"))?;

    let record = if clear {
        store.clear_name(&id)?
    } else {
        let name = name.expect("clap requires a name unless --clear was given");
        store.rename(&id, &SessionName::parse(name)?)?
    };

    // Read back from the row rather than from what was asked for: the point
    // of the line is that one column changed and another did not.
    let native = record
        .native_session_id
        .as_deref()
        .unwrap_or("none recorded");
    debug_assert_eq!(before.native_session_id, record.native_session_id);
    Ok(match &record.display_name {
        Some(name) => format!(
            "Session {} is now `{name}`.\nIts native session id is unchanged: {native}\n",
            crate::commands::shared::short_id(&record.id)
        ),
        None => format!(
            "Session {} has no name.\nIts native session id is unchanged: {native}\n",
            crate::commands::shared::short_id(&record.id)
        ),
    })
}

/// Tag a session with a lightweight purpose, or clear the tag — line 651.
pub(crate) fn tag_session(
    runtime: &Runtime,
    session: &str,
    purpose: Option<&str>,
    clear: bool,
) -> anyhow::Result<String> {
    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    let id = store.resolve_id(session)?;

    let record = if clear {
        store.clear_purpose(&id)?
    } else {
        let purpose = purpose.expect("clap requires a purpose unless --clear was given");
        store.set_purpose(&id, &SessionPurpose::parse(purpose)?)?
    };

    Ok(match &record.purpose {
        Some(purpose) => format!(
            "Session {} is tagged `{purpose}`.\n",
            crate::commands::shared::short_id(&record.id)
        ),
        None => format!(
            "Session {} has no purpose tag.\n",
            crate::commands::shared::short_id(&record.id)
        ),
    })
}

/// Capability map line 1290: *"allow the user to override reserve protection
/// for a specific task or session"* — the user-facing half.
///
/// # The scope is the whole point
///
/// The override is recorded as a **session identifier**, never as a flag.
/// There is no argument here that means "every session", and
/// [`glasshouse::routing::disposable::ReserveOverride`] has no constructor
/// that could express one: an override covering everything would be the
/// protected reserve disabled, which is a different capability from the one
/// this line asks for and a worse one, because the reserve exists to stop
/// background jobs exhausting the quota an interactive session needs.
///
/// The identifier is resolved through the session store first, so what lands
/// in the configuration is the canonical id rather than whatever prefix was
/// typed — the hook path that later reads it has resolved its own id the same
/// way, and two spellings of one session must not fail to match.
///
/// # Why the user layer
///
/// Writes go to the user-level configuration, like every other write outside
/// the settings UI: [`glasshouse::config::write_project_config_with_consent`]
/// puts a file inside the user's repository and its own doc comment reserves
/// that for a caller that has obtained explicit confirmation. Typing this
/// command is consent to record a preference, not consent to add a file to a
/// checked-out tree.
pub(crate) fn reserve_override_session(
    runtime: &Runtime,
    session: &str,
    clear: bool,
) -> anyhow::Result<String> {
    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    let id = store.resolve_id(session)?;
    let id = id.to_string();

    let mut user = UserConfig::load(runtime.paths())?;
    let mut granted: Vec<String> = user
        .routing()
        .reserve_override_sessions()
        .map(<[String]>::to_vec)
        .unwrap_or_default();
    granted.retain(|recorded| recorded != &id);
    if !clear {
        granted.push(id.clone());
    }
    // `Some(vec![])` rather than `None` once the user has touched this: an
    // empty list is "this layer says no sessions", which is a decision, and
    // `None` is "this layer never decided", which would defer to a project
    // layer the user has just tried to overrule. See the field's own doc.
    user.routing_mut()
        .set_reserve_override_sessions(Some(granted));
    user.save(runtime.paths())?;

    let short = &id[..id.len().min(8)];
    Ok(if clear {
        format!(
            "Session {short} no longer overrides reserve protection; its background jobs are \
             subject to the protected reserve again.\n"
        )
    } else {
        format!(
            "Session {short} may now spend protected quota reserve. No other session is \
             affected, and `glasshouse sessions reserve {short} --clear` withdraws it.\n"
        )
    })
}

/// The compiled-in adapter for a session's own recorded harness slug, or
/// `None` when the record names an integration this build has no adapter for
/// — a session recorded by a differently-built binary.
fn harness_adapter_for(
    harness_slug: &str,
) -> Option<&'static dyn glasshouse::harness::HarnessAdapter> {
    glasshouse::harness::all().find(|adapter| adapter.id().slug() == harness_slug)
}

/// This session's warmth for the restyle warning gate — line 619.
///
/// Deliberately simpler than `warm_session`, the router's own reader of the
/// same fact: that one asks whether a *candidate* session is reachable from a
/// routing decision being made about a different destination, which is why it
/// takes a `DestinationScope`. Here there is no candidate set — the session
/// named on the command line is the only session this question is ever about
/// — so an `Active` session is always the relevant one.
fn restyle_warmth(
    record: &SessionRecord,
    now_unix: i64,
) -> Option<glasshouse::config::pairing::WarmSession> {
    use glasshouse::config::pairing::{WarmSession, WarmSessionState};

    let state = match record.disposition() {
        SessionDisposition::Active => WarmSessionState::Live,
        SessionDisposition::Resumable => WarmSessionState::Resumable,
        SessionDisposition::Closed | SessionDisposition::Failed => return None,
    };
    Some(WarmSession {
        state,
        idle_seconds: (now_unix - record.last_activity_at).max(0),
    })
}

/// Refuse instruction text that could smuggle more than the one line it
/// promises, rather than trying to escape it.
///
/// The same conservatism `integrations::cmux`'s `PayloadHasBackslash` uses,
/// for the same reason: [`SessionApi::send_text`](glasshouse::session) appends
/// exactly one `\r` and writes the rest of the string as data, so a `\r` (or
/// any other control byte) already inside the text would submit as more than
/// one line once it reaches the pty. There is no correct way to transform
/// that away, so it is refused instead.
fn refuse_control_bytes(text: &str) -> anyhow::Result<()> {
    if text.chars().any(char::is_control) {
        anyhow::bail!(
            "this instruction contains a control byte (a line break or similar); refusing to \
             deliver it rather than trying to escape it, so it cannot submit as more than the \
             one line this override promises"
        );
    }
    Ok(())
}

/// Deliver one lightweight communication instruction into a running session,
/// for this turn only — capability map line 620.
///
/// Refuses by name for a harness whose communication-style declaration is
/// [`Declared::Unverified`](glasshouse::harness::Declared): typing an
/// unframed instruction at a harness nobody has read a mechanism for is a
/// guess, not an override, and 618's correction is explicit that inventing a
/// declaration here would invert the policy rather than merely degrade it.
/// Delivery itself goes through `crate::api::send_message` — the same input
/// path `glasshouse api send` and a person's own typing use — so it is never
/// a second copy of the write path, and it inherits that path's project scope
/// and liveness checks rather than repeating them.
pub(crate) fn tell_session(
    runtime: &Runtime,
    session: &str,
    instruction: &str,
) -> anyhow::Result<()> {
    refuse_control_bytes(instruction)?;

    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    let id = store.resolve_id(session)?;
    let record = store
        .get(&id)?
        .ok_or_else(|| anyhow::anyhow!("session `{id}` is not in this project"))?;

    let adapter = harness_adapter_for(&record.harness).ok_or_else(|| {
        anyhow::anyhow!(
            "no adapter registered for harness `{}` recorded on session {}",
            record.harness,
            crate::commands::shared::short_id(&id)
        )
    })?;

    if adapter.describe().communication_style.value().is_none() {
        anyhow::bail!(
            "{} declares no communication-style mechanism Glasshouse has read, so there is no \
             verified way to frame a one-turn instruction for it; refusing rather than typing \
             unframed text into session {}",
            adapter.id().display_name(),
            crate::commands::shared::short_id(&id)
        );
    }

    let framed = glasshouse::harness::response::one_turn_override(instruction);
    crate::api::send_message(runtime, id.as_str(), &framed)
}

/// Warn before, then carry out, a profile change on a running session —
/// capability map line 619.
///
/// The warning fires only when the adapter's own
/// [`StyleChange`](glasshouse::harness::StyleChange) declaration says the
/// harness needs a new native session for this change **and** the session is
/// genuinely warm ([`restyle_warmth`]); refusing it (no `--accept-loss`)
/// returns before anything is read from the harness's own declarations beyond
/// what decided the warning, so the session, its settings and its stored
/// response profile are left exactly as they were. A cold session, or one
/// whose harness can change style in place, proceeds straight to delivery.
///
/// Delivery reuses [`tell_session`]'s own mechanism — the resolved preset's
/// instruction text, framed the same way, sent through the same input path —
/// rather than writing a second copy of it: 619 asks for a warning in front
/// of a change, not a second way of making one.
pub(crate) fn restyle_session(
    runtime: &Runtime,
    session: &str,
    profile: &str,
    accept_loss: bool,
) -> anyhow::Result<()> {
    let preset = glasshouse::profile::response::presets()
        .iter()
        .find(|preset| preset.name == profile)
        .ok_or_else(|| {
            let names: Vec<&str> = glasshouse::profile::response::presets()
                .iter()
                .map(|preset| preset.name)
                .collect();
            anyhow::anyhow!(
                "`{profile}` is not a response preset Glasshouse knows; the presets are: {}",
                names.join(", ")
            )
        })?;

    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    let id = store.resolve_id(session)?;
    let record = store
        .get(&id)?
        .ok_or_else(|| anyhow::anyhow!("session `{id}` is not in this project"))?;

    let adapter = harness_adapter_for(&record.harness).ok_or_else(|| {
        anyhow::anyhow!(
            "no adapter registered for harness `{}` recorded on session {}",
            record.harness,
            crate::commands::shared::short_id(&id)
        )
    })?;

    let described = adapter.describe();
    let Some(style) = described.communication_style.value() else {
        anyhow::bail!(
            "{} declares no communication-style mechanism Glasshouse has read, so there is no \
             verified way to restyle session {} without guessing; refusing rather than typing an \
             unframed instruction into it",
            adapter.id().display_name(),
            crate::commands::shared::short_id(&id)
        );
    };
    let change = style.change;

    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    let warmth = restyle_warmth(&record, now_unix);

    if change == glasshouse::harness::StyleChange::NewSession
        && let Some(warm) = warmth
        && !accept_loss
    {
        anyhow::bail!(
            "restyling session {} to `{profile}` needs a new {} session — its communication-\
             style mechanism cannot change in place, and this session is warm ({}, idle {}s). \
             Refusing leaves the session, its settings and its stored response profile \
             untouched; re-run with --accept-loss to give it up and restyle anyway.",
            crate::commands::shared::short_id(&id),
            adapter.id().display_name(),
            warm.state,
            warm.idle_seconds
        );
    }

    let framed = glasshouse::harness::response::one_turn_override(&preset.profile.instruction());
    crate::api::send_message(runtime, id.as_str(), &framed)
}

/// Retire Glasshouse's record of a session — line 654.
///
/// The second line is the whole of the capability's second half, said out
/// loud: Glasshouse closed its own record and touched nothing the harness
/// owns. The native session identifier is printed because it is what a person
/// would use to find that history afterwards, and printing it is the proof
/// that closing did not take it away.
pub(crate) fn close_session(runtime: &Runtime, session: &str) -> anyhow::Result<String> {
    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    let id = store.resolve_id(session)?;
    let record = store.close(&id)?;

    let mut out = format!(
        "Closed Glasshouse's record of session {}.\n",
        crate::commands::shared::short_id(&record.id)
    );
    let kept = match &record.native_session_id {
        Some(native) => format!(
            "The {} session `{native}` was not touched: Glasshouse does not \
             own that history and did not delete it.\n",
            record.harness
        ),
        None => "No native session was ever recorded for it, so there was no \
                 harness history to keep or lose.\n"
            .to_owned(),
    };
    out.push_str(&kept);
    Ok(out)
}
