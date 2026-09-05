//! `commands::routing_destinations` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use glasshouse::Runtime;
use glasshouse::checkpoint::ProjectCheckpoints;
use glasshouse::config::EffectiveConfig;
use glasshouse::session::{ProjectSessions, SessionDisposition, SessionId, SessionRecord};

/// The identifier `--to` takes for "a new session under this profile".
///
/// Three parts, and each one is load-bearing. The `fresh:` prefix keeps a
/// destination that does not exist yet out of the namespace of recorded
/// session identifiers, which is what `--to` and `RoutingOverride::to`
/// compare against. The harness slug is there because `glasshouse route`
/// ranks across every enabled harness and **every one of them has a `native`
/// profile** — without it, `fresh:native` names between one and ten different
/// destinations and an override lands on whichever was built first.
pub(crate) fn fresh_destination_id(
    harness: glasshouse::integrations::IntegrationId,
    profile: &str,
) -> String {
    format!("fresh:{}:{profile}", harness.slug())
}

/// A fresh-destination id carrying the entitlement axis — 56A line 1953,
/// used only when **several** entitlements back one profile, so a project
/// with zero or one entitlement per resource keeps exactly the ids it has
/// always had (and every test pinned on them keeps passing). The `@` is the
/// router's own convention: `SessionRouter`'s override matching treats the
/// un-suffixed id as naming the profile and picks the best-ranked account
/// among its candidates.
fn entitled_fresh_destination_id(
    harness: glasshouse::integrations::IntegrationId,
    profile: &str,
    entitlement: &str,
) -> String {
    format!("fresh:{}:{profile}@{entitlement}", harness.slug())
}

/// Phase 56A line 1953's producer: every pool entry that backs `backend` on
/// `harness` — `EffectiveConfig::entitlement_for`'s matching rule without
/// its one-account assumption, because the axis exists exactly for the case
/// where several entries legitimately match (two Claude accounts behind one
/// provider), each of which becomes its own candidate. The gateway's
/// upstream is assigned when a session starts, so no entry matches it here
/// (56A-4's ground, unchanged).
fn pool_entitlements_for<'p>(
    pool: &'p [glasshouse::config::ResolvedEntitlement],
    harness: glasshouse::integrations::IntegrationId,
    backend: &glasshouse::profile::BackendResource,
) -> Vec<&'p glasshouse::config::ResolvedEntitlement> {
    use glasshouse::config::EntitlementBacking;
    use glasshouse::profile::BackendResource;

    let wanted = match backend {
        BackendResource::Native => EntitlementBacking::NativeHarness(harness),
        BackendResource::DirectProvider { provider } => {
            EntitlementBacking::Provider(provider.clone())
        }
        BackendResource::GlasshouseGateway => return Vec::new(),
    };
    pool.iter()
        .filter(|entry| *entry.backing() == wanted)
        .collect()
}

/// A resolved entitlement as the router carries it, 56A-2's facets included
/// — the bridge `ResolvedEntitlement::to_routing` deliberately leaves to
/// this caller, because the capacity band is derived against the user's own
/// thresholds. Every facet the telemetry could not answer stays `None`, and
/// the router's terms then contribute nothing and say so.
fn routing_entitlement(
    resolved: &glasshouse::config::ResolvedEntitlement,
    thresholds: &glasshouse::provider::quota::CapacityBandThresholds,
    effective: &EffectiveConfig<'_>,
    telemetry: &glasshouse::provider::resources::GatheredTelemetry,
) -> glasshouse::routing::Entitlement {
    use glasshouse::config::{EntitlementBacking, EntitlementModels, TelemetryScope};
    use glasshouse::routing::{EntitlementModelsFacet, EntitlementThrottleFacet};

    let budget_exhausted = match resolved.backing() {
        EntitlementBacking::Provider(provider) => {
            glasshouse::provider::resources::budget_exhausted_for(provider, effective, telemetry)
        }
        EntitlementBacking::NativeHarness(_) | EntitlementBacking::Unstated => None,
    };

    resolved
        .to_routing()
        .with_capacity(
            resolved
                .remaining_capacity()
                .map(|score| score.band(thresholds)),
            resolved.seconds_until_reset(),
        )
        .with_throttling(resolved.throttling().map(|reading| {
            EntitlementThrottleFacet::new(
                reading.throttled(),
                reading.scope() == TelemetryScope::PerAccount,
            )
        }))
        .with_models(resolved.models().map(|models| match models {
            EntitlementModels::Declared { models, .. } => {
                EntitlementModelsFacet::Declared(models.clone())
            }
            EntitlementModels::HarnessDecided => EntitlementModelsFacet::HarnessDecided,
        }))
        .with_budget_exhausted(budget_exhausted)
}

/// A pool candidate's backend, carrying **this account's own** credential
/// reference in place of the provider pool's first-declared name — what
/// makes two candidates of one provider two resources to the health pool,
/// the cache-locality rule and the quota label. A name only, like every
/// `CredentialId`; an entry with no credential of its own keeps the
/// provider-level default.
fn backend_for_entitlement(
    backend: &glasshouse::routing::Backend,
    entitlement: &glasshouse::config::ResolvedEntitlement,
) -> glasshouse::routing::Backend {
    use glasshouse::routing::{Backend, CredentialId};

    let Some(reference) = entitlement.credential() else {
        return backend.clone();
    };
    Backend::new(
        backend.provider().to_owned(),
        backend.protocol().to_owned(),
        backend.model().clone(),
        CredentialId::new(backend.provider().to_owned(), reference.clone()),
        backend.cost(),
        backend.tools(),
    )
    .with_tools_evidence(backend.tools_evidence())
}

/// Phase 56 line 1954's *"announce which subscription served each session"*,
/// said on stderr beside the routing announcements, before the session
/// exists. `None` is announced as what it is — no entry names this resource,
/// or the gateway has not assigned an upstream yet — rather than as a
/// entitlement nobody configured.
///
/// `gateway_provider` is read only for the `GlasshouseGateway` / `None` case:
/// the gateway's serving provider once it is known, so that case can say
/// *which* provider no entry names instead of the pre-Phase-56/1954-gateway
/// text that was true only because nothing asked yet. `None` there means
/// exactly what it always meant — the gateway has not resolved an upstream
/// for this call, which is still true of every caller other than
/// `launch_session`'s gateway branch and `resume_session`'s announcement.
pub(crate) fn announce_entitlement(
    entitlement: Option<&glasshouse::config::ResolvedEntitlement>,
    profile: &glasshouse::profile::LaunchProfile,
    gateway_provider: Option<&str>,
) {
    use glasshouse::profile::BackendResource;

    match entitlement {
        Some(entitlement) => {
            let served_by = entitlement.name();
            eprintln!(
                "glasshouse: entitlement `{served_by}` ({}) will serve this session.",
                entitlement.describe()
            );
        }
        None => match &profile.backend {
            BackendResource::DirectProvider { provider } => eprintln!(
                "glasshouse: no `[entitlements]` entry names provider `{provider}`, so no \
                 entitlement rule applies to this session."
            ),
            BackendResource::GlasshouseGateway => match gateway_provider {
                Some(provider) => eprintln!(
                    "glasshouse: no `[entitlements]` entry names the gateway's provider \
                     `{provider}`, so no entitlement rule applies to this session."
                ),
                None => eprintln!(
                    "glasshouse: the Glasshouse gateway assigns this session's upstream when it \
                     starts, so no entitlement is named at launch."
                ),
            },
            BackendResource::Native => eprintln!(
                "glasshouse: no entitlement describes {}'s own sign-in.",
                profile.harness.display_name()
            ),
        },
    }
}

/// Line 1954's refusal check, extracted once so the direct/native path
/// (asked before the gateway exists) and the gateway path (asked after it
/// starts, once its serving provider is known) apply exactly one spelling of
/// the refusal text — see practice §35 on what happens when a check like
/// this gets copied instead.
pub(crate) fn entitlement_refusal_message(
    entitlement: Option<&glasshouse::config::ResolvedEntitlement>,
    harness: glasshouse::integrations::IntegrationId,
    launch_profile_name: &str,
) -> Option<String> {
    let entitlement = entitlement?;
    let refused = entitlement.rules().refusal(harness, None)?;
    Some(format!(
        "glasshouse: not starting this session — entitlement `{}` does not serve {refused}, \
         and launch profile `{}` would charge it. Change the rule under `[entitlements.{}]`, \
         or launch under a profile whose entitlement serves this work.",
        entitlement.name(),
        launch_profile_name,
        entitlement.name()
    ))
}

