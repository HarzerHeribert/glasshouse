//! `commands::routing_cost` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use glasshouse::Runtime;

/// `glasshouse routing-cost` — capability map line 1464: what Glasshouse's
/// own routing model has spent, in tokens and requests, apart from every
/// other row this project's evidence ledger holds.
///
/// # Why the ledger is opened here, and nowhere earlier (practice §65)
///
/// The same reasoning [`record_classification_observation`]'s own header
/// gives: an open [`glasshouse::routing::evidence::EvidenceLedger`] holds a
/// SQLite handle for its whole lifetime, and a handle opened for work that
/// never happens blocks a later writer under Windows while staying invisible
/// under POSIX advisory locks. This command's handler is the one path that
/// actually reads the ledger, so it is opened here and nowhere upstream of
/// it.
pub(crate) fn routing_cost_report(runtime: &Runtime, hours: u32) -> anyhow::Result<String> {
    let ledger = glasshouse::routing::evidence::EvidenceLedger::open(runtime)?;
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let window_seconds = i64::from(hours) * 3600;
    let earliest_unix = now_unix.saturating_sub(window_seconds);
    let groups = ledger.consumption_by_purpose(now_unix, window_seconds)?;
    let translation = ledger.translation_cache_savings(now_unix, window_seconds)?;
    // Map line 2019's per-session clause: the same window and the same rows,
    // grouped by migration 24's `session_id` instead of by route and
    // credential.
    let translation_by_session =
        ledger.session_translation_cache_savings(now_unix, window_seconds)?;
    // Map line 2039's shadow measurement, gathered beside the two translation
    // facets above from the same window: the evidence for or against
    // `GH-EFFORT-CLAMP`, never the clamp itself.
    let effort_shadow = ledger.effort_shadow(now_unix, window_seconds)?;
    // Map line 1850, gathered beside the effort shadow from the same window:
    // whether effective TTFC separates usable turns from unusable ones
    // better than the other three responsiveness figures.
    let separation = ledger.responsiveness_separation(now_unix, window_seconds)?;
    // Fail-soft, the same posture `context_firewall_savings_summary` already
    // takes for `status`: a raw store this build cannot read yet renders as
    // "not counted", never as a hard error for a readout command.
    let store = glasshouse::firewall::RawStore::open(runtime.state_dir().join("context-firewall"));
    let firewall_savings = store.savings_in_window(earliest_unix, now_unix).ok();
    let bypass_count: usize = groups
        .iter()
        .filter(|group| {
            group.purpose.as_deref()
                == Some(glasshouse::routing::evidence::CONTEXT_FIREWALL_BYPASS_PURPOSE)
        })
        .map(|group| group.sample_count)
        .sum();
    Ok(render_routing_cost(
        runtime.project().id().as_str(),
        hours,
        &groups,
        firewall_savings,
        bypass_count,
        &translation,
        &translation_by_session,
        &effort_shadow,
        &separation,
    ))
}

