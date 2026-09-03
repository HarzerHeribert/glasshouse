//! `commands::resources` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use glasshouse::Runtime;
use glasshouse::config::{self, EffectiveConfig, ProjectConfig, UserConfig};

/// Render `glasshouse resources` — Phase 32B's production caller, and the
/// reason its boxes are closeable at all.
///
/// # What this function is for, beyond printing
///
/// Phase 32 recorded that `provider::registry::registry()` had no production
/// caller, and Phase 32A recorded that the launch path reads exactly one
/// projection out of `CapacityState` — its quota *shape* — with every pool,
/// window and rate ceiling below that proven only by tests. Both ledgers
/// named the same missing piece: something in the shipped binary that reads
/// the model. This is that, and every telemetry reader Phase 32B builds is
/// reached from here and from nowhere else in the binary.
///
/// # The order of reads, and why the cheap one is not optional
///
/// Harness status first, because it is a local process invocation of about a
/// quarter of a second that spends no quota and needs no credential — so the
/// bare command still takes a real reading, and a user who runs
/// `glasshouse resources` with no flags is not shown a screen of `unknown`
/// that Glasshouse could have filled in for free. Network probes are opt-in,
/// matching `glasshouse pairing` and `glasshouse response`, which is the
/// shape this command was modelled on.
///
/// # It cannot fail on telemetry
///
/// The `Result` here is for reading the user's own configuration files, which
/// is the same failure every other command in this file can have. No
/// telemetry read below can produce an `Err`: capability map line 1238 is
/// enforced in `provider::telemetry` and `provider::resources` by there being
/// no fallible signature to propagate.
pub(crate) fn resources_report(
    runtime: &Runtime,
    verbose: bool,
    probe: &[String],
    no_harness: bool,
    force_probe: bool,
) -> anyhow::Result<String> {
    let user = UserConfig::load(runtime.paths())?;
    let project = config::load_project_config(runtime.project())?;
    let effective = EffectiveConfig::new(&user, project.as_ref());
    let now_unix = glasshouse::provider::cache::now_unix_seconds();

    let mut telemetry = glasshouse::provider::resources::GatheredTelemetry::new();
    telemetry = telemetry.gather_gateway_quota(
        &glasshouse::provider::telemetry::GatewayQuotaCache::new(runtime.paths()),
    );
    telemetry = telemetry.gather_gateway_health(
        &glasshouse::provider::telemetry::GatewayHealthCache::new(runtime.paths()),
    );
    // Capability map lines 1316/1365: recent failures by class, from the
    // project's routing evidence ledger. Fail-soft: a project with no ledger
    // yet renders `unknown` on that line, as the caches do.
    if let Ok(ledger) = glasshouse::routing::evidence::EvidenceLedger::open(runtime) {
        telemetry = telemetry.gather_failure_classes(&ledger, now_unix);
        // Capability map line 1519: priced spend against every provider's
        // own configured money budget, from the same ledger, through
        // `pricing.toml`. Fail-soft exactly as the gather above.
        let prices =
            glasshouse::provider::pricing::PriceTable::load_from_dir(runtime.paths().config_dir());
        telemetry = telemetry.gather_budget_spend(&ledger, &prices, &effective, now_unix);
    }
    if !no_harness {
        telemetry = telemetry.gather_harness_status(now_unix);
    }

    let mut probes = String::new();
    if !probe.is_empty() {
        use std::fmt::Write as _;
        let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();
        let _ = writeln!(probes, "PROBES\n");
        for name in probe {
            let authorization = glasshouse::provider::resources::authorize_probe(
                &effective, &telemetry, name, now_unix,
            );
            let reading = match authorization {
                glasshouse::provider::resources::ProbeAuthorization::Refused(budget)
                    if !force_probe =>
                {
                    glasshouse::provider::resources::ProbeReading::Refused {
                        remaining: budget.remaining,
                        cost: budget.cost,
                    }
                }
                glasshouse::provider::resources::ProbeAuthorization::Refused(budget) => {
                    glasshouse::provider::resources::render_forced_probe(
                        &mut probes,
                        name,
                        &budget,
                    );
                    glasshouse::provider::resources::probe_provider(
                        &effective, &secrets, name, now_unix,
                    )
                }
                glasshouse::provider::resources::ProbeAuthorization::Allowed => {
                    glasshouse::provider::resources::probe_provider(
                        &effective, &secrets, name, now_unix,
                    )
                }
            };
            glasshouse::provider::resources::render_probe(&mut probes, name, &reading);
            if let glasshouse::provider::resources::ProbeReading::Answered {
                headers,
                observed_at_unix,
                ..
            } = reading
            {
                telemetry = telemetry.with_provider_headers(name, headers, observed_at_unix);
            }
        }
        probes.push('\n');
    }

    let options = glasshouse::provider::resources::ReportOptions { verbose, now_unix };
    let mut out = format!(
        "{probes}{}",
        glasshouse::provider::resources::report(&effective, &telemetry, options)
    );
    out.push('\n');
    render_routing_model(
        &mut out,
        runtime,
        &user,
        project.as_ref(),
        &effective,
        verbose,
    );
    render_routing_economics(&mut out, runtime, now_unix);
    Ok(out)
}