/// Which destinations a caller can actually *use*, which is not the same
/// question as which ones exist.
///
/// `glasshouse route` reports for a person, who can act on "your live session
/// is the best place for this" by switching to that terminal. A launch cannot:
/// there is no attach, and `SessionStore::open_for_resume` refuses a session
/// that is still running. Offering a launch a destination it would then fail
/// to enter is exactly the "producer with no reachable consumer" shape this
/// project keeps paying for, so the launch path asks for `Enterable` and the
/// diagnostic says out loud that it did not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DestinationScope<'a> {
    /// Every session with warmth to speak of, running ones included, and one
    /// fresh destination per configured launch profile.
    Everything,
    /// What *this* launch could actually enter: the sessions it could resume,
    /// and exactly **one** fresh destination — the profile this launch would
    /// have used anyway.
    ///
    /// # Why one profile and not all of them
    ///
    /// Phase 37 is a **session** router: lines 1593 and 1594 are *"prefer an
    /// existing relevant session"* against *"prefer a fresh session"*, and
    /// neither of them is about which launch profile a new session runs
    /// under. Offering the launch path a fresh destination per profile makes
    /// it one, and the consequence is not academic: an unadorned `glasshouse
    /// launch` moved off the implied Native profile onto a configured direct
    /// provider — a different credential, a different bill, and a pre-flight
    /// request to a provider the user had not asked for. Two existing tests
    /// caught it, and they were right.
    ///
    /// So the profile stays where it has always come from — `--profile`, or
    /// Native — and the router decides the thing it is for: whether to start
    /// that session at all, or continue one this project already has.
    /// `glasshouse route` still ranks every profile, because a person reading
    /// a diagnostic is choosing between them and a launch is not.
    Launchable { profile: &'a str },
    /// Map line 372's remaining clause: what this launch could actually
    /// enter, ranked across every *enabled* configured launch profile rather
    /// than pinned to the one `--profile` or the implied fallback would have
    /// used. Used only when automatic routing is on and the person did not
    /// name a profile — `launch_session` decides which of the two
    /// `Launchable` shapes applies before it asks for either.
    ///
    /// Session warmth is filtered exactly as plain `Launchable` filters it —
    /// this is still a launch, not `glasshouse route`, and a launch cannot
    /// enter a Live session whichever profile ends up deciding the ranking.
    /// Only the *fresh* side widens: one candidate per enabled profile,
    /// exactly as `Everything` offers them, so the ranking has more than the
    /// one destination `Launchable` would have handed it.
    LaunchableAcrossProfiles,
}

