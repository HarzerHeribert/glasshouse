//! `commands::launch` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use std::ffi::OsString;
use std::process::ExitCode;
use std::sync::Arc;

use glasshouse::config::response::ResponseRequest;
use glasshouse::config::{self, EffectiveConfig, UserConfig};
use glasshouse::events::{LifecycleEvent, ProcessExit};
use glasshouse::guardrails::GuardrailOverride;
use glasshouse::integrations::cmux;
use glasshouse::launch::HarnessLaunch;
use glasshouse::session;
use glasshouse::session::{
    NewSession, ProjectSessions, SessionId, SessionLifecycle, SessionPresentation,
};
use glasshouse::{Cli, Runtime};

/// Phase 9J line 576: the native-pairing preference and corrections in
/// effect, resolved into the form `crate::profile`'s gateway path accepts —
/// see `glasshouse::profile::GatewayPairing`'s own doc comment for why that
/// module cannot resolve this itself. Both of `launch_session`'s and
/// `resolve_resume_overlay`'s gateway-backed launches call this, so a
/// configured preference reaches a resumed session exactly as it reaches a
/// fresh one.
pub(crate) fn resolved_gateway_pairing(
    effective: &EffectiveConfig<'_>,
) -> glasshouse::profile::GatewayPairing {
    glasshouse::session::launch_profile::gateway_pairing(effective)
}

/// Everything a person typed about **where** this session goes and what it
/// boots from — the arguments `launch_session` reads before it resolves
/// anything.
///
/// One type rather than separate parameters because they are one statement:
/// `profile` and `from_checkpoint` are the two ways of saying "a new
/// session" without using that word, and `to`/`fresh` name an existing
/// session or ask for a new one outright (see `launch_session`'s own use of
/// them).
///
/// Ranking a set of candidate destinations (Phase 37) and classifying a
/// `--task` string (Phase 34D) both left this type on 2026-09-16
/// (design-decisions.md, "Glasshouse never decides which model is used"):
/// what remains is the explicit path that was always available as
/// `--no-routing` — `--to` continues exactly the session it names, `--fresh`
/// starts a new one, and naming neither also starts a new one.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LaunchDestination<'a> {
    /// `--profile`: the launch profile a **new** session runs under.
    pub(crate) profile: Option<&'a str>,
    /// `--from-checkpoint`: the handoff a new session opens with.
    pub(crate) from_checkpoint: Option<&'a str>,
    /// `--to`: continue this session, or start a fresh one under the named
    /// profile for a `fresh:<harness>:<profile>` identifier.
    pub(crate) to: Option<&'a str>,
    /// `--fresh`: start a new session rather than continuing one.
    ///
    /// `launch_session` never reads this field: it and `--to` are mutually
    /// exclusive (`cli.rs`'s `conflicts_with`), and with no `--to` a launch
    /// already starts a fresh session, so `--fresh` only ever asks for what
    /// was already going to happen. Kept on this type (`#[allow(dead_code)]`
    /// rather than dropped) so `main.rs`'s `Launch`/`Run` dispatch — frozen
    /// to losing only `task` and `no_routing` this round — still has
    /// somewhere to put the flag's value.
    #[allow(dead_code)]
    pub(crate) fresh: bool,
    /// `--checkpoint-first`: check point the session this work is leaving
    /// before it moves — capability map line 1716.
    pub(crate) checkpoint_first: bool,
}

/// The profile a `fresh:<harness>:<profile>` identifier names, when it names
/// one for `harness`.
///
/// `None` for a recorded session's identifier, and `None` for a fresh
/// identifier belonging to a different harness — which `launch_session`
/// then treats as naming a session id rather than silently reinterpreting it.
fn fresh_destination_profile(
    id: &str,
    harness: glasshouse::integrations::IntegrationId,
) -> Option<&str> {
    let profile = id
        .strip_prefix("fresh:")?
        .strip_prefix(harness.slug())?
        .strip_prefix(':')?;
    // 56A line 1953: a pool candidate's id carries its entitlement after an
    // `@` (`fresh:<harness>:<profile>@<entitlement>`); the profile is the
    // part before it. A profile whose own name contains `@` cannot be named
    // through such an identifier — recorded, not guessed around.
    Some(profile.split_once('@').map_or(profile, |(name, _)| name))
}

