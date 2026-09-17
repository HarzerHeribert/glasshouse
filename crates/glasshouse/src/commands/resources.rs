//! `commands::resources` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use glasshouse::Runtime;
use glasshouse::config::{self, EffectiveConfig, UserConfig};

/// Render `glasshouse resources` — Phase 32B's production caller, and the
/// reason its boxes are closeable at all: Phase 32 found
/// `provider::registry::registry()` had no production caller, and Phase 32A
/// found the launch path reads only `CapacityState`'s quota *shape*. This is
/// the thing in the shipped binary that reads the model, and every
/// telemetry reader Phase 32B builds is reached from here and nowhere else.
///
/// Harness status is read first — a local, quarter-second process
/// invocation that spends no quota and needs no credential — so a bare
/// `glasshouse resources` still takes a real reading instead of showing
/// `unknown` for free. Network probes are opt-in, matching `glasshouse
/// pairing` and `glasshouse response`.
///
/// The `Result` is only for the user's own configuration files: no
/// telemetry read below can produce an `Err` — capability map line 1238 is
/// enforced by `provider::telemetry` and `provider::resources` having no
/// fallible signature to propagate.
///
/// History: design-decisions.md, "Trims: commands module docs", resources_report.
pub(crate) fn resources_report(
    runtime: &Runtime,
    verbose: bool,
    probe: &[String],
    no_harness: bool,
    force_probe: bool,
) -> anyhow::Result<String> {
    let user = UserConfig::load(runtime.paths())?;
    let project = config::load_project_config(runtime.project())?;
    let gateway = config::GatewayCatalogue::for_paths(runtime.paths())?;
    let effective = EffectiveConfig::with_gateway(&user, project.as_ref(), &gateway);
    let now_unix = glasshouse::provider::cache::now_unix_seconds();

    let mut telemetry = glasshouse::provider::resources::GatheredTelemetry::new();
    telemetry =
        telemetry.gather_gateway_quota(&glasshouse::provider::telemetry::GatewayQuotaCache::new(
            runtime.paths().gateway_data_dir(),
        ));
    telemetry =
        telemetry.gather_gateway_health(&glasshouse::provider::telemetry::GatewayHealthCache::new(
            runtime.paths().gateway_data_dir(),
        ));
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
    Ok(out)
}
