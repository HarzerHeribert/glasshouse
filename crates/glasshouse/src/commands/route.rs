//! `commands::route` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use glasshouse::Runtime;
use glasshouse::config::{self, EffectiveConfig, UserConfig};

/// Whether a launch under `profile` would get past `glasshouse::profile::resolve`'s
/// protocol check, which refuses with `Refusal::ProtocolMismatch`.
///
/// # Why the launch path has to ask this and the diagnostic does not
///
/// The two are asking different questions and both answers are right. The
/// router's `ProtocolFit::Compatible` means *"not this protocol, but the
/// provider serves another one the harness does speak"* — a true statement
/// about whether that provider and that harness can work together at all, and
/// the reason `Destination::with_provider_protocols` exists. `profile::resolve`
/// asks something narrower: whether the harness can serve the protocol **this
/// profile declared**, and it refuses rather than quietly picking a different
/// one, which is that module's whole discipline.
///
/// So a profile can be `Compatible` to the router and refused by the launch,
/// and offering the launch path a destination it will then refuse would turn a
/// routing decision into a failed command. The diagnostic keeps ranking it,
/// because "this provider could serve this harness, but not over the protocol
/// you configured" is exactly what a person needs to read.
fn launch_can_resolve_protocol(profile: &glasshouse::profile::LaunchProfile) -> bool {
    let Some(expected) = profile.expected_protocol else {
        return true;
    };
    glasshouse::harness::adapter_for(profile.harness)
        .map(|adapter| adapter.describe().backends.protocols)
        .and_then(|declared| {
            declared
                .value()
                .map(|protocols| protocols.contains(&expected))
        })
        .unwrap_or(false)
}

/// Capability map line 1564's caller half: how the most recent exchange on
/// `current`'s backend ended, from the evidence ledger, keyed exactly as the
/// gateway writes it (`exchange.provider`, `assignment.backend().model().label()`).
/// `None` when the ledger cannot be opened or holds nothing for the pair — a
/// missing history is not a failure, and the router's explanation says so.
fn latest_failure_class(
    runtime: &Runtime,
    current: &glasshouse::routing::session::Destination,
) -> Option<glasshouse::routing::evidence::FailureClass> {
    use glasshouse::routing::evidence::{CLASSIFICATION_EVIDENCE_WINDOW_SECONDS, EvidenceLedger};

    let ledger = match EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::debug!(error = %err, "evidence ledger unavailable; no retry promotion");
            return None;
        }
    };
    ledger
        .latest_failure_class_for_model(
            current.backend().provider(),
            current.backend().model().label(),
            glasshouse::provider::cache::now_unix_seconds(),
            CLASSIFICATION_EVIDENCE_WINDOW_SECONDS,
        )
        .unwrap_or_else(|err| {
            tracing::debug!(error = %err, "could not read the last failure class");
            None
        })
}

/// `glasshouse route` — map lines 1601 and 1602: the command, which is
/// [`route_recommendation`] asked and [`render_route_recommendation`]
/// printed, and nothing else.
///
/// The moment is parsed here rather than inside the recommendation because
/// this is where a person's typed spelling arrives, and the message they get
/// back quotes it.
pub(crate) fn route_report(
    runtime: &Runtime,
    moment: &str,
    to: Option<&str>,
    fresh: bool,
    now: bool,
    task: Option<&str>,
) -> anyhow::Result<String> {
    let Some(parsed) = routing_moment_from_str(moment) else {
        anyhow::bail!(
            "`{moment}` is not a routing moment; use `session-start`, `task-boundary` or \
             `mid-turn`"
        )
    };

    let user = UserConfig::load(runtime.paths())?;
    let project = config::load_project_config(runtime.project())?;
    let effective = EffectiveConfig::new(&user, project.as_ref());

    let recommendation = route_recommendation(runtime, &effective, parsed, to, fresh, now, task)?;
    let mut report = render_route_recommendation(&recommendation);
    report.push('\n');
    report.push_str(&route_outcomes_section(runtime));
    report.push_str(&tier_outcome_section(runtime));
    report.push_str(&capability_suggestions_section(runtime, &effective));
    report.push_str(&harness_efficiency_section(runtime));
    report.push_str(&support_work_section(runtime));
    report.push_str(&route_correlations_section(runtime));
    report.push_str(&throttle_scope_section(runtime));
    Ok(report)
}

/// Capability map lines 1370, 1373, 1374, 1376 and 1852, printed for a
/// person: every pair of routes this project's ledger has observed at the
/// same moments, with the sample size **before** any confidence — line
/// 1376's rule, that a correlation is presented as meaningful only once
/// enough overlapping observations exist, and otherwise as the count that
/// fell short — and beneath them how many gateway failovers the correlation
/// term actually steered.
///
/// Reads the routing ledger over
/// [`glasshouse::routing::evidence::CLASSIFICATION_EVIDENCE_WINDOW_SECONDS`]
/// — the same seven days the failover itself reads — so what this prints is
/// what the router weighed. Practice §65: opened here, dropped on return,
/// and a ledger that cannot be opened costs this section and never the
/// recommendation above it.
fn route_correlations_section(runtime: &Runtime) -> String {
    use glasshouse::routing::evidence::{
        CLASSIFICATION_EVIDENCE_WINDOW_SECONDS, CORRELATION_PURPOSE, CorrelationVerdict,
        EvidenceLedger,
    };

    let header =
        "\nRoute correlations in this project, last 7 days (map lines 1370-1376)\n".to_owned();
    let ledger = match EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::debug!(
                error = %err,
                "could not open the routing evidence ledger for the route-correlation section"
            );
            return format!("{header}\n  the routing evidence ledger could not be opened\n");
        }
    };
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let correlations =
        match ledger.route_correlations(now_unix, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS) {
            Ok(correlations) => correlations,
            Err(err) => return format!("{header}\n  {err}\n"),
        };
    // Line 1852, counted back by purpose: every row the failover-prevention
    // sink wrote for a steered failover, and nothing else.
    let steered: usize =
        match ledger.consumption_by_purpose(now_unix, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS) {
            Ok(groups) => groups
                .iter()
                .filter(|group| group.purpose.as_deref() == Some(CORRELATION_PURPOSE))
                .map(|group| group.sample_count)
                .sum(),
            Err(err) => return format!("{header}\n  {err}\n"),
        };

    let mut out = header;
    out.push('\n');
    if correlations.is_empty() {
        out.push_str(
            "  no two routes have been observed at the same moment, so nothing is known about \
             whether any pair fails together\n",
        );
    }
    for correlation in correlations.iter() {
        let (a, b) = correlation.routes();
        match correlation.verdict() {
            CorrelationVerdict::InsufficientEvidence {
                sample_size,
                required,
            } => {
                let _ = writeln_str(
                    &mut out,
                    format!(
                        "  {a} and {b}: insufficient evidence — {sample_size} of the {required} \
                         overlapping observations a correlation needs; treated as no correlation"
                    ),
                );
            }
            CorrelationVerdict::Measured {
                confidence,
                sample_size,
            } => {
                let _ = writeln_str(
                    &mut out,
                    format!(
                        "  {a} and {b}: failed the same way at the same moment in {} of \
                         {sample_size} overlapping observations — correlation {confidence:.2}",
                        correlation.overlaps()
                    ),
                );
            }
        }
    }
    out.push('\n');
    if steered == 0 {
        out.push_str(
            "  no gateway failover in this window was steered by a measured route \
             correlation (map line 1852)\n",
        );
    } else {
        let _ = writeln_str(
            &mut out,
            format!(
                "  {steered} gateway failover(s) in this window went somewhere other than a route \
                 whose failures overlap the failed backend's — separate quota, not separate \
                 failure resilience (map line 1852)"
            ),
        );
    }
    out
}

