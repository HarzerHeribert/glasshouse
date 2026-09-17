//! `glasshouse cost` — per-session and per-window cost visibility, the one
//! thing the 2026-09-16 ruling (*Glasshouse never decides which model is
//! used*) keeps from the deleted router: what was spent still needs to be
//! seen, even though nothing here chooses where it is spent. Built entirely
//! over the evidence ledger's KEEP readers —
//! [`glasshouse::routing::evidence::EvidenceLedger::consumption_by_purpose`]
//! and
//! [`glasshouse::routing::evidence::EvidenceLedger::cached_share_for_session`]
//! — never a route score, a candidate ranking, or a classifier spend.

use glasshouse::Runtime;
use glasshouse::routing::evidence::{EvidenceLedger, HARNESS_TURN_PURPOSE, PurposeConsumption};

/// `glasshouse cost` with no `--session`: every purpose's request and token
/// consumption over the last `hours`, or since `since_unix` when given.
///
/// # Why the ledger is opened here (practice §65)
///
/// An open [`EvidenceLedger`] holds a SQLite handle for its whole lifetime,
/// and a handle opened for work that never happens blocks a later writer
/// under Windows while staying invisible under POSIX advisory locks. This
/// command's handler is the one path that actually reads the ledger, so it
/// is opened here and nowhere upstream of it.
pub(crate) fn cost_report(
    runtime: &Runtime,
    hours: u32,
    since_unix: Option<i64>,
) -> anyhow::Result<String> {
    let ledger = EvidenceLedger::open(runtime)?;
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let window_seconds = window_seconds(hours, since_unix, now_unix);
    let groups = ledger.consumption_by_purpose(now_unix, window_seconds)?;
    Ok(render_cost_by_purpose(
        runtime.project().id().as_str(),
        hours,
        &groups,
    ))
}

/// `glasshouse cost --session <id>`: that session's own cached-input share
/// over its own translated exchanges — never windowed, since a session's
/// exchanges are already a bounded set (see
/// [`EvidenceLedger::cached_share_for_session`]'s own doc for why windowing
/// them by recency would silently drop its earliest turns).
///
/// **The one decision this command makes**: a session filter narrows to
/// exactly that session's own reading, never the whole ledger's. There is no
/// path here that answers a `--session` request with unfiltered, project-wide
/// consumption.
pub(crate) fn cost_session_report(runtime: &Runtime, session_id: &str) -> anyhow::Result<String> {
    let ledger = EvidenceLedger::open(runtime)?;
    let savings = ledger.cached_share_for_session(session_id)?;
    Ok(render_cost_for_session(session_id, savings.as_ref()))
}

/// `glasshouse cost --json [--session <id>]`: one `serde_json` object per
/// observation in the window, filtered to `session_id` when given — the wire
/// shape the pane's meter reads (capability map line 2430).
///
/// The session filter is applied here, over rows this command already
/// fetched, rather than by asking the ledger for a per-session query: there
/// is exactly one reader that opens the database
/// ([`EvidenceLedger::consumption_in_window`]), and `--session` narrows its
/// output rather than adding a second query the ledger did not already need
/// (CLAUDE.md rule 8).
pub(crate) fn cost_json_report(
    runtime: &Runtime,
    hours: u32,
    since_unix: Option<i64>,
    session_id: Option<&str>,
) -> anyhow::Result<String> {
    let ledger = EvidenceLedger::open(runtime)?;
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let window_seconds = window_seconds(hours, since_unix, now_unix);
    // `consumption_in_window`, not `observations_in_window`: the pane's meter
    // reads every observation this window holds, including a relayed
    // exchange whose reply the gateway could not read and so recorded no
    // outcome — exactly the row `outcome:null` below must still print.
    let observations = ledger.consumption_in_window(now_unix, window_seconds)?;
    let mut out = String::new();
    for observation in &observations {
        if let Some(session_id) = session_id
            && observation.session_id.as_deref() != Some(session_id)
        {
            continue;
        }
        out.push_str(&serde_json::to_string(&observation_json(observation))?);
        out.push('\n');
    }
    Ok(out)
}

fn window_seconds(hours: u32, since_unix: Option<i64>, now_unix: i64) -> i64 {
    match since_unix {
        Some(since) => now_unix.saturating_sub(since).max(0),
        None => i64::from(hours) * 3600,
    }
}