/// Render [`routing_cost_report`]'s per-`(purpose, harness_recorded)` groups
/// as `glasshouse routing-cost` prints them.
///
/// **The one rule this function exists to hold:** a token figure nobody
/// counted prints as the words *not counted*, never as a digit and never as
/// `0` — the hazard this whole package was built to avoid, because "nothing
/// was spent" and "nobody counted it" are different facts and a reader who
/// cannot tell them apart has been handed a fabrication. It is the
/// coding-agent group, below, that this bites hardest: it has a real request
/// count and no token count at all, and the two must never be allowed to
/// look like the same kind of absence.
///
/// Capability map line 1331's gateway half applies the same rule to a
/// different pair of columns: `first-byte samples` is a real count (honestly
/// `0` when nothing timed), and `time to first byte` is `render_time_to_first_byte`'s
/// own *not recorded* — never `0ms` — for exactly that case. Unlike the token
/// columns above, the coding-agent group is the one group this build **can**
/// honestly time, because a first-byte instant is a clock reading rather than
/// a read of the response body the relay never parses.
///
/// `GH-STREAM-FIRST-EVENTS` (lines 1331/1332) adds two more such pairs beside
/// it, `first-token`/`TTFT` and `first-tool-call`/`TTFC` — but only a
/// **translated** exchange can ever supply a sample for either:
/// `crate::gateway::translate` decodes every provider event anyway, so the
/// instant a qualifying one passes is a clock reading, same as the byte
/// above; a relayed exchange leaves both `NULL`.
///
/// `GH-TOOL-ROUNDS-ON-TRANSLATED` (1334's last two quantities and 1350) adds
/// one more line, `tool rounds`, through [`render_tool_rounds`]: rounds
/// begun, repairs, and rounds per minute of the group's summed serving time
/// — *not recorded*, never `0`, for a group that never counted a round. It is
/// printed as an outcome-adjacent measure and never folded into a score, the
/// same restraint `render_savings_section` and this function's own token
/// figures already keep.
///
/// # Phase 33B's four figures, and why they are four lines
///
/// `GH-STREAM-TIMING-MS` gives those latency lines a resolution worth
/// comparing (`crate::database` migration 25) and names them for what each
/// one measures:
///
/// - **`TTFC (tool-using responsiveness)`** — line 1347. The primary
///   responsiveness measure for tool-using agent work, and *primary* here is
///   the label and the position: it leads the four, ahead of TTFT.
/// - **`TTFT (generation responsiveness)`** — line 1348. Kept as a separate
///   measure of generation responsiveness and never presented as agent
///   productivity, which is why it is not merged into the line above it.
/// - **`decode tokens/s (model serving)`** — line 1349, through
///   [`render_decode_rate`]. A characteristic of how the model is served,
///   never a statement about task progress.
/// - **`tool rounds`** — line 1350, unchanged.
///
/// **Line 1355 is the restraint that governs all four**: raw TTFC, TTFT,
/// throughput and rounds per minute stay visible separately rather than
/// collapsing into one performance headline. There is no total here, no
/// score, and no arithmetic joining any two of them — each is its own line
/// with its own label and its own *not recorded*. `effective TTFC`, the
/// fifth figure 1355 names, has no producer in this build at all; when
/// `GH-EFFECTIVE-TTFC` supplies one it gets a line of its own under the same
/// rule and never a slot inside one of these.
///
/// `(seconds only)` on a latency line is [`render_latency_ms`]'s marker for
/// a group whose rows all predate migration 25: the figure is a
/// one-second clock rendered in milliseconds, which is honest and nearly
/// useless, and saying so is cheaper than letting a reader compare it
/// against a measured one.
#[allow(clippy::too_many_arguments)]
fn render_routing_cost(
    project_id: &str,
    hours: u32,
    groups: &[glasshouse::routing::evidence::PurposeConsumption],
    firewall_savings: Option<glasshouse::firewall::WindowSavings>,
    firewall_bypasses: usize,
    translation: &[glasshouse::routing::evidence::TranslationSavings],
    translation_by_session: &[glasshouse::routing::evidence::SessionTranslationSavings],
    effort_shadow: &glasshouse::routing::evidence::EffortShadow,
    separation: &glasshouse::routing::evidence::SeparationReport,
) -> String {
    let mut out = format!("Routing consumption for project {project_id}, last {hours}h\n");
    if groups.is_empty() {
        out.push_str("\n  no routing observations recorded in this window\n");
    } else {
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
            out.push_str(&format!(
                "    first-byte samples  : {}\n",
                group.first_byte_sample_count
            ));
            out.push_str(&format!(
                "    time to first byte  : {}\n",
                render_latency_ms(
                    group.mean_time_to_first_byte_ms,
                    group.first_byte_ms_sample_count,
                    group.first_byte_sample_count,
                )
            ));
            // Line 1347 is the ordering as much as the label: TTFC leads the
            // four figures because it is the responsiveness measure for
            // tool-using agent work, and TTFT follows it as a separate
            // measure of something else (1348) rather than as its summary.
            out.push_str(&format!(
                "    first-tool-call samples : {}\n",
                group.first_tool_call_sample_count
            ));
            out.push_str(&format!(
                "    TTFC (tool-using responsiveness) : {}\n",
                render_latency_ms(
                    group.mean_time_to_first_tool_call_ms,
                    group.first_tool_call_ms_sample_count,
                    group.first_tool_call_sample_count,
                )
            ));
            // 1355's fifth figure, GH-RESPONSIVENESS-TERMS: never merged
            // into the TTFC line above it — its own line, right after, `not
            // recorded` or `(below floor)` per group.
            out.push_str(&format!(
                "    effective TTFC (reliability-adjusted) : {}\n",
                render_effective_ttfc(group)
            ));
            out.push_str(&format!(
                "    first-token samples : {}\n",
                group.first_token_sample_count
            ));
            out.push_str(&format!(
                "    TTFT (generation responsiveness) : {}\n",
                render_latency_ms(
                    group.mean_time_to_first_token_ms,
                    group.first_token_ms_sample_count,
                    group.first_token_sample_count,
                )
            ));
            out.push_str(&format!(
                "    decode tokens/s (model serving) : {}\n",
                render_decode_rate(group)
            ));
            out.push_str(&format!(
                "    tool rounds         : {}\n",
                render_tool_rounds(group)
            ));
        }
    }
    out.push_str(
        "\ncoding-agent consumption relayed through the gateway is never counted in this \
         build (the relay never parses a reply body), so the coding-agent group above always \
         has its tokens print as \"not counted\" even though its request count is real; \
         \"not counted\" always means nobody read a count, never that nothing was spent.\n",
    );
    out.push_str(&render_savings_section(
        firewall_savings,
        firewall_bypasses,
        translation,
        translation_by_session,
    ));
    out.push_str(&render_effort_shadow_section(effort_shadow));
    out.push_str(&render_responsiveness_separation(separation));
    out
}