/// Capability map line 1317, printed for a person: every route this
/// project's ledger has seen throttled, and whether that throttle reads as
/// this provider's own cadence limiter firing everywhere or as one model's
/// own limit — line 1317's "track", not "act": nothing here changes a
/// failover, only what a person reading `glasshouse route` is told about a
/// throttle that already happened.
///
/// Two of the map line's four scopes are never printed here, on purpose:
/// **account-specific** and **request-pool-specific** have no producer in
/// this build — see [`glasshouse::routing::evidence::ThrottleScope`]'s own
/// doc comment and refusal register row 531.
///
/// Reads the same
/// [`glasshouse::routing::evidence::CLASSIFICATION_EVIDENCE_WINDOW_SECONDS`]
/// window as [`route_correlations_section`], for the same reason: what this
/// prints should be what a real failover, reading the ledger at the same
/// moment, would see.
fn throttle_scope_section(runtime: &Runtime) -> String {
    use glasshouse::routing::evidence::{
        CLASSIFICATION_EVIDENCE_WINDOW_SECONDS, EvidenceLedger, ThrottleScope,
    };

    let header = "\nThrottle scope in this project, last 7 days (map line 1317)\n".to_owned();
    let ledger = match EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::debug!(
                error = %err,
                "could not open the routing evidence ledger for the throttle-scope section"
            );
            return format!("{header}\n  the routing evidence ledger could not be opened\n");
        }
    };
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let scopes = match ledger.throttle_scopes(now_unix, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS) {
        Ok(scopes) => scopes,
        Err(err) => return format!("{header}\n  {err}\n"),
    };

    let mut out = header;
    out.push('\n');
    if scopes.is_empty() {
        out.push_str("  no throttle has been observed on any route in this window\n");
    }
    for (route, scope) in scopes.iter() {
        match scope {
            ThrottleScope::ProviderWide => {
                let _ = writeln_str(
                    &mut out,
                    format!(
                        "  {route}: provider-wide — a throttle on this route overlapped a \
                         throttle on another model of the same provider"
                    ),
                );
            }
            ThrottleScope::ModelSpecific => {
                let _ = writeln_str(
                    &mut out,
                    format!(
                        "  {route}: model-specific — every observed throttle on this route \
                         overlapped a sibling model that was not throttled"
                    ),
                );
            }
            ThrottleScope::AccountSpecific => {
                let _ = writeln_str(
                    &mut out,
                    format!(
                        "  {route}: account-specific — this account's sibling models were \
                         throttled together while another account of the same provider kept \
                         serving"
                    ),
                );
            }
            ThrottleScope::Unknown {
                sample_size,
                required,
            } => {
                let _ = writeln_str(
                    &mut out,
                    format!(
                        "  {route}: insufficient evidence — {sample_size} of the {required} \
                         informative throttle events a scope needs; treated as unknown"
                    ),
                );
            }
        }
    }
    out
}

/// `writeln!` without the `use std::fmt::Write` every caller would need.
fn writeln_str(out: &mut String, line: String) -> std::fmt::Result {
    use std::fmt::Write as _;
    writeln!(out, "{line}")
}

/// `glasshouse rate-route <session> useful|not-useful [--note TEXT]` — the
/// door for [`glasshouse::evaluation::record_route_rating`], and map line
/// 1846's own design note, *"The routing half of RC-B"* (2026-09-05).
///
/// Modelled on `commands::memory::memory_rate`: the write is the command's
/// whole act, so a failure — including "this session was never routed" —
/// propagates with `?` rather than being swallowed the way a bookkeeping
/// side-effect would be. The destination printed back is read fresh after
/// the write, rather than threaded through the return value, because
/// [`glasshouse::evaluation::record_route_rating`]'s own signature (modelled
/// line for line on [`glasshouse::evaluation::record_memory_rating`]) hands
/// back only the appended `seq`.
pub(crate) fn rate_route(
    runtime: &Runtime,
    session: &str,
    verdict: glasshouse::evaluation::EvaluationOutcome,
    note: Option<&str>,
) -> anyhow::Result<String> {
    use glasshouse::evaluation::{EvaluationObservations, now_unix, record_route_rating};

    let seq = record_route_rating(runtime, session, verdict, note, now_unix())?;

    let destination = EvaluationObservations::open(runtime)?
        .routed_destination(session)?
        .unwrap_or_default();

    Ok(format!(
        "{seq} {session} routed to {destination} rated {}\n",
        verdict.as_str()
    ))
}