/// Capability map lines 1463, 1465 and 1466 — what routing itself costs, as
/// the block after `ROUTING MODEL` in `glasshouse resources`.
///
/// Three lines and a conditional fourth, every one carrying its
/// denominators: decisions *over* interactive hours, tokens *over* calls on
/// each side, and the overhead fraction beside the line it is judged
/// against. A figure nobody counted prints as *not counted*
/// ([`render_token_count`]'s rule), and a comparison that cannot be made
/// prints as *not comparable* with the reason — never `0%`, which would read
/// as "routing is free".
///
/// # Both ledgers are opened here and dropped here (practice §65)
///
/// Each store is opened inside the helper that reads it and closed before
/// the next one opens, so no handle is held across a read it is not part
/// of. A store that cannot be opened renders as *unavailable* with the
/// reason, and the command succeeds: [`resources_report`]'s own header says
/// no telemetry read may fail it, and this block is telemetry.
fn render_routing_economics(out: &mut String, runtime: &Runtime, now_unix: i64) {
    use glasshouse::routing::evidence::{
        CLASSIFICATION_EVIDENCE_WINDOW_SECONDS, ROUTING_OVERHEAD_WARNING_FRACTION,
    };
    use std::fmt::Write as _;

    let window_seconds = CLASSIFICATION_EVIDENCE_WINDOW_SECONDS;
    let from = now_unix.saturating_sub(window_seconds);
    let _ = writeln!(
        out,
        "\nROUTING ECONOMICS (last {} days)",
        window_seconds / (24 * 60 * 60)
    );

    match routing_decision_rate(runtime, from, now_unix) {
        Ok(rate) => {
            let per_hour = match rate.per_hour() {
                Some(per_hour) => format!("{per_hour:.1} per interactive hour"),
                None => "no interactive hour in the window, so no rate".to_owned(),
            };
            let _ = writeln!(
                out,
                "  {:<16}{} routing decisions over {} interactive hours — {per_hour}",
                "decisions", rate.decisions, rate.interactive_hours
            );
            let _ = writeln!(
                out,
                "  {:<16}an interactive hour is a wall-clock hour in which a session record \
                 shows activity (map line 1463)",
                ""
            );
        }
        Err(reason) => {
            let _ = writeln!(out, "  {:<16}unavailable — {reason:#}", "decisions");
        }
    }

    let tokens = |count: Option<i64>| match count {
        Some(count) => format!("{count} tokens"),
        None => "tokens not counted".to_owned(),
    };
    match routing_overhead(runtime, now_unix, window_seconds) {
        Ok(overhead) => {
            let _ = writeln!(
                out,
                "  {:<16}{} over {} classification calls",
                "routing spend",
                tokens(overhead.classification_tokens),
                overhead.classification_requests
            );
            let _ = writeln!(
                out,
                "  {:<16}{} over {} other calls — everything this project's ledger holds that \
                 is not classification (map line 1465)",
                "task spend",
                tokens(overhead.task_tokens),
                overhead.task_requests
            );
            // Capability map lines 1832 and 1833: what *"task spend"* above
            // is actually made of, each side with both its denominators —
            // tokens and calls. Every one of these is a subset of the line
            // above rather than a competing total, and they sum to it
            // exactly (`RoutingOverhead`'s own doc comment).
            let _ = writeln!(
                out,
                "  {:<16}{} over {} extraction calls — Glasshouse's own memory extraction, \
                 apart from the coding agent's work (map line 1832)",
                "extraction",
                tokens(overhead.extraction_tokens),
                overhead.extraction_requests
            );
            let _ = writeln!(
                out,
                "  {:<16}{} over {} decision rows — the routing model's own request \
                 consumption, which carries no tokens because a decision's latency is not a \
                 model call (map line 1833)",
                "routing model",
                tokens(overhead.routing_latency_tokens),
                overhead.routing_latency_requests
            );
            let _ = writeln!(
                out,
                "  {:<16}{} over {} decision rows — tier escalations and downgrades the session \
                 router acted on (map line 1566); no tokens, because a decision is not a model \
                 call",
                "tier movement",
                tokens(overhead.tier_movement_tokens),
                overhead.tier_movement_requests
            );
            let _ = writeln!(
                out,
                "  {:<16}{} over {} fallback rows — pool fallbacks the launch path acted on \
                 (map line 1970); no tokens, because a decision is not a model call",
                "pool fallback",
                tokens(overhead.entitlement_fallback_tokens),
                overhead.entitlement_fallback_requests
            );
            let _ = writeln!(
                out,
                "  {:<16}{} over {} relayed exchanges — interactive coding cost; the gateway \
                 relays a body it never parses, so a token count here is absent rather than \
                 zero",
                "coding agent",
                tokens(overhead.coding_agent_tokens),
                overhead.coding_agent_requests
            );
            let _ = writeln!(
                out,
                "  {:<16}{} over {} calls — rows written before this build stamped a purpose, \
                 counted as unstamped and never re-labelled",
                "unstamped",
                tokens(overhead.unstamped_tokens),
                overhead.unstamped_requests
            );
            match overhead.fraction() {
                Some(fraction) => {
                    let _ = writeln!(
                        out,
                        "  {:<16}{:.1}% of task spend — warns above {:.0}% (map line 1466)",
                        "overhead",
                        fraction * 100.0,
                        ROUTING_OVERHEAD_WARNING_FRACTION * 100.0
                    );
                    if overhead.exceeds(ROUTING_OVERHEAD_WARNING_FRACTION) {
                        let _ = writeln!(
                            out,
                            "  {:<16}routing is consuming {:.1}% of the task spend it exists to \
                             protect, above the {:.0}% line — the routing model is no longer \
                             cheap relative to what it saves",
                            "warning",
                            fraction * 100.0,
                            ROUTING_OVERHEAD_WARNING_FRACTION * 100.0
                        );
                    }
                }
                None => {
                    let why = if overhead.classification_tokens.is_none() {
                        "no classification call in the window carried a token count"
                    } else if overhead.task_tokens.is_none() {
                        "no other call in the window carried a token count"
                    } else {
                        "no task spend was counted in the window to compare against"
                    };
                    let _ = writeln!(out, "  {:<16}not comparable — {why}", "overhead");
                }
            }
        }
        Err(reason) => {
            let _ = writeln!(out, "  {:<16}unavailable — {reason:#}", "routing spend");
        }
    }
}