/// Every place this project's next piece of work could go, and the current
/// destination when the caller is standing in one.
///
/// Ordered sessions-first, most recently active first, then one fresh
/// destination per configured launch profile; `SessionRouter::choose` uses the
/// caller's order as its tiebreaker, and "what you were most recently doing"
/// is the honest tiebreaker for equal scores.
pub(crate) fn routing_destinations(
    runtime: &Runtime,
    effective: &EffectiveConfig<'_>,
    harness: glasshouse::integrations::IntegrationId,
    scope: DestinationScope<'_>,
    task: Option<&str>,
) -> anyhow::Result<Vec<glasshouse::routing::session::Destination>> {
    use glasshouse::profile::BackendResource;
    use glasshouse::routing::session::{Destination, EstimatedInputSize, SessionContextFacts};

    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let quota_cache = glasshouse::provider::telemetry::GatewayQuotaCache::new(runtime.paths());
    let telemetry = glasshouse::provider::resources::GatheredTelemetry::new()
        .gather_gateway_quota(&quota_cache);

    // 56A step 3: the entitlement pool, resolved once for the whole set with
    // 56A-2's telemetry facets — the same sources `status_report` reads, in
    // the same fail-soft way. The ledger handle is opened, read and dropped
    // here, before the session store below opens its own (practice §65); a
    // ledger that cannot be opened leaves the throttling facet honestly
    // unknown rather than "none observed". A contradiction in the
    // `[entitlements]` tables stops the routing decision, exactly as the
    // per-destination lookup it replaces did — but a provider several
    // entries back is not a contradiction any more: it is line 1953's axis.
    let model_cache = glasshouse::provider::cache::ModelCache::new(runtime.paths());
    // Two reads from one handle, opened and dropped here (practice §65).
    // `observations_in_window` is the outcome-carrying set 56A's facets
    // classify; `consumption_in_window` is every row, which is what a burn
    // rate counts — see that method's own doc for why the two cannot be one
    // read. A ledger that cannot be opened leaves both honestly unknown.
    let (observations, consumption) = glasshouse::routing::evidence::EvidenceLedger::open(runtime)
        .and_then(|ledger| {
            let observations = ledger.observations_in_window(
                now_unix,
                glasshouse::routing::evidence::CLASSIFICATION_EVIDENCE_WINDOW_SECONDS,
            )?;
            let consumption = ledger.consumption_in_window(
                now_unix,
                glasshouse::routing::evidence::CLASSIFICATION_EVIDENCE_WINDOW_SECONDS,
            )?;
            Ok((observations, consumption))
        })
        .map_err(|err| {
            tracing::debug!(
                error = %err,
                "could not read the routing evidence ledger for the entitlement pool's facets"
            );
        })
        .ok()
        .map_or((None, None), |(observations, consumption)| {
            (Some(observations), Some(consumption))
        });
    let consumption = consumption.as_deref();
    // Map line 1519: priced spend against every provider's own configured
    // money budget, for `routing_entitlement`'s `budget_exhausted_for` below
    // — a third, separate ledger open (practice §65's "one open per read"),
    // fail-soft exactly as the pair above.
    let telemetry = match glasshouse::routing::evidence::EvidenceLedger::open(runtime) {
        Ok(ledger) => {
            let prices = glasshouse::provider::pricing::PriceTable::load_from_dir(
                runtime.paths().config_dir(),
            );
            telemetry.gather_budget_spend(&ledger, &prices, effective, now_unix)
        }
        Err(err) => {
            tracing::debug!(
                error = %err,
                "could not read the routing evidence ledger to count budget spend for routing"
            );
            telemetry
        }
    };
    let mut entitlement_telemetry = glasshouse::config::EntitlementTelemetry::new(now_unix)
        .with_gateway_quota(&quota_cache)
        .with_model_catalogues(&model_cache);
    if let Some(observations) = observations.as_deref() {
        entitlement_telemetry = entitlement_telemetry.with_observations(observations);
    }
    let pool: Vec<glasshouse::config::ResolvedEntitlement> = effective
        .entitlements()?
        .into_iter()
        .map(|entry| entry.with_telemetry(&entitlement_telemetry))
        .collect();
    // The same thresholds the entitlements status line renders bands with —
    // the plain user-configured set, so a band a person read there and the
    // band the router weighs cannot disagree.
    let band_thresholds = effective.capacity_band_thresholds().value;

    let mut destinations = Vec::new();

    // 1. The sessions this project already has.
    //
    // Read and released before the checkpoint store below is opened:
    // sequential handles, never two live ones (practice §65).
    let records = {
        let sessions = ProjectSessions::open(runtime)?;
        sessions.store().list()?
    };
    // Phase 36's producers (lines 1582–1586), read once for the whole set
    // rather than once per session: the sticky classification cache is one
    // file, the task's named paths are one function of one string, and the
    // checkpoint store is one handle — dropped before
    // `latest_checkpoint_quality` opens its own. Each arrives at the router as
    // a value read here, on the same terms `warm_session` and
    // `destination_capacity` already meet; `SessionContextFacts` says which
    // absences mean *unknown*.
    let sticky = crate::commands::routing_classification::ClassificationStickyCache::new(
        runtime.paths(),
        runtime.project().id().as_str(),
    )
    .load();
    let trimmed_task = task.map(str::trim).filter(|text| !text.is_empty());
    let task_named_paths = trimmed_task.map(glasshouse::routing::session::paths_named_in);
    let checkpoints = ProjectCheckpoints::open(runtime).ok();
    for record in records {
        // A session on another harness is not a destination for a launch that
        // has already selected this one, and `resume` reads the harness off
        // the record rather than ranking across them.
        if record.harness != harness.slug() {
            continue;
        }
        let Some(warm) = warm_session(&record, now_unix, scope) else {
            continue;
        };
        let context = SessionContextFacts::UNREAD
            .with_observed_compactions(record.observed_compactions)
            .with_last_task(
                sticky
                    .as_ref()
                    .filter(|sticky| sticky.session() == record.id.as_str())
                    .and_then(|sticky| sticky.classification()),
            )
            .with_touched_files(session_touched_files(checkpoints.as_ref(), &record.id))
            .with_task_named_paths(task_named_paths.clone());
        // Map line 1299: a cold resume's honest approximation is that
        // session's own latest checkpoint — `warm.state`'s `Resumable` arm
        // only. A `Live` session carries no estimate at all: `WarmSession`
        // already refuses to guess at its accumulated context, and this
        // estimate does not overturn that refusal.
        let estimated_size = match warm.state {
            glasshouse::config::pairing::WarmSessionState::Resumable => {
                EstimatedInputSize::UNESTIMATED.with_checkpoint_tokens(session_checkpoint_tokens(
                    checkpoints.as_ref(),
                    &record.id,
                ))
            }
            glasshouse::config::pairing::WarmSessionState::Live => EstimatedInputSize::UNESTIMATED,
        };
        // The profile the session actually ran under, re-resolved so that its
        // backend, model and protocol are read the same way a fresh
        // destination's are. A profile that has since been deleted or renamed
        // leaves the session itself perfectly resumable, so it falls back to
        // the harness's implied Native profile rather than dropping the
        // destination.
        let profile = record
            .launch_profile
            .as_deref()
            .and_then(|name| effective.launch_profile(name, harness).ok())
            .map(|layered| layered.value)
            .unwrap_or_else(|| glasshouse::profile::LaunchProfile::native(harness));
        let (backend, protocols, wire_protocol) =
            destination_backend(effective, &profile, record.model.clone());
        // Line 1516's producer, read before the backend is moved into the
        // destination — see `destination_tier_ceiling` for why it is read off
        // the backend's resolved model rather than off the profile.
        let query = destination_capability_query(harness, &profile.name, wire_protocol);
        let ceiling = destination_tier_ceiling(effective, &backend, &query);
        // Line 1923's producer, read off the same in-scope `backend` and
        // `consumption` the fresh destinations below use — a resumed session
        // is scored by the same local evidence as a fresh one under the same
        // provider and model.
        let pairing_prior_evidence = pairing_prior_evidence_count(consumption, &backend);
        // Lines 1351/1352/1542/1543/1544's producer, read off the same
        // in-scope `backend` and `consumption` the pairing-prior evidence
        // above just used.
        let route_responsiveness = route_responsiveness_for(consumption, &backend);
        // Phase 56 line 1954's producer: the entitlement this session's
        // profile charges, so a rule the user has since written applies to
        // continuing it exactly as it applies to starting a fresh one. When
        // SEVERAL pool entries back the session's provider, no record says
        // which account actually served it — the serving account of an
        // existing session is 56A-4's rebinding ground — so the destination
        // honestly carries none rather than a guess.
        let matches = pool_entitlements_for(&pool, harness, &profile.backend);
        let entitlement = match matches.as_slice() {
            [only] => Some(routing_entitlement(
                only,
                &band_thresholds,
                effective,
                &telemetry,
            )),
            _ => None,
        };
        destinations.push(
            with_capacity(
                with_provider_protocols(
                    Destination::existing(
                        record.id.as_str(),
                        harness,
                        profile.name.clone(),
                        backend,
                        warm,
                    ),
                    protocols,
                ),
                destination_capacity(&profile, effective, &telemetry, now_unix, consumption),
            )
            .with_tier_ceiling(ceiling)
            .with_capability_tier(ceiling)
            .with_session_context(context)
            .with_entitlement(entitlement)
            .with_estimated_input_size(estimated_size)
            .with_pairing_prior_evidence(pairing_prior_evidence)
            .with_route_responsiveness(route_responsiveness),
        );
    }
    drop(checkpoints);

    // 2. One fresh destination per *enabled* configured launch profile, each
    //    carrying what the most recent checkpoint would give it to boot from.
    //
    //    This is where "which launch profiles may the router consider" is
    //    decided, so it is where `ProfileConfig::enabled` is read — not
    //    inside `EffectiveConfig::profile_names`, which every listing surface
    //    also calls and which has to keep naming a disabled profile so a
    //    person can find it and turn it back on. See
    //    `EffectiveConfig::profile_enabled`'s own doc for that split.
    //
    //    Only the fresh destinations are filtered. The sessions above are
    //    deliberately untouched: disabling a profile says what may be
    //    *started*, and a session that already exists under it stays
    //    resumable — dropping it here would make an existing conversation
    //    unreachable, which is a heavier thing than a routing preference and
    //    is not what a person disabling a profile asked for.
    //
    //    The filtered set is never empty: `profile_names` always contains the
    //    implied Native profile and `profile_enabled` always answers `true`
    //    for it, by construction rather than by configuration.
    let checkpoint = latest_checkpoint_quality(runtime);
    // Map line 1304's fresh-session estimate: project memory and the
    // project's own latest checkpoint, each measured once and shared by
    // every fresh destination below — neither depends on which profile a
    // candidate runs under. Bootstrap context and likely repository reads
    // stay unset — see `EstimatedInputSize`'s own doc comment for why.
    let fresh_estimated_size = EstimatedInputSize::UNESTIMATED
        .with_project_memory_tokens(
            trimmed_task.and_then(|task| estimated_project_memory_tokens(runtime, task)),
        )
        .with_checkpoint_tokens(latest_checkpoint_tokens(runtime));
    let offered: Vec<String> = match scope {
        DestinationScope::Everything | DestinationScope::LaunchableAcrossProfiles => effective
            .profile_names()
            .into_iter()
            .filter(|name| effective.profile_enabled(name).value)
            .collect(),
        DestinationScope::Launchable { profile } => vec![profile.to_owned()],
    };
    for name in offered {
        let Ok(profile) = effective.launch_profile(&name, harness) else {
            // A profile configured for another harness is not a destination
            // for this launch. `launch_profile` already refuses that rather
            // than substituting, so the skip here is reading its answer.
            continue;
        };
        let profile = profile.value;
        let (backend, protocols, wire_protocol) = destination_backend(effective, &profile, None);
        let query = destination_capability_query(harness, &profile.name, wire_protocol);
        let capacity = destination_capacity(&profile, effective, &telemetry, now_unix, consumption);
        // Map line 1517's producer: model-declared resource facts, read only
        // for a `DirectProvider` destination whose model name is known — the
        // same narrowing `destination_backend`'s own `Cost::Free` lookup
        // above applies to `model_cost`. Every other destination (`Native`,
        // `GlasshouseGateway`, or a harness-default model with no name)
        // keeps `ResourceFacts::UNVERIFIED`, exactly what every destination
        // carried before this producer existed. Computed once here, before
        // the entitlement branch below, because the provider and model this
        // reads never change across `backend_for_entitlement`'s per-account
        // rebuild — only the credential does.
        let resource_facts = match &profile.backend {
            BackendResource::DirectProvider { provider } => backend
                .model()
                .name()
                .map(|model_name| effective.model_facts(provider, model_name).value)
                .unwrap_or(glasshouse::routing::capability::ResourceFacts::UNVERIFIED),
            _ => glasshouse::routing::capability::ResourceFacts::UNVERIFIED,
        };
        // 56A line 1953 — the entitlement axis. One entry backing this
        // profile's resource (or none) keeps exactly the single candidate,
        // and the id, this function has always built. Several entries
        // produce one candidate EACH: the same harness and profile ranked
        // across every account that may serve it, each candidate carrying
        // that account's own entitlement (rules and 56A-2 facets) and its
        // own credential reference. Nothing is pre-filtered by the rules
        // here — a denied entitlement's candidate is refused by name by the
        // router's own hard constraint, which already exists and must stay
        // the one place that decides.
        let matches = pool_entitlements_for(&pool, harness, &profile.backend);
        if matches.len() > 1 {
            for resolved in matches {
                let backend = backend_for_entitlement(&backend, resolved);
                let ceiling = destination_tier_ceiling(effective, &backend, &query);
                let pairing_prior_evidence = pairing_prior_evidence_count(consumption, &backend);
                let route_responsiveness = route_responsiveness_for(consumption, &backend);
                destinations.push(
                    with_capacity(
                        with_provider_protocols(
                            Destination::fresh(
                                entitled_fresh_destination_id(harness, &name, resolved.name()),
                                harness,
                                profile.name.clone(),
                                backend,
                                checkpoint,
                            ),
                            protocols.clone(),
                        ),
                        capacity.clone(),
                    )
                    .with_tier_ceiling(ceiling)
                    .with_capability_tier(ceiling)
                    .with_entitlement(Some(routing_entitlement(
                        resolved,
                        &band_thresholds,
                        effective,
                        &telemetry,
                    )))
                    .with_estimated_input_size(fresh_estimated_size)
                    .with_resource_facts(resource_facts)
                    .with_pairing_prior_evidence(pairing_prior_evidence)
                    .with_route_responsiveness(route_responsiveness),
                );
            }
        } else {
            let ceiling = destination_tier_ceiling(effective, &backend, &query);
            let pairing_prior_evidence = pairing_prior_evidence_count(consumption, &backend);
            let route_responsiveness = route_responsiveness_for(consumption, &backend);
            let fresh_entitlement = matches.first().map(|resolved| {
                routing_entitlement(resolved, &band_thresholds, effective, &telemetry)
            });
            destinations.push(
                with_capacity(
                    with_provider_protocols(
                        Destination::fresh(
                            fresh_destination_id(harness, &name),
                            harness,
                            profile.name.clone(),
                            backend,
                            checkpoint,
                        ),
                        protocols,
                    ),
                    capacity,
                )
                .with_tier_ceiling(ceiling)
                .with_capability_tier(ceiling)
                .with_entitlement(fresh_entitlement)
                .with_estimated_input_size(fresh_estimated_size)
                .with_resource_facts(resource_facts)
                .with_pairing_prior_evidence(pairing_prior_evidence)
                .with_route_responsiveness(route_responsiveness),
            );
        }
    }

    Ok(destinations)
}

/// Line 1923's producer: how many of `consumption`'s rows this backend's own
/// provider and model account for — the same `(provider, model)` identity
/// `record_routing_latency`'s siblings write into the ledger, matched the way
/// `observed_provider_health`'s `FreeResource::new` already matches a
/// destination's backend against a rendered key. `None` (no ledger) counts as
/// zero, exactly as `destination_capacity` already treats an absent
/// `consumption` read.
fn pairing_prior_evidence_count(
    consumption: Option<&[glasshouse::routing::evidence::RoutingObservation]>,
    backend: &glasshouse::routing::Backend,
) -> u32 {
    let Some(rows) = consumption else {
        return 0;
    };
    rows.iter()
        .filter(|row| row.provider == backend.provider() && row.model == backend.model().label())
        .count() as u32
}

/// Lines 1351/1352/1542/1543/1544's producer: this backend's own responsiveness
/// and reliability reading, over the same `consumption` rows and the same
/// `(provider, model)` filter [`pairing_prior_evidence_count`] already
/// applies. `None` when no ledger was read, matching that function's own
/// `consumption: None` case.
fn route_responsiveness_for(
    consumption: Option<&[glasshouse::routing::evidence::RoutingObservation]>,
    backend: &glasshouse::routing::Backend,
) -> Option<glasshouse::routing::evidence::RouteResponsiveness> {
    let rows: Vec<glasshouse::routing::evidence::RoutingObservation> = consumption?
        .iter()
        .filter(|row| row.provider == backend.provider() && row.model == backend.model().label())
        .cloned()
        .collect();
    Some(glasshouse::routing::evidence::RouteResponsiveness::from_observations(&rows))
}