/// Map lines 1834, 1835, 1845 and 1854, printed for a person — the consumer
/// half of this package, and the reason its producers are not Cluster B.
///
/// # Why it lives in `glasshouse route` rather than in a command of its own
///
/// `route` is the report about routing: it already prints the ranking, the
/// override, and what the ranking could not see. *How the routes this project
/// took actually turned out* is the same subject and the same reader, and a
/// second command would have meant a new `Command` variant in `cli.rs` —
/// a file two other workers are editing this round (practice §77). Nothing
/// here decides anything; the recommendation above is computed without it.
///
/// # The rules this rendering exists to hold
///
/// Every ratio prints its denominator and no ratio prints a percentage — a
/// bare `60%` cannot be told from a `3 of 5` that is one lucky afternoon.
/// `unknown` is its own bucket in every table, never folded into a
/// neighbour, and a session whose harness never reported a turn end is
/// counted as exactly that: not a failure, and not a success.
///
/// # Practice §65
///
/// The ledger is opened here, in the one function that reads it, and dropped
/// when this returns. `route` is a command a person types; a ledger that
/// cannot be opened costs this section and never the report.
fn route_outcomes_section(runtime: &Runtime) -> String {
    use glasshouse::evaluation::{EvaluationKind, EvaluationObservations};

    let header = format!(
        "Past routes in this project, last {} days\n",
        crate::commands::routing_destinations::ROUTE_OUTCOME_WINDOW_DAYS
    );
    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::debug!(
                error = %format!("{err:#}"),
                "could not open the evaluation ledger for the routing-outcome section"
            );
            return format!("{header}\n  the evaluation ledger could not be opened\n");
        }
    };

    let to = glasshouse::evaluation::now_unix();
    let from = to - crate::commands::routing_destinations::ROUTE_OUTCOME_WINDOW_DAYS * 24 * 60 * 60;
    let by_class = ledger.route_outcomes_by(EvaluationKind::RoutingCostClassObserved, from, to);
    let by_pairing = ledger.route_outcomes_by_pairing_class(from, to);
    // Map line 1845's other five quantities, joined by session id — the
    // register's *"three producers, not a join"* is now a join.
    let pairing_responsiveness = ledger.pairing_class_responsiveness(from, to);
    let by_evidence = ledger.route_outcomes_by(EvaluationKind::RoutingEvidenceObserved, from, to);
    // Map line 1834. The bucket is the tier *with* its escalation, which is
    // what makes "did this tier succeed without escalation" a comparison
    // between two rows of one table rather than a join.
    let by_tier = ledger.route_outcomes_by(EvaluationKind::RoutingTierObserved, from, to);
    // Map line 1851. Counted rather than joined: these rows carry no
    // session, because the gateway that ranks a failover holds no Glasshouse
    // session id — see `EvaluationKind::FailoverPrevented`.
    let preventions = ledger.counts_by_subject(EvaluationKind::FailoverPrevented, from, to);
    // Map line 1837. Counted rather than joined, for the same reason: what
    // this asks is *how often*, over every high-tier launch in the window.
    let reserve_availability =
        ledger.counts_by_subject(EvaluationKind::ReserveAvailabilityObserved, from, to);

    let (
        by_class,
        by_pairing,
        pairing_responsiveness,
        by_evidence,
        by_tier,
        preventions,
        reserve_availability,
    ) = match (
        by_class,
        by_pairing,
        pairing_responsiveness,
        by_evidence,
        by_tier,
        preventions,
        reserve_availability,
    ) {
        (
            Ok(class),
            Ok(pairing),
            Ok(responsiveness),
            Ok(evidence),
            Ok(tier),
            Ok(preventions),
            Ok(reserve_availability),
        ) => (
            class,
            pairing,
            responsiveness,
            evidence,
            tier,
            preventions,
            reserve_availability,
        ),
        (Err(err), ..)
        | (_, Err(err), ..)
        | (_, _, Err(err), ..)
        | (_, _, _, Err(err), ..)
        | (_, _, _, _, Err(err), ..)
        | (_, _, _, _, _, Err(err), _)
        | (_, _, _, _, _, _, Err(err)) => {
            // `WindowNotRetained` is the honest one: retention trimmed rows
            // this window reaches back past, and a smaller number would be a
            // fabrication. It is reported, not rounded away.
            return format!("{header}\n  {err}\n");
        }
    };

    if by_class.is_empty() {
        // Map line 1851 is still printed here. A prevention row carries no
        // session — the gateway that ranks a failover holds no Glasshouse
        // session id — so a project that has failed over without ever
        // launching under a build that attributes routes has a real count and
        // no routed sessions, and returning early would hide it behind an
        // unrelated emptiness.
        return format!(
            "{header}\n  no routed sessions recorded in this window\n{}{}",
            render_failover_preventions(&preventions),
            render_reserve_availability(&reserve_availability)
        );
    }

    let mut out = header;
    out.push_str("\n  by cost class\n");
    out.push_str(&render_route_outcome_rows(&by_class));
    out.push_str(
        "\n  by pairing class (task success, usable tool calls, repair loops, effective TTFC, \
         reliability, user overrides — map line 1845)\n",
    );
    out.push_str(&render_pairing_class_rows(
        &by_pairing,
        &pairing_responsiveness,
    ));
    out.push_str(
        "\n  by evidence held about the destination when it was chosen (`observed` is a row \
         written before staleness was measured, never re-labelled)\n",
    );
    out.push_str(&render_route_outcome_rows(&by_evidence));
    out.push_str(
        "\n  by workload tier the decision used, and whether the conservative rule escalated \
         it (map line 1834)\n",
    );
    out.push_str(&render_route_outcome_rows(&by_tier));
    // Map line 1855's token half: a second ledger opened and dropped here,
    // beside the evaluation ledger above — practice §65's "one open per
    // read" is about not holding two handles on the *same* store, not about
    // reading only one store per function.
    out.push_str(&expected_vs_actual_output_tokens_block(runtime, from, to));
    out.push_str(&render_failover_preventions(&preventions));
    out.push_str(&render_reserve_availability(&reserve_availability));
    out.push_str(&render_pairing_prior_crossover(&ledger, from, to));
    out.push_str(
        "\nA session whose harness never reported a turn end is counted as neither a success \
         nor a failure; a quiet or exited process is never read as either.\n",
    );
    out
}

/// Map line 1846: from what point local pairing evidence predicts a routed
/// session's outcome at least as well as the same-vendor prior did, beside
/// [`render_reserve_availability`]'s own 1837 block and read from the same
/// already-open ledger (practice §65 — one handle, opened once in
/// [`route_outcomes_section`]).
///
/// **This section measures; it decides nothing.** It reads
/// [`glasshouse::evaluation::EvaluationObservations::pairing_prior_crossover`]
/// and renders what it found — nothing here touches
/// `glasshouse::routing::session::PAIRING_PRIOR`, which stands regardless of
/// what this comparison shows.
fn render_pairing_prior_crossover(
    ledger: &glasshouse::evaluation::EvaluationObservations,
    from: i64,
    to: i64,
) -> String {
    use glasshouse::routing::evidence::MIN_SAMPLE_FOR_SUMMARY;

    let header = "\n  local pairing evidence vs the same-vendor prior (1846):\n";
    let crossover = match ledger.pairing_prior_crossover(from, to) {
        Ok(crossover) => crossover,
        Err(err) => return format!("{header}    {err}\n"),
    };

    let mut out = header.to_owned();
    for bucket in &crossover.buckets {
        if bucket.sessions == 0 {
            out.push_str(&format!("    k {}: none\n", bucket.bucket));
        } else {
            out.push_str(&format!(
                "    k {}: prior right {}/{} \u{b7} local right {}/{}\n",
                bucket.bucket,
                bucket.prior_correct,
                bucket.sessions,
                bucket.local_correct,
                bucket.sessions
            ));
        }
    }
    match crossover.crossover {
        Some(bucket) => out.push_str(&format!(
            "    local evidence at least as predictive from bucket {bucket}\n"
        )),
        None => out.push_str(&format!(
            "    not yet: no bucket with at least {MIN_SAMPLE_FOR_SUMMARY} sessions where local \
             evidence matches the prior\n"
        )),
    }
    out
}

/// Map line 1855's token half, rendered for [`route_outcomes_section`]: the
/// median of *actual ÷ estimated* output tokens per task class, over the
/// same `[from, to]` window that section's other blocks read.
fn expected_vs_actual_output_tokens_block(runtime: &Runtime, from: i64, to: i64) -> String {
    use std::fmt::Write as _;

    let header = "\n  expected vs actual output tokens (1855):\n";
    let ledger = match glasshouse::routing::evidence::EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::debug!(
                error = %format!("{err:#}"),
                "could not open the routing evidence ledger for the output-token estimate block"
            );
            return format!("{header}    the routing evidence ledger could not be opened\n");
        }
    };
    let rows = match ledger.output_estimate_accuracy(to, (to - from).max(0)) {
        Ok(rows) => rows,
        Err(err) => return format!("{header}    {err}\n"),
    };
    if rows.is_empty() {
        return format!("{header}    no output-token estimates recorded in this window\n");
    }
    let mut out = header.to_owned();
    for row in &rows {
        match row.median_ratio {
            Some(ratio) => {
                let _ = writeln!(
                    out,
                    "    {}: median actual/estimated ×{ratio:.2} over {} sessions ({} pending)",
                    row.task_class, row.sample_count, row.pending
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "    {}: not enough sessions to score ({} pending)",
                    row.task_class, row.pending
                );
            }
        }
    }
    out
}

