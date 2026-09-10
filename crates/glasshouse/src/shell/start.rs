//! Starting a session from the shell.
//!
//! Split out of `shell/mod.rs` on 2026-09-08 because that file had reached the
//! Phase 59 ceiling and a `mod.rs` is dispatch and composition only. A pure
//! move: the function below is byte-identical to the one that lived there, and
//! its callers reach it through `use` rather than by having changed.
//!
//! BOUNDARY: three `#[cfg(test)]` scans read `shell/mod.rs` as a string and
//! assert on what they find in it — `launch.rs`'s
//! `every_production_harness_launch_site_strips_provider_credentials`,
//! `session/lifecycle.rs`'s writer scan, and `shell/tests/mod_tests.rs`. Each
//! one's include list names this file too, or the launch site below stops
//! being scanned and the count that guards it silently covers less.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, anyhow};

use crate::Runtime;
use crate::config::{self, EffectiveConfig, UserConfig};
use crate::integrations::IntegrationId;
use crate::launch::HarnessLaunch;
use crate::profile::{self, BackendResource, LaunchProfile};
use crate::pty::TerminalSize;
use crate::secret::native::PreferNativeSecretStore;
use crate::session::{
    self, NewSession, ProjectSessions, SessionId, SessionLifecycle, SessionPresentation,
    SessionRuntime,
};

use super::state::{Action, ShellState};

/// Process-scoped launch state that must live exactly as long as its child.
pub(super) struct LaunchResources {
    _gateway: Option<crate::gateway::Gateway>,
    _generated: Option<profile::EphemeralConfigs>,
}

#[cfg(test)]
impl LaunchResources {
    pub(super) fn gateway_base_url(&self) -> Option<String> {
        self._gateway
            .as_ref()
            .map(crate::gateway::Gateway::base_url)
    }
}

pub(super) fn launch_profiles(
    app_runtime: &Runtime,
    harness: Option<IntegrationId>,
) -> anyhow::Result<Vec<LaunchProfile>> {
    let user = UserConfig::load(app_runtime.paths())?;
    let project = config::load_project_config(app_runtime.project())?;
    let effective = EffectiveConfig::new(&user, project.as_ref());
    let selection = session::select::select(harness.map(IntegrationId::slug), effective)?;
    session::launch_profile::enabled_profiles(&effective, selection.id())
}

/// Turn a shell start action into either a concrete launch result or the
/// picker needed to finish choosing one. Keeping profile selection beside
/// profile launch leaves the main event loop responsible only for dispatch.
#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_session_start(
    action: &Action,
    app_runtime: &Runtime,
    live: &mut SessionRuntime,
    sessions: &ProjectSessions,
    state: &mut ShellState,
    size: TerminalSize,
    index_snapshots: &mut HashMap<SessionId, session::native_id::IndexSnapshot>,
    resources: &mut HashMap<SessionId, LaunchResources>,
) -> Option<(SessionPresentation, anyhow::Result<SessionId>)> {
    let (presentation, harness, exact_profile) = action.start_request()?;
    let start = match exact_profile {
        Some(profile_name) => Some(start_session_with_profile(
            app_runtime,
            live,
            sessions,
            presentation,
            harness,
            profile_name,
            size,
            index_snapshots,
            resources,
        )),
        None => match launch_profiles(app_runtime, harness) {
            Ok(profiles) if profiles.len() == 1 => Some(start_session(
                app_runtime,
                live,
                sessions,
                presentation,
                harness,
                size,
                index_snapshots,
            )),
            Ok(profiles) => {
                state.open_profile_choice(profiles, presentation);
                None
            }
            Err(err) => {
                if let Some(ids) = session::select::ambiguous_harnesses(&err) {
                    state.open_harness_choice(ids.to_vec(), presentation);
                } else {
                    tracing::warn!(error = %err, "could not prepare a session launch");
                    state.set_status(format!("could not start a session: {err:#}"));
                }
                None
            }
        },
    };
    start.map(|result| (presentation, result))
}