/// Map line 2034: what was saved, by purpose, each figure with its own
/// denominator — Phase 58's ingestion of Headroom's *"savings readout that
/// is a query over the ledger"* (design-decisions.md, *"Headroom,
/// compared"*, Taken item 4). Three facets, and the same rule
/// [`render_routing_cost`]'s own doc comment states for every other figure
/// in this report: a quantity nobody recorded prints as words, never as a
/// digit and never as `0`.
fn render_savings_section(
    firewall_savings: Option<glasshouse::firewall::WindowSavings>,
    firewall_bypasses: usize,
    translation: &[glasshouse::routing::evidence::TranslationSavings],
    translation_by_session: &[glasshouse::routing::evidence::SessionTranslationSavings],
) -> String {
    let mut out = String::from("\nSAVINGS\n");

    out.push_str("\n  context firewall\n");
    match firewall_savings {
        Some(savings) if savings.results > 0 || firewall_bypasses > 0 => {
            let total = savings.results + firewall_bypasses;
            let unestimated_note = if savings.unestimated > 0 {
                format!(" ({} without a recorded estimate)", savings.unestimated)
            } else {
                String::new()
            };
            out.push_str(&format!(
                "    kept local (estimated) {} tokens across {} reductions of {total} results \
                 above threshold{unestimated_note}\n",
                savings.kept_local, savings.results
            ));
        }
        _ => {
            out.push_str("    not counted: no context-firewall activity recorded in this window\n")
        }
    }

    if translation.is_empty() {
        out.push_str("\n  translation\n    not counted: no translated exchange recorded\n");
    } else {
        for row in translation {
            let route = row.route.as_deref().unwrap_or("(no route recorded)");
            let quota_context = row
                .quota_context
                .as_deref()
                .unwrap_or("(no credential recorded)");
            out.push_str(&format!("\n  translation {route} / {quota_context}\n"));
            let denominator = row.input_tokens + row.cached_input_tokens;
            let ratio = row
                .cache_read_ratio()
                .map(|fraction| format!("{:.1}%", fraction * 100.0))
                .unwrap_or_else(|| "not counted".to_owned());
            out.push_str(&format!(
                "    prompt-cache reads {} of {denominator} translated input tokens ({ratio})\n",
                row.cached_input_tokens
            ));
        }
    }

    // Map line 2019's per-session clause, beside the per-credential grouping
    // above it and read off the same rows over the same window — see
    // `SessionTranslationSavings`.
    if translation_by_session.is_empty() {
        out.push_str(
            "\n  translation by session\n    not counted: no translated exchange recorded\n",
        );
    } else {
        out.push_str("\n  translation by session\n");
        for row in translation_by_session {
            // Never an empty name and never a `0`: a group whose rows name no
            // session is a real reading about real exchanges, and it says in
            // words which fact it is. A gateway is told its session by the
            // launch, so this is what a build older than migration 24 wrote,
            // or what a gateway nobody told wrote.
            let session = row.session_id.as_deref().unwrap_or("(no session recorded)");
            let denominator = row.input_tokens + row.cached_input_tokens;
            // The ratio is a *rate*, so it sits behind the standing sample
            // floor every other rate on this ledger sits behind
            // (`MIN_SAMPLE_FOR_SUMMARY`), and below it prints as words —
            // `render_token_count`'s own rule, never a digit. The counts
            // beside it are counts, honest at any sample size, exactly as
            // the per-purpose groups' `requests` line above is.
            let ratio = if row.meets_sample_floor() {
                row.cache_read_ratio()
                    .map(|fraction| format!("{:.1}%", fraction * 100.0))
                    .unwrap_or_else(|| "not counted".to_owned())
            } else {
                format!(
                    "not counted: {} of {} exchanges needed",
                    row.sample_count,
                    glasshouse::routing::evidence::MIN_SAMPLE_FOR_SUMMARY
                )
            };
            out.push_str(&format!(
                "    {session}  {} exchanges, prompt-cache reads {} of {denominator} \
                 translated input tokens ({ratio})\n",
                row.sample_count, row.cached_input_tokens
            ));
        }
    }

    out.push_str(
        "\n  response profile\n    not counted: no exchange row carries a response profile \
         (migration 24 added the session column this could be joined through, but no producer \
         stamps a response profile on a routing-observation row, so there is still nothing to \
         count — capability map line 627's, not this package's)\n",
    );

    out
}