/// Map line 1480, printed for a person: whether each workload tier has
/// enough evidence to say how its routed sessions turned out, and what that
/// evidence says when it does.
///
/// A section of its own, beside [`route_correlations_section`] and
/// [`throttle_scope_section`] — not a change to [`route_outcomes_section`]'s
/// own "by workload tier" table, which map line 1834 already closed and
/// whose regression asserts raw, un-gated counts. Line 1834 asks what was
/// recorded; line 1480 asks whether enough of it exists to summarize, which
/// is [`glasshouse::evaluation::EvaluationObservations::outcomes_by_tier`]'s
/// own [`glasshouse::evaluation::TierOutcomeVerdict`] gate.
///
/// Same window as [`route_outcomes_section`] and the same practice §65
/// reasoning: the ledger is opened here, in the one function that reads it
/// for this section, and dropped when this returns.
fn tier_outcome_section(runtime: &Runtime) -> String {
    use glasshouse::evaluation::{EvaluationObservations, TierOutcomeVerdict};

    let header =
        "\nWorkload-tier outcomes in this project, last 30 days (map line 1480)\n".to_owned();
    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::debug!(
                error = %format!("{err:#}"),
                "could not open the evaluation ledger for the tier-outcome section"
            );
            return format!("{header}\n  the evaluation ledger could not be opened\n");
        }
    };
    let to = glasshouse::evaluation::now_unix();
    let from = to - crate::commands::routing_destinations::ROUTE_OUTCOME_WINDOW_DAYS * 24 * 60 * 60;
    let outcomes = match ledger.outcomes_by_tier(from, to) {
        Ok(outcomes) => outcomes,
        Err(err) => return format!("{header}\n  {err}\n"),
    };

    let mut out = header;
    out.push('\n');
    if outcomes.is_empty() {
        out.push_str("  no routed sessions recorded in this window\n");
        return out;
    }
    for outcome in outcomes.iter() {
        match outcome.verdict {
            TierOutcomeVerdict::InsufficientEvidence {
                sample_size,
                required,
            } => {
                let _ = writeln_str(
                    &mut out,
                    format!(
                        "  {}: insufficient evidence — {sample_size} of the {required} reported \
                         turns a tier summary needs; treated as no summary",
                        outcome.bucket
                    ),
                );
            }
            TierOutcomeVerdict::Measured {
                successful,
                failed,
                sample_size,
            } => {
                let _ = writeln_str(
                    &mut out,
                    format!(
                        "  {}: {successful} of {sample_size} reported turns succeeded, {failed} \
                         failed",
                        outcome.bucket
                    ),
                );
            }
        }
        if outcome.undecided > 0 {
            let _ = writeln_str(
                &mut out,
                format!(
                    "    {} session(s) with no turn end reported yet — undecided, never a \
                     failure",
                    outcome.undecided
                ),
            );
        }
    }
    out
}

/// Map line 1481, printed for a person: where a tier's observed outcomes
/// disagree with what a configured capability record says about a model at
/// that tier — a suggestion, never a rewrite.
///
/// **The evidence gate is [`glasshouse::evaluation::TierOutcomeVerdict::Measured`]
/// itself** — the same [`glasshouse::routing::evidence::MIN_SAMPLE_FOR_SUMMARY`]
/// floor [`tier_outcome_section`] already gates its own summary on, reused
/// rather than a second threshold invented for this section: a tier with too
/// few reported turns to summarize has too few to suggest a calibration
/// change from either. `TierOutcomeVerdict::InsufficientEvidence` — which
/// includes the empty-window and zero-sample case — is skipped outright.
///
/// **Read-only by construction.** This reads
/// [`glasshouse::config::EffectiveConfig::calibrated_model_ceilings`] and the
/// evaluation ledger and writes a string; nothing here holds a `&mut`
/// `ProviderConfig`, opens a config file for writing, or calls any `set_*`
/// method. The rendered line names the config key a person would edit
/// themselves — the suggestion stops at naming it.
fn capability_suggestions_section(runtime: &Runtime, effective: &EffectiveConfig<'_>) -> String {
    use glasshouse::config::ConfiguredWorkloadTier;
    use glasshouse::config::capability::CeilingResolution;
    use glasshouse::evaluation::{EvaluationObservations, TierOutcomeVerdict};

    let header = "\nCalibration suggestions from observed outcomes, last 30 days (map line \
                   1481)\n"
        .to_owned();
    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::debug!(
                error = %format!("{err:#}"),
                "could not open the evaluation ledger for the calibration-suggestions section"
            );
            return format!("{header}\n  the evaluation ledger could not be opened\n");
        }
    };
    let to = glasshouse::evaluation::now_unix();
    let from = to - crate::commands::routing_destinations::ROUTE_OUTCOME_WINDOW_DAYS * 24 * 60 * 60;
    let outcomes = match ledger.outcomes_by_tier(from, to) {
        Ok(outcomes) => outcomes,
        Err(err) => return format!("{header}\n  {err}\n"),
    };

    let calibrated = effective.calibrated_model_ceilings();
    let mut out = header;
    out.push('\n');
    let mut suggested = false;
    for outcome in &outcomes {
        // The gate: below MIN_SAMPLE_FOR_SUMMARY, `outcomes_by_tier` itself
        // has already declined to summarize this tier, and a suggestion
        // built from fewer reported turns than the section beside it trusts
        // for a summary would be a second, weaker threshold nobody asked for.
        let TierOutcomeVerdict::Measured {
            successful,
            failed,
            sample_size,
        } = outcome.verdict
        else {
            continue;
        };
        // The bucket is a display word (`RoutingTier::as_str`'s vocabulary,
        // e.g. `standard-escalated`); the escalation suffix is stripped
        // before parsing because a model's configured ceiling names the
        // tier itself, never whether a session was escalated into it.
        let Some(bucket_tier) =
            ConfiguredWorkloadTier::parse(outcome.bucket.trim_end_matches("-escalated"))
                .map(ConfiguredWorkloadTier::tier)
        else {
            continue;
        };
        for (provider, model, resolution) in &calibrated {
            let (configured_tier, provenance, config_key) = match resolution {
                CeilingResolution::UserCapabilityRecord(tier) => (
                    *tier,
                    "the user's own capability assignment",
                    format!("providers.{provider}.model_capabilities.{model}.ceiling"),
                ),
                CeilingResolution::Prior(Some(tier)) => (
                    *tier,
                    "a benchmark-derived prior",
                    format!("providers.{provider}.model_capabilities.{model}.ceiling"),
                ),
                _ => continue,
            };
            if configured_tier != bucket_tier {
                continue;
            }
            // The disagreement this section names: a majority of reported
            // turns at the model's own configured tier failed. Never acted
            // on — only rendered, with the exact key a person would edit.
            if failed > successful {
                suggested = true;
                let _ = writeln_str(
                    &mut out,
                    format!(
                        "  {provider}/{model} is configured at `{configured_tier}` by \
                         {provenance}, but {failed} of {sample_size} reported turns at this \
                         tier failed — consider lowering `{config_key}` (a suggestion; nothing \
                         was changed)"
                    ),
                );
            }
        }
    }
    if !suggested {
        out.push_str(
            "  no disagreement between observed outcomes and configured calibration met the \
             evidence gate\n",
        );
    }
    out
}

/// How many of [`support_work_section`]'s rows to print — the packet's own
/// "most recent N (say 10)".
const SUPPORT_WORK_RECENT_LIMIT: usize = 10;

