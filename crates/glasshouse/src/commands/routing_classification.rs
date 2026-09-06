//! `commands::routing_classification` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use glasshouse::Runtime;
use glasshouse::config::{self, EffectiveConfig, ProjectConfig, UserConfig};

/// What one routing decision classified the work as, and the facts that
/// answer was conditioned on — Phase 34D's answer beside Phase 34E's
/// fingerprint. `None` from [`classify_for_routing`] when no task was
/// stated, which is every launch and every `route` that reproduces the
/// pre-classification behaviour byte for byte.
pub(crate) struct ClassifiedRouting {
    pub(crate) answer: glasshouse::routing::request::RouterAnswer,
    pub(crate) fingerprint: glasshouse::routing::request::RoutingFingerprint,
}

/// Everything [`classify_for_routing`] needs to build the router request
/// from what its caller already holds — never a file, a transcript, an
/// environment variable or a credential, which is map lines 1425, 1426,
/// 1455 and 1456 made structural (see `routing::request`'s header).
pub(crate) struct RoutingClassificationSite<'a> {
    /// `--task`. Absent or blank means "classify nothing".
    pub(crate) task: Option<&'a str>,
    pub(crate) moment: glasshouse::routing::session::RoutingMoment,
    /// The harness this decision is for. `None` for `glasshouse route`,
    /// which ranks across every enabled harness.
    pub(crate) harness: Option<glasshouse::integrations::IntegrationId>,
    /// Whether the person named the harness on the command line (line
    /// 1450's "pinned harness") rather than letting the one enabled harness
    /// be selected.
    pub(crate) harness_named: bool,
    pub(crate) to: Option<&'a str>,
    pub(crate) fresh: bool,
    pub(crate) destinations: &'a [glasshouse::routing::session::Destination],
    pub(crate) health: &'a glasshouse::routing::free::FreePool,
    /// The sticky record to consult for line 1467. `Some` on the path that
    /// acts; `None` on the path that reports, which never reuses.
    pub(crate) sticky: Option<&'a ClassificationStickyCache>,
    /// Line 1469's text-keyed cache. `Some` on the path that acts; `None` on
    /// the path that reports — the same reason `sticky` is `None` there:
    /// `route`'s own comment says it always asks rather than reusing, and a
    /// diagnostic that answers from yesterday's cache is not a diagnostic.
    pub(crate) text_cache: Option<&'a ClassificationTextCache>,
    /// Capability map line 1419: the per-token price of the destination this
    /// launch lands on when classification does nothing — `Some` only on
    /// the path that acts (`launch_session`), and only when that launch's
    /// own fresh destination names a priced backend. `None` everywhere else,
    /// including every report: there is no chosen launch profile to protect.
    pub(crate) protected_capacity_price: Option<glasshouse::provider::pricing::ModelPrice>,
}

/// Deterministic heuristics' answer for `text`, with the reason they answered.
///
/// The one producer of a heuristic [`RouterAnswer`] in this binary, called
/// on every path that ends up not asking a model — no routing model
/// configured (line 1471), an explicit destination (line 1470), or a model
/// that did not answer — so those three paths cannot classify differently.
pub(crate) fn heuristic_answer(
    text: &str,
    reason: glasshouse::routing::request::HeuristicReason,
) -> glasshouse::routing::request::RouterAnswer {
    use glasshouse::routing::request::{AnswerProvenance, RouterAnswer};

    RouterAnswer::new(
        glasshouse::routing::classify::classify_heuristically(text),
        AnswerProvenance::Heuristic(reason),
    )
}

/// Returns `None` when no task was stated: no request is built, no model is
/// asked, no ledger is opened, and the caller hands the router
/// `TaskRequirements::default()` exactly as it did before this existed.
///
/// 1. **An explicit destination is deterministic** (line 1470). `--to` or
///    `--fresh` decides; heuristics classify for the explanation only and
///    no routing model is asked.
/// 2. **No routing model configured → heuristics** (line 1471). Everything
///    downstream — the tier gate, the capability terms, the explanation —
///    works on that answer exactly as it would on a model's.
/// 3. **A low-risk answer for the same sticky session is reused** (line
///    1467), and only when nothing it was conditioned on has changed (line
///    1468): the sticky record's own `reuse_for` is the whole rule.
/// 4. **Otherwise the routing model is asked**, through the same
///    `classify_with_routing_model` `glasshouse classify` uses, with the
///    rendered [`RouterRequest`] as the request text. A model that does not
///    answer usably falls back to heuristics and says so on stderr, exactly
///    as `glasshouse classify` does.
// History: design-decisions.md, "Trims: commands module docs, third packet", routing_classification.rs `classify_for_routing`.
pub(crate) fn classify_for_routing(
    runtime: &Runtime,
    effective: &EffectiveConfig<'_>,
    site: RoutingClassificationSite<'_>,
) -> Option<ClassifiedRouting> {
    use glasshouse::config::RoutingModelResolution;
    use glasshouse::routing::request::{
        AnswerProvenance, HeuristicReason, RouterAnswer, RouterRequest, RoutingFingerprint,
        UserConstraints, WarmSessionFact,
    };

    let text = site.task.map(str::trim).filter(|text| !text.is_empty())?;
    let bands = destination_bands(effective, site.destinations);
    let fingerprint = RoutingFingerprint::new(
        site.harness,
        &bands,
        site.health
            .observed()
            .into_iter()
            .map(|(resource, _)| resource.label()),
    );
    let constraints = UserConstraints::none()
        .with_pinned_harness(site.harness.filter(|_| site.harness_named))
        .with_destination(site.to)
        .with_fresh(site.fresh)
        .with_forbidden_providers(forbidden_providers(runtime, effective));
    let request = RouterRequest::new(text, site.moment)
        .with_warm_session(WarmSessionFact::among(site.destinations))
        .with_capacity(bands)
        .with_constraints(constraints);

    let resolution = effective.routing_model_resolution().value;
    let resolution_tag = classification_cache_resolution_tag(&resolution);

    let answer = if request.constraints().is_deterministic() {
        heuristic_answer(text, HeuristicReason::DeterministicOverride)
    } else {
        match resolution {
            RoutingModelResolution::Heuristics(_) => {
                heuristic_answer(text, HeuristicReason::NoRoutingModel)
            }
            RoutingModelResolution::Pinned { .. } | RoutingModelResolution::Automatic => {
                let reused = site.sticky.and_then(|cache| {
                    let record = cache.load()?;
                    match record.reuse_for(&fingerprint, site.destinations) {
                        Ok(classification) => {
                            let previously = classification.source().to_string();
                            Some(RouterAnswer::new(
                                classification,
                                AnswerProvenance::Reused {
                                    session: record.session().to_owned(),
                                    previously,
                                },
                            ))
                        }
                        Err(refusal) => {
                            tracing::debug!(
                                %refusal,
                                "the previous classification does not stand; asking the routing \
                                 model"
                            );
                            None
                        }
                    }
                });
                match reused {
                    Some(answer) => answer,
                    None => {
                        // Line 1469, read side: a normalised-text hit stands
                        // in for the model ask below when it is reusable —
                        // never below `Confidence::Low`, the same
                        // fingerprint, the same routing-model identity, and
                        // recorded recently. `resolution_tag` is `None` for
                        // `Automatic` (see `classification_cache_resolution_tag`),
                        // which keeps this lookup out of the arm entirely
                        // rather than risk serving one model's answer as
                        // another's.
                        let text_key = glasshouse::routing::request::normalised_task_key(text);
                        let text_cached = resolution_tag.as_deref().and_then(|tag| {
                            site.text_cache.and_then(|cache| {
                                let record = cache.lookup(&text_key)?;
                                let now = glasshouse::provider::cache::now_unix_seconds();
                                if !record.is_reusable_for(now, &fingerprint, tag) {
                                    return None;
                                }
                                let classification = record.classification()?;
                                let previously = classification.source().to_string();
                                Some(RouterAnswer::new(
                                    classification,
                                    AnswerProvenance::ReusedFromCache { previously },
                                ))
                            })
                        });
                        match text_cached {
                            Some(answer) => answer,
                            None => match classify_with_routing_model(
                                runtime,
                                &request,
                                site.protected_capacity_price,
                            ) {
                                ClassificationAttempt::NotConfigured => {
                                    heuristic_answer(text, HeuristicReason::NoRoutingModel)
                                }
                                ClassificationAttempt::Answered(classification) => {
                                    let provenance =
                                        AnswerProvenance::of_source(classification.source());
                                    // Line 1469, write side: only a real
                                    // model answer is worth remembering,
                                    // exactly the same rule
                                    // `remember_classification` applies to
                                    // the sticky cache.
                                    if let (Some(cache), Some(tag)) =
                                        (site.text_cache, resolution_tag.as_deref())
                                    {
                                        cache.store(
                                            glasshouse::routing::request::CachedClassification::new(
                                                text_key.clone(),
                                                fingerprint.clone(),
                                                tag,
                                                &classification,
                                                glasshouse::provider::cache::now_unix_seconds(),
                                            ),
                                        );
                                    }
                                    RouterAnswer::new(classification, provenance)
                                }
                                ClassificationAttempt::Failed(why) => {
                                    eprintln!(
                                        "glasshouse: {why}; deterministic heuristics answered \
                                         instead"
                                    );
                                    heuristic_answer(text, HeuristicReason::ModelFailed(why))
                                }
                            },
                        }
                    }
                }
            }
        }
    };
    Some(ClassifiedRouting {
        answer,
        fingerprint,
    })
}

/// Line 1469's routing-model identity, for the text-keyed cache: the model
/// label for a [`RoutingModelResolution::Pinned`] resolution — known without
/// asking anything, since a pin already names the exact model — and `None`
/// for [`RoutingModelResolution::Automatic`] and
/// [`RoutingModelResolution::Heuristics`].
///
/// `Automatic` is deliberately excluded rather than tagged with whichever
/// model last answered: the recon this package closes (`GH-RECON-1469`)
/// notes that automatic selection can differ call to call for the same
/// text, and the only way to know *which* model would currently answer is
/// [`automatic_classification_choice`] — a stateful, side-effecting local
/// pick (it writes `RoutingStickyCache`) that this cache has no business
/// calling just to decide whether to skip a lookup. So an `Automatic`
/// classification is never served from this cache; `Pinned`'s identity is
/// free, and is the case this cache actually saves a call for.
/// `Heuristics` never reaches the arm that would call this at all.
pub(crate) fn classification_cache_resolution_tag(
    resolution: &glasshouse::config::RoutingModelResolution,
) -> Option<String> {
    use glasshouse::config::RoutingModelResolution;

    match resolution {
        RoutingModelResolution::Pinned { provider, model } => {
            Some(format!("pinned:{provider}/{model}"))
        }
        RoutingModelResolution::Heuristics(_) | RoutingModelResolution::Automatic => None,
    }
}