/// Capability map line 2039: the shadow measurement
/// `docs/product/design-decisions.md`'s *Carrying effort across a translated
/// pairing* asks for before any clamp is offered, printed after `SAVINGS`
/// rather than folded into it — line 2039 is its own map line, not one of
/// 2034's three facets.
///
/// [`render_routing_cost`]'s own rule applies here too: a quantity nobody
/// recorded prints as words, never as a digit and never as `0` — the
/// `unread` count is the one exception, because it is a real count of rows
/// this build genuinely read and can name, not an absence.
fn render_effort_shadow_section(shadow: &glasshouse::routing::evidence::EffortShadow) -> String {
    let mut out = String::from("\nEFFORT SHADOW\n");
    if shadow.rows.is_empty() {
        out.push_str("\n  no translated exchanges recorded in this window\n");
    } else {
        for row in &shadow.rows {
            let effort = row
                .effort_level
                .map(glasshouse::routing::evidence::EffortLevel::as_str)
                .unwrap_or("(no effort recorded)");
            out.push_str(&format!("\n  {} / {effort}\n", row.turn_shape.as_str()));
            let median = match row.median_output_tokens {
                Some(tokens) => tokens.to_string(),
                None => format!(
                    "below the sample floor ({} of {} exchanges needed)",
                    row.sample_count,
                    glasshouse::routing::evidence::MIN_SAMPLE_FOR_SUMMARY
                ),
            };
            out.push_str(&format!(
                "    {} exchanges, median output tokens {median}\n",
                row.sample_count
            ));
            out.push_str(&format!(
                "    verdicts: {} completed, {} failed, {} unverdicted\n",
                row.completed, row.failed, row.unverdicted
            ));
        }
    }
    out.push_str(&format!(
        "\n  unread: {} (rows with no recorded turn shape — relayed, or written before the \
         column existed)\n",
        shadow.unread
    ));
    out.push_str("\n  a clamp is not offered; this section is the evidence for or against one.\n");
    out
}