/// Map line 1951, printed for a person: per-harness task efficiency —
/// tokens, wall-clock, request count, and outcome by task class — so that
/// harness choice can rest on evidence rather than on which vendor bills
/// for it.
///
/// Two ledgers, joined in Rust rather than in SQL, because they hold
/// different rows for different reasons. The outcome-and-task-class half
/// comes from
/// [`glasshouse::evaluation::EvaluationObservations::outcomes_by_tier_and_harness`]
/// — `evaluation_observations` joined to `sessions.harness`, exactly
/// [`tier_outcome_section`]'s own join with a harness dimension added, one
/// row per (harness, task class). The token/wall-clock/request-count half
/// comes from
/// [`glasshouse::routing::evidence::EvidenceLedger::request_stats_by_harness`]
/// — `routing_observations.harness`, written directly at record time — and
/// is computed once per harness rather than per task class, because
/// `routing_observations` carries no tier at all; the same per-harness
/// figures are printed on every task-class row for that harness. That is
/// the box line's own split: *"tokens, wall-clock, request count"*
/// unqualified, *"outcome … by task class"* qualified.
///
/// A harness with no `routing_observations` rows in the window (every
/// session routed but nothing dispatched yet) prints `0 request(s)` and
/// *"not exposed on 0 of 0 exchanges"* rather than being silently dropped
/// from the token/wall-clock figures.
fn harness_efficiency_section(runtime: &Runtime) -> String {
    use glasshouse::evaluation::{EvaluationObservations, TierOutcomeVerdict};
    use glasshouse::routing::evidence::EvidenceLedger;

    let header =
        "\nPer-harness task efficiency in this project, last 30 days (map line 1951)\n".to_owned();

    let to = glasshouse::evaluation::now_unix();
    let from = to - crate::commands::routing_destinations::ROUTE_OUTCOME_WINDOW_DAYS * 24 * 60 * 60;

    let outcomes = match EvaluationObservations::open(runtime) {
        Ok(ledger) => match ledger.outcomes_by_tier_and_harness(from, to) {
            Ok(outcomes) => outcomes,
            Err(err) => return format!("{header}\n  {err}\n"),
        },
        Err(err) => {
            tracing::debug!(
                error = %format!("{err:#}"),
                "could not open the evaluation ledger for the harness-efficiency section"
            );
            return format!("{header}\n  the evaluation ledger could not be opened\n");
        }
    };

    let stats = match EvidenceLedger::open(runtime) {
        Ok(ledger) => match ledger.request_stats_by_harness(from, to) {
            Ok(stats) => stats,
            Err(err) => return format!("{header}\n  {err}\n"),
        },
        Err(err) => {
            tracing::debug!(
                error = %err,
                "could not open the routing evidence ledger for the harness-efficiency section"
            );
            return format!("{header}\n  the routing evidence ledger could not be opened\n");
        }
    };

    let mut out = header;
    out.push('\n');
    if outcomes.is_empty() {
        out.push_str("  no routed sessions recorded in this window\n");
        return out;
    }

    for row in outcomes.iter() {
        let request_stats = stats.iter().find(|s| s.harness == row.harness);

        let outcome_clause = match row.outcome.verdict {
            TierOutcomeVerdict::InsufficientEvidence {
                sample_size,
                required,
            } => format!(
                "insufficient evidence — {sample_size} of the {required} reported turns a tier \
                 summary needs; treated as no summary"
            ),
            TierOutcomeVerdict::Measured {
                successful,
                failed,
                sample_size,
            } => {
                format!("{successful} of {sample_size} reported turns succeeded, {failed} failed")
            }
        };
        let undecided_clause = if row.outcome.undecided > 0 {
            format!("; {} undecided", row.outcome.undecided)
        } else {
            String::new()
        };

        let requests = request_stats.map_or(0, |stats| stats.requests);
        let wall_clock_clause = match request_stats.and_then(|stats| stats.wall_clock) {
            Some(wall_clock) => format!(
                ", median wall-clock {}ms across {} timed exchange(s)",
                wall_clock.median_ms, wall_clock.sample_count
            ),
            None => String::new(),
        };
        // Below, `token_rows_present == 0` must render as an absence, never
        // as a printed `0` — the mutation this line is written to catch is
        // exactly `input_tokens_sum`/`output_tokens_sum` printed unguarded
        // when every row in the group left both `NULL`.
        let tokens_clause = match request_stats {
            None => "tokens: not exposed on 0 of 0 exchanges".to_owned(),
            Some(stats) if stats.token_rows_present == 0 => format!(
                "tokens: not exposed on {} of {} exchanges",
                stats.requests, stats.requests
            ),
            Some(stats) if stats.token_rows_present == stats.requests => format!(
                "tokens: {} in / {} out",
                stats.input_tokens_sum, stats.output_tokens_sum
            ),
            Some(stats) => format!(
                "tokens: {} in / {} out on {} of {} exchanges; not exposed on {}",
                stats.input_tokens_sum,
                stats.output_tokens_sum,
                stats.token_rows_present,
                stats.requests,
                stats.requests - stats.token_rows_present
            ),
        };

        let _ = writeln_str(
            &mut out,
            format!(
                "  {} — {}: {outcome_clause}{undecided_clause}; {requests} request(s)\
                 {wall_clock_clause}; {tokens_clause}",
                row.harness, row.outcome.bucket
            ),
        );
    }
    out
}

/// Map line 1629, printed for a person: which resource performed important
/// support work — classification or memory extraction — most recently, so
/// a person debugging *"which model classified this task?"* finds the
/// answer here rather than in the raw ledger.
///
/// Reads
/// [`glasshouse::routing::evidence::EvidenceLedger::recent_support_work`],
/// the purpose-filtered sibling of `Self::recent` that this line needs:
/// `recent` requires the caller to already name one `(provider, model,
/// route, harness)` identity, and the whole point of this section is that
/// the identity is unknown in advance — it is the question a person is
/// asking.
fn support_work_section(runtime: &Runtime) -> String {
    use glasshouse::routing::evidence::EvidenceLedger;

    let header = "\nRecent support work in this project (map line 1629) — which resource \
                   classified or extracted, for debugging\n"
        .to_owned();
    let ledger = match EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::debug!(
                error = %err,
                "could not open the routing evidence ledger for the support-work section"
            );
            return format!("{header}\n  the routing evidence ledger could not be opened\n");
        }
    };
    let recent = match ledger.recent_support_work(SUPPORT_WORK_RECENT_LIMIT) {
        Ok(rows) => rows,
        Err(err) => return format!("{header}\n  {err}\n"),
    };

    let mut out = header;
    out.push('\n');
    if recent.is_empty() {
        out.push_str("  no classification or memory-extraction call recorded yet\n");
        return out;
    }
    for observation in recent.iter() {
        let purpose = observation
            .purpose
            .as_deref()
            .unwrap_or("(no purpose recorded)");
        let route = observation
            .route
            .as_deref()
            .unwrap_or("(no route recorded)");
        let outcome = observation
            .outcome
            .map(glasshouse::routing::evidence::Outcome::as_str)
            .unwrap_or("unknown");
        let wall_clock = observation
            .duration_ms()
            .map(|ms| format!("{ms}ms"))
            .unwrap_or_else(|| "(not timed)".to_owned());
        let _ = writeln_str(
            &mut out,
            format!(
                "  at {}: {purpose} by {} / {} via {route} — {outcome}, {wall_clock}",
                observation.observed_at_unix, observation.provider, observation.model
            ),
        );
    }
    out
}

/// Map line 1851, as one sentence with its denominator — *"3 of 7 failovers
/// were steered off a shared upstream"*.
///
/// # Why this is a count and not a table
///
/// A prevention row carries no session and no turn outcome — the gateway
/// that ranks a failover holds no Glasshouse session id — so there is
/// nothing to bucket a success rate by. What line 1851 asks is *how often*,
/// and the honest shape of that is a numerator over the total of every
/// bucket the ledger holds, printed together.
///
/// A window with no failover prints so, and prints nothing that could be
/// read as a rate: `0 of 0` is not `0%`, and an absent measurement is never
/// rendered as a measured zero.
fn render_failover_preventions(counts: &[(String, i64)]) -> String {
    use glasshouse::evaluation::FailoverPrevention;

    let total: i64 = counts.iter().map(|(_, count)| *count).sum();
    let prevented = counts
        .iter()
        .find(|(subject, _)| subject == FailoverPrevention::Prevented.as_str())
        .map(|(_, count)| *count)
        .unwrap_or_default();
    if total == 0 {
        return "\n  no gateway failover was ranked in this window, so nothing is recorded \
                about what failure-domain evidence did to one (map line 1851)\n"
            .to_owned();
    }
    format!(
        "\n  {prevented} of {total} gateway failovers went somewhere the failure-domain term \
         changed the winner to — that many were steered off a candidate sharing the failed \
         backend's provider (map line 1851)\n"
    )
}