/// A session record's warmth, or `None` when it is not a warm session at all.
///
/// `SessionDisposition` is what this is read off, exactly as
/// `config::pairing::WarmSessionState`'s own doc says: `Active` is `Live`,
/// `Resumable` is `Resumable`, and `Closed` or `Failed` are not warm sessions
/// and produce nothing rather than a third state.
fn warm_session(
    record: &SessionRecord,
    now_unix: i64,
    scope: DestinationScope<'_>,
) -> Option<glasshouse::config::pairing::WarmSession> {
    use glasshouse::config::pairing::{WarmSession, WarmSessionState};

    let state = match record.disposition() {
        SessionDisposition::Active if matches!(scope, DestinationScope::Everything) => {
            WarmSessionState::Live
        }
        // Live and unreachable from here — see `DestinationScope`.
        SessionDisposition::Active => return None,
        SessionDisposition::Resumable => WarmSessionState::Resumable,
        SessionDisposition::Closed | SessionDisposition::Failed => return None,
    };
    Some(WarmSession {
        state,
        idle_seconds: now_unix - record.last_activity_at,
    })
}

/// Line 1583's producer: the files a session's **own** latest checkpoint
/// lists — the handoff's `files` (the path part of each entry, before any
/// `::symbol` or note) and the working tree's changed files at capture.
///
/// `None` when the session has no checkpoint, which the router reads as
/// unknown; `Some(vec![])` when it has one that lists nothing. Read off
/// `CheckpointStore::latest_for`, the same reader `glasshouse checkpoint`
/// uses, and never off another session's checkpoint: a file touched by a
/// sibling session says nothing about this one.
///
/// `memory_files` (migration 17) is the other producer the map names, and
/// it is **not** read here: this build writes it and reads it nowhere, and a
/// reader is a query on `memories.source_session_id` that this package did
/// not add. When one exists its paths extend this list; the facet already
/// accepts any path set.
fn session_touched_files(
    checkpoints: Option<&ProjectCheckpoints>,
    session: &SessionId,
) -> Option<Vec<String>> {
    let stored = checkpoints?.store().latest_for(session).ok()??;
    let mut files: Vec<String> = stored
        .checkpoint
        .handoff
        .files
        .iter()
        .filter_map(|entry| {
            entry
                .split(|c: char| c.is_whitespace() || c == ':')
                .next()
                .filter(|path| !path.is_empty())
        })
        .map(str::to_owned)
        .collect();
    if let Some(tree) = &stored.checkpoint.working_tree {
        files.extend(tree.changed_files.iter().cloned());
    }
    files.sort();
    files.dedup();
    Some(files)
}

/// Map line 1299's cold-resume component: the rendered size of `session`'s
/// own latest checkpoint, project-scoped through [`ProjectCheckpoints`]
/// exactly as [`session_touched_files`] reads the same store. `None` when
/// there is no checkpoint store, or this session has never been check
/// pointed — the honest answer for a resume nothing measured, never `0`.
pub(crate) fn session_checkpoint_tokens(
    checkpoints: Option<&ProjectCheckpoints>,
    session: &SessionId,
) -> Option<u64> {
    let stored = checkpoints?.store().latest_for(session).ok()??;
    Some(glasshouse::firewall::estimate::estimate_tokens(
        &stored.checkpoint.render(),
    ))
}

/// Map line 1304's project-memory component of a fresh-session cost
/// estimate: [`glasshouse::firewall::estimate::estimate_tokens`] of the real
/// text [`glasshouse::memory::inject::briefing`] would inject for `task` —
/// measuring the actual injection rather than modeling it.
///
/// Nothing has been injected yet to skip: `glasshouse route`'s ranking is a
/// diagnostic over what WOULD be sent, not a delivery, so this reads with an
/// empty already-injected set on every call rather than a session's own
/// delivery history the way the control API's own memory-selection door
/// does (`api/unix.rs::select_memory`).
///
/// `None` — never `Some(0)` — whenever nothing was actually measured: the
/// store could not be opened, `briefing` itself failed, or `briefing` found
/// nothing to inject. All three degrade to "this component was not counted",
/// never "this component counts as zero" — only
/// [`glasshouse::routing::Cost::is_free`]'s zero is a fact this build is
/// certain of.
///
/// A [`glasshouse::memory::inject::BriefingOutcome::NothingMatched`] here is
/// map line 1865's retrieval miss and is recorded as one, at the `injection`
/// scope — `glasshouse route` is a diagnostic rather than a delivery, but the
/// search it runs is the same search a real launch would run, and a search
/// this project's own `glasshouse route` invocations run is real usage.
pub(crate) fn estimated_project_memory_tokens(runtime: &Runtime, task: &str) -> Option<u64> {
    use glasshouse::memory::ProjectMemory;
    use glasshouse::memory::inject::BriefingOutcome;

    let project = ProjectMemory::open(runtime).ok()?;
    // `None`: this is a diagnostic estimate (`glasshouse route`), never a
    // delivery, and reaching the rerank seat here would spend a real model
    // call on a number nobody asked to have reranked.
    let outcome = glasshouse::memory::inject::briefing(
        &project.store(),
        task,
        &std::collections::HashSet::new(),
        None,
        // The project root, so the estimate measures the block a real
        // briefing would deliver rather than one missing every
        // `freshness=` token — a token per file-aware row is real bytes,
        // and this function's whole job is to count them.
        Some(runtime.project().root()),
        // `None`: line 1129's confidence is read from the evaluation ledger
        // by a `Runtime`-holding caller, and this diagnostic estimate is
        // measuring the block a real briefing WOULD send, not deciding
        // whether one actually goes out — the real door (`select_memory`)
        // is where the withhold decision belongs.
        None,
    )
    .ok();
    // The memory connection is dropped before the evaluation ledger opens —
    // practice §65, the same shape `memory_search_grouped` uses — so a miss
    // recorded below never holds both handles at once.
    drop(project);

    match outcome {
        Some(BriefingOutcome::Injected(injection)) => Some(
            glasshouse::firewall::estimate::estimate_tokens(injection.text()),
        ),
        Some(BriefingOutcome::NothingMatched) => {
            glasshouse::evaluation::record_memory_retrieval_miss(
                runtime,
                glasshouse::evaluation::RetrievalScope::Injection,
                glasshouse::evaluation::now_unix(),
            );
            None
        }
        Some(BriefingOutcome::NothingNew)
        | Some(BriefingOutcome::WithheldLowConfidence(_))
        | None => None,
    }
}

/// Map line 1304's checkpoint component of a fresh-session cost estimate:
/// the rendered size of the project's own latest checkpoint — the same
/// document [`latest_checkpoint_quality`] reads its quality facts from,
/// measured rather than modeled. `None` when this project has no checkpoint
/// at all.
pub(crate) fn latest_checkpoint_tokens(runtime: &Runtime) -> Option<u64> {
    let checkpoints = ProjectCheckpoints::open(runtime).ok()?;
    let stored = checkpoints.store().latest().ok()??;
    Some(glasshouse::firewall::estimate::estimate_tokens(
        &stored.checkpoint.render(),
    ))
}