// Eight, and the eighth arrived at integration: `external` is Phase 17's and
// `guardrail` is Phase 21K's, written by two packages that never shared a
// tree. Neither belongs in `LaunchDestination` -- that bundle answers *where
// the work goes*, and one of these says where the session is *shown* while
// the other says how hard its premises are *gated*. Folding either in to
// satisfy a lint would put an unrelated fact in a named type.
#[allow(clippy::too_many_arguments)]
pub(crate) fn launch_session(
    runtime: &Runtime,
    harness: Option<&str>,
    destination: LaunchDestination<'_>,
    response: &ResponseRequest,
    headless: bool,
    no_memory: bool,
    external: ExternalPresentation,
    harness_args: &[String],
    guardrail: Option<GuardrailOverride>,
) -> anyhow::Result<ExitCode> {
    let LaunchDestination {
        profile: profile_name,
        from_checkpoint,
        to,
        // `--fresh` and `--to` are mutually exclusive (`cli.rs`'s
        // `conflicts_with`); with no `--to`, this launch starts a fresh
        // session whether or not `--fresh` was also typed, so there is
        // nothing left for this function to read it for.
        fresh: _,
        checkpoint_first,
    } = destination;
    let user = UserConfig::load(runtime.paths())?;
    let project = config::load_project_config(runtime.project())?;
    let gateway = config::GatewayCatalogue::for_paths(runtime.paths())?;
    let effective = EffectiveConfig::with_gateway(&user, project.as_ref(), &gateway);
    let selection = session::select::select(harness, effective)?;
    // Which profile a *new* session would run under: `--to
    // fresh:<harness>:<profile>` or `--profile`, because an identifier a
    // person pasted out of `glasshouse sessions` has to mean the same thing
    // here as it did there.
    let named_profile = to
        .and_then(|id| fresh_destination_profile(id, selection.id()))
        .or(profile_name);
    // A profile the user disabled is not a profile Glasshouse may start,
    // and being asked for it by name is the one case where saying nothing
    // would be worst.
    //
    // Refused *here*, before any pre-flight check, so a refusal costs
    // nothing — no probe, no session record, no process — matching the
    // harness-not-installed refusal below it in `session::select`.
    //
    // Only a name the person supplied is checked. `fresh_profile`'s fallback
    // is the implied Native profile, which nobody asked for and which
    // `profile_enabled` never reports as disabled anyway.
    if let Some(name) = named_profile {
        let enabled = effective.profile_enabled(name);
        if !enabled.value {
            eprintln!(
                "glasshouse: {}",
                config::ProfileDisabled::new(name, enabled.layer)
            );
            return Ok(ExitCode::FAILURE);
        }
    }
    // -----------------------------------------------------------------------
    // Phase 17 lines 754, 755, 757 and 761 — external presentation.
    //
    // Decided after the harness and the profile have been refused or
    // accepted, so a launch that would fail is refused *here*, in this
    // terminal, and never as a pane that opens and dies — and before the
    // router runs, because a launch that hands itself to a pane has not
    // routed anything: the launch inside the pane does all of that, once.
    //
    // Absence is a first-class path: every way cmux can be unavailable is a
    // reason printed and a session that runs embedded, byte for byte as it
    // would have without the flag.
    // -----------------------------------------------------------------------
    // "Here" is wherever this launch was going anyway: the flag asked for
    // a pane on top of that, and without one nothing else changes.
    let here = if headless { "headless" } else { "embedded" };
    let hosted_pane: Option<cmux::PaneRef> = match &external {
        ExternalPresentation::Embedded => None,
        ExternalPresentation::SpawnIn { pane_command } => match cmux::detect() {
            cmux::Availability::Available(control) => {
                return open_cmux_pane(runtime, &control, selection.id().slug(), pane_command);
            }
            cmux::Availability::Absent(reason) => {
                eprintln!("glasshouse: cmux is not available ({reason}); the session runs {here}");
                None
            }
        },
        // A reference given by hand is metadata the caller asserted;
        // recording it asks cmux nothing.
        ExternalPresentation::HostedBy(cmux::PaneRefRequest::Given(pane)) => Some(pane.clone()),
        ExternalPresentation::HostedBy(request @ cmux::PaneRefRequest::Caller) => {
            match cmux::resolve_pane_ref(request, &cmux::detect()) {
                Ok(pane) => Some(pane),
                Err(reason) => {
                    eprintln!("glasshouse: {reason}; the session runs {here}");
                    None
                }
            }
        }
    };
    let fresh_profile = named_profile.unwrap_or(glasshouse::profile::NATIVE_PROFILE_NAME);

    // Capability map line 1712's own words are what every launch does now:
    // `--to` continues exactly the session it names, `--fresh` starts a new
    // one, and naming neither also starts a new one under `fresh_profile`.
    // Nothing here ranks this project's warm sessions against a new one and
    // nothing classifies a task — design-decisions.md, 2026-09-16, "Glasshouse
    // never decides which model is used" (superseding Phase 37's ranking and
    // Phase 34D's `--task` classification, both removed from this path).
    //
    // A `fresh:<harness>:<profile>` identifier falls through instead: it
    // names a session that does not exist yet, and starting it is what the
    // rest of this function already does under `fresh_profile`, which
    // `named_profile` has already read that identifier's profile out of.
    if let Some(id) = to
        && fresh_destination_profile(id, selection.id()).is_none()
    {
        eprintln!("glasshouse: continuing session `{id}` because you named it.");
        if checkpoint_first {
            crate::commands::resume::checkpoint_before_moving(runtime, Some(id))?;
        }
        return crate::commands::resume::resume_session(
            runtime,
            id,
            harness_args,
            headless,
            crate::commands::resume::RouteOnResume::AlreadyRouted,
        );
    }

    // Line 1716, on every path that starts a fresh session rather than
    // continuing one. The flag is a no-op here and says so rather than
    // passing silently, because a person who asked for a checkpoint and got
    // none needs to know which of the two happened.
    if checkpoint_first {
        crate::commands::resume::checkpoint_before_moving(runtime, None)?;
    }

    // The fresh destination names the profile this launch resolves:
    // `--profile`, the profile named inside a `--to fresh:<harness>:<profile>`,
    // or the implied Native one.
    let requested_profile = fresh_profile.to_owned();

    // Resolve the launch profile *before* anything is recorded or started.
    // A refusal here must cost nothing: no session record, no process. See
    // `glasshouse::profile::resolve`'s doc for why a refusal never falls back
    // to a different mode.
    //
    // Resolved *before* the response profile below, on purpose: line 353's
    // sixth axis lives on this profile, and the response request has to be
    // able to read it.
    let launch_profile = match effective.launch_profile(&requested_profile, selection.id()) {
        Ok(resolved) => resolved.value,
        Err(err) => {
            eprintln!("glasshouse: {err}");
            return Ok(ExitCode::FAILURE);
        }
    };

    // Phase 56 line 1954, on the path that starts a session: which
    // entitlement it will be charged to, said before anything is recorded or
    // started. A rule about *this harness* is checked through the same
    // `EntitlementRules::refusal`, and a contradiction in the `[entitlements]`
    // tables is refused here for the same reason a bad profile is: it must
    // cost nothing.
    //
    // The one-account lookup is the whole answer now: this launch names no
    // destination-carried entitlement, so a provider several accounts
    // legitimately back is refused as ambiguous rather than guessed at.
    let is_gateway_backend = matches!(
        launch_profile.backend,
        glasshouse::profile::BackendResource::GlasshouseGateway
    );
    let mut entitlement = if is_gateway_backend {
        match crate::commands::resume::gateway_entitlement(&effective, &launch_profile, None) {
            Ok(entry) => entry,
            Err(err) => {
                eprintln!("glasshouse: {err}");
                return Ok(ExitCode::FAILURE);
            }
        }
    } else {
        match effective.entitlement_for(launch_profile.harness, &launch_profile.backend) {
            Ok(entitlement) => entitlement,
            Err(err) => {
                eprintln!("glasshouse: {err}");
                return Ok(ExitCode::FAILURE);
            }
        }
    };
    // Every backend but the gateway asks and announces right here, before
    // anything else is resolved. A `GlasshouseGateway` profile cannot be
    // asked yet — `entitlement_for` returns `None` for it by construction,
    // because no provider is assigned until the gateway starts below — so
    // its consult, refusal and announcement happen once that provider is
    // known (see `start_if_required_with_degrade_sink`, further down).
    if !is_gateway_backend || entitlement.is_some() {
        if let Some(message) = crate::commands::shared::entitlement_refusal_message(
            entitlement.as_ref(),
            launch_profile.harness,
            &launch_profile.name,
        ) {
            eprintln!("{message}");
            return Ok(ExitCode::FAILURE);
        }
        crate::commands::shared::announce_entitlement(entitlement.as_ref(), &launch_profile, None);
    }

    // Phase 9K: the response profile is resolved *here*, on the production
    // launch path, through the same `EffectiveConfig::response_profile`
    // `glasshouse response` prints — so what a user is shown and what a
    // session gets cannot disagree. Line 617 is why it happens at session
    // creation rather than per turn: the instruction becomes part of the
    // session's system prefix, and moving it later would invalidate the
    // prompt cache on every turn.
    //
    // Phase 9A line 353's sixth axis, given a production caller: a launch
    // profile that names a response preset supplies it at the `Session`
    // layer of `EffectiveConfig::response_stack` — the layer that doc already
    // describes as "a preset named for this session", which is exactly what
    // choosing this profile is. An explicit `--response-preset` (or
    // `--response-role`'s own preset) on the command line is a stronger,
    // one-time statement than a profile's standing default, so it is only
    // consulted when the request came with none of its own. This is
    // deliberately *not* a seventh `PrecedenceLayer`: the map's line 596
    // fixes that chain at six named layers and the box for it is already
    // closed, so a profile's answer has to arrive through one of the six
    // rather than beside them.
    let mut response_request = response.clone();
    if response_request.session_preset.is_none()
        && let Some(preset) = &launch_profile.response_preset
    {
        response_request.session_preset = Some(preset.clone());
    }
    let response_profile = effective.response_profile(&response_request);
    for problem in response_profile.problems() {
        // Reported, never guessed at — see `ResponseProfileEntry`.
        eprintln!("glasshouse: {problem}");
    }
    // Line 605: a session's response profile is always explicit. A worker
    // does not inherit a communication style from whatever started it; the
    // role was resolved above and the mechanism is recorded below.
    //
    // `mut`: `GH-LAUNCH-BRIEFING`'s rung one appends a second additive block
    // onto this same `Application`, below, once the session id exists.
    let mut response_application =
        glasshouse::harness::response::apply(selection.adapter(), response_profile.resolved());
    tracing::info!(
        harness = selection.id().slug(),
        profile = %config::response::one_line(&response_profile),
        mechanism = response_application.mechanism().category(),
        applied = %response_application.mechanism().describe(),
        "resolved the session's response profile"
    );

    // Resolved here, beside the profile, and for the same reason: a bad
    // identifier must cost nothing. No session record, no process — see
    // `glasshouse::profile::resolve`'s doc.
    let bootstrap =
        match crate::commands::resume::resolve_bootstrap_prompt(runtime, from_checkpoint) {
            Ok(prompt) => prompt,
            Err(err) => {
                eprintln!("glasshouse: {err:#}");
                return Ok(ExitCode::FAILURE);
            }
        };

    let acknowledged_bypass = effective.bypass_acknowledged(selection.id()).value;
    // A direct-provider profile names a provider; the *lookup* is the
    // caller's job, so `glasshouse::profile` never has to import
    // `glasshouse::config`. An unknown name is reported exactly as an unknown
    // profile name is, one step above: a line on stderr, `ExitCode::FAILURE`,
    // nothing recorded and nothing started.
    let provider = match &launch_profile.backend {
        glasshouse::profile::BackendResource::DirectProvider { provider } => {
            match effective.configured_provider(provider) {
                Ok(resolved) => Some(resolved.value),
                Err(err) => {
                    eprintln!("glasshouse: {err}");
                    return Ok(ExitCode::FAILURE);
                }
            }
        }
        _ => None,
    };
    // Phase 9E: prefer the operating system's own secure store where one is
    // available, and fall back to the environment where it is not — the
    // fallback is *labelled* rather than silent, so `glasshouse doctor` and
    // the settings surface both say which store answered.
    //
    // This is the line that puts the native store on the path that actually
    // starts a session. Without it "prefer the macOS Keychain" would be true
    // of the store, of `doctor` and of settings, but not of `glasshouse run`
    // — and a mechanism with no production caller does not get its box.
    let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();

    // Phase 9G: whether a local gateway exists at all is decided from the
    // active launch profiles, never from a flag — see
    // `glasshouse::gateway::gateway_is_required`. It now has to be bound
    // *before* the resolution below, because a gateway-backed profile
    // resolves into this gateway's own address and token. Nothing is bound
    // and no credential is resolved for a launch that needs no gateway: the
    // upstream is a closure, called only after the predicate says yes.
    // The guard lives to the end of this function, so the listener goes away
    // with the instance on every path out.
    //
    // Map line 1735: the relay is built here, before the gateway, because the
    // sink has to exist before the thing it writes into does — see
    // `DegradeRelay`. It is installed below, once the session record and the
    // event recorder are both real.
    use crate::commands::resume::evidence_ledger;
    let degrade_relay = crate::commands::resume::DegradeRelay::new();
    let gateway = match glasshouse::gateway::start_if_required_with_degrade_sink(
        &[launch_profile.backend_demand()],
        || {
            crate::commands::resume::gateway_upstream(
                &user,
                project.as_ref(),
                &effective,
                &secrets,
                entitlement.as_ref().filter(|_| is_gateway_backend),
                runtime.paths(),
            )
        },
        Some(glasshouse::provider::telemetry::GatewayQuotaCache::new(
            runtime.paths().data_dir(),
        )),
        // Capability map lines 1311/1321/1322/1324: the durable resource-
        // health cache, the same additive shape as the quota cache above and
        // read back by exactly the same `glasshouse resources` invocation.
        Some(glasshouse::provider::telemetry::GatewayHealthCache::new(
            runtime.paths().data_dir(),
        )),
        // Phase 33A: the routing evidence ledger, reached from the shipped
        // binary only here — the same shape `GatewayQuotaCache` had for a
        // batch before `QUOTA-LIVE` wired it.
        //
        // **Never `?`.** This argument is evaluated on every launch, gateway
        // or not, and a ledger that cannot be opened must cost an observation
        // rather than the user's session. Telemetry is the one subsystem in
        // this binary whose failure is always survivable, and a `?` here would
        // make a read-only data directory or a locked database into "glasshouse
        // will not start". The relay beside it receives what the ledger
        // does not record.
        glasshouse::routing::evidence::optional_observation_sink(
            evidence_ledger(runtime, std::slice::from_ref(&launch_profile)),
            Some(degrade_relay.sink()),
        ),
        // Capability map line 1851's producer (`FailoverPrevented`) is on
        // the removal list with the ranking it measured — design-decisions.md,
        // 2026-09-16, "Glasshouse never decides which model is used".
        // `None` reproduces `start_if_required_with_degrade_sink`'s own
        // pre-1851 behaviour.
        None,
    ) {
        Ok(gateway) => gateway,
        Err(err) => {
            eprintln!("glasshouse: {err}");
            return Ok(ExitCode::FAILURE);
        }
    };

    if is_gateway_backend && entitlement.is_none() {
        let gateway_provider = gateway
            .as_ref()
            .and_then(|gateway| gateway.serving_provider());
        entitlement = match &gateway_provider {
            Some(provider) => match effective.entitlement_for_provider(provider) {
                Ok(entry) => entry,
                Err(err) => {
                    eprintln!("glasshouse: {err}");
                    return Ok(ExitCode::FAILURE);
                }
            },
            None => None,
        };
        if let Some(message) = crate::commands::shared::entitlement_refusal_message(
            entitlement.as_ref(),
            launch_profile.harness,
            &launch_profile.name,
        ) {
            eprintln!("{message}");
            return Ok(ExitCode::FAILURE);
        }
        crate::commands::shared::announce_entitlement(
            entitlement.as_ref(),
            &launch_profile,
            gateway_provider.as_deref(),
        );
    }

    // An unpinned gateway is intentionally the legacy API-provider shape:
    // broker use is opt-in per profile or selected explicitly by routing.
    // Only this legacy branch waits for the gateway's serving provider;
    // broker identity was already resolved, announced, and started by its
    // exact entitlement name above.
    // 56A line 1969: the overlay may only resolve the serving account's own
    // credential — see `EntitlementScopedSecrets`. With zero or one
    // configured entitlement the foreign list is empty or names other
    // resources' accounts, and resolution answers exactly as before. The
    // gateway's own upstream resolution above deliberately keeps the
    // unwrapped store: which account serves a gateway-backed session is
    // assigned when the session starts (56A-4), not at this launch.
    let scoped_secrets = glasshouse::session::launch_profile::EntitlementScopedSecrets::new(
        &secrets,
        &effective,
        entitlement.as_ref().map(|entry| entry.name()),
    );
    let resolution = glasshouse::profile::Resolution {
        adapter: selection.adapter(),
        acknowledged_bypass,
        provider: provider.as_ref(),
        secrets: &scoped_secrets,
    };
    // Phase 9J line 576: the user's configured native-pairing preference and
    // corrections, resolved here — the same place `provider` above is — and
    // handed to the gateway path rather than looked up inside `profile/**`,
    // which may not import `crate::config`. See `resolved_gateway_pairing`.
    let pairing = resolved_gateway_pairing(&effective);
    let mut overlay = match glasshouse::profile::resolve_with_gateway(
        &launch_profile,
        &resolution,
        gateway.as_ref(),
        &pairing,
    ) {
        Ok(overlay) => overlay,
        Err(refusal) => {
            eprintln!("glasshouse: {refusal}");
            return Ok(ExitCode::FAILURE);
        }
    };

    // Phase 9F line 468: verify the combination this profile resolved to
    // before the session starts, when a cheap check is available.
    //
    // **After the resolution, never before it.** The backend is chosen from
    // the profile's declaration alone, and running the check on this side of
    // `resolve_with_gateway` is what makes that structurally true on the
    // production path rather than merely asserted in a unit test — see
    // `profile::preflight`'s own doc and
    // `a_capability_probe_cannot_influence_which_backend_resolve_selects`.
    //
    // And before `ProjectSessions::open` below, which is what "before
    // starting" buys the user: whatever this reports, they read it while
    // nothing has been recorded and no process exists.
    //
    // It reports; it decides nothing. A profile with no check available —
    // every `Native` and every gateway-backed one, so every launch that did
    // not name a direct provider — pays no request and gets one line in the
    // log. A check that fails still starts the session, on purpose: see the
    // four reasons on `profile::Preflight`, of which the shortest is that a
    // `GET` to a base URL serving none answers `404` for a healthy provider.
    let preflight = glasshouse::profile::preflight(&launch_profile, &resolution);
    tracing::info!(
        profile = %launch_profile.name,
        backend = %launch_profile.backend.slug(),
        preflight = preflight.summary(),
        "pre-flight capability check"
    );
    if let Some(warning) = preflight.warning() {
        // Not a refusal, and it must not read like one — the next thing this
        // process does is start the session.
        eprintln!("glasshouse: pre-flight check did not confirm {warning}");
        eprintln!("glasshouse: starting the session anyway; this check never refuses a launch.");
    }

    // Record the session before the harness exists, so a session that dies
    // during startup still leaves a trace. Failing to open the project
    // database is fatal here rather than a warning: `bootstrap` already
    // validated it, so a failure now means the project's state directory
    // broke underneath us, and starting a session Glasshouse cannot account
    // for is worse than not starting one.
    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    // Minted before the process exists, for a harness that accepts one, so
    // the session is identifiable even if the harness dies during startup.
    let native = selection
        .assigns_native_session_id()
        .then(|| store.new_native_session_id())
        .transpose()?;
    // The presentation is recorded before the process exists and is the same
    // value `run_headless` starts the session under, so a session's stored
    // presentation and its running one cannot disagree — which is what lets
    // the shell's overview say `headless` about a session it did not start.
    //
    // `External` when a pane hosts this process (Phase 17 line 760): the
    // runtime below still starts the session as embedded or headless —
    // that is what it *is* to the pane's terminal — and only the record
    // says the pane is where a person will find it.
    let presentation = if hosted_pane.is_some() {
        SessionPresentation::External
    } else if headless {
        SessionPresentation::Headless
    } else {
        SessionPresentation::Embedded
    };
    // Phase 10 line 645: the seven facts, recorded as seven facts.
    //
    // `pairing` is asked once and its three answers are read off separately —
    // the model, the class and the wire protocol — because they are three
    // different questions about the same session and a single "agent" string
    // holding all of them is exactly what this phase's second architectural
    // requirement forbids. The response profile beside them is communication
    // policy and nothing else: it cannot say which model ran, and the model
    // cannot say how the answer should read.
    let pairing = glasshouse::session::launch_profile::session_pairing(&effective, &launch_profile);
    let record = store.create(
        NewSession::embedded(selection.id().slug())
            .with_presentation(presentation)
            .with_presentation_ref(hosted_pane.as_ref().map(|pane| pane.as_str().to_owned()))
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
            // Phase 40 line 1646: the session this one was bootstrapped from,
            // if this launch is a `--from-checkpoint` handoff. `None` for
            // every other launch — a session not started from a checkpoint
            // must never record an invented source.
            .with_source_session(bootstrap.as_ref().map(|(_, source)| source.clone()))
            // Phase 56A line 1972, the durable half: the account that will be
            // charged for this session, recorded by name so that
            // `glasshouse entitlements` can answer *what it served* later.
            //
            // `entitlement` is the value resolved above and already announced
            // to the user by `announce_entitlement` — deliberately the same
            // binding and not a second lookup, so what a person was told and
            // what the record says cannot disagree. Where the router ran that
            // is `Routed::chosen`'s own account re-resolved by name; where it
            // did not (a routing-off launch), it is the one-account lookup,
            // which refuses a several-account provider rather than guessing.
            // Either way it is the entitlement that serves, which is the only
            // thing this column may hold.
            //
            // `backend_resource` above stays exactly as it was: it records
            // the KIND of resource and this records the INSTANCE, and the two
            // accounts of one vendor that motivate the column both slug to
            // `native` there.
            .with_entitlement(entitlement.as_ref().map(|entry| entry.name().to_owned())),
    )?;
    // Capability map line 2019 and `glasshouse::database` migration 24: tell
    // the gateway which session it is serving, so every routing-observation
    // row it writes from here on can name one.
    //
    // **Here, and not beside the gateway's own start**, for
    // `record_routing_decision`'s reason a few lines down: the id does not
    // exist up there. The gateway is started before the record so that the
    // overlay can name its address, and the record is minted by
    // `store.create` — so this is the first line at which both are real, and
    // it is still before the harness is spawned, which is before any
    // exchange can arrive.
    if let Some(gateway) = gateway.as_ref() {
        gateway.routing().serve_session(record.id.as_str());
        // Map line 1301 (`GH-TASK-CLASS-COST-JOIN`): this launch classifies
        // no task, so the gateway stamps `NULL` exactly as it does when
        // `serve_session` above is never called at all.
        gateway.routing().serve_task_class(None);
    }

    // `GH-LAUNCH-BRIEFING`: this project's memory, briefed to this session the
    // same way a door-spawned one already is — map lines 1125-1135, applied
    // to the CLI launch path. After `store.create` (the session id this
    // records against exists) and before `install_session_document` below
    // (rung one still needs to append to `response_application`'s
    // arguments). Rung two (headless, no adapter additive mechanism) cannot
    // be delivered yet — no session runtime exists — so it rides forward as
    // `deferred_briefing` into `run_headless`.
    let launch_briefing = brief_launch_session(
        runtime,
        &record.id,
        selection.adapter(),
        headless,
        no_memory,
        effective.inject_memory_at_launch().value,
        bootstrap.as_ref().map(|(text, _)| text.as_str()),
        &mut response_application,
    );
    let mut deferred_briefing = None;
    match launch_briefing {
        LaunchBriefing::Delivered(line) => eprintln!("glasshouse: {line}"),
        LaunchBriefing::Deferred(briefing) => deferred_briefing = Some(briefing),
        LaunchBriefing::NotBriefed(reason) => eprintln!("glasshouse: not briefed: {reason}"),
        LaunchBriefing::Nothing => {}
    }

    // Phase 21K line 1008: the person's per-task guardrail override,
    // recorded before the harness starts so that no preflight the agent runs
    // in this session answers without it. Best effort, like the hook
    // installation below: a launch is not refused for a bookkeeping row,
    // but the failure is said out loud, because a session gated against the
    // user's stated wish is the one outcome the override exists to prevent.
    if let Some(kind) = guardrail {
        match glasshouse::guardrails::record_override(
            runtime,
            record.id.as_str(),
            kind,
            glasshouse::guardrails::Origin::User,
        ) {
            Ok(row) => tracing::info!(
                session = %record.id,
                guardrail = %kind,
                seq = row.seq,
                "recorded a per-task guardrail override"
            ),
            Err(err) => eprintln!(
                "glasshouse: warning: `--guardrail {kind}` could not be recorded for session \
                 {}: {err:#}",
                record.id
            ),
        }
    }

    // Read before the harness runs, for a harness that keeps its identifiers
    // in one shared index: such an index carries no per-entry timestamp, so
    // "this project's entry changed during the session" is the only thing
    // standing between Glasshouse and adopting a stale entry somebody else's
    // session refreshed. Empty, and free, for every other harness — see
    // `session::native_id::snapshot`.
    let index_before = session::native_id::snapshot(&record.harness, runtime.project().root());

    tracing::info!(
        session = %record.id,
        harness = selection.id().slug(),
        // The resolved path and the layer that chose it are diagnostics a
        // user needs when a session starts the wrong binary. Neither is a
        // secret; harness *arguments* are never logged, because those can
        // carry session tokens.
        executable = %selection.executable().path().display(),
        source = %selection.source(),
        root = %runtime.project().display_root().display(),
        profile = %launch_profile.name,
        backend = %launch_profile.backend.slug(),
        mechanisms = %crate::commands::resume::mechanism_summary(&overlay),
        presentation = %presentation,
        "opening a harness session"
    );

    // Phase 9A line 362. The generated configuration documents this profile
    // needs are written now — the session directory exists only once the
    // record does — into the directory Glasshouse owns for this session, and
    // removed when `_generated` drops at the end of this function, which is
    // after `session::attach` has returned. Fatal rather than best effort: a
    // harness pointed at a configuration document that was not written would
    // start on the user's own account instead of the backend they asked for.
    let session_dir = runtime.session_dir(record.id.as_str());
    let _generated =
        overlay.install(glasshouse::harness::GeneratedConfigSite::new(&session_dir))?;

    // Adapter args (and, for a harness that lets Glasshouse assign one, its
    // session identifier) first — no user arguments yet, so the overlay's
    // arguments land strictly between them and the user's own.
    let mut args = selection.start_args(native.as_deref(), std::iter::empty::<&str>());
    let project_hooks_consent = effective.project_hooks(selection.id()).value;
    args.splice(
        0..0,
        crate::commands::resume::install_session_document(
            runtime,
            &selection,
            &record.id,
            project_hooks_consent,
            &response_application,
        ),
    );
    // Map lines 1991-1996: the context firewall's Claude Code bridge. Never
    // changes `args` itself — it only merges a `PostToolUse` entry into the
    // settings document `install_session_document` just wrote (a second
    // `--settings` flag would silently discard the first, so this can never
    // add one of its own), which keeps `mode = "off"` byte-identical to a
    // session built before this phase existed by construction: the function
    // returns before touching anything in that case.
    //
    // Map lines 2023/2024: the resolved entitlement and this launch's own
    // backend/profile name travel in too, so the reduction policy can be
    // keyed on the entitlement's kind and overridden by the profile or the
    // entitlement — never by the firewall core or the hook subprocess, which
    // stay entitlement-blind (see `install_context_firewall_hook`'s own doc).
    crate::commands::resume::install_context_firewall_hook(
        runtime,
        &selection,
        effective,
        &session_dir,
        entitlement.as_ref(),
        &launch_profile.backend,
        &launch_profile.name,
        &record.id,
    );
    // Map lines 2402-2405: Phase 60's edit-intent coordination hook, merged
    // into the same settings document and after the firewall's own entry —
    // the two touch different event keys, so neither can disturb the other
    // (`claude_code::merge_hook_entry`, pinned by
    // `both_tool_hooks_coexist_in_one_document`). Ordered second so that a
    // failure here is one a session with a working firewall survives; both
    // are best effort and neither touches `args`.
    install_edit_intent_hook(&selection, effective, &session_dir, &record.id);
    let mut launch = HarnessLaunch::new(selection.into_executable(), runtime.project()).args(args);
    // Map line 1973: the child inherits this process's environment, so
    // another entitlement's credential variable would reach a session that
    // account is not serving. Removed before the overlay applies, so the
    // overlay's own `env` entries — the serving credential among them —
    // always win per key.
    for var in effective.foreign_entitlement_credential_vars(entitlement.as_ref().map(|e| e.name()))
    {
        launch = launch.env_remove(var);
    }
    // Map line 488: a configured provider's credential stays inside the
    // Glasshouse process — exactly the names the configuration's providers
    // read from are removed, never a guess list of well-known variables, and
    // before the overlay applies so its own `env` of the same name wins.
    let launch = launch.without_provider_credentials(&effective);
    // The overlay is the only thing that may put its own arguments or
    // environment onto the launch — see `LaunchOverlay::apply`'s doc.
    let launch = overlay.apply(launch);
    let launch = glasshouse::launch::with_active_entitlement(
        launch,
        entitlement.as_ref().map(|entry| entry.name()),
    );
    // A checkpoint's handoff, if one was named, as the harness's opening
    // prompt — exactly where a person typing it after `--` would have put it.
    let launch = match &bootstrap {
        Some((prompt, _)) => launch.args(std::iter::once(prompt.as_str())),
        None => launch,
    };
    // The user's own `--` arguments always come last, so they can win.
    let launch = launch.args(harness_args.iter().map(String::as_str));

    // From here on, a bookkeeping failure must never change what the user
    // sees. The session is real and running; losing a state transition is a
    // diagnostics problem, whereas turning it into an error would make a
    // database hiccup look like a harness failure.
    crate::commands::resume::note_lifecycle(&store, &record.id, SessionLifecycle::Running);

    // Phase 18's "record session creation events", on the path that actually
    // creates one from the command line. The shell's own runtime publishes
    // the same event for a session started there; this is the other entry
    // point, and a log that only knew about one of them would be a log with a
    // hole in it exactly where a user was not using the interactive
    // interface.
    let events = Arc::new(crate::commands::resume::EventRecorder::open(runtime));
    events.record(&record.id, LifecycleEvent::SessionStarted);

    // Map line 1735, the other half of `DegradeRelay`: from here on a failed
    // gateway upstream is recorded against this session, by the gateway's own
    // thread, while the harness below keeps running. The record is the one
    // this process owns and its `backend_resource` was written above, so
    // `degrade_resource` can already tell whether this session was on the
    // resource that failed.
    degrade_relay.install(Arc::clone(&events), vec![record.clone()]);

    let session = if headless {
        crate::commands::resume::run_headless(
            runtime,
            &store,
            &record.id,
            launch,
            deferred_briefing,
        )
    } else {
        session::attach(launch)
    };
    let status = match session {
        Ok(status) => status,
        Err(err) => {
            crate::commands::resume::note_lifecycle(&store, &record.id, SessionLifecycle::Failed);
            return Err(err);
        }
    };

    // The session is over, so this is the tightest the discovery window will
    // ever be — see `session::native_id::capture`'s doc comment.
    session::native_id::capture(&store, &record, runtime.project().root(), &index_before);

    // One definition of "did it crash", and it is `ProcessExit`'s. This used
    // to be an inline `status.success()` split, which is a second place the
    // same classification lived — and two definitions of that eventually
    // disagree about a signal, which is the case that matters least often and
    // costs most when it is wrong.
    let exit = ProcessExit::from_status(&status);
    events.record(
        &record.id,
        LifecycleEvent::ProcessExited { exit: exit.clone() },
    );
    crate::commands::resume::note_lifecycle(&store, &record.id, exit.session_state());

    if !status.success() {
        // The harness failing is not Glasshouse failing, so this is a plain
        // note on stderr rather than an error: the exit code below already
        // carries the outcome to whatever invoked Glasshouse.
        eprintln!("glasshouse: the harness {status}");
    }
    Ok(crate::commands::resume::exit_code_for(&status))
}