/// Map line 1837: how often protected quota remained available for a
/// high-tier task at the moment it was routed.
///
/// `counts` is [`EvaluationKind::ReserveAvailabilityObserved`]'s subjects —
/// [`glasshouse::provider::quota::CapacityBand`] words, or `"unknown"` for a
/// destination with no reading. *Available* sums every band above
/// [`glasshouse::provider::quota::CapacityBand::Reserve`]
/// (`Tight`, `Healthy`, `Plenty`); below
/// [`glasshouse::routing::evidence::MIN_SAMPLE_FOR_SUMMARY`] total rows,
/// the sentence honestly says there is not enough to summarise rather than
/// printing a ratio nobody could act on.
fn render_reserve_availability(counts: &[(String, i64)]) -> String {
    use glasshouse::provider::quota::CapacityBand;
    use glasshouse::routing::evidence::MIN_SAMPLE_FOR_SUMMARY;

    let count_of = |band: &str| -> i64 {
        counts
            .iter()
            .find(|(subject, _)| subject == band)
            .map(|(_, count)| *count)
            .unwrap_or_default()
    };
    let total: i64 = counts.iter().map(|(_, count)| *count).sum();
    if total < MIN_SAMPLE_FOR_SUMMARY as i64 {
        return format!(
            "\n  protected quota for high-tier tasks (1837): not enough high-tier launches \
             ({total})\n"
        );
    }

    let available = [
        CapacityBand::Tight,
        CapacityBand::Healthy,
        CapacityBand::Plenty,
    ]
    .iter()
    .map(|band| count_of(band.as_str()))
    .sum::<i64>();
    let at_reserve = count_of(CapacityBand::Reserve.as_str());
    let exhausted = count_of(CapacityBand::Exhausted.as_str());
    let unknown = count_of("unknown");

    format!(
        "\n  protected quota for high-tier tasks (1837): available {available} · at reserve \
         {at_reserve} · exhausted {exhausted} · unknown {unknown} of {total} high-tier \
         launches\n"
    )
}

/// One table of [`RouteOutcomeCounts`], aligned on the bucket name.
fn render_route_outcome_rows(counts: &[glasshouse::evaluation::RouteOutcomeCounts]) -> String {
    if counts.is_empty() {
        return "    (nothing recorded)\n".to_owned();
    }
    let width = counts
        .iter()
        .map(|row| row.bucket.chars().count())
        .max()
        .unwrap_or_default();
    counts
        .iter()
        .map(|row| {
            format!(
                "    {:width$} : {}\n",
                row.bucket,
                render_route_outcome_line(row)
            )
        })
        .collect()
}

/// One bucket's counts, as a sentence with both its denominators in it.
///
/// **Never a percentage, and never a completion count without the number of
/// turns it is out of.** The two denominators are different quantities —
/// turns a harness reported on, and sessions that were routed — and a
/// rendering that dropped either would let a reader divide the wrong pair.
///
/// **Rated and proxy counts are never summed** (map line 1846's design note,
/// *"The routing half of RC-B"*, 2026-09-05): a bucket with no rated session
/// prints exactly what it always has, byte-identical, and only a bucket
/// carrying a rated session gains a trailing clause naming the rated counts
/// apart from the proxy figures above.
fn render_route_outcome_line(counts: &glasshouse::evaluation::RouteOutcomeCounts) -> String {
    let reported = counts.reported_turns();
    let verdicts = if reported == 0 {
        "no reported turns".to_owned()
    } else {
        format!(
            "{} of {reported} reported turns completed",
            counts.completed
        )
    };
    let sessions = format!(
        "{} session{} routed",
        counts.sessions,
        if counts.sessions == 1 { "" } else { "s" }
    );
    let mut line = if counts.sessions_without_outcome > 0 {
        format!(
            "{verdicts}; {sessions}, {} with no turn end reported",
            counts.sessions_without_outcome
        )
    } else {
        format!("{verdicts}; {sessions}")
    };
    if counts.rated_useful > 0 || counts.rated_not_useful > 0 {
        line.push_str(&format!(
            " · rated {} useful / {} not-useful",
            counts.rated_useful, counts.rated_not_useful
        ));
    }
    line
}

/// Map line 1845, the whole line: task success (from `by_pairing`, the same
/// [`glasshouse::evaluation::RouteOutcomeCounts`] every other bucket table on
/// this section renders) beside the five quantities the session-id join
/// (`responsiveness`) supplies — usable tool calls, repair loops, effective
/// TTFC, reliability, user overrides. Each figure prints *not enough* below
/// [`glasshouse::routing::evidence::MIN_SAMPLE_FOR_SUMMARY`] with its own
/// sample, per the ruling; a bucket `by_pairing` named that the
/// responsiveness join never reached prints *not enough* on all five rather
/// than a fabricated number.
fn render_pairing_class_rows(
    by_pairing: &[glasshouse::evaluation::RouteOutcomeCounts],
    responsiveness: &[glasshouse::evaluation::PairingClassResponsiveness],
) -> String {
    if by_pairing.is_empty() {
        return "    (nothing recorded)\n".to_owned();
    }
    let mut out = String::new();
    for counts in by_pairing {
        out.push_str(&format!(
            "    {}\n      task success            : {}\n",
            counts.bucket,
            render_route_outcome_line(counts)
        ));
        let five = responsiveness
            .iter()
            .find(|row| row.bucket == counts.bucket);
        out.push_str(&format!(
            "      usable tool calls       : {}\n",
            render_share(
                five.and_then(|row| row.usable_tool_calls),
                five.map_or(0, |row| row.usable_tool_calls_sample),
            )
        ));
        out.push_str(&format!(
            "      repair loops            : {}\n",
            render_mean(
                five.and_then(|row| row.repair_loops),
                five.map_or(0, |row| row.repair_loops_sample),
            )
        ));
        out.push_str(&format!(
            "      effective TTFC          : {}\n",
            match five.and_then(|row| row.effective_ttfc_ms) {
                Some(ms) => format!(
                    "{}ms (mean, {} rows)",
                    ms.round() as i64,
                    five.map_or(0, |row| row.effective_ttfc_sample)
                ),
                None => "not enough evidence".to_owned(),
            }
        ));
        out.push_str(&format!(
            "      reliability             : {}\n",
            render_share(
                five.and_then(|row| row.reliability),
                five.map_or(0, |row| row.reliability_sample),
            )
        ));
        out.push_str(&format!(
            "      user overrides          : {}\n",
            render_share(
                five.and_then(|row| row.user_overrides),
                five.map_or(0, |row| row.user_overrides_sample as usize),
            )
        ));
    }
    out
}

/// A fraction printed as a percentage with its sample, or *not enough
/// evidence* below [`glasshouse::routing::evidence::MIN_SAMPLE_FOR_SUMMARY`]
/// — [`render_pairing_class_rows`]'s shared renderer for `usable tool
/// calls`, `reliability` and `user overrides`, all three of which are the
/// same shape: a share of a sample, never a raw count alone.
fn render_share(fraction: Option<f64>, sample: usize) -> String {
    match fraction {
        Some(fraction) => format!("{:.1}% (over {sample} rows)", fraction * 100.0),
        None => "not enough evidence".to_owned(),
    }
}