/// Line 1449's producer: one capacity **band** per candidate provider, read
/// off the quota reading `routing_destinations` already attached to each
/// destination and banded with the same thresholds `glasshouse resources`
/// and the disposable router use — never the reading itself.
fn destination_bands(
    effective: &EffectiveConfig<'_>,
    destinations: &[glasshouse::routing::session::Destination],
) -> Vec<glasshouse::routing::request::ProviderBand> {
    use glasshouse::routing::request::ProviderBand;

    let mut seen = std::collections::BTreeSet::new();
    let mut bands = Vec::new();
    for destination in destinations {
        let provider = destination.backend().provider();
        if !seen.insert(provider.to_owned()) {
            continue;
        }
        let band = destination.capacity().map(|score| {
            let thresholds = effective
                .capacity_band_thresholds()
                .value
                .with_resource_reserve(effective.reserve_percent(provider).value.get());
            score.band(&thresholds)
        });
        bands.push(ProviderBand::new(provider, band));
    }
    bands
}

/// Line 1450's "forbidden providers": every configured provider the person
/// has disabled. The one way this configuration can forbid a provider today;
/// a provider that is merely absent is not forbidden, it is unknown.
///
/// Best-effort on a configuration that will not load — an empty list and a
/// log line — because the request is being built for a decision the caller
/// has already loaded that configuration for once.
fn forbidden_providers(runtime: &Runtime, effective: &EffectiveConfig<'_>) -> Vec<String> {
    let (Ok(user), Ok(project)) = (
        UserConfig::load(runtime.paths()),
        config::load_project_config(runtime.project()),
    ) else {
        tracing::debug!("could not re-read configuration for forbidden providers");
        return Vec::new();
    };
    effective
        .provider_names()
        .into_iter()
        .filter(|name| {
            project
                .as_ref()
                .and_then(|p| p.providers().get(name))
                .or_else(|| user.providers().get(name))
                .is_some_and(|provider| !provider.enabled())
        })
        .collect()
}

/// Where the previous decision's classification is kept between launches —
/// map line 1467's memory, project-scoped for the same reason
/// [`glasshouse::provider::telemetry::RoutingStickyCache`] is, and in its
/// shape: one JSON file, written to a temporary name and renamed, and every
/// read failure answering `None` rather than an error.
pub(crate) struct ClassificationStickyCache {
    path: std::path::PathBuf,
}

impl ClassificationStickyCache {
    pub(crate) fn new(paths: &glasshouse::paths::RuntimePaths, project_id: &str) -> Self {
        Self {
            path: paths
                .project_state_dir(project_id)
                .join("routing-classification.json"),
        }
    }

    pub(crate) fn load(&self) -> Option<glasshouse::routing::request::StickyClassification> {
        let bytes = std::fs::read(&self.path).ok()?;
        glasshouse::routing::request::StickyClassification::from_json(&bytes)
    }

    pub(crate) fn store(&self, record: &glasshouse::routing::request::StickyClassification) {
        let attempt = (|| -> std::io::Result<()> {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let encoded = record
                .to_json()
                .map_err(|err| std::io::Error::other(err.to_string()))?;
            glasshouse::provider::cache::write_json_atomically(&self.path, &encoded)
        })();
        if let Err(err) = attempt {
            tracing::debug!(error = %err, "could not persist the routing classification");
        }
    }
}

/// The most entries [`ClassificationTextCache`] keeps. Past this, the oldest
/// recorded entry is dropped before a new one is written — a small, named
/// cap rather than a file that grows for as long as a project is worked in.
pub(crate) const CLASSIFICATION_TEXT_CACHE_CAPACITY: usize = 64;

/// Where line 1469's text-keyed cache is kept — the same project-scoped
/// directory as [`ClassificationStickyCache`] and
/// [`glasshouse::provider::telemetry::RoutingStickyCache`], and the same
/// file shape, except the record is a map keyed by
/// [`glasshouse::routing::request::normalised_task_key`] rather than a
/// single value: one JSON file, written to a temporary name and renamed,
/// every read failure answering an empty cache rather than an error.
pub(crate) struct ClassificationTextCache {
    path: std::path::PathBuf,
}

impl ClassificationTextCache {
    pub(crate) fn new(paths: &glasshouse::paths::RuntimePaths, project_id: &str) -> Self {
        Self {
            path: paths
                .project_state_dir(project_id)
                .join("routing-classification-cache.json"),
        }
    }

    pub(crate) fn load(
        &self,
    ) -> std::collections::BTreeMap<String, glasshouse::routing::request::CachedClassification>
    {
        std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// The record for `key`, if one is on disk. Every gate beyond "a record
    /// exists" is [`glasshouse::routing::request::CachedClassification::is_reusable_for`]'s,
    /// not this method's.
    pub(crate) fn lookup(
        &self,
        key: &str,
    ) -> Option<glasshouse::routing::request::CachedClassification> {
        self.load().remove(key)
    }

    pub(crate) fn store(&self, record: glasshouse::routing::request::CachedClassification) {
        let mut entries = self.load();
        entries.insert(record.key().to_owned(), record);
        while entries.len() > CLASSIFICATION_TEXT_CACHE_CAPACITY {
            let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, record)| record.recorded_at_unix())
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            entries.remove(&oldest);
        }
        let attempt = (|| -> std::io::Result<()> {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let encoded = serde_json::to_vec_pretty(&entries)
                .map_err(|err| std::io::Error::other(err.to_string()))?;
            glasshouse::provider::cache::write_json_atomically(&self.path, &encoded)
        })();
        if let Err(err) = attempt {
            tracing::debug!(error = %err, "could not persist the classification text cache");
        }
    }
}

/// What `routing_observations.purpose` records for a memory-extraction call
/// — capability map line 1832. Aliased from the ledger's own constant for
/// [`CLASSIFICATION_PURPOSE`]'s reason.
pub(crate) const EXTRACTION_PURPOSE: &str = glasshouse::routing::evidence::EXTRACTION_PURPOSE;

/// Phase 21 line 834's consent: the cheap or local model the user actually
/// chose, when they chose one.
///
/// # This field is the whole of the consent, and it is the default
///
/// This is `Some` only when
/// [`glasshouse::config::EffectiveConfig::memory_extraction_model`] names a
/// provider and model — a field that is `None` until a person writes it. A
/// user who has configured providers, free models, routing preferences and
/// nothing else gets `None` here and therefore exactly today's behaviour:
/// [`disposable_extraction_model`] chooses a resource, says so, and calls
/// nothing.
///
/// **What consent does not decide is *which* resource serves.** Once it is
/// given, [`disposable_extraction_model`] puts the named model into the
/// candidate set beside the user's own free ones and lets
/// `DisposableRouting::choose` rank them — line 530's *prefer free models
/// when quality is sufficient*, on the path that actually spends something.
// History: design-decisions.md, "Trims: commands module docs, third packet", routing_classification.rs `configured_extraction_choice`.
fn configured_extraction_choice(
    effective: &EffectiveConfig<'_>,
) -> Option<glasshouse::config::ExtractionModelRef> {
    effective.memory_extraction_model().value
}

/// A configured provider whose credential variables resolve from neither the
/// native secure store nor this process's environment — map line 488's
/// consequence for a hook. Names only: nothing here reads a value past
/// [`glasshouse::secret::SecretStore::is_present`].
pub(crate) struct WithheldCredential {
    pub(crate) provider: String,
    pub(crate) vars: Vec<String>,
}

/// Every configured provider — or the one `only` names — whose credential
/// variables all fail to resolve through `secrets`.
///
/// Invariant (map line 488): the harness child, and every hook it runs, no
/// longer inherits a provider key the launching shell exported, so a hook
/// that finds no credential says which provider and which variable rather
/// than falling silent. A provider naming no variable needs none and is
/// never listed; an entry that does not resolve contributes nothing, since
/// nothing can read a credential through it either.
pub(crate) fn withheld_provider_credentials(
    effective: &EffectiveConfig<'_>,
    secrets: &dyn glasshouse::secret::SecretStore,
    only: Option<&str>,
) -> Vec<WithheldCredential> {
    use glasshouse::secret::SecretRef;

    effective
        .provider_names()
        .into_iter()
        .filter(|name| only.is_none_or(|only| only == name))
        .filter_map(|name| {
            effective
                .configured_provider(&name)
                .ok()
                .map(|layered| (name, layered.value.credential_env))
        })
        .filter(|(_, vars)| {
            !vars.is_empty()
                && !vars
                    .iter()
                    .any(|var| secrets.is_present(&SecretRef::Environment { var: var.clone() }))
        })
        .map(|(provider, vars)| WithheldCredential { provider, vars })
        .collect()
}

/// The one sentence both hooks print when [`withheld_provider_credentials`]
/// found something, or `None`. It says what Glasshouse withholds and why
/// (map line 488), names each provider and its variables, and gives the
/// store instruction — a value never enters this function.
pub(crate) fn withheld_credential_notice(withheld: &[WithheldCredential]) -> Option<String> {
    if withheld.is_empty() {
        return None;
    }
    let listed: Vec<String> = withheld
        .iter()
        .map(|entry| {
            format!(
                "provider `{}`'s credential ({}) resolves from neither this hook's environment \
                 nor the native secure store — {}",
                entry.provider,
                entry.vars.join(", "),
                glasshouse::integrations::store_credential_instruction(&entry.vars[0]),
            )
        })
        .collect();
    Some(format!(
        "Glasshouse withholds provider credentials from the harness and the hooks it runs (map \
         line 488), so a key exported only in the launching shell no longer reaches memory \
         extraction or the context-firewall reducer: {}",
        listed.join("; ")
    ))
}

/// The provider behind a name the user's own configuration holds, resolved
/// through the layering rule every other reader applies — project winning
/// over user.
///
/// Every failure is `None` after one log line: an unreadable provider, one
/// that is not in the table, a disabled one, or a template that does not
/// resolve is a choice that cannot produce a call, and never a guess at a
/// correction.
fn configured_provider(
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    provider_name: &str,
    subject: &str,
) -> Option<glasshouse::provider::Provider> {
    let Some(provider_config) = project
        .and_then(|p| p.providers().get(provider_name))
        .or_else(|| user.providers().get(provider_name))
    else {
        tracing::warn!(
            provider = provider_name,
            subject,
            "names a provider this project has not configured"
        );
        return None;
    };
    if !provider_config.enabled() {
        tracing::warn!(
            provider = provider_name,
            subject,
            "names a disabled provider"
        );
        return None;
    }
    match provider_config.to_provider(provider_name) {
        Ok(provider) => Some(provider),
        Err(err) => {
            tracing::warn!(error = %err, subject, "the provider does not resolve");
            None
        }
    }
}