/// Reconcile a completed launch with the shell view. Both presentation modes
/// select the session that was actually created; only an embedded session
/// gives its viewport the keyboard immediately.
pub(super) fn finish_session_start(
    state: &mut ShellState,
    sessions: &ProjectSessions,
    presentation: SessionPresentation,
    start: anyhow::Result<SessionId>,
) {
    match start {
        Ok(id) => {
            if let Ok(records) = sessions.store().list() {
                state.refresh(records);
            }
            // `refresh` reconciles onto the session that was presented before
            // the key, so explicitly follow the identifier just returned.
            let named = super::state::short_session_id(&id);
            if presentation == SessionPresentation::Headless {
                state.select_session(&id);
                state.set_status(format!("started headless session `{named}` — `o` lists it"));
            } else {
                state.session_started(&id);
                state.set_status(format!("started session `{named}`"));
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "could not start a session");
            state.set_status(format!("could not start a session: {err:#}"));
        }
    }
}

/// Resolve a harness, record a new session, and start it — the same
/// selection seam `main.rs: launch_session` uses, minus attaching to this
/// process's own terminal: the shell gives the session the viewport once its
/// output arrives instead. `presentation` is the only difference between `n`
/// and `N` — everything else is shared, so a headless session is an ordinary
/// one not shown. `size` is the viewport's own inner size at the moment `n`
/// was pressed, not the terminal's outer size — see `view::viewport_slot`
/// and `HarnessLaunch::size`: a harness TUI lays itself out from the size it
/// sees at startup, so the wrong geometry draws its first frame short.
///
/// Returns the new session's identifier, because the caller has to select it:
/// `ShellState::refresh` reconciles onto whatever was presented before the
/// key, so a start that returned nothing left the session it created invisible
/// and unaddressable — see the `Action::StartSession` arm.
pub(super) fn start_session(
    app_runtime: &Runtime,
    live: &mut SessionRuntime,
    sessions: &ProjectSessions,
    presentation: SessionPresentation,
    harness: Option<IntegrationId>,
    size: TerminalSize,
    index_snapshots: &mut HashMap<SessionId, session::native_id::IndexSnapshot>,
) -> anyhow::Result<SessionId> {
    let mut resources = HashMap::new();
    start_session_with_profile(
        app_runtime,
        live,
        sessions,
        presentation,
        harness,
        profile::NATIVE_PROFILE_NAME,
        size,
        index_snapshots,
        &mut resources,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn start_session_with_profile(
    app_runtime: &Runtime,
    live: &mut SessionRuntime,
    sessions: &ProjectSessions,
    presentation: SessionPresentation,
    harness: Option<IntegrationId>,
    profile_name: &str,
    size: TerminalSize,
    index_snapshots: &mut HashMap<SessionId, session::native_id::IndexSnapshot>,
    resources: &mut HashMap<SessionId, LaunchResources>,
) -> anyhow::Result<SessionId> {
    let user = UserConfig::load(app_runtime.paths())?;
    let project_config = config::load_project_config(app_runtime.project())?;
    let effective = EffectiveConfig::new(&user, project_config.as_ref());
    let selection = session::select::select(harness.map(IntegrationId::slug), effective)?;

    if !effective.profile_enabled(profile_name).value {
        return Err(anyhow!("launch profile `{profile_name}` is disabled"));
    }
    let launch_profile = effective
        .launch_profile(profile_name, selection.id())?
        .value;
    let synthesized_native = profile_name == profile::NATIVE_PROFILE_NAME;

    let is_gateway = matches!(launch_profile.backend, BackendResource::GlasshouseGateway);
    let mut entitlement = match &launch_profile.backend {
        BackendResource::GlasshouseGateway => {
            session::launch_profile::gateway_entitlement(&effective, &launch_profile, None)?
        }
        BackendResource::DirectProvider { .. } => {
            effective.entitlement_for(launch_profile.harness, &launch_profile.backend)?
        }
        BackendResource::Native => {
            match effective.entitlement_for(launch_profile.harness, &launch_profile.backend) {
                Ok(entitlement) => entitlement,
                Err(err) => {
                    // Preserve quick-open's established best effort: ambiguous
                    // native account metadata must not prevent the harness from
                    // using its own sign-in. With no serving account selected,
                    // the scrub below removes every entitlement credential.
                    tracing::warn!(
                        error = %err,
                        "could not resolve the serving entitlement for a native shell session"
                    );
                    None
                }
            }
        }
    };

    let provider = match &launch_profile.backend {
        BackendResource::DirectProvider { provider } => {
            Some(effective.configured_provider(provider)?.value)
        }
        _ => None,
    };
    let secrets = PreferNativeSecretStore::detect();
    let gateway = crate::gateway::start_if_required_with_degrade_sink(
        &[launch_profile.backend_demand()],
        || {
            session::launch_profile::gateway_upstream(
                &user,
                project_config.as_ref(),
                &effective,
                &secrets,
                entitlement.as_ref().filter(|_| is_gateway),
                app_runtime.paths(),
            )
        },
        Some(crate::provider::telemetry::GatewayQuotaCache::new(
            app_runtime.paths(),
        )),
        crate::routing::evidence::EvidenceLedger::open(app_runtime)
            .map(Arc::new)
            .map_err(|err| tracing::warn!(%err, "routing evidence unavailable"))
            .ok(),
        Some(crate::provider::telemetry::GatewayHealthCache::new(
            app_runtime.paths(),
        )),
        None,
        None,
    )?;
    if is_gateway && entitlement.is_none() {
        entitlement = gateway
            .as_ref()
            .map(|gateway| effective.entitlement_for_provider(gateway.serving_provider()))
            .transpose()?
            .flatten();
    }
    let scoped_secrets = session::launch_profile::EntitlementScopedSecrets::new(
        &secrets,
        &effective,
        entitlement.as_ref().map(|entry| entry.name()),
    );
    let resolution = profile::Resolution {
        adapter: selection.adapter(),
        acknowledged_bypass: effective.bypass_acknowledged(selection.id()).value,
        provider: provider.as_ref(),
        secrets: &scoped_secrets,
    };
    // The reserved native profile is the shell's historical quick-open. It
    // starts the harness with its native argv plus the existing session
    // document only; profile resolution would add automatic-review flags and
    // change that established behavior. Named configured profiles still take
    // the complete shared CLI resolution path below.
    let mut overlay = if synthesized_native {
        None
    } else {
        Some(
            profile::resolve_with_gateway(
                &launch_profile,
                &resolution,
                gateway.as_ref(),
                &session::launch_profile::gateway_pairing(&effective),
            )
            .map_err(anyhow::Error::from)?,
        )
    };

    let store = sessions.store();
    let native = selection
        .assigns_native_session_id()
        .then(|| store.new_native_session_id())
        .transpose()?;

    let response_request = config::response::ResponseRequest {
        session_preset: launch_profile.response_preset.clone(),
        ..Default::default()
    };
    let response_profile = effective.response_profile(&response_request);
    for problem in response_profile.problems() {
        // `eprintln!` would corrupt the alternate-screen viewport this
        // process owns — the diagnostic channel every shell warning uses.
        tracing::warn!(problem, "could not read part of the response profile");
    }
    let response_application =
        crate::harness::response::apply(selection.adapter(), response_profile.resolved());
    let pairing = session::launch_profile::session_pairing(&effective, &launch_profile);

    // Recorded before the process exists and is the single source of truth:
    // `live.start` below gets `record.presentation`, so it cannot disagree.
    let record = store.create(
        NewSession::embedded(selection.id().slug())
            .with_presentation(presentation)
            .with_native_session_id(native.clone())
            .with_launch_profile(Some(launch_profile.name.clone()))
            .with_backend_resource(Some(launch_profile.backend.slug()))
            .with_model(Some(pairing.model().clone()))
            .with_pairing_class(Some(session::session_pairing_class(pairing.class())))
            .with_protocol(Some(session::session_protocol(pairing.route().protocol)))
            .with_response_profile(Some(response_profile.resolved().profile()))
            .with_response_mechanism(Some(session::session_response_mechanism(
                response_application.mechanism(),
            )))
            .with_entitlement(entitlement.as_ref().map(|entry| entry.name().to_owned())),
    )?;

    if let Some(gateway) = gateway.as_ref() {
        gateway.routing().serve_session(record.id.as_str());
    }

    // Before the harness runs — see `index_snapshots` in `run`.
    index_snapshots.insert(
        record.id.clone(),
        session::native_id::snapshot(&record.harness, app_runtime.project().root()),
    );

    tracing::info!(
        session = %record.id,
        harness = selection.id().slug(),
        executable = %selection.executable().path().display(),
        source = %selection.source(),
        "starting a session from the shell"
    );

    // No user arguments here: the shell's `n` opens a session, and anything
    // extra would be a Glasshouse invention rather than something asked for.
    let mut args = selection.start_args(native.as_deref(), Vec::<String>::new());
    // Best effort: a session that reports nothing is still a session, and is
    // a far smaller loss than refusing to start one the user asked for.
    let project_hooks_consent = effective.project_hooks(selection.id()).value;
    // `install_session_document` rather than `install_hooks`: hooks and the
    // response profile now share one document, exactly as
    // `main.rs::launch_session`'s already does.
    let document_args = std::env::current_exe()
        .map_err(anyhow::Error::from)
        .and_then(|program| {
            let report = crate::harness::HookCommand::new(
                program,
                record.id.as_str(),
                app_runtime.session_dir(record.id.as_str()),
                app_runtime.project().root(),
                app_runtime.paths().data_dir(),
                app_runtime.paths().config_dir(),
            );
            selection.install_session_document(
                &report,
                project_hooks_consent,
                &response_application,
            )
        });
    match document_args {
        Ok(document) => {
            args.splice(0..0, document.args);
        }
        Err(err) => {
            tracing::warn!(session = %record.id, error = %err, "could not install lifecycle hooks");
        }
    }
    let generated = match overlay.as_mut() {
        Some(overlay) => Some(
            overlay
                .install(crate::harness::GeneratedConfigSite::new(
                    &app_runtime.session_dir(record.id.as_str()),
                ))
                .context("could not install launch-profile configuration")?,
        ),
        None => None,
    };
    let mut launch = HarnessLaunch::new(selection.into_executable(), app_runtime.project())
        .args(args)
        .size(size)
        .without_provider_credentials(&effective);
    // Map lines 1973 and 488: the scrubs `launch_session` applies — the child
    // inherits neither another entitlement's credential variable from this
    // process's environment nor any configured provider's.
    for var in effective.foreign_entitlement_credential_vars(entitlement.as_ref().map(|e| e.name()))
    {
        launch = launch.env_remove(var);
    }
    let launch = match overlay {
        Some(overlay) => overlay.apply(launch),
        None => launch,
    };
    let launch = crate::launch::with_active_entitlement(
        launch,
        entitlement.as_ref().map(|entry| entry.name()),
    );
    if let Err(err) = live.start(record.id.clone(), record.presentation, &launch) {
        // Never polled for its exit, so its snapshot has nothing to pair with.
        index_snapshots.remove(&record.id);
        if let Err(store_err) = store.set_lifecycle(&record.id, SessionLifecycle::Failed) {
            tracing::warn!(
                session = %record.id,
                error = %store_err,
                "could not record a failed session start"
            );
        }
        return Err(err);
    }
    resources.insert(
        record.id.clone(),
        LaunchResources {
            _gateway: gateway,
            _generated: generated,
        },
    );

    // A shell-started session leaves `Starting` exactly when the CLI path's
    // does, and for the same reason: `live.start` returning `Ok` means
    // `HarnessLaunch::spawn` produced a child, so a harness is serving. The
    // record written above is not that proof — it exists before the process
    // does, and the branch above turns the same record `Failed` when the
    // spawn is what fails. Without this the shell recorded no success
    // transition at all and `n` sat in `Starting` until the session ended.
    //
    // Best effort, `commands::launch`'s precedent: from here the session is
    // real and running, so a store error is a diagnostics problem. Turning it
    // into an error would make a database hiccup look like a harness failure
    // and tear down a session the user is already talking to.
    if let Err(store_err) = store.set_lifecycle(&record.id, SessionLifecycle::Running) {
        tracing::warn!(
            session = %record.id,
            error = %store_err,
            "could not record a started session"
        );
    }

    Ok(record.id)
}
