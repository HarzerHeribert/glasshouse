//! `commands::entitlements` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use glasshouse::Runtime;
use glasshouse::config::{self, EffectiveConfig, UserConfig};
use glasshouse::session::ProjectSessions;

/// One entitlement's four telemetry facets, as `glasshouse status` renders
/// them — map line 1965's consumer.
///
/// `unknown` is a rendered word, never a number: a facet nothing measured
/// says so. Every reading shared beyond this account carries its scope word
/// (`provider-wide`); a reading narrowed to this account's own rows says
/// `this account`.
/// The configured entitlement pool with map line 1965's telemetry resolved
/// against it — the sources read **once** and handed to the one resolver.
///
/// Extracted so `glasshouse status`'s entitlement lines and `glasshouse
/// entitlements`' view cannot read different sources and disagree about the
/// same account. Every read is fail-soft in the same way it already was: a
/// project whose evidence ledger will not open still gets its pool, with the
/// throttling facet honestly `unknown` rather than "none observed".
pub(crate) fn entitlement_pool_with_telemetry(
    runtime: &Runtime,
    effective: &EffectiveConfig,
) -> anyhow::Result<Vec<glasshouse::config::ResolvedEntitlement>> {
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let quota_cache = glasshouse::provider::telemetry::GatewayQuotaCache::new(runtime.paths());
    let model_cache = glasshouse::provider::cache::ModelCache::new(runtime.paths());
    let observations = glasshouse::routing::evidence::EvidenceLedger::open(runtime)
        .and_then(|ledger| {
            Ok(ledger.observations_in_window(
                now_unix,
                glasshouse::routing::evidence::CLASSIFICATION_EVIDENCE_WINDOW_SECONDS,
            )?)
        })
        .map_err(|err| {
            tracing::debug!(
                error = %err,
                "could not read the routing evidence ledger for the entitlement pool"
            );
        })
        .ok();
    // Map line 1245's "historical sessions" input to the headroom estimator
    // — this project's own count of sessions charged to each account
    // (`sessions.entitlement`, migration 22), read fail-soft exactly like
    // the ledger rows above: a project whose sessions store will not open
    // still gets its pool, with the estimator simply missing this one input.
    let session_counts: std::collections::BTreeMap<String, usize> = ProjectSessions::open(runtime)
        .and_then(|sessions| Ok(sessions.store().list()?))
        .map_err(|err| {
            tracing::debug!(
                error = %err,
                "could not read the project sessions for the entitlement pool"
            );
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|record| record.entitlement)
        .fold(std::collections::BTreeMap::new(), |mut counts, name| {
            *counts.entry(name).or_insert(0) += 1;
            counts
        });

    let mut telemetry = glasshouse::config::EntitlementTelemetry::new(now_unix)
        .with_gateway_quota(&quota_cache)
        .with_model_catalogues(&model_cache)
        .with_session_counts(&session_counts);
    if let Some(observations) = observations.as_deref() {
        telemetry = telemetry.with_observations(observations);
    }
    Ok(effective.configured_entitlements_with_telemetry(&telemetry)?)
}

pub(crate) fn entitlements_report(runtime: &Runtime) -> anyhow::Result<String> {
    use std::collections::BTreeMap;
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "Entitlement pool");
    let _ = writeln!(out, "================");
    let _ = writeln!(out);
    let _ = writeln!(out, "Project  {}", runtime.project().name());
    let _ = writeln!(out);

    let user = UserConfig::load(runtime.paths())?;
    let project_config = config::load_project_config(runtime.project())?;
    let effective = EffectiveConfig::new(&user, project_config.as_ref());
    let entitlements = entitlement_pool_with_telemetry(runtime, &effective)?;

    // What each account served, from migration 22's column. One pass over
    // the project's own sessions — the same `list()` `glasshouse status` and
    // `glasshouse sessions` read, so this view is scoped to the active
    // project exactly as the rest of the sessions table is, and it can see
    // no other project's rows.
    let sessions = ProjectSessions::open(runtime)?;
    let records = sessions.store().list()?;
    let mut served: BTreeMap<&str, (usize, &glasshouse::session::SessionRecord)> = BTreeMap::new();
    for record in &records {
        let Some(name) = record.entitlement.as_deref() else {
            continue;
        };
        served
            .entry(name)
            // `list()` is ordered by activity, newest first, so the first row
            // an account is seen on is its most recent one and later rows
            // only raise the count.
            .and_modify(|(count, _)| *count += 1)
            .or_insert((1, record));
    }

    if entitlements.is_empty() {
        let _ = writeln!(
            out,
            "No `[entitlements]` entries are configured, so Glasshouse describes no pool."
        );
        let _ = writeln!(
            out,
            "Add one under `[entitlements.<name>]` to name an account it may charge."
        );
    } else {
        let thresholds = effective.capacity_band_thresholds().value;
        // Map line 1836: opened once for the whole pool, fail-soft — a
        // ledger this build cannot open leaves every entry's replay at its
        // honest `not enough throttles to score` default rather than
        // failing the report.
        let now_unix = glasshouse::provider::cache::now_unix_seconds();
        let evidence_ledger = glasshouse::routing::evidence::EvidenceLedger::open(runtime).ok();
        for entry in &entitlements {
            let _ = writeln!(out, "`{}`  ({})", entry.name(), entry.describe());
            let replay = headroom_replay_for(evidence_ledger.as_ref(), now_unix, entry);
            // The same renderer `glasshouse status` uses, deliberately: the
            // two commands describing one account differently would be a
            // defect nobody could act on.
            let _ = writeln!(out, "  {}", entitlement_facets(entry, &thresholds));
            let _ = writeln!(out, "  served: {}", served_phrase(served.get(entry.name())));
            let _ = writeln!(out, "  {}", headroom_replay_facet(&replay));
            let _ = writeln!(out);
        }
    }

    // Sessions charged to an account the configuration no longer describes.
    // Recorded history does not vanish when a person edits a file, and a view
    // that silently dropped those rows would under-report what the pool has
    // served.
    let configured: Vec<&str> = entitlements.iter().map(|entry| entry.name()).collect();
    let orphaned: Vec<&str> = served
        .keys()
        .copied()
        .filter(|name| !configured.contains(name))
        .collect();
    if !orphaned.is_empty() {
        let _ = writeln!(
            out,
            "Also served, by entries no longer configured: {}",
            orphaned
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    Ok(out)
}

/// How the view says what an account served.
///
/// Split out so the "nothing recorded" wording has one home and cannot drift
/// into the `unknown` the telemetry facets use — see
/// [`entitlements_report`]'s own note on why these are different facts.
fn served_phrase(entry: Option<&(usize, &glasshouse::session::SessionRecord)>) -> String {
    match entry {
        None => "nothing recorded".to_owned(),
        Some((count, latest)) => format!(
            "{count} session{} — most recently {} ({})",
            if *count == 1 { "" } else { "s" },
            crate::commands::shared::short_id(&latest.id),
            crate::commands::shared::format_age(latest.last_activity_at)
        ),
    }
}

/// Map line 1836's replay counts for one entitlement — shared by
/// `glasshouse entitlements` and `glasshouse status`'s own pool listing, so
/// the two commands cannot describe one account's throttle history
/// differently. `None` ledger, or a backing that names no provider (a
/// native-harness or unstated entry, which no routing row is ever recorded
/// under), both fall back to the honest zero-throttle default —
/// [`entitlement_facets`] renders that as *not enough throttles to score*,
/// the same wording an opened ledger with too few rows gets.
pub(crate) fn headroom_replay_for(
    ledger: Option<&glasshouse::routing::evidence::EvidenceLedger>,
    now_unix: i64,
    entry: &glasshouse::config::ResolvedEntitlement,
) -> glasshouse::routing::evidence::HeadroomReplayCounts {
    match (ledger, entry.backing()) {
        (Some(ledger), glasshouse::config::EntitlementBacking::Provider(provider)) => ledger
            .headroom_replay(
                provider,
                now_unix,
                glasshouse::routing::evidence::CLASSIFICATION_EVIDENCE_WINDOW_SECONDS,
            )
            .unwrap_or_default(),
        _ => glasshouse::routing::evidence::HeadroomReplayCounts::default(),
    }
}

pub(crate) fn entitlement_facets(
    entry: &glasshouse::config::ResolvedEntitlement,
    thresholds: &glasshouse::provider::quota::CapacityBandThresholds,
) -> String {
    use glasshouse::config::{EntitlementModels, TelemetryScope};
    use glasshouse::routing::evidence::{
        HeadroomBand, HeadroomBasis, LongWindowPressure, ResetBasis,
    };

    fn band_str(band: HeadroomBand) -> &'static str {
        match band {
            HeadroomBand::Exhausted => "exhausted",
            HeadroomBand::Low => "low",
            HeadroomBand::Moderate => "moderate",
            HeadroomBand::Ample => "ample",
        }
    }

    let scope_note = |scope: TelemetryScope| format!(" ({})", scope.as_str());

    let capacity = match entry.remaining_capacity() {
        Some(score) => format!(
            "capacity: {}{}",
            score.band(thresholds),
            entry.capacity_scope().map(scope_note).unwrap_or_default()
        ),
        None => "capacity: unknown".to_owned(),
    };

    let reset = match entry.seconds_until_reset() {
        Some(seconds) if seconds >= 0 => format!(
            "reset: in {seconds}s{}",
            entry.capacity_scope().map(scope_note).unwrap_or_default()
        ),
        // The window already turned by this machine's clock — say so
        // rather than rendering a negative wait.
        Some(_) => format!(
            "reset: due{}",
            entry.capacity_scope().map(scope_note).unwrap_or_default()
        ),
        None => "reset: unknown".to_owned(),
    };

    let throttling = match entry.throttling() {
        Some(reading) if reading.throttled() == 0 => {
            format!("throttling: none observed{}", scope_note(reading.scope()))
        }
        Some(reading) => format!(
            "throttling: {} recent{}",
            reading.throttled(),
            scope_note(reading.scope())
        ),
        None => "throttling: unknown".to_owned(),
    };

    let models = match entry.models() {
        Some(EntitlementModels::Declared { models, scope }) => {
            if models.len() <= 4 {
                format!("models: {}{}", models.join(", "), scope_note(*scope))
            } else {
                format!("models: {} declared{}", models.len(), scope_note(*scope))
            }
        }
        Some(EntitlementModels::HarnessDecided) => "models: the harness decides".to_owned(),
        None => "models: unknown".to_owned(),
    };

    // Map lines 1244/1245/1246/1250/1251/1254's headroom estimate — always
    // labelled `estimate` and never merged into `capacity`, exactly the
    // "never dressed as an authoritative reading" the packet asks for.
    // Never a number: a band, its confidence, its basis, and whose reading
    // it is.
    //
    // Map line 1252's override is checked first and rendered in its own
    // vocabulary — "your reading" rather than a confidence and a basis — so
    // a user's correction can never be mistaken for Glasshouse's own
    // inference; it is still only ever a band, never a percentage or a
    // token figure, so 1250/1251 hold for it too. Map line 1255's disabled
    // scope reaches here as `None`, indistinguishable from genuinely
    // unknown, unless an override is also set — an override is the user's
    // own stated reading and disabling the *derived* estimate does not
    // retract it.
    let headroom_estimate = match (entry.headroom_override(), entry.headroom_estimate()) {
        (Some(band), _) => {
            format!(
                "headroom estimate: ~{} (your reading, overrides the estimate)",
                band_str(band)
            )
        }
        (None, Some(estimate)) => {
            let band = band_str(estimate.band);
            let basis = match estimate.basis {
                HeadroomBasis::RequestActivity => "request activity",
                HeadroomBasis::TokenUsage => "token usage",
            };
            let scope = if estimate.account_narrowed {
                "this account"
            } else {
                "provider-wide"
            };
            let mut rendered = format!(
                "headroom estimate: ~{band} ({scope}, {}, {basis})",
                estimate.confidence.as_str()
            );
            // Map line 1248: an inferred reset window must never render
            // identically to the provider's own stated word.
            if estimate.reset_basis == ResetBasis::Learned {
                rendered.push_str(", reset: learned");
            }
            // Map line 1249: only the positive, evidence-backed distinction
            // is worth a consumer's attention — `Undistinguished` and
            // `NoPressure` both render nothing new, which is also what
            // keeps the no-new-config regression byte-identical to
            // `4f0c1cf`'s output.
            if estimate.long_window_pressure == LongWindowPressure::Present {
                rendered.push_str(", persistent pressure beyond the short window");
            }
            // Map line 1247's reachable half: once a regime change has been
            // detected, the estimate above is already derived only from
            // rows at or after it (`config::populate_provider_facets`'s
            // floor) — this says so, through the same age formatter every
            // other facet on this line uses. `format_age` already renders
            // "ago" (or "just now"), so this appends its output as-is rather
            // than doubling the word.
            if let Some(since_unix) = estimate.since_unix {
                rendered.push_str(&format!(
                    "; limits changed {}",
                    crate::commands::shared::format_age(since_unix)
                ));
            }
            rendered
        }
        (None, None) => "headroom estimate: unknown".to_owned(),
    };

    format!("{capacity} · {reset} · {throttling} · {models} · {headroom_estimate}")
}

/// Map line 1836's own line, kept apart from [`entitlement_facets`]'s so the
/// two callers can place it: `glasshouse entitlements` prints it *after* the
/// `served:` line, because the per-account block there is name, facets,
/// served — three lines that `tests/entitlement_broker.rs` reads by position
/// and that the wave-107 trailing sweep found this line had pushed apart;
/// `glasshouse status` prints it as the facets line's continuation.
pub(crate) fn headroom_replay_facet(
    replay: &glasshouse::routing::evidence::HeadroomReplayCounts,
) -> String {
    use glasshouse::routing::evidence::MIN_SAMPLE_FOR_SUMMARY;
    if replay.throttles() < MIN_SAMPLE_FOR_SUMMARY {
        "headroom estimate vs throttles (1836): not enough throttles to score".to_owned()
    } else {
        let reset_clause = match replay.observed_reset_lag_median_seconds {
            Some(seconds) => format!(
                "observed reset lag median {seconds}s over {}",
                replay.observed_reset_lag_sample_count
            ),
            None => "no observed resets".to_owned(),
        };
        format!(
            "headroom estimate vs throttles (1836): warned {} / missed {} / unestimable {} of \
             {} throttles; {reset_clause}",
            replay.warned,
            replay.missed,
            replay.unestimable,
            replay.throttles()
        )
    }
}

/// Versioned read-only catalogue for Pane. Provider declarations describe scope,
/// not an assertion that every account currently has quota for every model.
pub(crate) fn entitlements_json(runtime: &Runtime, refresh: bool) -> anyhow::Result<String> {
    use glasshouse::config::{EntitlementBacking, EntitlementModels};
    let user = UserConfig::load(runtime.paths())?;
    let project = config::load_project_config(runtime.project())?;
    let effective = EffectiveConfig::new(&user, project.as_ref());
    if refresh {
        refresh_missing_catalogues(runtime, &effective)?;
    }
    let entries = entitlement_pool_with_telemetry(runtime, &effective)?;
    let mut accounts: Vec<_> = entries.iter().map(|entry| {
        let provider = match entry.backing() {
            EntitlementBacking::Provider(name) => Some(name.as_str()),
            _ => None,
        };
        let (mut models, scope) = match entry.models() {
            Some(EntitlementModels::Declared { models, .. }) => (models.clone(), "provider-declared"),
            Some(EntitlementModels::HarnessDecided) => (vec![], "harness-decides"),
            None => (vec![], "unknown"),
        };
        models.sort();
        models.dedup();
        serde_json::json!({"account":entry.name(),"provider":provider,"models":models,"scope":scope})
    }).collect();
    accounts.sort_by(|a, b| a["account"].as_str().cmp(&b["account"].as_str()));
    Ok(serde_json::to_string(&serde_json::json!({"version":1,"accounts":accounts}))? + "\n")
}

/// Reuses the shell's bounded discovery and credential resolver. Invoked only
/// by `entitlements --json --refresh`, never on startup or a timer.
fn refresh_missing_catalogues(
    runtime: &Runtime,
    effective: &EffectiveConfig<'_>,
) -> anyhow::Result<()> {
    use glasshouse::config::EntitlementBacking;
    use glasshouse::provider::cache::{ModelCache, ModelCatalogue};
    use glasshouse::provider::discovery::{self, ModelFetch, ProbeRequest, ProbeTarget};
    use glasshouse::secret::SecretStore;
    let cache = ModelCache::new(runtime.paths());
    let entries = entitlement_pool_with_telemetry(runtime, effective)?;
    let mut providers = std::collections::BTreeMap::new();
    for entry in entries {
        let EntitlementBacking::Provider(name) = entry.backing() else {
            continue;
        };
        if cache.load(name).is_some() {
            continue;
        }
        let Ok(provider) = effective.configured_provider(name) else {
            continue;
        };
        if !provider.value.model_list_endpoint.is_known_present() {
            continue;
        }
        providers
            .entry(name.clone())
            .or_insert((provider.value, entry.credential().cloned()));
    }
    let jobs: Vec<_> = providers.into_values().collect();
    for batch in jobs.chunks(4) {
        std::thread::scope(|scope| {
            for (provider, account_credential) in batch {
                let cache_root = cache.root().to_path_buf();
                scope.spawn(move || {
                    let Some(protocol) = provider.protocols.first() else {
                        return;
                    };
                    let store = glasshouse::secret::native::PreferNativeSecretStore::detect();
                    let credential = account_credential
                        .as_ref()
                        .and_then(|r| store.resolve(r))
                        .or_else(|| provider.secret_refs().iter().find_map(|r| store.resolve(r)));
                    let request = ProbeRequest::new(
                        &provider.name,
                        protocol.protocol,
                        &protocol.base_url,
                        ProbeTarget::ModelList,
                        provider.headers.clone(),
                        credential,
                    );
                    if let ModelFetch::Catalogue(models) =
                        discovery::model_catalogue(&request, discovery::ProbeTimeouts::default())
                    {
                        let catalogue = ModelCatalogue::new(
                            &provider.name,
                            &protocol.base_url,
                            request.url(),
                            glasshouse::provider::cache::now_unix_seconds(),
                            models,
                        );
                        let _ = ModelCache::at(cache_root).store(&catalogue);
                    }
                });
            }
        });
    }
    Ok(())
}