/// [`cost_report`]'s prose rendering. The one rule this function exists to
/// hold: a token figure nobody counted prints as the words *not counted*,
/// never as a digit and never as `0` — "nothing was spent" and "nobody
/// counted it" are different facts, and a reader who cannot tell them apart
/// has been handed a fabrication.
fn render_cost_by_purpose(project_id: &str, hours: u32, groups: &[PurposeConsumption]) -> String {
    let mut out = format!("Cost for project {project_id}, last {hours}h\n");
    if groups.is_empty() {
        out.push_str("\n  no observations recorded in this window\n");
        return out;
    }
    for group in groups {
        let label = purpose_group_label(group);
        out.push_str(&format!("\n  {label}\n"));
        out.push_str(&format!(
            "    requests            : {}\n",
            group.sample_count
        ));
        out.push_str(&format!(
            "    input tokens        : {}\n",
            render_token_count(group.input_tokens)
        ));
        out.push_str(&format!(
            "    output tokens       : {}\n",
            render_token_count(group.output_tokens)
        ));
        out.push_str(&format!(
            "    cached input tokens : {}\n",
            render_token_count(group.cached_input_tokens)
        ));
    }
    out
}

fn purpose_group_label(group: &PurposeConsumption) -> &str {
    match (group.purpose.as_deref(), group.harness_recorded) {
        (Some(HARNESS_TURN_PURPOSE), _) | (None, true) => "coding-agent (gateway relay)",
        (Some(purpose), _) => purpose,
        (None, false) => "(no purpose or harness recorded)",
    }
}

/// `Some(n)` as a digit, `None` as the phrase [`render_cost_by_purpose`]'s
/// own doc comment names — never `0` for a count this build never read.
fn render_token_count(value: Option<i64>) -> String {
    match value {
        Some(count) => count.to_string(),
        None => "not counted".to_owned(),
    }
}

/// [`cost_session_report`]'s prose rendering.
fn render_cost_for_session(
    session_id: &str,
    savings: Option<&glasshouse::routing::evidence::SessionTranslationSavings>,
) -> String {
    let Some(savings) = savings else {
        return format!(
            "Session {session_id}: no translated exchange has reported cached-input tokens \
             for this session\n"
        );
    };
    let denominator = savings.input_tokens + savings.cached_input_tokens;
    let ratio = savings
        .cache_read_ratio()
        .map(|fraction| format!("{:.1}%", fraction * 100.0))
        .unwrap_or_else(|| "not counted".to_owned());
    format!(
        "Session {session_id}: {} exchanges, prompt-cache reads {} of {denominator} translated \
         input tokens ({ratio})\n",
        savings.sample_count, savings.cached_input_tokens
    )
}

/// One `--json` line's exact shape — a view type owned here rather than a
/// `Serialize` impl on [`glasshouse::routing::evidence::RoutingObservation`]
/// itself: the readout owns its own wire shape, and the ledger gains no new
/// dependents for one reader (CLAUDE.md rule 8).
#[derive(serde::Serialize)]
struct ObservationJson<'a> {
    seq: i64,
    observed_at: i64,
    session_id: Option<&'a str>,
    harness: Option<&'a str>,
    provider: &'a str,
    model: &'a str,
    route: Option<&'a str>,
    purpose: Option<&'a str>,
    quota_context: Option<&'a str>,
    dispatched_at: Option<i64>,
    completed_at: Option<i64>,
    first_byte_ms: Option<i64>,
    completed_ms: Option<i64>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    outcome: Option<&'static str>,
    failure_class: Option<&'static str>,
    tool_rounds: Option<i64>,
    retries: Option<i64>,
    repairs: Option<i64>,
    failovers: Option<i64>,
    cost_micro_usd: Option<i64>,
    cost_confidence: Option<&'static str>,
}

/// [`ObservationJson`]'s only constructor — every `None` stays `None`
/// through to serialization, where `serde_json` renders it `null`.
fn observation_json(
    observation: &glasshouse::routing::evidence::RoutingObservation,
) -> ObservationJson<'_> {
    ObservationJson {
        seq: observation.seq,
        observed_at: observation.observed_at_unix,
        session_id: observation.session_id.as_deref(),
        harness: observation.harness.as_deref(),
        provider: &observation.provider,
        model: &observation.model,
        route: observation.route.as_deref(),
        purpose: observation.purpose.as_deref(),
        quota_context: observation.quota_context.as_deref(),
        dispatched_at: observation.dispatched_at_unix,
        completed_at: observation.completed_at_unix,
        first_byte_ms: observation.first_byte_ms,
        completed_ms: observation.completed_ms,
        input_tokens: observation.input_tokens,
        output_tokens: observation.output_tokens,
        cached_input_tokens: observation.cached_input_tokens,
        outcome: observation
            .outcome
            .map(glasshouse::routing::evidence::Outcome::as_str),
        failure_class: observation
            .failure_class
            .map(glasshouse::routing::evidence::FailureClass::as_str),
        tool_rounds: observation.tool_rounds,
        retries: observation.retries,
        repairs: observation.repairs,
        failovers: observation.failovers,
        cost_micro_usd: observation.cost.map(|cost| cost.micro_usd),
        cost_confidence: observation.cost.map(|cost| cost.confidence.as_str()),
    }
}
