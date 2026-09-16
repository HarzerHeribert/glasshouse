//! `commands::routing_classification` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use glasshouse::Runtime;
use glasshouse::config::{self, EffectiveConfig, ProjectConfig, UserConfig};

/// What one routing decision classified the work as — Phase 34D's answer.
/// `None` from [`classify_for_routing`] when no task was stated, which is
/// every launch and every `route` that reproduces the pre-classification
/// behaviour byte for byte.
///
/// Carried no `fingerprint` field as of 2026-09-16: its only reader was
/// `launch_session`'s sticky-cache write, and `launch_session` no longer
/// classifies a task at all (design-decisions.md, "Glasshouse never decides
/// which model is used"). `glasshouse route` — this type's other
/// constructor — never wrote the sticky cache either (`sticky: None`), so
/// the field had exactly one reader and it is gone.
pub(crate) struct ClassifiedRouting {
    pub(crate) answer: glasshouse::routing::request::RouterAnswer,
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
    Some(ClassifiedRouting { answer })
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

    /// No production caller as of 2026-09-16: `launch_session`'s own
    /// `remember_classification` was the only writer, and `launch_session`
    /// no longer classifies a task at all (design-decisions.md, "Glasshouse
    /// never decides which model is used"). `#[allow(dead_code)]` rather
    /// than deleting it — `src/tests.rs`'s
    /// `concurrent_sticky_classification_writes_never_produce_a_mixed_file`
    /// still calls it directly, and that file is this packet's to leave
    /// alone; the deletion package removes this type along with the rest of
    /// the module.
    #[allow(dead_code)]
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

/// `[memory] extraction_model` resolved into a callable model — Glasshouse
/// never decides which model runs memory extraction (design-decisions.md,
/// 2026-09-16): the key names a provider and a model, or extraction does not
/// run. No candidate list, no health, no reserve, no pool — every one of
/// those was `DisposableRouting`'s, and nothing here consults it.
///
/// [`glasshouse::memory::extract::model::configured_extraction_model`] does
/// the resolving and the credential-label/timing stamping; this wrapper turns
/// its two outcomes (unset key, key set but unusable) into the two notices
/// [`crate::commands::memory_extraction::noting_missing_consent`] appends,
/// and folds in the withheld-credential notice exactly as before.
// History: design-decisions.md, "Trims: commands module docs, third packet", routing_classification.rs `disposable_extraction_model`.
pub(crate) fn disposable_extraction_model(
    runtime: &Runtime,
    _session: &glasshouse::session::SessionId,
) -> Box<dyn glasshouse::memory::ExtractionModel> {
    let user = match UserConfig::load(runtime.paths()) {
        Ok(user) => user,
        Err(err) => {
            tracing::debug!(error = %err, "could not read configuration for memory extraction");
            return Box::new(crate::commands::memory_extraction::NoExtractionModel);
        }
    };
    let project = match config::load_project_config(runtime.project()) {
        Ok(project) => project,
        Err(err) => {
            tracing::debug!(
                error = %err,
                "could not read project configuration for memory extraction"
            );
            return Box::new(crate::commands::memory_extraction::NoExtractionModel);
        }
    };
    let gateway = config::GatewayCatalogue::for_paths(runtime.paths()).unwrap_or_default();
    let effective = EffectiveConfig::with_gateway(&user, project.as_ref(), &gateway);
    let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();

    let chosen = effective.memory_extraction_model().value;
    let model: Box<dyn glasshouse::memory::ExtractionModel> = match &chosen {
        Some(chosen) => {
            match glasshouse::memory::extract::model::configured_extraction_model(
                &user,
                project.as_ref(),
                chosen,
            ) {
                Ok(model) => model,
                Err(reason) => crate::commands::memory_extraction::noting_missing_consent(
                    Box::new(crate::commands::memory_extraction::NoExtractionModel),
                    format!("the configured memory-extraction model {reason}"),
                ),
            }
        }
        None => crate::commands::memory_extraction::noting_missing_consent(
            Box::new(crate::commands::memory_extraction::NoExtractionModel),
            extraction_model_unset_notice(),
        ),
    };

    // Map line 488, said out loud: when the extraction provider's credential
    // resolves from nowhere this hook can see, the model's own description —
    // the string the outcome stores and the hook's stderr line prints —
    // carries the notice. `only` never names a provider the decision did not
    // choose: the configured provider when one is named, else every
    // configured provider.
    let only = chosen.as_ref().map(|chosen| chosen.provider().to_owned());
    let withheld = withheld_provider_credentials(&effective, &secrets, only.as_deref());
    crate::commands::memory_extraction::noting_withheld_credentials(model, &withheld)
}

/// The notice [`disposable_extraction_model`] appends when `[memory]
/// extraction_model` names nothing at all — map line 488's sibling for
/// consent rather than credentials.
fn extraction_model_unset_notice() -> String {
    "no memory-extraction model is configured — set `[memory] extraction_model = { provider = \
     \"...\", model = \"...\" }` in the user config or the project's .glasshouse/config.toml to \
     let Glasshouse run it"
        .to_owned()
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
    let gateway = config::GatewayCatalogue::for_paths(runtime.paths()).unwrap_or_default();
    EffectiveConfig::with_gateway(&user, project.as_ref(), &gateway)
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
    let gateway = config::GatewayCatalogue::for_paths(runtime.paths()).unwrap_or_default();
    EffectiveConfig::with_gateway(&user, project.as_ref(), &gateway)
        .memory_extraction_diagnostics()
        .value
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
        &glasshouse::provider::telemetry::GatewayQuotaCache::new(
            runtime.paths().gateway_data_dir(),
        ),
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
    // re-ranking every call. This roots the cache at the project's own
    // `RuntimePaths::project_state_dir`, unlike the account-scoped
    // `GatewayQuotaCache` above, so a pick never leaks between projects.
    let sticky_cache = RoutingStickyCache::new(
        &runtime
            .paths()
            .project_state_dir(runtime.project().id().as_str()),
    );
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
    let gateway = config::GatewayCatalogue::for_paths(runtime.paths()).unwrap_or_default();
    let effective = EffectiveConfig::with_gateway(&user, project.as_ref(), &gateway);

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