/// A briefing selected for a launch but not yet delivered — `GH-LAUNCH-BRIEFING`'s
/// rung two, handed from [`brief_launch_session`] to [`run_headless`] because
/// nothing can deliver it until a session runtime holds the PTY.
#[derive(Debug)]
pub(crate) struct DeferredBriefing {
    pub(crate) injection: glasshouse::memory::inject::Injection,
    binding: usize,
    failed_attempts: usize,
}

impl DeferredBriefing {
    /// The line printed once this briefing is actually delivered — shared
    /// between the rung-one and rung-two paths so the two report identically.
    pub(crate) fn announcement(&self) -> String {
        briefing_announcement(
            self.injection.memories().len(),
            self.binding,
            self.failed_attempts,
        )
    }
}

/// Map lines 2402-2405: register Phase 60's edit-intent `PreToolUse` hook
/// for a Claude Code session, unless a configuration layer turned
/// coordination off. Never a second `--settings` flag (Claude Code keeps
/// only the last one), so this reads the document `install_session_document`
/// already wrote, adds one `PreToolUse` key, and writes it back in place;
/// `args` is never touched.
///
/// `mode = "off"` installs nothing at all — line 2405's own words, and the
/// reason this returns before reading the executable path or the session
/// directory; an inert hook would still spawn a process per `Edit`.
///
/// Best effort: a failure here is a session that starts without
/// coordination rather than one that fails to start, and it is logged
/// rather than propagated. No version floor and no probe: the worst a
/// build that ignores the entry can do is not run it.
///
/// History: design-decisions.md, "Trims: commands module docs", install_edit_intent_hook.
fn install_edit_intent_hook(
    selection: &session::HarnessSelection,
    effective: EffectiveConfig<'_>,
    session_dir: &std::path::Path,
    session: &SessionId,
) {
    use glasshouse::config::firewall::EditIntentMode;
    use glasshouse::harness::claude_code;

    if selection.id() != glasshouse::integrations::IntegrationId::ClaudeCode {
        // Map line 2404: where a harness exposes no structured pre-tool
        // hook, the feature is simply absent for that harness and nothing is
        // substituted for it. `glasshouse doctor` says so out loud per
        // adapter (`integrations::write_adapter_report`); this line is the
        // per-launch trace, at `debug` so it is not spam.
        tracing::debug!(
            harness = selection.id().slug(),
            "edit intent: no verified PreToolUse hook for this harness; coordination is              absent for this session"
        );
        return;
    }

    if effective.edit_intent_mode().value == EditIntentMode::Off {
        return;
    }

    let program = match std::env::current_exe() {
        Ok(program) => program,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "edit intent: could not find the Glasshouse executable; not registered"
            );
            return;
        }
    };

    let command_line = claude_code::edit_intent_command_line(&program, session.as_str());
    let hook_entry = claude_code::edit_intent_hook_entry(&command_line);
    let path = session_dir.join(claude_code::SETTINGS_FILE_NAME);
    let existing = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) => {
            tracing::warn!(
                error = %err,
                path = %path.display(),
                "edit intent: could not read the settings document to merge its hook into;                  not registered"
            );
            return;
        }
    };
    match claude_code::merge_edit_intent_hook(&existing, &hook_entry) {
        Ok(merged) => {
            if let Err(err) = std::fs::write(&path, merged) {
                tracing::warn!(
                    error = %err,
                    path = %path.display(),
                    "edit intent: could not write the merged settings document; not registered"
                );
            }
        }
        Err(err) => {
            tracing::warn!(
                error = %err,
                "edit intent: could not merge the PreToolUse hook; not registered"
            );
        }
    }
}