/// The backend a destination running `profile` would serve on, and every wire
/// protocol its provider offers.
///
/// Two returns rather than one because `Destination::with_provider_protocols`
/// is a builder step and an **empty** list is not the same as an absent one:
/// the constructor's default is the backend's own single protocol, and
/// overwriting that with an empty vector would make `ProtocolFit::Compatible`
/// unreachable and every non-native destination `Incompatible` — see
/// `routing::session`'s note on the field. `with_provider_protocols` below is
/// the one place that distinction is applied.
///
/// `recorded_model` is a recorded session's own assigned model, which is a
/// fact about that session and outranks re-deriving one from the profile.
///
/// `Cost` is the one fact that decides "premium" for the subscription-pressure
/// terms (`routing::pressure`, lines 1570–1575): a direct-provider profile
/// whose named model the user marked in that provider's `free_models` is
/// `Cost::Free`, through `ProviderConfig::cost_of` — the same rule
/// `disposable_candidates` and `gateway_upstream` already apply — and
/// everything else is `Cost::Metered`, the fail-closed value the rest of this
/// project uses when nobody has marked a model free. A native subscription
/// and the gateway are always metered here: a subscription is the premium
/// resource those lines are about, and the gateway's cost is whichever
/// upstream it is bound to, which this launch does not know yet.
fn destination_backend(
    effective: &EffectiveConfig<'_>,
    profile: &glasshouse::profile::LaunchProfile,
    recorded_model: Option<glasshouse::routing::AssignedModel>,
) -> (
    glasshouse::routing::Backend,
    Vec<glasshouse::harness::WireProtocol>,
    Option<glasshouse::harness::WireProtocol>,
) {
    use glasshouse::profile::BackendResource;
    use glasshouse::routing::{Backend, Cost, CredentialId};
    use glasshouse::secret::SecretRef;

    let pairing = session_pairing(effective, profile);
    let model = recorded_model.unwrap_or_else(|| pairing.model().clone());
    // Line 1482's own context: the wire protocol this pairing actually
    // resolved to, read once here and carried back to the caller — which has
    // the harness and launch profile already — rather than re-derived by
    // calling `session_pairing` a second time at each ceiling call site.
    let wire_protocol = pairing.route().protocol;
    let protocol = wire_protocol
        .map(|protocol| protocol.slug().to_owned())
        .unwrap_or_default();

    let (provider, credential, protocols, cost) = match &profile.backend {
        BackendResource::DirectProvider { provider } => {
            match effective.configured_provider(provider) {
                Ok(resolved) => {
                    let resolved = resolved.value;
                    // Line 1575's "zero-cost resource", from the one place
                    // the lookup lives. A model the harness picks itself is
                    // not a model anyone marked free.
                    let cost = model
                        .name()
                        .map(|name| effective.model_cost(provider, name).value)
                        .unwrap_or(Cost::Metered);
                    // Line 1595's input: every protocol the provider declares
                    // a usable base URL for, which is the same filter
                    // `EffectiveConfig::pairing_queries` applies for
                    // `glasshouse pairing`.
                    let protocols = resolved
                        .protocols
                        .iter()
                        .filter(|support| !support.base_url.is_empty())
                        .map(|support| support.protocol)
                        .collect();
                    // The first declared name, and a name only: which key of
                    // a pool serves is a routing decision one layer down, and
                    // resolving a value here would put a secret in a
                    // diagnostic's data path for nothing.
                    let reference = resolved
                        .credential_env
                        .first()
                        .map(|var| SecretRef::Environment { var: var.clone() })
                        .unwrap_or_else(|| SecretRef::Environment {
                            var: format!("{provider}(no credential configured)"),
                        });
                    (
                        provider.clone(),
                        CredentialId::new(provider.clone(), reference),
                        protocols,
                        cost,
                    )
                }
                // A profile naming a provider this configuration no longer
                // has is reported by `launch_profile` on the path that starts
                // a session; here it is a destination that scores on what is
                // known about it, which is its harness and its warmth.
                Err(_) => (
                    provider.clone(),
                    CredentialId::new(
                        provider.clone(),
                        SecretRef::Environment {
                            var: format!("{provider}(not configured)"),
                        },
                    ),
                    Vec::new(),
                    Cost::Metered,
                ),
            }
        }
        // A Native profile runs on the harness vendor's own sign-in. There is
        // no Glasshouse credential and inventing an environment variable for
        // one would be a lie in a report a person reads, so the credential
        // names the harness's own account — which is a name, like every other
        // `CredentialId`, and never a value.
        BackendResource::Native => (
            profile.harness.slug().to_owned(),
            CredentialId::new(
                profile.harness.slug(),
                SecretRef::OsCredential {
                    service: profile.harness.slug().to_owned(),
                    account: "the harness's own sign-in".to_owned(),
                },
            ),
            Vec::new(),
            Cost::Metered,
        ),
        // A gateway-backed profile is assigned its provider when the session
        // starts, so the serving provider genuinely is not known here — the
        // same answer `glasshouse pairing` gives for one.
        BackendResource::GlasshouseGateway => (
            "the Glasshouse gateway".to_owned(),
            CredentialId::new(
                "the Glasshouse gateway",
                SecretRef::OsCredential {
                    service: "glasshouse-gateway".to_owned(),
                    account: "assigned when the session starts".to_owned(),
                },
            ),
            Vec::new(),
            Cost::Metered,
        ),
    };

    (
        Backend::new(
            provider,
            protocol,
            model,
            credential,
            cost,
            pairing.tool_semantics(),
        )
        .with_tools_evidence(pairing.tool_evidence()),
        protocols,
        wire_protocol,
    )
}

/// The sink `launch_session` and the resume path hand their gateway —
/// **capability map line 1851**'s one production caller.
///
/// # Why a closure over a `Runtime` and not a ledger
///
/// `crate::gateway` has never had a database in scope and must not gain one:
/// `gateway::session::FailoverPreventionSink`'s own doc comment records that
/// this is what keeps that module incapable of reaching a project's files.
/// The closure carries a [`Runtime`] — cheap, `Clone`, three paths — and
/// opens the evaluation ledger inside
/// [`glasshouse::evaluation::record_failover_prevention`] at the one moment a
/// failover has actually been taken, which is practice §65's rule that a
/// resource is acquired where its consumer starts and not a connection held
/// for the life of a session that may never fail over at all.
///
/// The row is written from the gateway's own exchange thread, so nothing on
/// the person's path waits for it, and a ledger that cannot be opened costs
/// the observation rather than the exchange.
pub(crate) fn failover_prevention_sink(
    runtime: &Runtime,
) -> glasshouse::gateway::session::FailoverPreventionSink {
    let runtime = runtime.clone();
    std::sync::Arc::new(
        move |effect: &glasshouse::routing::interactive::FailureDomainEffect| {
            let prevention = if effect.prevented() {
                glasshouse::evaluation::FailoverPrevention::Prevented
            } else {
                glasshouse::evaluation::FailoverPrevention::NotPrevented
            };
            glasshouse::evaluation::record_failover_prevention(
                &runtime,
                prevention,
                effect.displaced(),
                glasshouse::evaluation::now_unix(),
            );
            // Capability map line 1852: the route the *measured* correlation
            // steered this failover off — one that looked independent by
            // provider and was not by observation. Its own row in the
            // routing ledger, because that is where the observations it was
            // derived from live and where `glasshouse route` reads it back.
            if let Some(route) = effect.correlation_displaced() {
                record_correlation_steer(&runtime, route, glasshouse::evaluation::now_unix());
            }
        },
    )
}

/// Capability map line 1852's producer: one `routing_observations` row
/// under [`glasshouse::routing::evidence::CORRELATION_PURPOSE`] per failover
/// the correlation term steered, naming the route it steered off.
///
/// The row is an observation *about* `displaced` — its `provider` and
/// `model` — and nothing else: no outcome, no failure class, no harness and
/// no tokens, so every reader keyed on an exchange having happened ignores
/// it by construction (see the purpose constant's own doc comment), and
/// [`glasshouse::routing::evidence::correlate_routes`] never reads it back
/// as evidence for the correlation that produced it. Best-effort for the
/// same reason `record_failover_prevention` is: a ledger that cannot be
/// opened costs the measurement, never the failover.
pub(crate) fn record_correlation_steer(
    runtime: &Runtime,
    displaced: &glasshouse::routing::evidence::RouteIdentity,
    now_unix: i64,
) {
    let ledger = match glasshouse::routing::evidence::EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::debug!(
                error = %err,
                "routing evidence ledger unavailable; a correlation-steered failover is not \
                 recorded"
            );
            return;
        }
    };
    let row = glasshouse::routing::evidence::NewObservation::new(
        displaced.provider.clone(),
        displaced.model.clone(),
    )
    .with_purpose(Some(glasshouse::routing::evidence::CORRELATION_PURPOSE))
    .with_timing(Some(now_unix), Some(now_unix));
    if let Err(err) = ledger.record(row, now_unix) {
        tracing::debug!(error = %err, "could not record a correlation-steered failover");
    }
}

/// Apply line 1595's protocol list, and only when there is one.
///
/// See `destination_backend`: an empty list would *remove* the constructor's
/// default rather than add to it, and §4.1 of the router's own report records
/// that dropping this step is what makes every non-native destination
/// `Incompatible` instead of scored.
fn with_provider_protocols(
    destination: glasshouse::routing::session::Destination,
    protocols: Vec<glasshouse::harness::WireProtocol>,
) -> glasshouse::routing::session::Destination {
    if protocols.is_empty() {
        destination
    } else {
        destination.with_provider_protocols(protocols)
    }
}