/// # `None` is not a refusal, it is *not expressible as a candidate*
///
/// A [`glasshouse::routing::disposable::DisposableCandidate`] carries a
/// [`glasshouse::routing::CredentialId`], which carries a
/// [`glasshouse::secret::SecretRef`], and there is no honest `SecretRef` for
/// a provider that names no credential variable at all. That is the **local**
/// case — a runner on loopback, which `ConfiguredModel::new` builds without
/// one and which line 834 names first — and it is why
/// [`disposable_extraction_model`] keeps a bypass for exactly it. Nothing is
/// lost by not routing a local model: line 530 prefers *free* resources, and
/// a model running on the user's own machine has no marginal cost to prefer
/// something else over.
///
/// The cost is [`glasshouse::config::ProviderConfig::cost_of`] — the user's
/// own marking, never a guess — so a named model that is also in the
/// provider's free list is `Free`, and one nobody marked is `Metered` and is
/// gated by Phase 32F's protected reserve exactly like any other metered
/// candidate.
// History: design-decisions.md, "Trims: commands module docs, third packet", routing_classification.rs `configured_extraction_candidate`.
fn configured_extraction_candidate(
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    effective: &EffectiveConfig<'_>,
    chosen: &glasshouse::config::ExtractionModelRef,
    secrets: &dyn glasshouse::secret::SecretStore,
    telemetry: &glasshouse::provider::resources::GatheredTelemetry,
    now_unix: i64,
) -> Option<glasshouse::routing::disposable::DisposableCandidate> {
    use glasshouse::routing::CredentialId;
    use glasshouse::routing::disposable::DisposableCandidate;
    use glasshouse::secret::SecretRef;

    let provider_config = project
        .and_then(|p| p.providers().get(chosen.provider()))
        .or_else(|| user.providers().get(chosen.provider()))?;
    if !provider_config.enabled() {
        return None;
    }
    // The first variable that actually resolves, the same order
    // `disposable_candidates` walks. A provider that names none is the local
    // case and is not expressible here at all.
    let reference = provider_config
        .credential_env()
        .iter()
        .map(|var| SecretRef::Environment { var: var.clone() })
        .find(|reference| secrets.resolve(reference).is_some())?;

    let capacity = disposable_candidate_capacity(chosen.provider(), effective, telemetry, now_unix);
    let locality =
        glasshouse::provider::registry::ResourceKind::from_direct_provider(chosen.provider())
            .locality();
    let entitlement = match effective.entitlement_for_provider(chosen.provider()) {
        Ok(entitlement) => entitlement.map(|entitlement| entitlement.to_routing()),
        Err(err) => {
            tracing::warn!(
                provider = chosen.provider(),
                error = %err,
                "the [entitlements] tables could not be resolved; the configured extraction \
                 model is ranked with no entitlement rule"
            );
            None
        }
    };

    Some(
        DisposableCandidate::new(
            chosen.provider().to_owned(),
            chosen.model().to_owned(),
            CredentialId::new(chosen.provider().to_owned(), reference),
            provider_config.cost_of(chosen.model()),
        )
        .with_capacity(capacity)
        .with_locality(locality)
        .with_entitlement(entitlement),
    )
}

/// The local, credential-less half of line 834: build the model the user
/// named directly, because it cannot be expressed as a routing candidate.
///
/// See [`configured_extraction_candidate`] for why that is a fact about
/// [`glasshouse::routing::CredentialId`] rather than a preference, and why
/// line 530 has nothing to prefer here.
fn configured_extraction_model(
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    chosen: &glasshouse::config::ExtractionModelRef,
) -> Option<Box<dyn glasshouse::memory::ExtractionModel>> {
    match extraction_client_for(user, project, chosen.provider(), chosen.model(), None) {
        Ok(model) => Some(Box::new(model)),
        Err(reason) => {
            tracing::warn!(reason, "the configured extraction model cannot be used");
            None
        }
    }
}

/// Build the extraction client for `provider`/`model`, or say in one sentence
/// why it cannot be built.
///
/// [`classification_model`]'s exact shape, for extraction's own job name:
/// both turn a provider name and a model name into a real
/// [`glasshouse::memory::ConfiguredModel`] after something else has already
/// decided them.
///
/// `credential` is the reference to resolve when the caller already knows
/// which one applies — `DisposableRouting`'s choice names the exact
/// `SecretRef` that resolved when its candidate was built, and re-deriving it
/// here could pick a different one. `None` is the local case, where nobody
/// has resolved anything and the first variable that resolves wins; a
/// provider that names none needs none, and `ConfiguredModel::new` builds it
/// without one.
fn extraction_client_for(
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    provider_name: &str,
    model_name: &str,
    credential: Option<&glasshouse::secret::SecretRef>,
) -> Result<glasshouse::memory::ConfiguredModel, String> {
    use glasshouse::memory::ConfiguredModel;
    use glasshouse::secret::{SecretRef, SecretStore as _};

    let provider = configured_provider(user, project, provider_name, "the extraction model")
        .ok_or_else(|| {
            format!("the extraction model names `{provider_name}`, which this project cannot use")
        })?;

    let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();
    let credential = match credential {
        Some(reference) => secrets.resolve(reference),
        None => provider
            .credential_env
            .iter()
            .find_map(|var| secrets.resolve(&SecretRef::Environment { var: var.clone() })),
    };

    ConfiguredModel::new(&provider, model_name, credential)
        .map_err(|err| format!("the extraction model cannot be used: {err}"))
}

/// What one routed support job learned about the resource that served it,
/// made durable for the next process that dispatches one — Phase 9I line
/// 534's other half, across a process boundary.
///
/// # The merge, and the one thing it costs
///
/// [`glasshouse::provider::telemetry::GatewayHealthCache::store`] replaces a
/// provider's whole file, and its other producer (the gateway) writes a
/// snapshot of its entire live pool at one instant. This producer holds
/// **one** resource, so it reads the file, replaces the entry for that
/// resource and writes every other entry back untouched — never dropping
/// readings this process happens not to have.
///
/// What that costs is the file's date: `observed_at_unix` is per file, so a
/// carried-forward entry is re-dated to now and reads as fresher than it
/// earned (map line 1854's *stale* half). The alternative is discarding it
/// outright, which is worse, and the deadline that actually gates scheduling
/// is an absolute unix second on the entry itself and is unaffected.
// History: design-decisions.md, "Trims: commands module docs, third packet", routing_classification.rs `persist_support_work_health`.
fn persist_support_work_health(
    paths: &glasshouse::paths::RuntimePaths,
    resource: &glasshouse::routing::free::FreeResource,
    outcome: glasshouse::routing::free::WorkloadOutcome,
) {
    use glasshouse::provider::telemetry::{GatewayHealthCache, GatewayHealthReading};
    use glasshouse::routing::free::FreePool;

    let cache = GatewayHealthCache::new(paths);
    let provider = resource.credential().provider().to_owned();
    let label = resource.credential().label();
    let model = resource.model().to_owned();

    let mut entries: Vec<GatewayHealthReading> = cache
        .load_all()
        .into_iter()
        .find(|(name, _)| *name == provider)
        .map(|(_, entries)| entries)
        .unwrap_or_default();
    // Found once and reused for both the seed and the write-back, so the two
    // can never disagree about which entry this resource's is.
    let existing = entries
        .iter()
        .position(|entry| entry.credential_label == label && entry.model == model);

    // One pair, read together, for both directions of the conversion —
    // `observed_health_of`'s hazard 2, and it applies to the write side for
    // the same reason.
    let now = std::time::Instant::now();
    let now_unix = glasshouse::provider::cache::now_unix_seconds();

    // Seeded from what is already on disk so `consecutive_failures`
    // accumulates across processes rather than restarting at one every time —
    // which is the whole of what makes `FAILURES_BEFORE_COOLDOWN` mean
    // anything to a dispatcher that lives for a second.
    let mut pool = FreePool::new();
    if let Some(stored) = existing.map(|index| &entries[index]) {
        pool.adopt_observed(
            resource,
            stored.consecutive_failures,
            stored.cooling_down_until(now, now_unix),
            stored.cooldown_cause,
            stored.credential_rejected,
        );
    }
    pool.observe(resource, outcome, now);

    let health = pool.health(resource);
    let reading = GatewayHealthReading {
        credential_label: label.clone(),
        model: model.clone(),
        consecutive_failures: health.consecutive_failures(),
        cooling_down_until_unix: health
            .cooling_down_until()
            .map(|until| now_unix + until.saturating_duration_since(now).as_secs() as i64),
        cooldown_cause: health.cooldown_cause(),
        credential_rejected: health.credential_was_rejected(),
    };
    match existing {
        Some(index) => entries[index] = reading,
        None => entries.push(reading),
    }

    cache.store(&provider, &entries, now_unix);
}