/// The label one [`PurposeConsumption`][glasshouse::routing::evidence::PurposeConsumption]
/// group prints under.
///
/// `purpose` alone cannot tell coding-agent consumption apart from every
/// other unstamped producer — both leave it `NULL` — so a `None` purpose is
/// read alongside `harness_recorded`, exactly as
/// [`glasshouse::routing::evidence::PurposeConsumption`]'s own doc comment
/// explains: only the gateway relay names a harness on every row it writes.
///
/// Capability map line 1330 began stamping that same relay traffic with
/// [`glasshouse::routing::evidence::HARNESS_TURN_PURPOSE`], so the stamped and
/// the unstamped rows are **one fact across a build boundary**, not two — the
/// identical treatment `RoutingOverhead::from_consumption` gives them. Without
/// the first arm below a stamped row falls through to the general case and the
/// report prints the raw constant `harness-turn` where a person used to read
/// this label; `tests/routing_cost.rs` caught exactly that.
fn purpose_group_label(group: &glasshouse::routing::evidence::PurposeConsumption) -> &str {
    match (group.purpose.as_deref(), group.harness_recorded) {
        (Some(glasshouse::routing::evidence::HARNESS_TURN_PURPOSE), _) | (None, true) => {
            "coding-agent (gateway relay)"
        }
        (Some(purpose), _) => purpose,
        (None, false) => "(no purpose or harness recorded)",
    }
}

/// `Some(n)` as a digit, `None` as the phrase [`render_routing_cost`]'s own
/// doc comment names — never `0` for a count this build never read.
fn render_token_count(value: Option<i64>) -> String {
    match value {
        Some(count) => count.to_string(),
        None => "not counted".to_owned(),
    }
}

/// Capability map line 1331's gateway half, rendered — `render_token_count`'s
/// own rule applied to a timing column rather than a token count: a group
/// with no timed rows prints the words *not recorded*, never a digit and
/// never `0ms`, because "the mean was zero" and "nothing was timed" are
/// different facts and this build must never let them look the same.
///
/// `mean_ms` is [`None`] exactly when
/// [`glasshouse::routing::evidence::PurposeConsumption::first_byte_sample_count`]
/// is `0` — see that field's own doc comment — so there is nothing else this
/// function needs to check.
///
/// Despite its name, the shape is generic over any mean-milliseconds column
/// with the same "sample count zero means `None`" contract, so
/// `render_routing_cost` also calls it for `mean_time_to_first_token_ms` and
/// `mean_time_to_first_tool_call_ms` — one renderer for all three timing
/// pairs rather than three copies of the same match.
fn render_time_to_first_byte(mean_ms: Option<f64>) -> String {
    match mean_ms {
        Some(ms) => format!("{}ms (mean)", ms.round() as i64),
        None => "not recorded".to_owned(),
    }
}

/// [`render_time_to_first_byte`] plus migration 25's honesty marker and the
/// independent verifier's own finding on the Red package
/// (`docs/product/evidence/phase-33b.md`'s 1347/1348/1349 entry): the count
/// printed beside a mixed group's mean must be the count of rows the mean
/// was *actually measured from*, not the seconds-resolution count above it
/// on the readout, which a mixed group's mean is not honestly described by.
///
/// The mean a group carries is computed over both kinds of row — the
/// measured `*_ms` offset where one exists, the second-resolution difference
/// where it does not — so a figure alone cannot say which it is. When no row
/// in the group carried a measured offset, *(seconds only)* is appended:
/// the number is real, and it is a one-second clock rounded into
/// milliseconds, which for a time-to-first-token is very nearly always `0`
/// or `1000`. Every group with a figure at all — mixed or fully measured —
/// additionally gets `(N of M measured)`: `ms_sample_count` of
/// `total_sample_count`, so a reader can tell a mean over five measured rows
/// apart from one over five where only two were.
///
/// *not recorded* is never marked: there is no figure to qualify.
fn render_latency_ms(
    mean_ms: Option<f64>,
    ms_sample_count: usize,
    total_sample_count: usize,
) -> String {
    let rendered = render_time_to_first_byte(mean_ms);
    if mean_ms.is_none() {
        return rendered;
    }
    let seconds_only_marker = if ms_sample_count == 0 {
        " (seconds only)"
    } else {
        ""
    };
    format!("{rendered}{seconds_only_marker} ({ms_sample_count} of {total_sample_count} measured)")
}