/// Capability map line 1463's two stores, each opened for exactly its own
/// read: the session store for activity spans, then the evaluation ledger
/// for the count.
fn routing_decision_rate(
    runtime: &Runtime,
    from: i64,
    to: i64,
) -> anyhow::Result<glasshouse::evaluation::RoutingDecisionRate> {
    let spans: Vec<(i64, i64)> = {
        let sessions = glasshouse::session::ProjectSessions::open(runtime)?;
        let records = sessions.store().list()?;
        records
            .into_iter()
            .map(|record| (record.created_at, record.last_activity_at))
            .collect()
    };
    let ledger = glasshouse::evaluation::EvaluationObservations::open(runtime)?;
    Ok(ledger.routing_decision_rate(spans, from, to)?)
}

/// Capability map line 1465's reading, from the evidence ledger alone.
fn routing_overhead(
    runtime: &Runtime,
    now_unix: i64,
    window_seconds: i64,
) -> anyhow::Result<glasshouse::routing::evidence::RoutingOverhead> {
    let ledger = glasshouse::routing::evidence::EvidenceLedger::open(runtime)?;
    let groups = ledger.consumption_by_purpose(now_unix, window_seconds)?;
    Ok(glasshouse::routing::evidence::RoutingOverhead::from_consumption(&groups))
}

