//! `commands::shared` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use glasshouse::Runtime;
use glasshouse::config::{self, EffectiveConfig, ProjectConfig, UserConfig};
use glasshouse::session::{SessionDisposition, SessionRecord};

/// An objective as one table cell: first line only, and bounded.
///
/// A checkpoint's objective is free text a person wrote and may well be a
/// paragraph. A listing that let one row become forty would be unreadable, so
/// the table shows the first line and `checkpoint show` prints the rest.
pub(crate) fn one_line(text: &str) -> String {
    const WIDTH: usize = 60;
    let first = text.lines().next().unwrap_or("").trim();
    if first.chars().count() <= WIDTH {
        return first.to_owned();
    }
    // By characters, never by bytes: cutting a multi-byte character in half
    // would put invalid text on a terminal.
    let cut: String = first.chars().take(WIDTH - 1).collect();
    format!("{cut}…")
}

/// Enough of an identifier to name a session in conversation.
///
/// The full identifier stays available in `--log-level` output and is what any
/// command taking a session takes; this is only for the eye.
pub(crate) fn short_id(id: &glasshouse::session::SessionId) -> String {
    id.as_str().chars().take(12).collect()
}

/// Which of the four categories a session list has to separate.
///
/// One function, used by both the listing and the detail view, so the two can
/// never disagree about whether a session is resumable.
pub(crate) fn disposition_word(record: &SessionRecord) -> &'static str {
    match record.disposition() {
        SessionDisposition::Active => "active",
        SessionDisposition::Resumable => "resumable",
        SessionDisposition::Closed => "closed",
        SessionDisposition::Failed => "failed",
    }
}

/// A rough "how long ago", which is what a session list is actually read for.
pub(crate) fn format_age(timestamp: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    // A timestamp in the future is possible — a clock corrected backwards
    // between writing the row and reading it — and produces a negative value
    // here, because `saturating_sub` saturates at `i64::MIN`, not at zero. The
    // first arm covers it: reporting "just now" is the honest answer, and it
    // avoids printing a confident negative age. An explicit `< 0` guard used
    // to sit here returning the same string, which only obscured that.
    let seconds = now.saturating_sub(timestamp);
    match seconds {
        s if s < 60 => "just now".to_owned(),
        s if s < 3_600 => format!("{}m ago", s / 60),
        s if s < 86_400 => format!("{}h ago", s / 3_600),
        s => format!("{}d ago", s / 86_400),
    }
}

/// Phase 56 line 1954's *"announce which subscription served each session"*,
/// said on stderr beside the launch/resume announcements, before the session
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
///
/// Relocated from `commands::routing_destinations` (Phase 59 decomposition;
/// design-decisions.md, 2026-09-16, "Glasshouse never decides which model is
/// used") since that module is scheduled for deletion and carries no ranking
/// logic of its own — it is pure rendering, called by both `launch::launch_session`
/// and `resume::resume_session`.
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
///
/// Relocated from `commands::routing_destinations` alongside
/// [`announce_entitlement`], for the same reason.
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

/// What `routing_observations.purpose` records for a memory-extraction call
/// — capability map line 1832. Aliased from the ledger's own constant.
///
/// Relocated from `commands::routing_classification` (design-decisions.md,
/// 2026-09-16, "Glasshouse never decides which model is used") alongside the
/// rest of this file's block: it still has real production callers
/// (`disposable_extraction_model` below), it just carries no ranking logic.
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
/// doors — `commands::launch` and `glasshouse::api::unix::select_memory`,
/// which is a library module that cannot call anything in this binary crate.
/// Putting the logic in the library is what lets both doors call the same
/// seat; this is the thin wrapper that keeps it named beside its sibling
/// here.
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

/// One resource `context_firewall::disposable_reducer` may filter to by
/// name — a trimmed survivor of the deleted
/// `routing::disposable::DisposableCandidate` (2026-09-16 ruling): this no
/// longer feeds a ranking policy, only that filter-to-what-is-named join, so
/// the capacity/price/classification/latency facets only a ranking policy
/// ever read are not carried forward.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DisposableCandidate {
    provider: String,
    model: String,
    locality: Option<glasshouse::provider::registry::Locality>,
    entitlement: Option<glasshouse::routing::Entitlement>,
}

impl DisposableCandidate {
    pub(crate) fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            locality: None,
            entitlement: None,
        }
    }

    #[must_use]
    pub(crate) fn with_entitlement(
        mut self,
        entitlement: Option<glasshouse::routing::Entitlement>,
    ) -> Self {
        self.entitlement = entitlement;
        self
    }

    pub(crate) fn entitlement(&self) -> Option<&glasshouse::routing::Entitlement> {
        self.entitlement.as_ref()
    }

    #[must_use]
    pub(crate) fn with_locality(
        mut self,
        locality: glasshouse::provider::registry::Locality,
    ) -> Self {
        self.locality = Some(locality);
        self
    }

    pub(crate) fn locality(&self) -> Option<glasshouse::provider::registry::Locality> {
        self.locality
    }

    pub(crate) fn provider(&self) -> &str {
        &self.provider
    }

    pub(crate) fn model(&self) -> &str {
        &self.model
    }
}

/// Every resource `context_firewall::disposable_reducer` may filter to by
/// name — free and metered alike — built the same way `build_settings`
/// builds a `ProviderRow`'s configuration in `shell/mod.rs`: a provider's
/// whole configuration comes from whichever layer actually holds its name,
/// project winning over user.
///
/// Relocated from `commands::routing_classification` (design-decisions.md,
/// 2026-09-16): it still has a real production caller
/// (`context_firewall::disposable_reducer`) that filters this list down to
/// what `[context_firewall] reducer`/`reducer_model` name, never to rank —
/// ranking among what is left is exactly the decision the ruling takes away.
/// A budget-exhausted provider is still excluded from a metered candidate
/// here (map line 1519); capacity bands and prices are not, since nothing
/// downstream of this deletion reads them any more.
pub(crate) fn disposable_candidates(
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    effective: &EffectiveConfig<'_>,
    secrets: &dyn glasshouse::secret::SecretStore,
    telemetry: &glasshouse::provider::resources::GatheredTelemetry,
) -> Vec<DisposableCandidate> {
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
        // Map line 1519, for support work: a provider whose own money budget
        // has been counted as exhausted excludes its metered candidates
        // here, before one exists at all.
        let budget_exhausted =
            glasshouse::provider::resources::budget_exhausted_for(&name, effective, telemetry);
        // Map lines 1427 and 1438: where this provider's compute runs, from
        // the one place this build already says so — the registry's
        // local-inference slugs — never from a base URL that happens to
        // point at loopback.
        let locality =
            glasshouse::provider::registry::ResourceKind::from_direct_provider(name.as_str())
                .locality();
        // Map line 1947's job-kind clause: the entitlement charged for work
        // sent to this provider.
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
        let any_credential_resolves = provider_config.credential_env().iter().any(|var| {
            secrets
                .resolve(&SecretRef::Environment { var: var.clone() })
                .is_some()
        });
        if !any_credential_resolves {
            continue;
        }
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
            candidates.push(
                DisposableCandidate::new(name.clone(), model.clone())
                    .with_locality(locality)
                    .with_entitlement(entitlement.clone()),
            );
        }
    }
    candidates
}