/// A plain mean with its sample — [`render_share`]'s sibling for `repair
/// loops`, which is a count per row rather than a fraction.
fn render_mean(mean: Option<f64>, sample: usize) -> String {
    match mean {
        Some(mean) => format!("{mean:.2} (over {sample} rows)"),
        None => "not enough evidence".to_owned(),
    }
}

/// The three spellings `glasshouse route --moment` accepts and the control
/// door's `recommend_route` answers in, written down exactly once.
///
/// These are **not** `RoutingMoment::as_str`, which is prose for a person
/// (`"session start"`, with a space) and is what a rendered report prints.
/// A wire vocabulary a caller sends and gets back has to round-trip, and a
/// table read in both directions is how the sending spelling and the
/// answering spelling are kept from drifting apart.
const ROUTING_MOMENTS: [(&str, glasshouse::routing::session::RoutingMoment); 3] = [
    (
        "session-start",
        glasshouse::routing::session::RoutingMoment::SessionStart,
    ),
    (
        "task-boundary",
        glasshouse::routing::session::RoutingMoment::TaskBoundary,
    ),
    (
        "mid-turn",
        glasshouse::routing::session::RoutingMoment::MidTurn,
    ),
];

/// [`ROUTING_MOMENTS`], read as a parser.
///
/// Answers an `Option` rather than an error because the two callers must
/// phrase the refusal differently: the command echoes back what the person
/// typed at their own terminal, and the door — where the string arrived over
/// a socket — names the three valid spellings without repeating the one it
/// was handed.
pub(crate) fn routing_moment_from_str(
    moment: &str,
) -> Option<glasshouse::routing::session::RoutingMoment> {
    ROUTING_MOMENTS
        .iter()
        .find(|(spelling, _)| *spelling == moment)
        .map(|(_, moment)| *moment)
}

/// [`ROUTING_MOMENTS`], read the other way: the spelling a caller may send
/// back, for the control door's answer.
///
/// Not gated, since Phase 43. It used to be `#[cfg(unix)]` to match its
/// only consumer, `api::unix`, which was itself gated; the handlers in that
/// module now compile on every platform because the MCP door reaches them
/// over stdio, so this is live everywhere too — and a gate left here would
/// be a Windows build error, which the cross-check caught.
pub(crate) fn routing_moment_slug(
    moment: glasshouse::routing::session::RoutingMoment,
) -> &'static str {
    ROUTING_MOMENTS
        .iter()
        .find(|(_, candidate)| *candidate == moment)
        .map(|(spelling, _)| *spelling)
        // Unreachable while the table covers the enum, and an honest fallback
        // rather than a panic if a variant is ever added without it.
        .unwrap_or_else(|| moment.as_str())
}

/// What a routing question was answered with, before anything renders it —
/// the structured half of [`route_report`], and the whole of what the
/// control door's `recommend_route` reports (map line 1681).
///
/// This exists so there is exactly **one** ranking. `memory_search_grouped`
/// and `render_memory_report` already have this shape for the memory door:
/// one function computes, another renders, and the door reads the computed
/// form rather than parsing the rendered one. A door that asked the router
/// its own question would be a second implementation of the same policy, and
/// the two could disagree about where work should go without anything
/// failing.
pub(crate) enum RouteRecommendation {
    /// `SessionRouter::choose` answered. Boxed because the ranking carries
    /// every candidate it weighed and the other variant carries a word;
    /// `clippy::large_enum_variant` is right that the difference should not
    /// be paid by every value of this type.
    Ranked(Box<RankedRoute>),
    /// It did not, and [`NoRoute`] says which of its two situations applies.
    Nowhere(NoRoute),
}

/// A routing decision together with the two things [`routing_caveats`] needs
/// in order to say what the ranking could not see.
pub(crate) struct RankedRoute {
    pub(crate) routed: glasshouse::routing::session::Routed,
    /// Every candidate the router was offered, kept because a caveat is
    /// about the candidate set rather than about the winner.
    pub(crate) destinations: Vec<glasshouse::routing::session::Destination>,
    /// The `Destination::id` of every fresh candidate `glasshouse launch`
    /// would itself refuse — see [`launch_can_resolve_protocol`].
    pub(crate) refused_by_launch: Vec<String>,
    /// Every resource `observed_provider_health` could attribute a persisted
    /// reading to. Kept rather than recomputed because a caveat about what
    /// the ranking could not see has to be answered from the pool the ranking
    /// was actually given.
    pub(crate) health_observed: Vec<String>,
}

/// Why there is no recommendation.
///
/// `SessionRouter::choose` answers `None` in exactly two situations, and they
/// are different facts about this project rather than one error — which is
/// why this is an enum a caller can match on rather than a sentence it would
/// have to parse.
pub(crate) enum NoRoute {
    /// No session to continue, and no launch profile to start one under.
    NoDestination,
    /// The moment does not take routing (line 1592), and there is no session
    /// for the work to stay on either.
    MomentDoesNotRoute(glasshouse::routing::session::RoutingMoment),
}