/// # The order, and what each step decides
///
/// 1. **Consent** ([`configured_extraction_choice`]): no `[memory]
///    extraction_model`, no outbound request, exactly as before. The routing
///    decision is still made, still explained and still recorded.
/// 2. **The local bypass**: a configured provider that names no credential
///    variable cannot be a routing candidate at all — see
///    [`configured_extraction_candidate`] — and is built and used directly.
/// 3. **The choice**: every free and metered candidate the configuration
///    yields, plus the configured extraction model, ranked by
///    `DisposableRouting::choose` against health read back off disk.
/// 4. **The client**: resolved for the resource that won, by
///    [`extraction_client_for`], through the same `SecretStore` path
///    everything else here uses.
// History: design-decisions.md, "Trims: commands module docs, third packet", routing_classification.rs `disposable_extraction_model`.
pub(crate) fn disposable_extraction_model(
    runtime: &Runtime,
    session: &glasshouse::session::SessionId,
) -> Box<dyn glasshouse::memory::ExtractionModel> {
    let user = match UserConfig::load(runtime.paths()) {
        Ok(user) => user,
        Err(err) => {
            tracing::debug!(error = %err, "could not read configuration for disposable routing");
            return Box::new(crate::commands::memory_extraction::NoExtractionModel);
        }
    };
    let project = match config::load_project_config(runtime.project()) {
        Ok(project) => project,
        Err(err) => {
            tracing::debug!(error = %err, "could not read project configuration for disposable routing");
            return Box::new(crate::commands::memory_extraction::NoExtractionModel);
        }
    };
    let effective = EffectiveConfig::new(&user, project.as_ref());
    let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let telemetry = glasshouse::provider::resources::GatheredTelemetry::new().gather_gateway_quota(
        &glasshouse::provider::telemetry::GatewayQuotaCache::new(runtime.paths()),
    );
    // Map line 1519: priced spend against every provider's own configured
    // money budget, for `disposable_candidates`' own exclusion. Fail-soft
    // exactly as every other gather on this path.
    let telemetry = match glasshouse::routing::evidence::EvidenceLedger::open(runtime) {
        Ok(ledger) => {
            let prices = glasshouse::provider::pricing::PriceTable::load_from_dir(
                runtime.paths().config_dir(),
            );
            telemetry.gather_budget_spend(&ledger, &prices, &effective, now_unix)
        }
        Err(err) => {
            tracing::debug!(
                error = %err,
                "could not read the routing evidence ledger to count budget spend for support work"
            );
            telemetry
        }
    };

    let consented = configured_extraction_choice(&effective);
    let configured_candidate = consented.as_ref().and_then(|chosen| {
        configured_extraction_candidate(
            &user,
            project.as_ref(),
            &effective,
            chosen,
            &secrets,
            &telemetry,
            now_unix,
        )
    });
    // Step 2: named, credential-less, and therefore not rankable. Nothing is
    // routed and nothing is lost — a local model has no marginal cost for
    // line 530 to prefer something else over.
    //
    // `configured_extraction_candidate` also answers `None` for a provider
    // that is missing, disabled, or whose named credential is unset. Those
    // are not bypasses: the direct build below fails for each of them too
    // (`configured_provider`, and `ConfiguredModelError::NoCredential`), so
    // they fall through to the router and end in its refusal. The `let Some`
    // is what keeps the three cases apart without a second condition to keep
    // in step with the first.
    if let Some(chosen) = &consented
        && configured_candidate.is_none()
        && let Some(model) = configured_extraction_model(&user, project.as_ref(), chosen)
    {
        return model;
    }

    let candidates = disposable_candidates(
        &user,
        project.as_ref(),
        &effective,
        &secrets,
        &telemetry,
        now_unix,
    );
    // Map line 1539's reader half, right after `disposable_candidates`
    // builds the list — never inside that function itself, which a live
    // worker is editing this same round.
    let mut candidates = attach_latency_records(runtime, candidates, now_unix);
    // Added rather than substituted, and only when the configuration did not
    // already yield it: a model named in a provider's `free_models` **and**
    // in `[memory] extraction_model` is one resource, ranked once.
    if let Some(candidate) = configured_candidate
        && !candidates.iter().any(|existing| {
            existing.provider() == candidate.provider() && existing.model() == candidate.model()
        })
    {
        candidates.push(candidate);
    }

    // Line 534, read side: what other short-lived dispatchers learned. Until
    // this batch this path passed `FreePool::new()`, so `choose`'s health
    // filter was handed a pool that could never exclude anything (map line
    // 1433, practice §36).
    let health = crate::commands::routing_destinations::observed_health_of(
        runtime,
        candidates.iter().map(|candidate| {
            glasshouse::routing::free::FreeResource::new(
                candidate.credential().clone(),
                candidate.model(),
            )
        }),
    );

    let free_preferences = glasshouse::routing::free::FreePreferences::new()
        .with_order(
            effective
                .free_resource_order()
                .value
                .iter()
                .map(|order| order.to_key())
                .collect(),
        )
        .with_disabled(
            effective
                .free_resource_disabled()
                .value
                .iter()
                .map(|disabled| disabled.to_key())
                .collect(),
        )
        .with_pin(
            effective
                .free_resource_pin()
                .value
                .as_ref()
                .map(|pin| pin.to_key()),
        );
    // Capability map line 1290's production wiring: the sessions the user
    // named, paired with the session this decision is actually for.
    // `ReserveOverride::applies` is what makes those two facts one input, and
    // it is false for every session the user did not name — including when
    // the list is empty, which is every user who has never run `glasshouse
    // sessions reserve`.
    let reserve_override = glasshouse::routing::disposable::ReserveOverride::for_sessions(
        effective.reserve_override_sessions().value,
    )
    .deciding_for(session.to_string());
    // Map lines 1294 and 1610's production wiring, on the same shape and for
    // the same reason as the override above: what somebody declared, paired
    // with the session this decision is for, and false for every session
    // nobody declared.
    let task_progress = glasshouse::routing::disposable::DeclaredTaskProgress::for_sessions(
        crate::commands::sessions::declared_task_progress_sessions(runtime),
    )
    .deciding_for(session.to_string());
    let routing = glasshouse::routing::disposable::DisposableRouting::for_support_work(
        effective.prefer_free_routing().value,
        free_preferences,
    )
    .with_reserve_override(reserve_override)
    .with_task_progress(task_progress)
    // Capability map line 1577's background half, on the path that acts.
    // Memory extraction is a support job Glasshouse runs on its own behalf,
    // so the scope is `Background` and the selection is made here — by
    // `ReservePolicies::for_scope`, the one function in the build that maps
    // a scope to a field — rather than inside the router, which is held to
    // never carrying the other scope's policy.
    .with_reserve_policy(
        effective
            .reserve_policies()
            .for_scope(glasshouse::routing::pressure::ReserveScope::Background),
    );
    let job = glasshouse::routing::disposable::JobKind::MemoryExtraction;

    // Line 1367, read side: what the *other* short-lived dispatchers are
    // already about to spend. Health says which resources have been failing;
    // this says which of the requests that are left have been claimed but
    // not yet paid, which no cache in this build carried until now and which
    // is the whole difference between two processes pacing one allowance and
    // two processes spending it twice.
    let reservations =
        glasshouse::provider::telemetry::DispatchReservationCache::new(runtime.paths());
    let mut pool = health.pool().clone();
    let mut notes = Vec::new();
    withhold_reserved_requests(
        &reservations,
        &mut pool,
        &candidates,
        &effective,
        &telemetry,
        now_unix,
        &mut notes,
    );

    let mut routed = glasshouse::memory::RoutedModel::new(job, &candidates, &routing, &pool);

    // Line 1367, write side. Only with consent — a run that will call
    // nothing must not hold a request out of a pool somebody else could
    // spend — and only for a resource whose pool has a *measured* remainder,
    // because a claim against a ceiling nobody stated would refuse dispatches
    // on an invented number.
    //
    // The netting above is a read with no lock, so two dispatchers can both
    // pass it. This is the lock: `claim` is an exclusive create, exactly one
    // of them wins it, and the loser learns here rather than at the
    // provider's rate limiter. It then withholds that credential's whole
    // remainder and asks the policy again — which is a *different* decision
    // with a changed input, not the second ask this function's own header
    // refuses to make for the ledger's benefit. Bounded by the candidate
    // count, so a pool that keeps losing races ends in `NoResource` rather
    // than in a loop, and **nothing waits**: a hook process is a guest in
    // the harness's turn.
    // History: design-decisions.md, "Trims: commands module docs, third packet", routing_classification.rs `disposable_extraction_model` (claim-refusal note).
    let mut lease = None;
    if consented.is_some() {
        let mut refused: Vec<String> = Vec::new();
        for _ in 0..=candidates.len() {
            let Some((provider, model, credential)) = routed.choice().ok().map(|choice| {
                (
                    choice.provider().to_owned(),
                    choice.model().to_owned(),
                    choice.credential().clone(),
                )
            }) else {
                break;
            };
            let label = credential.label();
            if refused.contains(&label) {
                break;
            }
            let Some(remaining) =
                paced_request_remainder(&provider, &effective, &telemetry, now_unix)
            else {
                // An unknown or token-priced pool reserves nothing and
                // dispatches exactly as it did before this line had a
                // producer.
                break;
            };
            match reservations.claim(&label, &model, remaining, now_unix) {
                Some(claimed) => {
                    lease = Some(claimed);
                    break;
                }
                None => {
                    pool.withhold_in_flight(&credential, remaining, remaining);
                    notes.push(reserved_elsewhere_note(&label, &model, remaining));
                    refused.push(label);
                    routed =
                        glasshouse::memory::RoutedModel::new(job, &candidates, &routing, &pool);
                }
            }
        }
    }

    // Step 4. Only with consent, and only for a resource the policy actually
    // chose: a client built for a candidate the router refused would be a
    // model reached around the protected-reserve gate, which is the whole
    // thing `automatic_classification_model`'s own header says must not
    // happen.
    if consented.is_some()
        && let Ok(choice) = routed.choice()
    {
        let credential = choice.credential().clone();
        let client = extraction_client_for(
            &user,
            project.as_ref(),
            choice.provider(),
            choice.model(),
            Some(credential.reference()),
        );
        if let Err(reason) = &client {
            tracing::warn!(reason, "the routed extraction model cannot be used");
        }
        let paths = runtime.paths().clone();
        routed =
            routed
                .with_client(client, credential.label())
                .observing(move |resource, outcome| {
                    persist_support_work_health(&paths, resource, outcome)
                });
    }

    // The reservation is given back when the exchange finishes, and by the
    // model's own drop when there was no exchange — see
    // `RoutedModel::releasing`. A lease held for a client that could not be
    // built is released here instead: nothing will be called, and the
    // request belongs to whoever asks next.
    if let Some(claimed) = lease {
        match routed.can_call() {
            true => routed = routed.releasing(move || claimed.release()),
            false => claimed.release(),
        }
    }
    if !notes.is_empty() {
        routed = routed.noting(notes.join("; "));
    }

    // The decision is made above and, until this line existed, died in a
    // `tracing::info!` a few frames later. `describe()` is the string
    // production already renders — the chosen model, its provider, its cost,
    // the `UseReason`, and every named contribution behind it, or the reason
    // no resource could serve — so what reaches the ledger is the rationale
    // that was *used*, not a second decision made for the ledger's benefit.
    // Asking `routing.choose` again here would produce a different `Instant`
    // and could produce a different answer.
    // History: design-decisions.md, "Trims: commands module docs, third packet", routing_classification.rs `disposable_extraction_model` (thread-safety note).
    //
    // Every branch that reaches here made a routing decision, including the
    // consented one — which is the change: the model that gets called is now
    // the model that was routed, so there is no longer a path whose rationale
    // would be a record of something that did not happen. The one branch that
    // still records nothing is the local bypass above, which returns before
    // this line because no decision was made for it.
    glasshouse::evaluation::record_disposable_route(
        runtime,
        job,
        session.as_str(),
        &glasshouse::memory::ExtractionModel::describe(&routed),
        glasshouse::evaluation::now_unix(),
    );

    // The routed choice, named once here so both the consent sentence and
    // the withheld-credential notice's `only` filter agree on the same
    // provider rather than each re-deriving it from `routed`.
    let routed_choice = routed
        .choice()
        .ok()
        .map(|choice| (choice.provider().to_owned(), choice.model().to_owned()));

    // Map line 488, said out loud: when the extraction provider's credential
    // resolves from nowhere this hook can see, the model's own description —
    // the string the outcome stores and the hook's stderr line prints —
    // carries the notice. The routed model itself is untouched. `only`
    // never names a provider the decision did not choose: the consented
    // provider when there is consent, else the routed choice's provider —
    // never every configured provider — so an unconsented run whose routing
    // picked one candidate does not also warn about an unrelated one.
    let only = consented
        .as_ref()
        .map(|chosen| chosen.provider().to_owned())
        .or_else(|| routed_choice.as_ref().map(|(provider, _)| provider.clone()));
    let withheld = withheld_provider_credentials(&effective, &secrets, only.as_deref());

    let model: Box<dyn glasshouse::memory::ExtractionModel> = if consented.is_none() {
        let notice = consent_missing_notice(routed_choice.as_ref());
        crate::commands::memory_extraction::noting_missing_consent(Box::new(routed), notice)
    } else {
        Box::new(routed)
    };
    crate::commands::memory_extraction::noting_withheld_credentials(model, &withheld)
}