/// Capability map line 1443 — *"show the currently selected routing model in
/// resource diagnostics"* — as the last block of `glasshouse resources`.
///
/// # Why this surface, and why it is not the settings screen
///
/// The Settings overlay already renders the configured
/// [`glasshouse::config::RoutingModelChoice`], and `docs/product/evidence/phase-34c.md`
/// ruled that showing a value on the screen where you set it is
/// configuration, not diagnosis. This is the diagnostic surface: the routing
/// model is named next to the capacity, health and quota of the very
/// resources it would be chosen from, which is where the question *"why did
/// routing behave that way"* is actually asked.
///
/// # The honesty constraint, and it is the point of the block
///
/// `Automatic` is an intent — the word the Settings overlay shows — and
/// naming only that would answer a different question than a person reading
/// `glasshouse resources` is asking. So the block runs the real decision
/// ([`automatic_classification_choice`], the same function `glasshouse
/// classify` calls) and names the resource it picked.
///
/// **And it says `would`, in every arm.** Nothing in this build classifies
/// anything on its own: `routing::classify::classify`'s only production
/// caller is the `glasshouse classify` diagnostic, and nothing else asks a
/// routing model a question. Rendering a "currently selected routing model"
/// beside live capacity numbers with no signal that it classifies nothing is
/// the spectacle Phase 47 exists to prevent, so the `in use` row says so in
/// as many words and is not conditional on anything.
///
/// # No credential, ever
///
/// [`glasshouse::routing::disposable::DisposableChoice`] carries a
/// [`glasshouse::routing::CredentialId`], and nothing below reads it. A
/// provider name, a model name and the policy's own explanation are what this
/// block prints — the same rule `memory::extract::model`'s header states for
/// the label a classification is attributed to.
fn render_routing_model(
    out: &mut String,
    runtime: &Runtime,
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    effective: &EffectiveConfig<'_>,
    verbose: bool,
) {
    use glasshouse::config::{RoutingFallback, RoutingModelResolution};
    use std::fmt::Write as _;

    let resolution = effective.routing_model_resolution();
    out.push_str("ROUTING MODEL\n");

    let configured = match &resolution.value {
        RoutingModelResolution::Automatic => "automatic".to_owned(),
        RoutingModelResolution::Pinned { provider, model } => format!("{provider}/{model}"),
        RoutingModelResolution::Heuristics(RoutingFallback::ProviderNotConfigured {
            provider,
            ..
        }) => format!("deterministic heuristics (`{provider}` is no longer configured)"),
        RoutingModelResolution::Heuristics(_) => "deterministic heuristics".to_owned(),
    };
    let _ = writeln!(
        out,
        "  {:<16}{configured} ({})",
        "configured",
        resolution.layer.describe_source()
    );

    match &resolution.value {
        RoutingModelResolution::Automatic => {
            // `None`: this report has no request in hand, and `choose`
            // documents that value as the fixed `WorkloadTier::Leaf` the
            // policy used before a classification existed to ask. A request
            // invented here to fill the argument would make the reported pick
            // depend on words nobody typed.
            // Line 1419: this report has chosen no launch profile either —
            // there is nothing here to protect.
            match crate::commands::routing_classification::automatic_classification_choice(
                runtime, user, project, effective, None, None,
            ) {
                Ok(choice) => {
                    let _ = writeln!(
                        out,
                        "  {:<16}{} on {} — {}, {}",
                        "would select",
                        choice.model(),
                        choice.provider(),
                        choice.cost().as_str(),
                        choice.reason()
                    );
                    let _ = writeln!(
                        out,
                        "  {:<16}for a request of unknown demand; a classified request can \
                         select another",
                        ""
                    );
                    if verbose {
                        for line in choice.explanation().render().lines() {
                            let _ = writeln!(out, "  {:<16}{line}", "");
                        }
                    }
                }
                Err(reason) => {
                    let _ = writeln!(out, "  {:<16}nothing — {reason}", "would select");
                }
            }
        }
        RoutingModelResolution::Pinned { provider, model } => {
            let _ = writeln!(
                out,
                "  {:<16}{model} on {provider} — pinned, so no ranking runs",
                "would select"
            );
        }
        RoutingModelResolution::Heuristics(_) => {
            let _ = writeln!(
                out,
                "  {:<16}no model — deterministic heuristics classify without asking one",
                "would select"
            );
        }
    }

    let _ = writeln!(
        out,
        "  {:<16}nothing yet. `glasshouse classify` is the only command that asks a routing \
         model;\n  {:<16}no other Glasshouse decision calls one, so this names a choice rather \
         than a habit.",
        "in use", ""
    );
}