fn briefing_announcement(memories: usize, binding: usize, failed_attempts: usize) -> String {
    format!(
        "briefed with {memories} memories ({binding} binding, {failed_attempts} failed approaches)"
    )
}

/// What `GH-LAUNCH-BRIEFING`'s delivery ladder decided for one launch — map
/// lines 1125-1135's briefing, applied to `glasshouse launch` itself rather
/// than only to a door-spawned session (`docs/product/design-decisions.md`,
/// *Memory is the project's, not the launch path's*).
///
/// Every variant except [`Self::Deferred`] is a launch that already knows its
/// final outcome; [`Self::Deferred`] is the one rung whose delivery depends on
/// a session runtime that does not exist yet.
#[derive(Debug)]
pub(crate) enum LaunchBriefing {
    /// Rung one: delivered by riding the adapter's own additive mechanism,
    /// already appended to the response application's arguments.
    Delivered(String),
    /// Rung two: no additive mechanism, but this launch is headless, so a
    /// session runtime will hold the PTY and can carry the door's own
    /// labelled machine message once it starts — see [`run_headless`].
    Deferred(DeferredBriefing),
    /// Rung three: neither exists for this launch.
    NotBriefed(String),
    /// The opt-out fired, or there was nothing this project's memory had to
    /// say. Not an error and not announced as one — a launch with memory
    /// disabled or empty must read exactly as it did before this feature
    /// existed.
    Nothing,
}