/// Line 1598's input, read from the same on-disk quota cache
/// `glasshouse resources` reads and with no request of its own — and, from
/// the same reading, lines 1570–1574's: the band that score falls in and how
/// far off the next reset is.
///
/// The band is resolved exactly as [`disposable_candidate_capacity`] resolves
/// it for the disposable router: the user's thresholds (line 1270) with the
/// provider's own protected reserve percentage applied (line 1288) — which is
/// what makes the pressure policy tunable rather than fixed (line 1612). A
/// native subscription and the gateway are not keys in the provider table, so
/// they take the global thresholds, the same answer
/// `provider::resources::capacity_band_thresholds_for` gives them.
///
/// Both halves are `None` for a provider with no cached reading, and every
/// pressure term is inert on that and says so.
fn destination_capacity(
    profile: &glasshouse::profile::LaunchProfile,
    effective: &EffectiveConfig<'_>,
    telemetry: &glasshouse::provider::resources::GatheredTelemetry,
    now_unix: i64,
    consumption: Option<&[glasshouse::routing::evidence::RoutingObservation]>,
) -> (
    Option<glasshouse::provider::quota::RemainingCapacityScore>,
    glasshouse::routing::pressure::CapacityFacts,
    Option<glasshouse::routing::burn::ExhaustionForecast>,
) {
    use glasshouse::profile::BackendResource;
    use glasshouse::provider::registry::ResourceKind;
    use glasshouse::routing::pressure::CapacityFacts;

    let kind = match &profile.backend {
        BackendResource::Native => ResourceKind::NativeSubscription {
            harness: profile.harness,
        },
        BackendResource::DirectProvider { provider } => {
            ResourceKind::from_direct_provider(provider.clone())
        }
        BackendResource::GlasshouseGateway => ResourceKind::GlasshouseGateway,
    };
    let state =
        glasshouse::provider::resources::observed_capacity(&kind, effective, telemetry, now_unix);
    let score = state.remaining_capacity_score();
    let thresholds = effective.capacity_band_thresholds().value;
    let thresholds = match &kind {
        ResourceKind::DirectProvider { provider, .. } => {
            thresholds.with_resource_reserve(effective.reserve_percent(provider).value.get())
        }
        _ => thresholds,
    };
    let band = score.as_ref().map(|score| score.band(&thresholds));
    let seconds_until_reset = state.seconds_until_reset(now_unix);
    let facts = CapacityFacts::new(band, seconds_until_reset);
    // **Line 1280's producer.** The forecast is resolved from the same
    // `CapacityState` the band and reset came from — its own remaining
    // *request* pool, never the percentage — and from the ledger rows the
    // caller already read. `None` at every step that is not established, and
    // there are four of them:
    //
    // - the caller read no ledger (`consumption` is `None`);
    // - this resource is not a `[providers.*]` key, so no row's `provider`
    //   column names it. A native subscription and the gateway both reach
    //   this arm: rows say `glasshouse` or the upstream provider, and
    //   inventing a join from a harness name to a provider name is exactly
    //   the mismatch this package was told to stop at rather than paper over;
    // - `glasshouse::routing::burn::forecast` itself answers `None` — too
    //   few rows, no measured request-unit remaining amount, a zero rate.
    //
    // `quota_context` is `None` here on purpose: a launch profile names a
    // resource, not one of that resource's credentials, so the honest key is
    // the provider-wide one. `burn_rate` reports that choice back as
    // `account_narrowed: false` rather than letting a caller mistake a
    // provider total for one account's.
    let forecast = match (&kind, consumption) {
        (ResourceKind::DirectProvider { provider, .. }, Some(rows)) => {
            glasshouse::routing::burn::forecast(
                rows,
                glasshouse::routing::burn::ResourceKey {
                    provider,
                    quota_context: None,
                },
                state.requests().remaining(),
                now_unix,
                seconds_until_reset,
            )
        }
        _ => None,
    };
    (score, facts, forecast)
}

/// Map line 1482's own context, built from exactly what `routing_destinations`
/// has in hand for a destination: the harness it is iterating, the launch
/// profile's own name, and the wire protocol [`destination_backend`]
/// resolved. One place this is assembled so the three call sites in
/// `routing_destinations` cannot state it three different ways.
fn destination_capability_query(
    harness: glasshouse::integrations::IntegrationId,
    launch_profile: &str,
    protocol: Option<glasshouse::harness::WireProtocol>,
) -> glasshouse::config::capability::CapabilityQuery<'_> {
    glasshouse::config::capability::CapabilityQuery {
        harness: Some(harness),
        launch_profile: Some(launch_profile),
        protocol,
    }
}

/// **Map line 1516's missing producer**, and the reason the tier gate stops
/// being inert on the shipped binary: the highest workload tier this
/// destination's model is established to serve, as the user configured it
/// (`providers.<p>.model_ceilings`, map line 1796, or a Phase 34F capability
/// record scoped to `query`).
///
/// Read off the [`glasshouse::routing::Backend`] rather than from the
/// profile, because the backend is where the *resolved* model lives — a
/// recorded session's own assigned model outranks re-deriving one, and
/// `destination_backend` has already applied that rule. Reading the profile
/// again here would give a warm session the ceiling of the model it *would*
/// be started with rather than the one it is actually running.
///
/// `query` is `routing_destinations`' own launch context — harness, launch
/// profile, and the wire protocol `destination_backend` resolved — which is
/// map line 1482's closing half: a capability record scoped to one of those
/// axes reaches exactly the destinations it applies to, through
/// [`glasshouse::config::EffectiveConfig::model_ceiling_for`], rather than
/// staying inert to every context-bearing caller.
///
/// `None` — no ceiling established, which the router never reads as a
/// refusal — in three honest cases, none of them a guess:
///
/// - the harness picked its own model ([`AssignedModel::HarnessDefault`]),
///   so there is no model identifier to look a ceiling up by;
/// - the destination's provider is not a `[providers.*]` key at all, which
///   is every native subscription and the gateway — a ceiling is a statement
///   about a named model on a named provider, and inventing one for a
///   resource the user never configured is exactly what
///   `ProviderConfig::cost_of` refuses to do for cost;
/// - the provider is configured and this model is simply not in its map.
fn destination_tier_ceiling(
    effective: &EffectiveConfig<'_>,
    backend: &glasshouse::routing::Backend,
    query: &glasshouse::config::capability::CapabilityQuery<'_>,
) -> Option<glasshouse::routing::classify::WorkloadTier> {
    backend.model().name().and_then(|model| {
        effective
            .model_ceiling_for(backend.provider(), model, query)
            .value
    })
}

/// Attach [`destination_capacity`]'s three halves to a destination.
fn with_capacity(
    destination: glasshouse::routing::session::Destination,
    (score, facts, forecast): (
        Option<glasshouse::provider::quota::RemainingCapacityScore>,
        glasshouse::routing::pressure::CapacityFacts,
        Option<glasshouse::routing::burn::ExhaustionForecast>,
    ),
) -> glasshouse::routing::session::Destination {
    destination
        .with_capacity(score)
        .with_capacity_facts(facts)
        .with_burn_forecast(forecast)
}