/// The one sentence [`disposable_extraction_model`] appends when
/// [`configured_extraction_choice`] found no consent: what is missing (the
/// config key, map line 488's sibling for consent rather than credentials),
/// and the routed choice so a person can act on the one decision that was
/// actually made rather than guess at one. `routed` is `None` when routing
/// itself found no candidate at all, in which case nothing was chosen and
/// the placeholders stay literal.
fn consent_missing_notice(routed: Option<&(String, String)>) -> String {
    let (provider, model) = match routed {
        Some((provider, model)) => (provider.as_str(), model.as_str()),
        None => ("<provider>", "<model>"),
    };
    format!(
        "no extraction model is consented — the route above was decided and recorded but \
         nothing was called: set memory_extraction_model = {{ provider = \"{provider}\", model \
         = \"{model}\" }} in the user config or the project's .glasshouse/config.toml to let \
         Glasshouse call it"
    )
}

/// `[memory] rerank_model` resolved into a callable model, for
/// `brief_launch_session`'s call into [`glasshouse::memory::inject::select_briefing`]
/// — the extraction seat's four steps for `JobKind::Reranking`, map lines
/// 1089-1092.
///
/// The implementation lives in
/// [`glasshouse::memory::rerank::resolve_rerank_model`] rather than here:
/// unlike [`disposable_extraction_model`], this seat is reached from **two**
/// doors — this file's own [`brief_launch_session`] and
/// `glasshouse::api::unix::select_memory`, which is a library module that
/// cannot call anything in this binary crate. Putting the logic in the
/// library is what lets both doors call the same seat; this is the thin
/// wrapper that keeps it named beside its sibling here, as the packet asks.
pub(crate) fn disposable_rerank_model(
    runtime: &Runtime,
    _session: &glasshouse::session::SessionId,
) -> Option<Box<dyn glasshouse::memory::ExtractionModel>> {
    glasshouse::memory::rerank::resolve_rerank_model(runtime)
}

/// `[memory] retrieval_diagnostics`, resolved — map line 1094's gate on
/// whether a briefing writes `memory-retrieval.jsonl`. A configuration that
/// cannot be read is `false`, matching every other automatic-behaviour
/// default's fail-safe direction on this path.
pub(crate) fn memory_retrieval_diagnostics_enabled(runtime: &Runtime) -> bool {
    let Ok(user) = UserConfig::load(runtime.paths()) else {
        return false;
    };
    let Ok(project) = config::load_project_config(runtime.project()) else {
        return false;
    };
    EffectiveConfig::new(&user, project.as_ref())
        .memory_retrieval_diagnostics()
        .value
}

/// `[memory] extraction_diagnostics`, resolved — map line 1769's gate on
/// whether an extraction run writes `memory-extraction.jsonl`, mirroring
/// [`memory_retrieval_diagnostics_enabled`]'s own fail-safe direction: a
/// configuration that cannot be read is `false`.
pub(crate) fn memory_extraction_diagnostics_enabled(runtime: &Runtime) -> bool {
    let Ok(user) = UserConfig::load(runtime.paths()) else {
        return false;
    };
    let Ok(project) = config::load_project_config(runtime.project()) else {
        return false;
    };
    EffectiveConfig::new(&user, project.as_ref())
        .memory_extraction_diagnostics()
        .value
}

/// Take the requests other dispatches have already claimed out of what each
/// candidate's pool is known to have left, so the policy ranks what is
/// actually spendable — capability map line 1367's read side.
///
/// # Where the exclusion happens, and why it is not a new rule
///
/// Nowhere here. This only writes a truthful remainder into the pool through
/// [`glasshouse::routing::free::FreePool::withhold_in_flight`]; the
/// *decision* is
/// [`glasshouse::routing::free::Allowance::is_exhausted`]'s, reached through
/// `FreePool::is_available` — the identical gate a cooling-down resource
/// fails, and the only one `DisposableRouting::choose` puts a free candidate
/// through. So a resource whose every remaining request is spoken for
/// becomes unavailable by exactly the path a cooldown takes, `choose` falls
/// to the next candidate or to `NoResource` in today's words, and
/// `routing::disposable` neither gains a rule nor learns that a cache
/// exists.
// History: design-decisions.md, "Trims: commands module docs, third packet", routing_classification.rs `withhold_reserved_requests`.
fn withhold_reserved_requests(
    reservations: &glasshouse::provider::telemetry::DispatchReservationCache,
    pool: &mut glasshouse::routing::free::FreePool,
    candidates: &[glasshouse::routing::disposable::DisposableCandidate],
    effective: &EffectiveConfig<'_>,
    telemetry: &glasshouse::provider::resources::GatheredTelemetry,
    now_unix: i64,
    notes: &mut Vec<String>,
) {
    let mut asked: Vec<String> = Vec::new();
    for candidate in candidates {
        let label = candidate.credential().label();
        if asked.contains(&label) {
            continue;
        }
        asked.push(label.clone());
        let Some(remaining) =
            paced_request_remainder(candidate.provider(), effective, telemetry, now_unix)
        else {
            continue;
        };
        let in_flight = reservations.reserved(&label, now_unix);
        if in_flight == 0 {
            continue;
        }
        pool.withhold_in_flight(candidate.credential(), remaining, in_flight);
        if in_flight >= remaining {
            notes.push(reserved_elsewhere_note(
                &label,
                candidate.model(),
                remaining,
            ));
        }
    }
}

/// Why a resource that looked usable is not being used, in one clause a
/// person can act on.
///
/// Two names and a model — [`glasshouse::routing::CredentialId::label`]'s
/// own guarantee — and never a value, because this is rendered by
/// `ExtractionModel::describe` into the command's output and into the
/// routing ledger's rationale.
fn reserved_elsewhere_note(credential_label: &str, model: &str, remaining: u32) -> String {
    format!(
        "{model} ({credential_label}): its {remaining} remaining request(s) are reserved by \
         another dispatch"
    )
}

/// Every resource Glasshouse's disposable-job routing may choose from — free
/// and metered alike — built the same way `build_settings` builds a
/// `ProviderRow`'s configuration in `shell/mod.rs`: a provider's whole
/// configuration comes from whichever layer actually holds its name, project
/// winning over user.
///
/// A provider that named neither a free model
/// ([`ProviderConfig::free_models`]) nor a metered one
/// ([`ProviderConfig::metered_models`]), or whose credential does not
/// currently resolve, contributes nothing — never a candidate with an
/// invented model name or a credential this process cannot actually use.
///
/// A model named in both lists resolves through
/// [`ProviderConfig::cost_of`] — `Free` wins, and it is added once, not
/// twice.
// History: design-decisions.md, "Trims: commands module docs, third packet", routing_classification.rs `disposable_candidates`.
pub(crate) fn disposable_candidates(
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    effective: &EffectiveConfig<'_>,
    secrets: &dyn glasshouse::secret::SecretStore,
    telemetry: &glasshouse::provider::resources::GatheredTelemetry,
    now_unix: i64,
) -> Vec<glasshouse::routing::disposable::DisposableCandidate> {
    use glasshouse::routing::CredentialId;
    use glasshouse::routing::disposable::DisposableCandidate;
    use glasshouse::secret::SecretRef;

    let mut candidates = Vec::new();
    for name in effective.provider_names() {
        let found = project
            .and_then(|p| p.providers().get(&name))
            .or_else(|| user.providers().get(&name));
        let Some(provider_config) = found else {
            continue;
        };
        let free_models = provider_config.free_models();
        let metered_models = provider_config.metered_models();
        if !provider_config.enabled() || (free_models.is_empty() && metered_models.is_empty()) {
            continue;
        }
        let capacity = disposable_candidate_capacity(&name, effective, telemetry, now_unix);
        // Map line 1519, for support work: a provider whose own money budget
        // has been counted as exhausted is excluded here, before a candidate
        // for it exists at all — `routing::disposable` stays untouched
        // (classifier-time-price is live there), and a free-tier candidate is
        // never excluded by a money budget (checked per model below, since
        // cost is a fact about the model, not the provider). Unlike
        // `glasshouse route`'s `hard_constraint`, there is no per-destination
        // explanation to carry the reason into here — a candidate that never
        // exists cannot be named in one — so this is a recorded limit: an
        // excluded model does not appear in a disposable choice's rejection
        // list the way an entitlement job-kind or spend-ceiling refusal does.
        let budget_exhausted =
            glasshouse::provider::resources::budget_exhausted_for(&name, effective, telemetry);
        // A free candidate must not inherit the metered ones' capacity
        // reading when it was the money budget that zeroed it: `capacity` is
        // one `CapacityState` per provider, shared by every model of it, and
        // `routing::disposable`'s existing "known zero headroom" gate (line
        // 1434) does not distinguish which dimension bound the score — only
        // that it reads zero. Computed against a telemetry value with this
        // provider's budget spend stripped, so nothing about money reaches a
        // free candidate's own capacity at all.
        let free_capacity = if budget_exhausted.is_some() {
            let without_budget = telemetry.clone().without_provider_budget_spend(&name);
            disposable_candidate_capacity(&name, effective, &without_budget, now_unix)
        } else {
            capacity.clone()
        };
        // Map lines 1427 and 1438: where this provider's compute runs, from
        // the one place this build already says so — the registry's
        // local-inference slugs — never from a base URL that happens to
        // point at loopback.
        let locality =
            glasshouse::provider::registry::ResourceKind::from_direct_provider(name.as_str())
                .locality();
        // Map line 1947's job-kind clause: the entitlement charged for work
        // sent to this provider, so `DisposableRouting::choose` can refuse a
        // job kind its rules do not serve — by the entitlement's name, in
        // the choice's own explanation, never as a silent pre-filter here.
        // A contradiction in the `[entitlements]` tables refuses a *launch*
        // outright; a bounded support job degrades to "no rule" with a
        // warning instead, because failing memory extraction over a config
        // contradiction the next launch will already report would punish the
        // wrong actor.
        let entitlement = match effective.entitlement_for_provider(&name) {
            Ok(entitlement) => entitlement.map(|entitlement| entitlement.to_routing()),
            Err(err) => {
                tracing::warn!(
                    provider = %name,
                    error = %err,
                    "the [entitlements] tables could not be resolved; support work \
                     proceeds with no entitlement rule for this provider"
                );
                None
            }
        };
        for var in provider_config.credential_env() {
            let reference = SecretRef::Environment { var: var.clone() };
            if secrets.resolve(&reference).is_none() {
                continue;
            }
            let credential_id = CredentialId::new(name.clone(), reference);
            let models = free_models
                .iter()
                .chain(metered_models.iter().filter(|m| !free_models.contains(m)));
            for model in models {
                let cost = provider_config.cost_of(model);
                if budget_exhausted.is_some() && !cost.is_free() {
                    tracing::debug!(
                        provider = %name,
                        model = %model,
                        "excluding a support-work candidate: its provider's money budget is \
                         counted as exhausted"
                    );
                    continue;
                }
                let candidate_capacity = if cost.is_free() {
                    &free_capacity
                } else {
                    &capacity
                };
                candidates.push(
                    DisposableCandidate::new(
                        name.clone(),
                        model.clone(),
                        credential_id.clone(),
                        cost,
                    )
                    .with_capacity(candidate_capacity.clone())
                    .with_locality(locality)
                    .with_entitlement(entitlement.clone()),
                );
            }
        }
    }
    candidates
}