/// `GH-LAUNCH-BRIEFING`: select and, where a rung can deliver it immediately,
/// deliver this project's memory to a session `glasshouse launch` is about to
/// start — the same briefing a door-spawned session already gets (map lines
/// 1125-1135), applied to the CLI launch path the design ruling found never
/// called it at all.
///
/// Called in `launch_session` between `store.create` (`session` exists) and
/// `install_session_document` (`response_application`'s arguments are read),
/// so a rung-one delivery can still ride `response_application`.
///
/// `query` is the checkpoint's bootstrap text when this launch resumes one —
/// [`glasshouse::memory::inject::select_briefing`]'s `Some` case — and `None`
/// otherwise, which selects the standing set instead of running no search at
/// all.
#[allow(clippy::too_many_arguments)]
pub(crate) fn brief_launch_session(
    runtime: &Runtime,
    session: &SessionId,
    adapter: &dyn glasshouse::harness::HarnessAdapter,
    headless: bool,
    no_memory: bool,
    inject_at_launch: bool,
    query: Option<&str>,
    response_application: &mut glasshouse::harness::response::Application,
) -> LaunchBriefing {
    use glasshouse::memory::inject::{self, BriefingOutcome};
    use glasshouse::memory::{MemoryAuthority, MemoryKind, ProjectMemory};

    // Opt-out, not opt-in (the design ruling's own wording): neither the
    // store nor anything else on this path is even touched, so a launch with
    // memory disabled is byte-identical to one built before this feature
    // existed.
    if no_memory || !inject_at_launch {
        return LaunchBriefing::Nothing;
    }

    let project = match ProjectMemory::open(runtime) {
        Ok(project) => project,
        Err(err) => {
            tracing::warn!(
                session = %session,
                error = %format!("{err:#}"),
                "could not open this project's memory to brief a launch"
            );
            return LaunchBriefing::Nothing;
        }
    };
    let rerank_model = crate::commands::shared::disposable_rerank_model(runtime, session);
    let diagnostics = crate::commands::shared::memory_retrieval_diagnostics_enabled(runtime)
        .then_some(inject::DiagnosticsRequest {
            runtime,
            session: Some(session.as_str()),
        });
    let outcome = match inject::select_briefing_traced(
        &project.store(),
        query,
        &std::collections::HashSet::new(),
        rerank_model.as_deref(),
        diagnostics,
        Some(runtime.project().root()),
        // `None`: line 1129's confidence is `GH-INJECTION-CONFIDENCE`'s
        // scope for the machine door's `select_memory` alone — this launch
        // path is a different caller and is not part of that package.
        None,
    ) {
        Ok((outcome, _trace)) => Some(outcome),
        Err(err) => {
            tracing::warn!(
                session = %session,
                error = %err,
                "could not select project memory to brief a launch"
            );
            None
        }
    };

    let (injection, binding, failed_attempts) = match outcome {
        Some(BriefingOutcome::Injected(injection)) => {
            // Counted while the connection is still open, using the ids the
            // selection just chose — cheap (at most `MAX_INJECTED_MEMORIES`
            // lookups) and avoids a second retrieval implementation ranking
            // candidates a second way.
            let mut binding = 0usize;
            let mut failed_attempts = 0usize;
            for id in injection.memories() {
                if let Ok(Some(record)) = project.store().get(id) {
                    if record.authority.is_some_and(MemoryAuthority::is_binding) {
                        binding += 1;
                    }
                    if record.kind == MemoryKind::FailedAttempt {
                        failed_attempts += 1;
                    }
                }
            }
            (injection, binding, failed_attempts)
        }
        Some(BriefingOutcome::NothingMatched) => {
            // Map line 1865: this launch is a briefing door too, so a search
            // that matched nothing is a retrieval miss exactly as it is for
            // the machine door.
            glasshouse::evaluation::record_memory_retrieval_miss(
                runtime,
                glasshouse::evaluation::RetrievalScope::Injection,
                glasshouse::evaluation::now_unix(),
            );
            drop(project);
            return LaunchBriefing::Nothing;
        }
        Some(BriefingOutcome::NothingNew)
        | Some(BriefingOutcome::WithheldLowConfidence(_))
        | None => {
            drop(project);
            return LaunchBriefing::Nothing;
        }
    };
    // Practice §65: the memory connection is dropped before the evaluation
    // ledger below opens, the same shape `select_memory`'s own caller uses.
    drop(project);

    if response_application.append_additive_text(adapter, injection.text()) {
        glasshouse::evaluation::record_memory_retrieval(
            runtime,
            glasshouse::evaluation::RetrievalScope::Injection,
            injection
                .memories()
                .iter()
                .map(glasshouse::memory::MemoryId::as_str),
            Some(session.as_str()),
            glasshouse::evaluation::now_unix(),
        );
        return LaunchBriefing::Delivered(briefing_announcement(
            injection.memories().len(),
            binding,
            failed_attempts,
        ));
    }

    if headless {
        return LaunchBriefing::Deferred(DeferredBriefing {
            injection,
            binding,
            failed_attempts,
        });
    }

    LaunchBriefing::NotBriefed(format!(
        "{} declares no mechanism for adding an instruction beside its own system prompt, and \
         this launch has no session runtime to deliver a machine message through",
        glasshouse::harness::response::harness_name(adapter.id())
    ))
}