/// `glasshouse route`'s decision, and the control door's — map lines 1601,
/// 1602 and 1681.
///
/// **Decides nothing and starts nothing.** It assembles exactly the inputs
/// `launch_session` assembles, asks the same `SessionRouter` the same
/// question, and hands back what it answered. That is what makes it a
/// diagnostic worth reading rather than a second implementation that could
/// drift: if this and a launch ever disagreed, one of them would be lying,
/// and there is one function each of them calls.
///
/// Nothing on this path writes. It opens the project's session store and
/// checkpoint store to *read* candidates (`routing_destinations`,
/// `latest_checkpoint_quality`), and records no session, no event, and no
/// routing observation — which is the whole of line 1681's *"without
/// executing it"*, and is asserted over the shipped binary in
/// `tests/routing_api.rs`.
///
/// One qualification since Phase 34D: with a routing model **configured** and
/// a `--task` stated, this asks that model exactly as a launch would — so the
/// two cannot disagree about the classification — and the *cost* of that one
/// call is recorded under `purpose = "classification"`, as every routing-model
/// call is. That is a fact about what the diagnostic spent, not about the
/// work, which is still not executed. It never writes the sticky
/// classification record a launch leaves behind (`remember_classification`),
/// because a preview is not a decision.
///
/// Two differences from the launch path, both stated in the rendered output
/// rather than hidden:
///
/// 1. it ranks across **every enabled harness**, because a caller asking
///    where work should go has not yet chosen one, whereas a launch has;
/// 2. it includes sessions that are still **running** (`DestinationScope`),
///    because "switch to that terminal" is an answer a person can act on and
///    is not one a second process can carry out.
pub(crate) fn route_recommendation(
    runtime: &Runtime,
    effective: &EffectiveConfig<'_>,
    moment: glasshouse::routing::session::RoutingMoment,
    to: Option<&str>,
    fresh: bool,
    now: bool,
    task: Option<&str>,
) -> anyhow::Result<RouteRecommendation> {
    use glasshouse::integrations::{IntegrationId, IntegrationKind};
    use glasshouse::routing::session::{RouterInputs, RoutingMoment};

    let mut destinations = Vec::new();
    // Which of them `glasshouse launch` would refuse, so the report can say so
    // about the one it recommends — see `launch_can_resolve_protocol`.
    let mut refused_by_launch: Vec<String> = Vec::new();
    for harness in IntegrationId::ALL
        .iter()
        .copied()
        .filter(|id| id.kind() == IntegrationKind::Harness)
        .filter(|id| effective.enabled(*id, false).value)
    {
        let everything = crate::commands::routing_destinations::routing_destinations(
            runtime,
            effective,
            harness,
            crate::commands::routing_destinations::DestinationScope::Everything,
            task,
        )?;
        refused_by_launch.extend(
            everything
                .iter()
                .filter(|destination| destination.is_fresh())
                .filter(|destination| {
                    effective
                        .launch_profile(destination.launch_profile(), harness)
                        .is_ok_and(|profile| !launch_can_resolve_protocol(&profile.value))
                })
                .map(|destination| destination.id().to_owned()),
        );
        destinations.extend(everything);
    }

    let overrides = effective.pairing_overrides();
    // Line 1599's bridge, on the path that *reports*. The live pool still
    // belongs to a running gateway's session lock and this diagnostic has no
    // gateway — but `glasshouse launch` weighs what a previous gateway
    // persisted, so a report that skipped it would explain a different
    // ranking from the one the acting path produces, which is the one defect
    // a routing explanation cannot have. `routing_caveats` below says which
    // of the two happened rather than asserting the empty case.
    let health = crate::commands::routing_destinations::observed_provider_health(
        runtime,
        effective,
        &destinations,
    );
    // Phase 34D on the path that reports: the same classifier the launch
    // path calls, over the same destinations, so the explanation printed
    // here is the one a launch would act on. No sticky record is consulted
    // — this path ranks across every harness and decides nothing, so the
    // one difference from a launch is that it always asks rather than
    // reusing, and `classify_for_routing`'s doc says so.
    let classified = crate::commands::routing_classification::classify_for_routing(
        runtime,
        effective,
        crate::commands::routing_classification::RoutingClassificationSite {
            task,
            moment,
            harness: None,
            harness_named: false,
            to,
            fresh,
            destinations: &destinations,
            health: health.pool(),
            sticky: None,
            text_cache: None,
            // Line 1419: this report has not chosen a launch profile — it
            // ranks across every harness — so there is no protected capacity
            // to name.
            protected_capacity_price: None,
        },
    );
    let inputs = RouterInputs {
        overrides: &overrides,
        health: health.pool(),
        now: std::time::Instant::now(),
        requirements: classified
            .as_ref()
            .map(|classified| classified.answer.requirements())
            .unwrap_or_default(),
    };

    let mut user_override = crate::commands::routing_destinations::routing_override(to, fresh);
    if now {
        user_override = user_override.and_route_now();
    }

    // `current` is what the work is on, and the moment decides whether there
    // is one. `RoutingMoment::SessionStart`'s own doc is explicit — *"no
    // session exists yet for this work"* — so `None` there is the type's
    // answer, not a shortcut. At a task boundary and mid-turn the work is
    // somewhere, and the most recently active session is the honest reading
    // of where.
    //
    // Load-bearing for line 1597: `prompt_cache_state` is defined as a
    // comparison `CacheLocality::between(from, to)`, so a caller that never
    // supplies a `from` has an inert term and an explanation that cannot say
    // why. This is the `from`.
    let current = match moment {
        RoutingMoment::SessionStart => None,
        RoutingMoment::TaskBoundary | RoutingMoment::MidTurn => destinations
            .iter()
            .find(|destination| !destination.is_fresh())
            .cloned(),
    };

    // Line 1564's caller: at a task boundary the work is somewhere, and how
    // its last exchange there ended is a fact the ledger holds. Read only
    // when there is a current destination — a session start has no last
    // attempt to promote after — and handed to the router, which promotes
    // on a model-capability class and names any other.
    let retry_after = current
        .as_ref()
        .and_then(|current| latest_failure_class(runtime, current));
    let Some(routed) =
        crate::commands::routing_destinations::session_router(runtime, effective, user_override)
            .with_retry_after(retry_after)
            .choose(moment, current.as_ref(), &destinations, &inputs)
    else {
        // `choose` answers `None` in exactly two situations, and they are
        // different facts about this project rather than one error.
        return Ok(RouteRecommendation::Nowhere(if moment.permits_routing() {
            NoRoute::NoDestination
        } else {
            NoRoute::MomentDoesNotRoute(moment)
        }));
    };

    Ok(RouteRecommendation::Ranked(Box::new(RankedRoute {
        routed,
        destinations,
        refused_by_launch,
        health_observed: health
            .pool()
            .observed()
            .into_iter()
            .map(|(resource, _)| resource.label())
            .collect(),
    })))
}

/// [`RouteRecommendation`] as `glasshouse route` prints it — the rendering
/// half, layered on top of the decision rather than computed beside it.
pub(crate) fn render_route_recommendation(recommendation: &RouteRecommendation) -> String {
    match recommendation {
        RouteRecommendation::Nowhere(NoRoute::NoDestination) => {
            "There is nowhere for this work to go: this project has no session to continue \
             and no launch profile to start one under. `glasshouse doctor` reports which \
             harnesses are installed.\n"
                .to_owned()
        }
        RouteRecommendation::Nowhere(NoRoute::MomentDoesNotRoute(moment)) => format!(
            "Nothing is routed at a {moment} moment (line 1592), and this project has no \
             session for the work to stay on either. Ask at a session start or a task \
             boundary, or pass --now to decide here anyway.\n"
        ),
        RouteRecommendation::Ranked(ranked) => {
            let mut out = ranked.routed.render_overview();
            out.push('\n');
            out.push_str(&routing_caveats(
                &ranked.routed,
                &ranked.destinations,
                &ranked.refused_by_launch,
                &ranked.health_observed,
            ));
            out
        }
    }
}

/// What the ranking above could not see, said out loud.
///
/// A routing explanation whose silent terms are invisible is worse than one
/// that is short: a reader cannot tell "provider health was equal" from
/// "provider health was never read". Every line here names an input that
/// contributed nothing and why.
pub(crate) fn routing_caveats(
    routed: &glasshouse::routing::session::Routed,
    destinations: &[glasshouse::routing::session::Destination],
    refused_by_launch: &[String],
    health_observed: &[String],
) -> String {
    use glasshouse::routing::session::Continuation;
    use std::fmt::Write as _;

    let mut out = String::from("what this ranking could not see\n");
    if health_observed.is_empty() {
        let _ = writeln!(
            out,
            "  provider health   nothing observed — no gateway has yet persisted a health \
             reading for any of these credentials, so the term is 0.0 for every destination"
        );
    }
    if destinations
        .iter()
        .all(|destination| destination.is_fresh())
    {
        let _ = writeln!(
            out,
            "  session affinity  this project has recorded no session that is still warm, so \
             every candidate is a fresh start"
        );
    }
    if destinations
        .iter()
        .all(|destination| destination.capacity().is_none())
    {
        let _ = writeln!(
            out,
            "  quota pressure    no quota reading has been cached for any of these providers; \
             `glasshouse resources --probe <provider>` takes one"
        );
    }
    if refused_by_launch.contains(&routed.chosen().id().to_owned()) {
        let _ = writeln!(
            out,
            "  not launchable    profile `{}` declares a protocol its harness does not speak, \
             so `glasshouse launch` refuses it — this ranking answers whether the provider \
             could serve that harness at all, which is a different question",
            routed.chosen().launch_profile()
        );
    }
    if matches!(
        routed.chosen().continuation(),
        Continuation::Existing(warm)
            if warm.state == glasshouse::config::pairing::WarmSessionState::Live
    ) {
        let _ = writeln!(
            out,
            "  still running     `{}` is live, so it is the best place for this work and not \
             one `glasshouse run` can enter — switch to that terminal, or pass `--fresh`",
            routed.chosen().id()
        );
    }
    out
}