/// What `routing_observations.purpose` records for a call `glasshouse
/// classify` made.
///
/// Spelled once — in `routing::evidence`, beside the reader that keys on it
/// (`EvidenceLedger::classification_record`), and only re-named here.
/// `purpose` is a `TEXT` column with no `CHECK` (`database.rs`'s migration
/// 11), so the only thing keeping the producer and the reader on one
/// spelling is that there is exactly one.
pub(crate) const CLASSIFICATION_PURPOSE: &str =
    glasshouse::routing::evidence::CLASSIFICATION_PURPOSE;

/// One resource `glasshouse classify` may ask, by name: the provider and
/// model a configuration or a routing choice named, plus the exact
/// credential reference the choice resolved — `None` for a pinned model or
/// a fallback-chain entry, where [`classification_model`] resolves the first
/// variable that answers. Built into a `ConfiguredModel` only at the moment
/// it is about to be called, inside [`classify_through_chain`], so a chain
/// entry that is never reached is never built and never resolves anything.
struct ClassifierRef {
    provider: String,
    model: String,
    credential: Option<glasshouse::secret::SecretRef>,
}

impl ClassifierRef {
    fn named(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            credential: None,
        }
    }
}

/// What happened when `glasshouse classify` tried to have a model classify a
/// request.
///
/// Three outcomes rather than an `Option`, because "the user configured no
/// routing model" and "the routing model they configured could not answer"
/// are different facts that a caller must say differently: the first is
/// Phase 35's ordinary state and deserves no message at all, and the second
/// is a degrade the user is entitled to be told about. Collapsing them would
/// make a broken configuration look like an absent one.
pub(crate) enum ClassificationAttempt {
    /// No routing model is configured. Deterministic heuristics answer,
    /// exactly as they did before this command could call anything.
    NotConfigured,
    /// A model answered, in the schema.
    Answered(glasshouse::routing::classify::TaskClassification),
    /// A model was configured, and no classification came back. The sentence
    /// is chosen in this file — see [`routing_model_failure`].
    Failed(String),
}

/// A [`glasshouse::memory::ModelError`] as one sentence about the **routing**
/// model.
///
/// That type's own `Display`, and the `&'static str` phrases
/// `memory/extract/model.rs` builds its `Failed` variant from, say
/// *"extraction model"* in every arm. That is accurate for the job the type
/// was written for and wrong for this one: a user told their extraction model
/// is rate limited when it is their *routing* model would go and edit the
/// wrong configuration key. So the subject is named here, where the job is
/// known, and the transport's own words go to the log rather than to a
/// sentence that would mis-attribute them.
fn routing_model_failure(err: &glasshouse::memory::ModelError) -> String {
    use glasshouse::memory::ModelError;

    tracing::warn!(error = %err, "the routing model could not classify this request");
    match err {
        ModelError::Unavailable => "the routing model could not be reached".to_owned(),
        ModelError::Refused => "the routing model declined the request".to_owned(),
        ModelError::TimedOut => "the routing model did not answer within its bound".to_owned(),
        ModelError::Failed { .. } => {
            "the routing model's call produced no usable answer".to_owned()
        }
        // Not produced on this path today — `ModelError::Declined` is the
        // rerank seat's own bypass reason — but the reason is already a
        // full sentence Glasshouse composed, so it needs no subject-renaming
        // the way the fixed phrases above do.
        ModelError::Declined { reason } => reason.clone(),
    }
}

/// Build the model `provider`/`model` names, or say in one sentence why it
/// cannot be built.
///
/// The provider's whole configuration comes from whichever layer actually
/// holds its name, project winning over user — the same rule
/// [`configured_extraction_model`] and [`disposable_candidates`] apply, and
/// for the same reason.
///
/// `credential` is the reference to resolve when the caller already knows
/// which one applies — `DisposableRouting`'s choice names the exact
/// `SecretRef` that resolved when its candidate was built, and re-deriving it
/// here could pick a different one. `None` is the pinned case, where nobody
/// has resolved anything yet and the first variable that resolves wins, the
/// same order `disposable_candidates` walks.
fn classification_model(
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    provider_name: &str,
    model_name: &str,
    credential: Option<&glasshouse::secret::SecretRef>,
) -> Result<glasshouse::memory::ConfiguredModel, String> {
    use glasshouse::memory::{ConfiguredModel, ConfiguredModelError};
    use glasshouse::secret::{SecretRef, SecretStore as _};

    let Some(provider_config) = project
        .and_then(|p| p.providers().get(provider_name))
        .or_else(|| user.providers().get(provider_name))
    else {
        return Err(format!(
            "the routing model names `{provider_name}`, which this project has not configured"
        ));
    };
    if !provider_config.enabled() {
        return Err(format!(
            "the routing model names `{provider_name}`, which is disabled"
        ));
    }
    let provider = provider_config
        .to_provider(provider_name)
        .map_err(|err| format!("the routing model's provider does not resolve: {err}"))?;

    let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();
    let credential = match credential {
        Some(reference) => secrets.resolve(reference),
        None => provider
            .credential_env
            .iter()
            .find_map(|var| secrets.resolve(&SecretRef::Environment { var: var.clone() })),
    };

    ConfiguredModel::new(&provider, model_name, credential).map_err(|err| match err {
        // Every other arm of this error already reads as a statement about a
        // provider, and is rendered as it stands. This one names the *job* —
        // "extraction speaks OpenAI chat completions" — which is the one
        // thing about it that is not true here.
        ConfiguredModelError::UnsupportedProtocol { protocol, .. } => format!(
            "classification speaks OpenAI chat completions, and `{provider_name}` serves \
             `{protocol}`; configure a provider that serves openai-chat"
        ),
        other => format!("the routing model cannot be used: {other}"),
    })
}

/// The `Automatic` half of `RoutingModelChoice`: ask
/// `DisposableRouting::choose` which resource should classify this request,
/// and name the model it chose — built into a `ConfiguredModel` only when
/// [`classify_through_chain`] is about to call it.
///
/// # Why this goes through `choose` rather than building a model directly
///
/// `choose` is the **only** production call site of
/// `provider::quota::evaluate_reserve_spend` — Phase 32F's protected-reserve
/// gate. `configured_extraction_model` returns before that gate is consulted,
/// which is defensible for extraction (it runs once per completed turn, on a
/// model the user named by hand) and would not be for classification: a
/// classifier is asked on every routing decision, which is a request per
/// decision, and it is the spend Phase 34E's own lines exist to bound. So a
/// model reached around this function is a model whose cost nothing decided,
/// and `tests/classification_call.rs` mutates this call away to prove
/// something is watching.
// History: design-decisions.md, "Trims: commands module docs, third packet", routing_classification.rs `automatic_classification_model`.
fn automatic_classification_model(
    runtime: &Runtime,
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    effective: &EffectiveConfig<'_>,
    request_text: &str,
    protected_capacity_price: Option<glasshouse::provider::pricing::ModelPrice>,
) -> Result<ClassifierRef, String> {
    // The tier this job's own demand implies, from the request itself. This
    // is `RoutedModel::new_for_request`'s fifth link, made by the one
    // `JobKind` its doc comment says the constructor was waiting for — a
    // request, not a transcript of a finished turn.
    let requirement = glasshouse::routing::classify::classify_heuristically(request_text);
    let choice = automatic_classification_choice(
        runtime,
        user,
        project,
        effective,
        Some(&requirement),
        protected_capacity_price,
    )
    .map_err(|reason| format!("no resource is available to classify this request: {reason}"))?;

    Ok(ClassifierRef {
        provider: choice.provider().to_owned(),
        model: choice.model().to_owned(),
        credential: Some(choice.credential().reference().clone()),
    })
}

/// Which configured resource automatic routing-model selection picks right
/// now — the decision itself, separated from building the model so that a
/// diagnostic can name the same pick without asking anything to classify.
///
/// `classification` is `None` for a caller with no request in hand, which is
/// exactly what [`DisposableRouting::choose`] documents that value as meaning
/// — the fixed [`WorkloadTier::Leaf`] the policy used before a classification
/// existed to ask. The report says so rather than implying a request was
/// classified.
///
/// **No `ReserveOverride`.** That input is scoped to sessions the user named
/// by hand with `glasshouse sessions reserve`, and this decision is made for
/// no session at all — there is no identity here for the override to apply
/// to, and inventing one would grant a reserve exemption nobody asked for.
// History: design-decisions.md, "Trims: commands module docs, third packet", routing_classification.rs `automatic_classification_choice`.
pub(crate) fn automatic_classification_choice(
    runtime: &Runtime,
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    effective: &EffectiveConfig<'_>,
    classification: Option<&glasshouse::routing::classify::TaskClassification>,
    // Capability map line 1419: the premium capacity this decision
    // protects, when the caller has one — see `RoutingClassificationSite`'s
    // own doc for who does and does not.
    protected_capacity_price: Option<glasshouse::provider::pricing::ModelPrice>,
) -> Result<
    glasshouse::routing::disposable::DisposableChoice,
    glasshouse::routing::disposable::NoResource,