/// **Line 1599's bridge**: what a gateway has actually observed about these
/// destinations' resources, in the shape `provider_health` reads.
///
/// A read of [`glasshouse::provider::telemetry::GatewayHealthCache`], which is
/// [`destination_capacity`]'s own cost and its sibling directory under the
/// same `--data-dir` — no network, no subprocess, no credential, and **no
/// handle kept**: `load_all` reads the files and returns owned values, so
/// nothing here is still open when this function returns (practice §65, which
/// was paid for by a database handle opened on a path nobody was asserting
/// about).
///
/// An empty pool when the cache is empty. That is the same inert `0.0`
/// contribution for every destination this path produced before the bridge
/// existed, and it is correct: an absent reading is an absent contribution,
/// never an invented one.
///
/// # Hazard 1 — identity, which is what makes this a design and not a wiring
///
/// [`glasshouse::routing::free::FreeResource`] is keyed by a
/// [`glasshouse::routing::CredentialId`]; a persisted
/// [`glasshouse::provider::telemetry::GatewayHealthReading`] carries only the
/// **rendered** `credential_label`. That rendering is not reversible —
/// `CredentialId::label` prints `provider/var` for a `SecretRef::Environment`
/// and `provider/service:account` for a `SecretRef::OsCredential`, so a parse
/// would have to guess both where the provider ends and which variant it was
/// looking at, and a guess here does not weaken the policy, it inverts it
/// (map line 1294): the router would avoid a healthy resource on another's
/// evidence.
///
/// **So nothing here parses a label.** The consumer already tells us the key
/// it will look up — `provider_health` builds
/// `FreeResource::new(destination.backend().credential().clone(),
/// destination.backend().model().label())` — and both of those are in hand
/// here, before `choose` is called. This walks the *destinations* and renders
/// each one's label with the very function the write side rendered it with
/// (`gateway::session::SessionRouting::health_readings_for` calls
/// `credential().label()`, and `model_key` is `AssignedModel::label`). The
/// match is string equality between two calls of one renderer, in the forward
/// direction only.
///
/// # GH-POOL-ALLOWANCE — the allowance half, beside the health half
///
/// This is also where `FreePool::allowance` gets a value instead of
/// answering `unknown_pool()` for every credential. For each destination's
/// provider, the same [`glasshouse::provider::resources::observed_capacity`]
/// [`destination_capacity`] already calls is asked again, from a freshly
/// gathered [`glasshouse::provider::resources::GatheredTelemetry`] — the same
/// cheap, local, no-network read `routing_destinations` performs per call,
/// never shared with it because nothing here outlives one call (Hazard 1's
/// own reasoning applies again: cheap enough to redo, too easy to get wrong
/// to smuggle across a boundary). Its own remaining-requests reading, when
/// the provider published one, becomes `FreePool::record_pool` — the
/// provider's own numbers, nothing derived. Absent that, a `pricing.toml`
/// entry for the pair, for a destination the user has not marked free, is
/// `FreePool::declare_token_priced`. Neither: `unknown_pool()`, exactly as
/// before this package.
///
/// Three things it therefore refuses to do:
///
/// - **attribute across providers.** The provider whose file a reading came
///   from must be the credential's own provider. Two providers configured
///   with the same `credential_env` variable are *"two separate allowances"*
///   (`CredentialId`'s own doc) and share nothing; the label keeps them apart
///   because the provider is part of it, and this check keeps a mislabelled
///   file from getting around that.
/// - **attribute across models.** Health is per credential *and* model —
///   `FreeResource`'s own doc says a router sharing one entry across a
///   provider's models would take every model out of service because one was
///   busy.
/// - **choose between two readings that name the same resource and disagree.**
///   A file this program wrote cannot contain those, because
///   `health_readings_for` maps over a pool already keyed by resource. A file
///   it did not write can, and it is also the shape a genuine label collision
///   would take — two distinct credentials rendering one label, which is
///   exactly the ambiguity that must not be resolved by picking. Contradictory
///   readings leave the resource unobserved.
///
/// # Hazard 2 — the time base
///
/// [`glasshouse::provider::telemetry::GatewayHealthReading::cooling_down_until`]
/// does the conversion and documents it. Both clocks are read **once**, here,
/// so every reading in one cache is placed against the same pair rather than
/// against a clock that moved between them.
pub(crate) fn observed_provider_health(
    runtime: &Runtime,
    effective: &EffectiveConfig<'_>,
    destinations: &[glasshouse::routing::session::Destination],
) -> ObservedHealth {
    use glasshouse::provider::registry::ResourceKind;
    use glasshouse::provider::resources::{GatheredTelemetry, observed_capacity};
    use glasshouse::provider::telemetry::GatewayQuotaCache;
    use glasshouse::routing::free::{FreeResource, PoolReading};

    let mut health = observed_health_of(
        runtime,
        destinations.iter().map(|destination| {
            FreeResource::new(
                destination.backend().credential().clone(),
                destination.backend().model().label(),
            )
        }),
    );

    // GH-POOL-ALLOWANCE, this function's own doc section above: the same
    // telemetry `routing_destinations` gathers for `destination_capacity`,
    // re-read here because nothing survives from that call to this one, and
    // the same price table `session_router` loads for `expected_marginal_cost`.
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let now = std::time::Instant::now();
    let telemetry =
        GatheredTelemetry::new().gather_gateway_quota(&GatewayQuotaCache::new(runtime.paths()));
    let price_table =
        glasshouse::provider::pricing::PriceTable::load_from_dir(runtime.paths().config_dir());
    // Capability map line 1366's *learn* half: read once for every
    // destination this call covers, fail-soft exactly like every other
    // ledger open on this path — an unreadable ledger simply leaves the
    // learner with nothing to learn from, the same honest "no cadence is
    // known" a fresh project already reads today.
    let throttle_rows = glasshouse::routing::evidence::EvidenceLedger::open(runtime)
        .ok()
        .and_then(|ledger| {
            ledger
                .observations_in_window(
                    now_unix,
                    glasshouse::routing::evidence::CLASSIFICATION_EVIDENCE_WINDOW_SECONDS,
                )
                .ok()
        })
        .unwrap_or_default();

    for destination in destinations {
        let backend = destination.backend();
        let credential = backend.credential();
        let provider = backend.provider();
        let kind = ResourceKind::from_direct_provider(provider);
        let state = observed_capacity(&kind, effective, &telemetry, now_unix);

        if let Some(remaining) = state.requests().remaining().reading() {
            // The provider's own numbers, nothing derived: `limit` and
            // `resets_in` are each `None` on their own if the provider did
            // not also publish them, exactly as `PoolReading`'s own doc
            // requires.
            let limit = state
                .requests()
                .limit()
                .reading()
                .and_then(|reading| u32::try_from(reading.value().value()).ok());
            let remaining = u32::try_from(remaining.value().value()).ok();
            // Reused, never guessed: the same reset `destination_capacity`
            // hands `CapacityFacts` and the burn forecast, converted to a
            // duration only when it has not already passed.
            let resets_in = state
                .seconds_until_reset(now_unix)
                .filter(|seconds| *seconds > 0)
                .map(|seconds| std::time::Duration::from_secs(seconds as u64));
            // Capability map line 1366: a stated window always wins, and a
            // learned one is only ever asked for when neither a window nor a
            // reset was stated — `is_exhausted` would otherwise have nothing
            // to reason from once `remaining` reaches zero.
            let stated_window = telemetry.stated_pool_window(provider);
            let (window, resets_in) = if resets_in.is_none() && stated_window.is_none() {
                match glasshouse::routing::free::learned_window(&throttle_rows, provider, now_unix)
                {
                    Some((window, last_throttle)) => {
                        let seconds = match window {
                            glasshouse::routing::free::Window::Learned { seconds, .. } => seconds,
                            glasshouse::routing::free::Window::Stated { .. } => {
                                unreachable!("learned_window only ever returns Window::Learned")
                            }
                        };
                        let candidate = last_throttle + i64::from(seconds) - now_unix;
                        let resets_in = (candidate > 0)
                            .then(|| std::time::Duration::from_secs(candidate as u64));
                        (Some(window), resets_in)
                    }
                    None => (None, resets_in),
                }
            } else {
                (stated_window, resets_in)
            };
            health.pool.record_pool(
                credential,
                &PoolReading {
                    limit,
                    remaining,
                    resets_in,
                    window,
                },
                now,
            );
        } else if let Some(model) = backend.model().name()
            && effective.model_cost(provider, model).value == glasshouse::routing::Cost::Metered
            && price_table.price_for(provider, model).is_some()
        {
            health.pool.declare_token_priced(credential);
        }
    }

    health
}

/// The pool the router is handed, and **when each adopted reading was
/// written** — capability map line 1854's *stale* half.
///
/// # Why the age travels beside the pool rather than inside it
///
/// [`glasshouse::routing::free::FreePool`] is the router's own input type and
/// its health entries carry no observation time — see
/// [`routing_evidence_for`]'s own header, and
/// [`glasshouse::evaluation::EvaluationKind::RoutingEvidenceObserved`]'s. The
/// age is not a routing input: nothing in the ranking reads it, and adding it
/// to `FreePool` would put a field in the policy's input that the policy must
/// not use. It is a fact about the *evidence a decision was made with*, which
/// is what this ledger records and nothing else.
///
/// The age is per **provider file**, which is per reading:
/// [`glasshouse::provider::telemetry::GatewayHealthCache::load_all_dated`]'s
/// own doc comment has why those are the same number here.
pub(crate) struct ObservedHealth {
    pub(crate) pool: glasshouse::routing::free::FreePool,
    /// Every resource adopted into `pool`, with the unix second its file was
    /// written. A `Vec` rather than a map because it is walked once per
    /// routed destination and holds one entry per configured destination.
    pub(crate) observed_at: Vec<(glasshouse::routing::free::FreeResource, i64)>,
}

impl ObservedHealth {
    pub(crate) fn pool(&self) -> &glasshouse::routing::free::FreePool {
        &self.pool
    }

    /// When the reading this pool holds for `resource` was written, or
    /// [`None`] when it holds none.
    ///
    /// There is no third answer: a file that could not be dated was never
    /// loaded at all (`load_all_dated` skips it, as it skips a truncated
    /// one), so a resource in `pool` always has a date and a resource with
    /// no date is not in `pool`. That is what makes *"a reading whose age is
    /// unknown is `absent`, never fresh"* structural rather than a rule
    /// somebody has to remember.
    pub(crate) fn observed_at(
        &self,
        resource: &glasshouse::routing::free::FreeResource,
    ) -> Option<i64> {
        self.observed_at
            .iter()
            .find(|(candidate, _)| candidate == resource)
            .map(|(_, at)| *at)
    }
}

/// The persisted gateway-health readings that name any of `resources`, as a
/// [`glasshouse::routing::free::FreePool`].
///
/// This is [`observed_provider_health`]'s whole body, keyed by the type that
/// function already built internally, so a second caller with resources in a
/// different shape reads the same cache under the same three refusals rather
/// than growing a second matcher that could disagree with this one. The
/// caller supplies the keys because only the caller knows them — see
/// [`observed_provider_health`]'s own header for why nothing here parses a
/// label.
///
/// The second caller is [`automatic_classification_choice`], whose keys are
/// [`glasshouse::routing::disposable::DisposableCandidate`]s rather than
/// destinations. Without it, `glasshouse classify` handed
/// `DisposableRouting::choose` an empty pool, and a filter that is never fed
/// a candidate that could fail it is not applied (practice §36).
pub(crate) fn observed_health_of(
    runtime: &Runtime,
    resources: impl IntoIterator<Item = glasshouse::routing::free::FreeResource>,
) -> ObservedHealth {
    use glasshouse::provider::telemetry::{GatewayHealthCache, GatewayHealthReading};
    use glasshouse::routing::free::FreePool;

    let mut pool = FreePool::new();
    let mut observed_at = Vec::new();
    // `load_all_dated` rather than `load_all`: the same three refusals below,
    // plus the unix second each provider's file was written, which line
    // 1854's *stale* half is read from. A file with no date fails to
    // deserialize and never reaches this loop.
    let stored = GatewayHealthCache::new(runtime.paths()).load_all_dated();
    if stored.is_empty() {
        return ObservedHealth { pool, observed_at };
    }

    // Hazard 2: one pair, read together, for every reading below.
    let now = std::time::Instant::now();
    let now_unix = glasshouse::provider::cache::now_unix_seconds();

    for resource in resources {
        let credential = resource.credential();
        let label = credential.label();
        let model = resource.model().to_owned();

        let mut named: Option<(&GatewayHealthReading, i64)> = None;
        let mut contradicted = false;
        for (reading, written_at) in stored
            .iter()
            .filter(|(provider, _, _)| provider == credential.provider())
            .flat_map(|(_, written_at, readings)| {
                readings.iter().map(move |reading| (reading, *written_at))
            })
            .filter(|(reading, _)| reading.credential_label == label && reading.model == model)
        {
            match named {
                None => named = Some((reading, written_at)),
                // Two entries saying the same thing are one reading written
                // twice, not a disagreement. The file dates may still differ
                // — the same reading persisted twice — and the comparison is
                // deliberately of the reading alone, so a duplicate does not
                // become a contradiction because two files were written a
                // second apart.
                Some((first, _)) if first == reading => {}
                Some(_) => {
                    contradicted = true;
                    break;
                }
            }
        }
        let Some((reading, written_at)) = named.filter(|_| !contradicted) else {
            continue;
        };

        pool.adopt_observed(
            &resource,
            reading.consecutive_failures,
            reading.cooling_down_until(now, now_unix),
            reading.cooldown_cause,
            reading.credential_rejected,
        );
        observed_at.push((resource, written_at));
    }

    ObservedHealth { pool, observed_at }
}

