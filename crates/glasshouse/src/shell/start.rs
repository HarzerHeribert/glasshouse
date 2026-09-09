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

use crate::Runtime;
use crate::config::{self, EffectiveConfig, UserConfig};
use crate::integrations::IntegrationId;
use crate::launch::HarnessLaunch;
use crate::pty::TerminalSize;
use crate::session::{
    self, NewSession, ProjectSessions, SessionId, SessionLifecycle, SessionPresentation,
    SessionRuntime,
};

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
    let user = UserConfig::load(app_runtime.paths())?;
    let project_config = config::load_project_config(app_runtime.project())?;
    let effective = EffectiveConfig::new(&user, project_config.as_ref());
    let selection = session::select::select(harness.map(IntegrationId::slug), effective)?;

    let store = sessions.store();
    let native = selection
        .assigns_native_session_id()
        .then(|| store.new_native_session_id())
        .transpose()?;

    // Phase 9A line 368. The shell's quick-open resolves no launch profile or
    // response request of its own, so both take the implied defaults: the
    // `Native` profile and the `Interactive` role — the same kind of answer
    // `glasshouse launch <harness>` records unadorned, not `-` for every
    // column `main.rs::launch_session` fills in.
    let launch_profile = crate::profile::LaunchProfile::native(selection.id());
    let pairing = {
        use crate::harness::Declared;
        use crate::harness::pairing::{PairingQuery, ServingRoute, classify};
        use crate::routing::AssignedModel;

        // The same fallback `main.rs::session_pairing` builds for `Native`:
        // `pairing_queries` never lists it, so a lookup here would always
        // miss anyway.
        let query = PairingQuery {
            harness: launch_profile.harness,
            model: AssignedModel::HarnessDefault,
            route: ServingRoute {
                provider: None,
                gateway: None,
                protocol: None,
            },
            tool_calls: Declared::Unverified,
            provider_protocols: Vec::new(),
        };
        classify(&query, &effective.pairing_overrides())
    };
    let response_profile =
        effective.response_profile(&config::response::ResponseRequest::default());
    for problem in response_profile.problems() {
        // `eprintln!` would corrupt the alternate-screen viewport this
        // process owns — the diagnostic channel every shell warning uses.
        tracing::warn!(problem, "could not read part of the response profile");
    }
    let response_application =
        crate::harness::response::apply(selection.adapter(), response_profile.resolved());

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
            ))),
    )?;

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
    let mut launch = HarnessLaunch::new(selection.into_executable(), app_runtime.project())
        .args(args)
        .size(size)
        .without_provider_credentials(&effective);
    // Map lines 1973 and 488: the scrubs `launch_session` applies — the child
    // inherits neither another entitlement's credential variable from this
    // process's environment nor any configured provider's.
    let entitlement =
        match effective.entitlement_for(launch_profile.harness, &launch_profile.backend) {
            Ok(entitlement) => entitlement,
            Err(err) => {
                tracing::warn!(
                    session = %record.id,
                    error = %err,
                    "could not resolve the serving entitlement for the credential scrub"
                );
                None
            }
        };
    for var in effective.foreign_entitlement_credential_vars(entitlement.as_ref().map(|e| e.name()))
    {
        launch = launch.env_remove(var);
    }
    let launch = launch;
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