> {
    use glasshouse::provider::telemetry::RoutingStickyCache;
    use glasshouse::routing::disposable::{AutomaticClassificationDecision, DisposableRouting};

    let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let telemetry = glasshouse::provider::resources::GatheredTelemetry::new().gather_gateway_quota(
        &glasshouse::provider::telemetry::GatewayQuotaCache::new(runtime.paths()),
    );
    // Map line 1519: priced spend against every provider's own configured
    // money budget, for `disposable_candidates`' own exclusion. Fail-soft
    // exactly as every other gather on this path.
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
                "could not read the routing evidence ledger to count budget spend for automatic \
                 classification-model selection"
            );
            telemetry
        }
    };
    let candidates =
        disposable_candidates(user, project, effective, &secrets, &telemetry, now_unix);
    let candidates = attach_classification_records(runtime, candidates, now_unix);
    // Map line 1539's reader half, right after `disposable_candidates` builds
    // the list `DisposableRouting::score` will rank — never inside that
    // function itself, which a live worker is editing this same round.
    let candidates = attach_latency_records(runtime, candidates, now_unix);
    // Map line 1436's producer: the same `pricing.toml` read `session_router`
    // already loads from the same config directory. Fail-soft, like that
    // caller — an absent or malformed file yields an empty table and every
    // candidate reads as unpriced, never as a fabricated zero.
    let prices =
        glasshouse::provider::pricing::PriceTable::load_from_dir(runtime.paths().config_dir());
    let candidates = attach_prices(candidates, &prices);
    let health = crate::commands::routing_destinations::observed_health_of(
        runtime,
        candidates.iter().map(|candidate| {
            glasshouse::routing::free::FreeResource::new(
                candidate.credential().clone(),
                candidate.model(),
            )
        }),
    );
    let free_preferences = glasshouse::routing::free::FreePreferences::new()
        .with_order(
            effective
                .free_resource_order()
                .value
                .iter()
                .map(|order| order.to_key())
                .collect(),
        )
        .with_disabled(
            effective
                .free_resource_disabled()
                .value
                .iter()
                .map(|disabled| disabled.to_key())
                .collect(),
        )
        .with_pin(
            effective
                .free_resource_pin()
                .value
                .as_ref()
                .map(|pin| pin.to_key()),
        );
    // Map lines 1427, 1435 and 1436: the user's classification requirements,
    // layered like every other `[routing]` value. `max_router_latency_ms`
    // and `max_marginal_cost` both have defaults, so each ceiling is always
    // stated; whether it *applies* to a candidate is decided by whether that
    // candidate has a measured median or a known price — see
    // `routing::disposable::classification_verdict`.
    let routing = DisposableRouting::for_support_work(
        effective.prefer_free_routing().value,
        free_preferences,
    )
    .with_classification_policy(
        glasshouse::routing::disposable::ClassificationPolicy::new()
            .with_max_latency_ms(Some(effective.max_router_latency().value.get()))
            .with_local_only(effective.classification_local_only().value)
            // Map line 1436: the user's own price ceiling, layered like
            // every other `[routing]` value and always stated (it has a
            // default), exactly as `max_router_latency` is above.
            .with_max_marginal_cost_micro_usd(Some(effective.max_router_cost().value.get()))
            // Map line 1419: the premium capacity this decision protects,
            // when the caller named one.
            .with_protected_capacity_price(protected_capacity_price),
    )
    // Capability map line 1577's background half. Automatic classification
    // is the other support job Glasshouse runs on its own behalf, and it
    // takes the same scope as extraction for the same reason: nobody typed
    // this request, so the reserve a person set aside for their own work is
    // not the policy that should decide it.
    .with_reserve_policy(
        effective
            .reserve_policies()
            .for_scope(glasshouse::routing::pressure::ReserveScope::Background),
    );

    // Map lines 1441/1442: reuse a recent healthy pick rather than
    // re-ranking every call. `RoutingStickyCache::new` roots the cache at
    // `RuntimePaths::project_state_dir(project_id)`, unlike the
    // account-scoped `GatewayQuotaCache` above, so a pick never leaks
    // between projects.
    let sticky_cache = RoutingStickyCache::new(runtime.paths(), runtime.project().id().as_str());
    let decision = routing.choose_for_automatic_classification(
        &candidates,
        health.pool(),
        std::time::Instant::now(),
        now_unix,
        classification,
        sticky_cache.load(),
    )?;
    match decision {
        AutomaticClassificationDecision::Fresh(choice, pick) => {
            sticky_cache.store(&pick);
            Ok(choice)
        }
        AutomaticClassificationDecision::Retained(choice) => Ok(choice),
    }
}

/// Ask the configured routing model to classify `request_text`.
///
/// # The three resolutions, and which one changes nothing
///
/// `RoutingModelResolution::Heuristics` returns before anything is read,
/// built, opened or sent. A build with no routing model configured — which is
/// every build until somebody configures one — asks nothing, opens no
/// database, and prints exactly what it printed before this function existed.
/// `tests/classification_call.rs` holds that byte-for-byte against the
/// heuristic's own output.
// History: design-decisions.md, "Trims: commands module docs, third packet", routing_classification.rs `classify_with_routing_model`.
pub(crate) fn classify_with_routing_model(
    runtime: &Runtime,
    request: &glasshouse::routing::request::RouterRequest,
    // Capability map line 1419: the launch's own protected capacity, when
    // this call is on the path that acts — see `RoutingClassificationSite`.
    protected_capacity_price: Option<glasshouse::provider::pricing::ModelPrice>,
) -> ClassificationAttempt {
    use glasshouse::config::RoutingModelResolution;

    let user = match UserConfig::load(runtime.paths()) {
        Ok(user) => user,
        Err(err) => {
            tracing::debug!(error = %err, "could not read configuration for the routing model");
            return ClassificationAttempt::NotConfigured;
        }
    };
    let project = match config::load_project_config(runtime.project()) {
        Ok(project) => project,
        Err(err) => {
            tracing::debug!(
                error = %err,
                "could not read project configuration for the routing model"
            );
            return ClassificationAttempt::NotConfigured;
        }
    };
    let effective = EffectiveConfig::new(&user, project.as_ref());

    let first = match effective.routing_model_resolution().value {
        RoutingModelResolution::Heuristics(_) => return ClassificationAttempt::NotConfigured,
        RoutingModelResolution::Pinned { provider, model } => {
            Ok(ClassifierRef::named(provider, model))
        }
        RoutingModelResolution::Automatic => automatic_classification_model(
            runtime,
            &user,
            project.as_ref(),
            &effective,
            request.task_text(),
            protected_capacity_price,
        ),
    };
    let first = match first {
        Ok(first) => first,
        Err(why) => return ClassificationAttempt::Failed(why),
    };

    let prompt = glasshouse::memory::extract::Prompt::for_request(
        glasshouse::routing::classify::CLASSIFICATION_PROMPT_CONTRACT,
        glasshouse::routing::classify::CLASSIFICATION_RESPONSE_SCHEMA,
        &request.render(),
    );

    // The call, the row it leaves and the fallback chain are all
    // `classify_through_chain`'s — see its header for what one attempt
    // records and when the next model is tried.
    classify_through_chain(runtime, &user, project.as_ref(), &effective, first, &prompt)
}

/// # The chain is walked once, and never back onto itself
///
/// Each `(provider, model)` is tried at most once per classification: a
/// chain entry naming the model that just failed is skipped, not retried, so
/// a chain of `[a, b]` after `a` was chosen automatically makes exactly two
/// calls. `tests/routing_economics.rs` holds this.
///
/// # The walk is named in the classification's own label
///
/// A classification that arrived through the chain is attributed to the
/// model that answered, and its label — the `source` line `glasshouse
/// classify` prints — says which models were tried first and why they
/// failed. Names only: every phrase in it is a provider name, a model name,
/// a route, or one of this file's own fixed sentences — never a base URL, a
/// credential, or a provider's response body, which
/// [`routing_model_failure`] already keeps out of the sentence.
// History: design-decisions.md, "Trims: commands module docs, third packet", routing_classification.rs `classify_through_chain`.
fn classify_through_chain(
    runtime: &Runtime,
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    effective: &EffectiveConfig<'_>,
    first: ClassifierRef,
    prompt: &glasshouse::memory::extract::Prompt,
) -> ClassificationAttempt {
    use glasshouse::memory::ExtractionModel as _;
    use glasshouse::provider::registry::{Locality, ResourceKind};
    use glasshouse::routing::evidence::Outcome;

    let local_only = effective.classification_local_only().value;
    let chain = effective.routing_model_fallback().value;
    let mut tried: Vec<(String, String)> = Vec::new();
    // `(name, why)` per failed attempt — rendered bare when there was only
    // one, and as `name: why` once the chain was walked.
    let mut failures: Vec<(String, String)> = Vec::new();

    let attempts = std::iter::once(first).chain(
        chain
            .iter()
            .map(|entry| ClassifierRef::named(entry.provider(), entry.model())),
    );
    for attempt in attempts {
        let key = (attempt.provider.clone(), attempt.model.clone());
        if tried.contains(&key) {
            continue;
        }
        tried.push(key);
        let name = format!("{} on {}", attempt.model, attempt.provider);

        // Map line 1427: decided from the provider's *name*, the one fact
        // the registry states for every provider, before anything is built
        // — a model that would be refused must not even resolve a
        // credential.
        if local_only
            && ResourceKind::from_direct_provider(attempt.provider.as_str()).locality()
                != Locality::Local
        {
            failures.push((
                name,
                "remote, and classification is confined to local models — no request was sent"
                    .to_owned(),
            ));
            continue;
        }

        let model = match classification_model(
            user,
            project,
            &attempt.provider,
            &attempt.model,
            attempt.credential.as_ref(),
        ) {
            Ok(model) => model,
            Err(why) => {
                failures.push((name, why));
                continue;
            }
        };

        // `describe()` names the provider, the model and the route, and
        // neither the base URL nor the credential — see
        // `memory::extract::model`'s header for why the base URL is excluded
        // even though it looks harmless. This is the label the
        // classification is attributed to, and it comes from the model this
        // process built, never from anything the reply said.
        let label = if failures.is_empty() {
            model.describe()
        } else {
            format!(
                "{}, after {}",
                model.describe(),
                render_chain_failures(&failures)
            )
        };

        let dispatched_at_unix = glasshouse::provider::cache::now_unix_seconds();
        let reply = match model.complete_observed(prompt) {
            Ok(reply) => reply,
            Err(err) => {
                failures.push((name, routing_model_failure(&err)));
                continue;
            }
        };
        let completed_at_unix = glasshouse::provider::cache::now_unix_seconds();

        let parsed = glasshouse::routing::classify::parse_classification(&reply.reply, label);
        if let Some(call) = &reply.call {
            let outcome = if parsed.is_ok() {
                Outcome::Succeeded
            } else {
                Outcome::Failed
            };
            record_classification_observation(
                runtime,
                call,
                outcome,
                dispatched_at_unix,
                completed_at_unix,
            );
        }
        match parsed {
            Ok(classification) => return ClassificationAttempt::Answered(classification),
            Err(err) => failures.push((name, err.to_string())),
        }
    }

    ClassificationAttempt::Failed(match failures.as_slice() {
        [(_, only)] => only.clone(),
        _ => format!(
            "every routing model in the chain failed — {}",
            render_chain_failures(&failures)
        ),
    })
}