/// Capability map line 1349, rendered: decode tokens per second, **as a
/// model-serving characteristic and never as task progress**.
///
/// Its own line, its own label, and no arithmetic joining it to TTFC, TTFT
/// or the rounds rate beside it — line 1355's rule, which this function
/// exists to keep by construction rather than by intention.
/// `render_token_count`'s rule again for the absence: a group whose rows
/// carry no millisecond offsets has nothing to divide, and says so rather
/// than printing `0.00`.
fn render_decode_rate(group: &glasshouse::routing::evidence::PurposeConsumption) -> String {
    match group.decode_tokens_per_second() {
        Some(rate) => format!("{rate:.2} tok/s (mean)"),
        None => "not recorded".to_owned(),
    }
}

/// Line 1334's last two quantities and line 1350, rendered — the group's
/// rounds begun, its repairs, and rounds per minute of summed serving time.
/// `render_token_count`'s own rule again: a group with no counted
/// `tool_rounds` prints *not recorded*, never `0`, because "nobody read a
/// count" and "the count was zero" are different facts.
///
/// Gated on [`glasshouse::routing::evidence::PurposeConsumption::tool_rounds`]
/// alone, matching the OBJECTIVE's own rule — a real gateway group that
/// carries rounds always carries repairs and serving time too, since all
/// three are read off the same decoded translated exchanges.
fn render_tool_rounds(group: &glasshouse::routing::evidence::PurposeConsumption) -> String {
    let (Some(rounds), Some(repairs), Some(serving_seconds)) =
        (group.tool_rounds, group.repairs, group.serving_seconds)
    else {
        return "not recorded".to_owned();
    };
    let per_minute = group
        .tool_rounds_per_minute()
        .map(|rate| format!("{rate:.2}/min"))
        .unwrap_or_else(|| "not recorded".to_owned());
    format!("{rounds} begun, {repairs} repairs, {per_minute} over {serving_seconds}s served")
}

/// Line 1355's fifth figure, `GH-RESPONSIVENESS-TERMS`: reliability-adjusted
/// TTFC, printed on its own line directly after the raw `TTFC` line and
/// never merged into it. `not recorded` when the group carries no raw TTFC
/// at all; `(below floor)` when it does but
/// [`glasshouse::routing::evidence::PurposeConsumption::effective_ttfc_ms`]
/// still answers `None` — too few measured rows on either half of the
/// formula, or a failure rate that could not be computed.
fn render_effective_ttfc(group: &glasshouse::routing::evidence::PurposeConsumption) -> String {
    if group.mean_time_to_first_tool_call_ms.is_none() {
        return "not recorded".to_owned();
    }
    match group.effective_ttfc_ms() {
        Some(ms) => format!("{}ms (mean)", ms.round() as i64),
        None => "(below floor)".to_owned(),
    }
}

/// Map line 1850, printed at the end of `routing-cost`: whether effective
/// TTFC separates usable agent turns from unusable ones better than raw
/// TTFC, TTFT or decode tokens per second. *Separates*, never *predicts* —
/// this is a comparison of medians over exchanges with a harness-reported
/// verdict, not a claim of causation.
fn render_responsiveness_separation(
    separation: &glasshouse::routing::evidence::SeparationReport,
) -> String {
    let mut out = String::from("\nresponsiveness vs usable turns (1850):\n");
    for row in &separation.rows {
        match row.separation() {
            Some(fraction) => out.push_str(&format!(
                "  {:<15} : separates {:.1}% ({} usable, {} unusable turns)\n",
                row.measure,
                fraction * 100.0,
                row.usable_sample,
                row.unusable_sample
            )),
            None => out.push_str(&format!(
                "  {:<15} : not enough evidence ({} usable, {} unusable turns; \
                 {MIN_SAMPLE_SEPARATION} needed on each side)\n",
                row.measure, row.usable_sample, row.unusable_sample
            )),
        }
    }
    out
}

/// [`render_responsiveness_separation`]'s own floor, named for the message
/// rather than repeating the number — [`glasshouse::routing::evidence::MIN_SAMPLE_FOR_SUMMARY`]
/// verbatim, the same floor [`SeparationMeasure::separation`] itself gates
/// on.
const MIN_SAMPLE_SEPARATION: usize = glasshouse::routing::evidence::MIN_SAMPLE_FOR_SUMMARY;