/// Where a launch is presented, beyond this terminal — Phase 17 lines 757
/// and 761, decided from `--presentation` and `--presentation-ref` before
/// anything is resolved.
///
/// The two flags are the two sides of one pane: the outer process asks to
/// *spawn into* a backend, and the process it starts inside the pane is told
/// it is *hosted by* one. `clap` refuses both on one command line.
#[derive(Debug)]
pub(crate) enum ExternalPresentation {
    /// Neither flag: the session is shown where it always was.
    Embedded,
    /// `--presentation <backend>`: open a pane and run this launch again
    /// inside it. `pane_command` is the whole command line the pane runs,
    /// already quoted for the shell.
    SpawnIn { pane_command: String },
    /// `--presentation-ref <ref|caller>`: this process is the one inside the
    /// pane; record where it is and otherwise launch normally.
    HostedBy(cmux::PaneRefRequest),
}

/// Read the two flags into an [`ExternalPresentation`], building the pane's
/// command only when one is actually needed.
///
/// An unknown backend and a malformed reference are both refused here, by
/// name, before a harness is selected or a database opened: a launch that
/// cannot say where it wants to be shown has not asked for anything yet.
pub(crate) fn external_presentation(
    backend: Option<&str>,
    reference: Option<&str>,
    pane_command: impl FnOnce() -> anyhow::Result<String>,
) -> anyhow::Result<ExternalPresentation> {
    match (backend, reference) {
        (Some(word), _) => {
            let cmux::Backend::Cmux = cmux::Backend::parse(word)?;
            Ok(ExternalPresentation::SpawnIn {
                pane_command: pane_command()?,
            })
        }
        (None, Some(reference)) => Ok(ExternalPresentation::HostedBy(cmux::PaneRefRequest::parse(
            reference,
        )?)),
        (None, None) => Ok(ExternalPresentation::Embedded),
    }
}