/// What the most recent checkpoint would give a fresh session to boot from —
/// line 1600's bootstrap half.
///
/// `None` when this project has no checkpoint at all, which is the honest
/// answer and the one `switching_and_bootstrap_cost` prices as "would start
/// from nothing". Never an error: a checkpoint store that cannot be opened
/// must cost a routing input rather than the command.
fn latest_checkpoint_quality(
    runtime: &Runtime,
) -> Option<glasshouse::routing::session::CheckpointQuality> {
    use glasshouse::routing::session::CheckpointQuality;

    let checkpoints = ProjectCheckpoints::open(runtime).ok()?;
    let stored = checkpoints.store().latest().ok()??;
    Some(CheckpointQuality::new(
        !stored.checkpoint.handoff.next_actions.is_empty(),
        !stored.checkpoint.trimmed,
    ))
}

/// The user's override, from the two flags every routing caller takes.
///
/// Line 1602 is *"allow the user to override every automatic routing
/// choice"*, and the word that makes it checkable is "every": the same two
/// flags mean the same thing on `route`, on `launch` and on `run`, so a
/// person who read the diagnostic can paste the identifier straight into the
/// command that acts.
pub(crate) fn routing_override(
    to: Option<&str>,
    fresh: bool,
) -> glasshouse::routing::session::RoutingOverride {
    use glasshouse::routing::session::RoutingOverride;

    match (to, fresh) {
        (Some(id), _) => RoutingOverride::to(id),
        (None, true) => RoutingOverride::fresh(),
        (None, false) => RoutingOverride::none(),
    }
}

/// How far back `glasshouse route`'s outcome section looks.
///
/// Thirty days, comfortably inside
/// [`glasshouse::evaluation::Retention`]'s ninety, so the window is one a
/// ledger that has been pruning can still answer. A person reading a
/// recommendation wants recent behaviour; a longer window would average this
/// month's routes with a configuration two months dead.
pub(crate) const ROUTE_OUTCOME_WINDOW_DAYS: i64 = 30;

/// The session router with the user's reserve configuration attached —
/// lines 1571 and 1577 (`routing.reserve.interactive`) and line 1290
/// (`routing.reserve_override_sessions`) on every path that ranks, so the
/// path that acts and the path that reports cannot disagree about either.
pub(crate) fn session_router(
    runtime: &Runtime,
    effective: &EffectiveConfig<'_>,
    user_override: glasshouse::routing::session::RoutingOverride,
) -> glasshouse::routing::session::SessionRouter {
    glasshouse::routing::session::SessionRouter::with_override(user_override)
        .with_reserve_policies(effective.reserve_policies())
        .with_reserve_override_sessions(effective.reserve_override_sessions().value)
        // Map lines 1294 and 1610, read HERE for the same reason as the
        // score weights below: this is the one constructor every real
        // ranking goes through, so the path that acts and the path that
        // reports cannot disagree about whether a task was declared nearly
        // complete. The set comes from the project's own store and holds
        // only declarations that are inside their horizon and whose session
        // is still live; a project that has declared nothing yields an empty
        // set, which is what every ranking saw before this line had a
        // producer.
        .with_declared_task_progress(crate::commands::sessions::declared_task_progress_sessions(
            runtime,
        ))
        // Map lines 1357/1358: the configured score weights are read HERE,
        // in the one constructor every real ranking goes through. Until
        // 2026-09-02 this line did not exist: `[routing.score_weights]`
        // parsed, layered and round-tripped correctly and changed no routing
        // decision, because nothing ever handed the resolved value to the
        // router (`with_score_weights` had zero callers of any kind). An
        // audit's tripwire through this constructor found it; that test is
        // now the acceptance test below.
        .with_score_weights(effective.score_weights().value)
        // Map lines 1305/1306: the price metadata is read HERE, from the one
        // function every ranking path already goes through, so a user's
        // `pricing.toml` reaches the path that acts and the path that reports
        // alike. An absent or malformed file yields `PriceTable::empty()` and
        // routing behaves exactly as it did before the table existed — the
        // state of every user who has not written one.
        .with_price_table(glasshouse::provider::pricing::PriceTable::load_from_dir(
            runtime.paths().config_dir(),
        ))
        // Map line 1952: the harness-efficiency summary is read HERE too, in
        // the same one constructor, so `route_recommendation` and
        // `launch_session` — and every other caller of this function — see
        // the same evidence `harness_efficiency_section` prints in
        // `glasshouse route`'s report. A ledger this build cannot open, or
        // one with no rows, yields an empty summary, which the router treats
        // as inert — the ranking every caller saw before this term existed.
        .with_harness_efficiency(harness_efficiency_summary(runtime))
        // Map line 1301: the comparable-output window is read HERE too, in
        // the same one constructor, from the same evidence ledger and the
        // same window `routing_destinations`'s own burn reading uses
        // (`CLASSIFICATION_EVIDENCE_WINDOW_SECONDS`). A ledger this build
        // cannot open, or one with no rows for a class, yields no comparable
        // reading for it, which `expected_marginal_cost` renders as an
        // honest *unmeasured* — the ranking every caller saw before this
        // term existed.
        .with_comparable_output_tokens(comparable_output_tokens(runtime))
}

/// Map line 1301's reader, at the same site
/// [`harness_efficiency_summary`] reads from: the routing evidence ledger's
/// own window, reduced to what [`glasshouse::routing::session::
/// expected_marginal_cost`] needs — a median output-token size per task
/// class, never a raw row.
///
/// Fail-soft like every ledger read on the launch path (`routing_destinations`'s
/// own `consumption_in_window` read is the same shape): a ledger that cannot
/// be opened, or a window read that fails, costs this estimate and nothing
/// else — never the launch.
pub(crate) fn comparable_output_tokens(
    runtime: &Runtime,
) -> Vec<glasshouse::routing::burn::ClassOutput> {
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let Ok(ledger) = glasshouse::routing::evidence::EvidenceLedger::open(runtime) else {
        return Vec::new();
    };
    let Ok(rows) = ledger.consumption_in_window(
        now_unix,
        glasshouse::routing::evidence::CLASSIFICATION_EVIDENCE_WINDOW_SECONDS,
    ) else {
        return Vec::new();
    };
    glasshouse::routing::burn::output_tokens_by_class(
        &rows,
        now_unix,
        glasshouse::routing::evidence::CLASSIFICATION_EVIDENCE_WINDOW_SECONDS,
    )
}

/// Map line 1952's reader — the same producer and window
/// `harness_efficiency_section` prints (map line 1951), reduced to what a
/// routing decision needs: per-(harness, task class) success counts, not the
/// token or wall-clock figures that section also renders for a person.
fn harness_efficiency_summary(
    runtime: &Runtime,
) -> glasshouse::routing::session::HarnessEfficiencySummary {
    use glasshouse::evaluation::EvaluationObservations;
    use glasshouse::routing::session::HarnessEfficiencySummary;

    let to = glasshouse::evaluation::now_unix();
    let from = to - ROUTE_OUTCOME_WINDOW_DAYS * 24 * 60 * 60;

    let Ok(ledger) = EvaluationObservations::open(runtime) else {
        return HarnessEfficiencySummary::empty();
    };
    let Ok(outcomes) = ledger.outcomes_by_tier_and_harness(from, to) else {
        return HarnessEfficiencySummary::empty();
    };
    HarnessEfficiencySummary::from_outcomes(&outcomes)
}

/// The pairing this session's launch profile answers to — Phase 9J's
/// question, asked on the launch path so a session can record the answer.
///
/// # It goes through `pairing_queries`, which is what `glasshouse pairing`
/// prints from
///
/// A second construction of the same `PairingQuery` here would be a second
/// place for the provider lookup, the protocol fallback and the tool-call
/// declaration to be wrong, and the two would eventually disagree about the
/// same profile — one of them on screen and the other in the database, where
/// nobody would see it. So the configured profiles are asked by name.
///
/// # The one profile that is not in that list
///
/// The implied Native profile exists for every harness by construction rather
/// than by configuration, so `pairing_queries` deliberately omits it — see
/// its own doc comment. For that profile the question has one honest answer
/// and it needs no lookup: it names no model and no provider, so nothing
/// establishes a relationship, and the classifier is still the thing that
/// says so rather than a constant written here.
pub(crate) fn session_pairing(
    effective: &EffectiveConfig<'_>,
    profile: &glasshouse::profile::LaunchProfile,
) -> glasshouse::harness::pairing::Pairing {
    use glasshouse::harness::Declared;
    use glasshouse::harness::pairing::{PairingQuery, ServingRoute, classify};
    use glasshouse::routing::AssignedModel;

    let overrides = effective.pairing_overrides();
    let configured = effective
        .pairing_queries()
        .into_iter()
        .find(|configured| configured.name() == profile.name)
        .and_then(|configured| configured.query().cloned());

    let query = configured.unwrap_or_else(|| PairingQuery {
        harness: profile.harness,
        model: match &profile.model {
            Some(model) => AssignedModel::named(model),
            None => AssignedModel::HarnessDefault,
        },
        route: ServingRoute {
            provider: None,
            gateway: None,
            protocol: profile.expected_protocol,
        },
        tool_calls: Declared::Unverified,
        provider_protocols: Vec::new(),
    });

    classify(&query, &overrides)
}