/// `name: why; name: why` — the walk, as one phrase for a label or a
/// failure sentence.
fn render_chain_failures(failures: &[(String, String)]) -> String {
    failures
        .iter()
        .map(|(name, why)| format!("{name}: {why}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Append what one classification call cost — and whether its reply parsed
/// — to the routing evidence ledger, under `purpose = "classification"`.
///
/// # This is the producer capability map lines 1422/1432 and 1421/1435 lacked
///
/// Recorded **after** the reply is parsed, so the row carries its outcome:
/// [`glasshouse::routing::evidence::Outcome::Succeeded`] for a reply in the
/// schema and `Failed` for one outside it. Migration 11's `CHECK` fixes the
/// vocabulary to `succeeded`, `failed`, `cancelled` and `unknown`; a new
/// value would be a migration, and *failed at its purpose* is exactly what
/// a reply that could not be read as a classification did, so no new value
/// is invented. A transport failure never reaches this function — there is
/// no `ModelCall` — so a classification row's outcome is always a statement
/// about a reply that arrived.
///
/// No error channel, for the same reason [`record_extraction_observation`]
/// has none: a classification a person asked for is not made worse by the
/// bookkeeping failing, and Glasshouse's books are never more important than
/// the answer they are about.
// History: design-decisions.md, "Trims: commands module docs, third packet", routing_classification.rs `record_classification_observation`.
fn record_classification_observation(
    runtime: &Runtime,
    call: &glasshouse::memory::extract::ModelCall,
    outcome: glasshouse::routing::evidence::Outcome,
    dispatched_at_unix: i64,
    completed_at_unix: i64,
) {
    let ledger = match glasshouse::routing::evidence::EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "routing evidence ledger unavailable; what this classification cost is not recorded"
            );
            return;
        }
    };
    let observation = call
        .observation()
        .with_purpose(Some(CLASSIFICATION_PURPOSE))
        .with_timing(Some(dispatched_at_unix), Some(completed_at_unix))
        .with_outcome(outcome);
    if let Err(err) = ledger.record(observation, glasshouse::provider::cache::now_unix_seconds()) {
        tracing::warn!(error = %err, "could not record what classification cost");
    }
}

/// Read what the evidence ledger holds about each candidate as a classifier
/// — the reader half of capability map lines 1422/1432 and 1421/1435 — and
/// attach it, so `DisposableRouting::choose_for_automatic_classification`'s
/// filters and preferences act on measured quantities.
///
/// # Opened here, after the candidate list exists (practice §65)
///
/// Nothing is opened when there is no candidate to read about, and the
/// handle is dropped before the routing decision runs. A ledger that cannot
/// be opened, or a record that cannot be read, leaves that candidate
/// unmeasured — every filter built on it is then inert and says so in the
/// explanation — rather than failing the classification: Glasshouse's books
/// are never more important than the answer they are about.
fn attach_classification_records(
    runtime: &Runtime,
    candidates: Vec<glasshouse::routing::disposable::DisposableCandidate>,
    now_unix: i64,
) -> Vec<glasshouse::routing::disposable::DisposableCandidate> {
    use glasshouse::routing::evidence::{CLASSIFICATION_EVIDENCE_WINDOW_SECONDS, EvidenceLedger};

    if candidates.is_empty() {
        return candidates;
    }
    let ledger = match EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "routing evidence ledger unavailable; automatic classification ranks every \
                 candidate as unmeasured"
            );
            return candidates;
        }
    };
    candidates
        .into_iter()
        .map(|candidate| {
            let record = match ledger.classification_record(
                candidate.provider(),
                candidate.model(),
                now_unix,
                CLASSIFICATION_EVIDENCE_WINDOW_SECONDS,
            ) {
                Ok(record) => Some(record),
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        provider = candidate.provider(),
                        model = candidate.model(),
                        "could not read a candidate's classification record; it ranks as unmeasured"
                    );
                    None
                }
            };
            candidate.with_classification_record(record)
        })
        .collect()
}

/// Read what the evidence ledger holds about each candidate's own median
/// support-work latency — the reader half of capability map line 1539 — and
/// attach it, so `DisposableRouting::score`'s expected-latency term acts on
/// a measured quantity.
///
/// Beside [`attach_classification_records`] rather than folded into it: this
/// reads [`glasshouse::routing::evidence::EXTRACTION_PURPOSE`] rows, not
/// [`glasshouse::routing::evidence::CLASSIFICATION_PURPOSE`] ones, and it is
/// called from both dispatch functions that score support-work candidates —
/// `disposable_extraction_model` has no classification record to attach at
/// all.
///
/// # Opened here, after the candidate list exists (practice §65)
///
/// Same posture as [`attach_classification_records`]: nothing is opened for
/// an empty candidate list, and a ledger or a record that cannot be read
/// leaves that candidate unmeasured — the term is then inert and says so —
/// rather than failing the dispatch.
fn attach_latency_records(
    runtime: &Runtime,
    candidates: Vec<glasshouse::routing::disposable::DisposableCandidate>,
    now_unix: i64,
) -> Vec<glasshouse::routing::disposable::DisposableCandidate> {
    use glasshouse::routing::evidence::{CLASSIFICATION_EVIDENCE_WINDOW_SECONDS, EvidenceLedger};

    if candidates.is_empty() {
        return candidates;
    }
    let ledger = match EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "routing evidence ledger unavailable; every candidate's expected latency ranks \
                 as unmeasured"
            );
            return candidates;
        }
    };
    candidates
        .into_iter()
        .map(|candidate| {
            let record = match ledger.support_work_latency(
                candidate.provider(),
                candidate.model(),
                now_unix,
                CLASSIFICATION_EVIDENCE_WINDOW_SECONDS,
            ) {
                Ok(record) => Some(record),
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        provider = candidate.provider(),
                        model = candidate.model(),
                        "could not read a candidate's support-work latency record; it ranks as \
                         unmeasured"
                    );
                    None
                }
            };
            candidate.with_latency(record)
        })
        .collect()
}

/// Attach each candidate's real per-token price from `prices` — capability
/// map line 1436's producer, `PriceTable::price_for(provider, model)`. A
/// pair the table names nothing for is left unpriced, exactly as
/// [`attach_classification_records`] leaves an unmeasured candidate
/// unmeasured: [`glasshouse::routing::disposable::classification_verdict`]'s
/// price-ceiling gate reads that as inert, never as a fabricated zero.
fn attach_prices(
    candidates: Vec<glasshouse::routing::disposable::DisposableCandidate>,
    prices: &glasshouse::provider::pricing::PriceTable,
) -> Vec<glasshouse::routing::disposable::DisposableCandidate> {
    candidates
        .into_iter()
        .map(|candidate| {
            let price = prices.price_for(candidate.provider(), candidate.model());
            candidate.with_price(price)
        })
        .collect()
}

/// What real telemetry says about `provider`'s remaining capacity right now
/// — map lines 1536, 1549 and 1550's inputs, read the same way
/// [`resources_report`] reads them for `glasshouse resources`, from the same
/// on-disk [`glasshouse::provider::telemetry::GatewayQuotaCache`] and no
/// network call of its own.
///
/// A credential is still the right key for the *reservation* — see
/// `glasshouse::provider::telemetry::DispatchReservationCache` — and this is
/// the one place the two granularities meet: a provider's stated remainder is
/// read as the ceiling on what one of its credentials may have in flight.
/// Where a provider has several credentials that is conservative in the
/// direction that spends more, not less, and it is recorded as a limit rather
/// than hidden.
///
/// [`None`] is *"nothing is known"*, never *"nothing is left"*: an unmeasured
/// pool and a token-priced resource both answer it, and both mean this
/// dispatch reserves nothing and behaves exactly as it did before line 1367
/// had a producer.
// History: design-decisions.md, "Trims: commands module docs, third packet", routing_classification.rs `paced_request_remainder`.
fn paced_request_remainder(
    provider: &str,
    effective: &EffectiveConfig<'_>,
    telemetry: &glasshouse::provider::resources::GatheredTelemetry,
    now_unix: i64,
) -> Option<u32> {
    let kind = glasshouse::provider::registry::ResourceKind::from_direct_provider(provider);
    let state =
        glasshouse::provider::resources::observed_capacity(&kind, effective, telemetry, now_unix);
    let reading = state.requests().remaining().reading()?;
    u32::try_from(reading.value().value()).ok()
}

fn disposable_candidate_capacity(
    provider: &str,
    effective: &EffectiveConfig<'_>,
    telemetry: &glasshouse::provider::resources::GatheredTelemetry,
    now_unix: i64,
) -> glasshouse::routing::disposable::CandidateCapacity {
    let kind = glasshouse::provider::registry::ResourceKind::from_direct_provider(provider);
    let state =
        glasshouse::provider::resources::observed_capacity(&kind, effective, telemetry, now_unix);
    let remaining_capacity = state.remaining_capacity_score();
    let seconds_until_reset = state.seconds_until_reset(now_unix);
    let thresholds = effective
        .capacity_band_thresholds()
        .value
        .with_resource_reserve(effective.reserve_percent(provider).value.get());
    let band = remaining_capacity
        .as_ref()
        .map(|score| score.band(&thresholds));

    glasshouse::routing::disposable::CandidateCapacity::new()
        .with_remaining_capacity(remaining_capacity)
        .with_seconds_until_reset(seconds_until_reset)
        .with_band(band)
}