/// The process-wide flags a pane's Glasshouse needs to be *this* Glasshouse:
/// the same project, the same data and configuration directories — resolved
/// values, not whatever the pane's login shell would derive — and the same
/// logging choices. Nothing else: no credential is a flag, and none becomes
/// one here.
pub(crate) fn pane_global_args(cli: &Cli, runtime: &Runtime) -> Vec<OsString> {
    let paths = runtime.paths();
    let mut args: Vec<OsString> = vec![
        "--scope".into(),
        runtime.project().display_root().as_os_str().to_owned(),
        "--data-dir".into(),
        paths.data_dir().as_os_str().to_owned(),
        "--config-dir".into(),
        paths.config_dir().as_os_str().to_owned(),
    ];
    if cli.allow_unsafe_scope {
        args.push("--allow-unsafe-scope".into());
    }
    if let Some(level) = &cli.log_level {
        args.push("--log-level".into());
        args.push(level.into());
    }
    if let Some(file) = &cli.log_file {
        args.push("--log-file".into());
        args.push(file.into());
    }
    if cli.log_stderr {
        args.push("--log-stderr".into());
    }
    args
}

/// The launch a pane runs: the same launch the person typed, minus
/// `--presentation` and plus `--presentation-ref caller`, so the process
/// inside the pane records where it is and otherwise does exactly what this
/// one would have done. One field per flag `launch` takes, so a flag added
/// to `Command::Launch` and not carried here is a compile error at the call
/// site rather than a pane that silently ignores it.
pub(crate) struct PaneLaunch<'a> {
    pub(crate) harness: Option<&'a str>,
    pub(crate) response_profile: Option<&'a str>,
    pub(crate) response_role: Option<&'a str>,
    pub(crate) profile: Option<&'a str>,
    pub(crate) from_checkpoint: Option<&'a str>,
    pub(crate) to: Option<&'a str>,
    pub(crate) fresh: bool,
    pub(crate) headless: bool,
    pub(crate) harness_args: &'a [String],
}

pub(crate) fn pane_launch_args(launch: PaneLaunch<'_>) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["launch".into()];
    if let Some(harness) = launch.harness {
        args.push(harness.into());
    }
    for (flag, value) in [
        ("--response-profile", launch.response_profile),
        ("--response-role", launch.response_role),
        ("--profile", launch.profile),
        ("--from-checkpoint", launch.from_checkpoint),
        ("--to", launch.to),
    ] {
        if let Some(value) = value {
            args.push(flag.into());
            args.push(value.into());
        }
    }
    if launch.fresh {
        args.push("--fresh".into());
    }
    if launch.headless {
        args.push("--headless".into());
    }
    args.push("--presentation-ref".into());
    args.push("caller".into());
    if !launch.harness_args.is_empty() {
        args.push("--".into());
        args.extend(launch.harness_args.iter().map(OsString::from));
    }
    args
}

/// Open a cmux workspace in the project root running `pane_command`, wait
/// briefly for the session inside it to record itself, and say what
/// happened — Phase 17 lines 757 and 761.
///
/// This process starts nothing else: no harness, no record, no runtime. The
/// pane hosts a normal launch, and that launch is what writes the session
/// down (with `External` and the workspace it asked cmux for). The wait is
/// bounded and its expiry is reported, not treated as failure — the pane is
/// real either way, and `glasshouse sessions` lists the session once it has
/// recorded itself.
fn open_cmux_pane(
    runtime: &Runtime,
    control: &impl cmux::CmuxControl,
    harness: &str,
    pane_command: &str,
) -> anyhow::Result<ExitCode> {
    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    let before = cmux::recorded_panes(&store)?;
    let workspace = cmux::NewWorkspace {
        name: format!("glasshouse {harness}"),
        cwd: runtime.project().display_root().to_path_buf(),
        command: pane_command.to_owned(),
        // A person asked to see it.
        focus: true,
    };
    let pane = control
        .create_workspace(&workspace)
        .map_err(|err| anyhow::anyhow!("cmux could not open a workspace for the session: {err}"))?;
    match cmux::await_session_at(&store, &pane, &before, cmux::RECORD_WAIT)? {
        Some(id) => println!("glasshouse: session {id} is running in cmux {pane}"),
        None => println!(
            "glasshouse: opened cmux {pane}; the session has not recorded itself yet — \
             `glasshouse sessions` lists it once it has"
        ),
    }
    Ok(ExitCode::SUCCESS)
}
