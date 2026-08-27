use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use std::io::IsTerminal;

use glasshouse::checkpoint::{
    Checkpoint, CheckpointReason, CheckpointStore, Handoff, ProjectCheckpoints, Stored,
};
use glasshouse::cli::{ApiCommand, CheckpointCommand};
use glasshouse::config::response::{ResponseProfileEntry, ResponseRequest};
use glasshouse::config::{self, EffectiveConfig, ProjectConfig, UserConfig};
use glasshouse::events::{
    EventBus, EventLog, LifecycleEvent, Observation, ProcessExit, TurnOutcome,
};
use glasshouse::integrations::Discovery;
use glasshouse::launch::HarnessLaunch;
use glasshouse::onboarding;
use glasshouse::platform::HostPlatform;
use glasshouse::profile::response::{Dimension, Role as ResponseRole};
use glasshouse::pty::ExitStatus;
use glasshouse::session;
use glasshouse::session::{
    NewSession, ProjectSessions, SessionDisposition, SessionId, SessionLifecycle, SessionName,
    SessionPresentation, SessionProtocol, SessionPurpose, SessionRecord, SessionRuntime,
};
use glasshouse::shim::{self, ShimRequest};
use glasshouse::{Cli, Command, MemoryCommand, Runtime, SessionCommand, logging, shutdown};

use clap::Parser;

mod api;

fn main() -> ExitCode {
    // Installed before anything can touch the terminal so a failure on any path
    // still leaves the user with a usable shell.
    shutdown::install_panic_hook();

    let cli = Cli::parse();

    match run(&cli) {
        Ok(code) => code,
        Err(err) => {
            shutdown::restore_terminal();
            eprintln!("glasshouse: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> anyhow::Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let runtime = glasshouse::bootstrap(cli, &cwd)?;

    // `Project::discover` runs before logging is initialized below, and
    // logging is off by default, so a `tracing::warn!` there can go
    // completely unseen. An overridden safety refusal is user-facing, not
    // diagnostics: it always gets a line on stderr, log or no log.
    if let Some(refusal) = runtime.project().overridden_refusal() {
        eprintln!("glasshouse: warning: {refusal}");
        eprintln!("glasshouse: continuing because --allow-unsafe-scope was given");
    }

    let log_config = logging::LogConfig::resolve(
        cli.log_level.as_deref(),
        cli.log_file.as_deref(),
        cli.log_stderr,
        &runtime.log_dir(),
    );
    let log_path = logging::init(&log_config)?;

    shutdown::install_signal_handler()?;

    tracing::info!(
        version = glasshouse::VERSION,
        project = %runtime.project().id(),
        root = %runtime.project().display_root().display(),
        "glasshouse started"
    );

    match &cli.command {
        Some(Command::Status) => {
            print!("{}", status_report(&runtime)?);
        }
        Some(Command::Doctor) => {
            print!("{}", glasshouse::integrations::doctor_report(&runtime));
        }
        Some(Command::Setup) => {
            if !setup(&runtime, SetupTrigger::Requested)? {
                return Ok(ExitCode::FAILURE);
            }
        }
        Some(Command::Pairing { model, harness }) => {
            let user = UserConfig::load(runtime.paths())?;
            let project = config::load_project_config(runtime.project())?;
            let effective = EffectiveConfig::new(&user, project.as_ref());
            print!(
                "{}",
                config::pairing::report(&effective, model.as_deref(), harness.as_deref())
            );
        }
        Some(Command::Response {
            role,
            session,
            verbosity,
            audience,
            narration,
            evidence,
            format,
        }) => {
            let user = UserConfig::load(runtime.paths())?;
            let project = config::load_project_config(runtime.project())?;
            let effective = EffectiveConfig::new(&user, project.as_ref());
            let request = match response_request(
                role.as_deref(),
                session.clone(),
                [
                    (Dimension::Verbosity, verbosity.clone()),
                    (Dimension::Audience, audience.clone()),
                    (Dimension::Narration, narration.clone()),
                    (Dimension::Evidence, evidence.clone()),
                    (Dimension::Format, format.clone()),
                ],
            ) {
                Ok(request) => request,
                Err(err) => {
                    eprintln!("glasshouse: {err}");
                    return Ok(ExitCode::FAILURE);
                }
            };
            print!("{}", config::response::report(&effective, &request));
        }
        Some(Command::Resources {
            verbose,
            probe,
            no_harness,
        }) => {
            print!(
                "{}",
                resources_report(&runtime, *verbose, probe, *no_harness)?
            );
        }
        Some(Command::Classify { text }) => {
            print!("{}", glasshouse::routing::classify::report(&text.join(" ")));
        }
        Some(Command::Sessions { command }) => match command {
            // The bare command still lists, which is what every existing
            // caller and every printed identifier assumes.
            None => print!("{}", session_report(&runtime)?),
            Some(SessionCommand::Show { session }) => {
                print!("{}", session_detail(&runtime, session)?);
            }
            Some(SessionCommand::Rename {
                session,
                name,
                clear,
            }) => match rename_session(&runtime, session, name.as_deref(), *clear) {
                Ok(report) => print!("{report}"),
                Err(err) => {
                    eprintln!("glasshouse: {err}");
                    return Ok(ExitCode::FAILURE);
                }
            },
            Some(SessionCommand::Tag {
                session,
                purpose,
                clear,
            }) => match tag_session(&runtime, session, purpose.as_deref(), *clear) {
                Ok(report) => print!("{report}"),
                Err(err) => {
                    eprintln!("glasshouse: {err}");
                    return Ok(ExitCode::FAILURE);
                }
            },
            Some(SessionCommand::Close { session }) => match close_session(&runtime, session) {
                Ok(report) => print!("{report}"),
                Err(err) => {
                    eprintln!("glasshouse: {err}");
                    return Ok(ExitCode::FAILURE);
                }
            },
        },
        // `run` and `launch` dispatch through this one arm on purpose — see
        // `Command::Run`'s doc. A change to how a launch is assembled can
        // only ever be made here, once, so the two can never diverge.
        Some(Command::Launch {
            harness,
            response_profile,
            response_role,
            profile,
            from_checkpoint,
            headless,
            harness_args,
        })
        | Some(Command::Run {
            harness,
            response_profile,
            response_role,
            profile,
            from_checkpoint,
            headless,
            harness_args,
        }) => {
            let response =
                match response_request(response_role.as_deref(), response_profile.clone(), []) {
                    Ok(request) => request,
                    Err(err) => {
                        eprintln!("glasshouse: {err}");
                        return Ok(ExitCode::FAILURE);
                    }
                };
            return launch_session(
                &runtime,
                harness.as_deref(),
                profile.as_deref(),
                from_checkpoint.as_deref(),
                &response,
                *headless,
                harness_args,
            );
        }
        Some(Command::Resume {
            session,
            harness_args,
        }) => {
            return resume_session(&runtime, session, harness_args);
        }
        Some(Command::Memory { command }) => match command {
            MemoryCommand::Search {
                query,
                history,
                limit,
            } => {
                print!(
                    "{}",
                    memory_report(&runtime, &query.join(" "), *history, *limit)?
                );
            }
            MemoryCommand::Promote { id, authority } => {
                print!("{}", memory_promote(&runtime, id, authority)?);
            }
            MemoryCommand::Extract {
                session,
                activity,
                from_events,
                reply_from,
            } => {
                print!(
                    "{}",
                    memory_extract(
                        &runtime,
                        session,
                        activity.as_deref(),
                        *from_events,
                        reply_from
                    )?
                );
            }
        },
        Some(Command::Checkpoint { command }) => {
            return checkpoint_command(&runtime, command);
        }
        Some(Command::Hook { session, event }) => {
            install_quiet_panic_hook();
            report_hook(&runtime, session, event);
        }
        Some(Command::Shim {
            harness,
            profile,
            dir,
            name,
            force,
        }) => {
            return run_shim(harness, profile, dir, name.as_deref(), *force);
        }
        Some(Command::Api { command }) => match command {
            ApiCommand::Serve { socket } => {
                api::serve(&runtime, socket.clone())?;
            }
        },
        None => {
            // Setup runs by itself the first time, so a new user does not have
            // to know a command exists before Glasshouse is useful.
            setup(&runtime, SetupTrigger::FirstRun)?;

            // With a terminal on both ends, this is the interactive shell.
            // Without one — a pipe, a redirect, CI — there is nothing to drive
            // a full-screen interface, so fall through to the plain summary
            // rather than failing or drawing into a file.
            if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
                glasshouse::shell::run(&runtime)?;
                return Ok(ExitCode::SUCCESS);
            }

            let project = runtime.project();
            println!("glasshouse {}", glasshouse::VERSION);
            println!("project     {}", project.name());
            println!("root        {}", project.display_root().display());
            println!("project id  {}", project.id());
            println!("scope from  {}", project.source());
            println!("state dir   {}", runtime.state_dir().display());
            if let Some(path) = log_path {
                println!("log file    {}", path.display());
            }
        }
    }

    Ok(ExitCode::SUCCESS)
}

/// Open a harness session attached to this terminal.
///
/// This is the production consumer of the sanctioned launch path: the harness
/// is chosen and its executable resolved from configuration (project level
/// overriding user level), the requested launch profile is resolved against
/// its adapter (Phase 9A/9F — see [`glasshouse::profile`]), and only then is
/// anything started through [`HarnessLaunch`] — the only route that exists,
/// and the one that derives the child's working directory from the active
/// project rather than from whatever directory Glasshouse happened to be run
/// in.
///
/// Setup is deliberately not triggered here. A user who has named a harness
/// has already said what they want; interrupting that with a first-run wizard
/// would be answering a question they did not ask.
/// A [`ResponseRequest`] from the command line, refusing an unknown role by
/// name.
///
/// A role is refused rather than reported, because a role selects which
/// *defaults* apply: a mistyped `--response-role reviwer` that fell back to
/// `interactive` would silently give a session the wrong communication policy,
/// and the user would have no way to tell. An axis value is different — it is
/// carried through and reported by name if this build does not know it, which
/// is the visible-degradation rule the rest of the configuration follows.
fn response_request(
    role: Option<&str>,
    session_preset: Option<String>,
    axes: impl IntoIterator<Item = (Dimension, Option<String>)>,
) -> anyhow::Result<ResponseRequest> {
    let role = match role {
        Some(slug) => Some(ResponseRole::from_slug(slug).ok_or_else(|| {
            anyhow::anyhow!(
                "`{slug}` is not a role Glasshouse knows; the roles are: {}",
                ResponseRole::names()
            )
        })?),
        None => None,
    };
    let mut task = ResponseProfileEntry::default();
    for (dimension, value) in axes {
        task.set_axis(dimension, value);
    }
    Ok(ResponseRequest {
        role,
        session_preset,
        task,
    })
}

/// Phase 9J line 576: the native-pairing preference and corrections in
/// effect, resolved into the form `crate::profile`'s gateway path accepts —
/// see `glasshouse::profile::GatewayPairing`'s own doc comment for why that
/// module cannot resolve this itself. Both of `launch_session`'s and
/// `resolve_resume_overlay`'s gateway-backed launches call this, so a
/// configured preference reaches a resumed session exactly as it reaches a
/// fresh one.
fn resolved_gateway_pairing(
    effective: &EffectiveConfig<'_>,
) -> glasshouse::profile::GatewayPairing {
    let (preference, _source) = effective.native_pairing_preference();
    glasshouse::profile::GatewayPairing {
        preference_slug: preference.slug(),
        overrides: effective.pairing_overrides(),
    }
}

fn launch_session(
    runtime: &Runtime,
    harness: Option<&str>,
    profile_name: Option<&str>,
    from_checkpoint: Option<&str>,
    response: &ResponseRequest,
    headless: bool,
    harness_args: &[String],
) -> anyhow::Result<ExitCode> {
    let user = UserConfig::load(runtime.paths())?;
    let project = config::load_project_config(runtime.project())?;
    let effective = EffectiveConfig::new(&user, project.as_ref());
    let selection = session::select::select(harness, effective)?;

    // Resolve the launch profile *before* anything is recorded or started.
    // A refusal here must cost nothing: no session record, no process. See
    // `glasshouse::profile::resolve`'s doc for why a refusal never falls back
    // to a different mode.
    //
    // Resolved *before* the response profile below, on purpose: line 353's
    // sixth axis lives on this profile, and the response request has to be
    // able to read it.
    let requested_profile = profile_name.unwrap_or(glasshouse::profile::NATIVE_PROFILE_NAME);
    let launch_profile = match effective.launch_profile(requested_profile, selection.id()) {
        Ok(resolved) => resolved.value,
        Err(err) => {
            eprintln!("glasshouse: {err}");
            return Ok(ExitCode::FAILURE);
        }
    };

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
    let response_application =
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
    let bootstrap = match resolve_bootstrap_prompt(runtime, from_checkpoint) {
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
    let gateway = match glasshouse::gateway::start_if_required_with_telemetry(
        std::slice::from_ref(&launch_profile),
        || gateway_upstream(&user, project.as_ref(), &effective, &secrets),
        Some(glasshouse::provider::telemetry::GatewayQuotaCache::new(
            runtime.paths(),
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
        // will not start".
        evidence_ledger(runtime, std::slice::from_ref(&launch_profile)),
    ) {
        Ok(gateway) => gateway,
        Err(err) => {
            eprintln!("glasshouse: {err}");
            return Ok(ExitCode::FAILURE);
        }
    };

    let resolution = glasshouse::profile::Resolution {
        adapter: selection.adapter(),
        acknowledged_bypass,
        provider: provider.as_ref(),
        secrets: &secrets,
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
    let presentation = if headless {
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
    let pairing = session_pairing(&effective, &launch_profile);
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
        mechanisms = %mechanism_summary(&overlay),
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
        install_session_document(
            runtime,
            &selection,
            &record.id,
            project_hooks_consent,
            &response_application,
        ),
    );
    let launch = HarnessLaunch::new(selection.into_executable(), runtime.project()).args(args);
    // The overlay is the only thing that may put its own arguments or
    // environment onto the launch — see `LaunchOverlay::apply`'s doc.
    let launch = overlay.apply(launch);
    // A checkpoint's handoff, if one was named, as the harness's opening
    // prompt — exactly where a person typing it after `--` would have put it.
    let launch = match &bootstrap {
        Some(prompt) => launch.args(std::iter::once(prompt.as_str())),
        None => launch,
    };
    // The user's own `--` arguments always come last, so they can win.
    let launch = launch.args(harness_args.iter().map(String::as_str));

    // From here on, a bookkeeping failure must never change what the user
    // sees. The session is real and running; losing a state transition is a
    // diagnostics problem, whereas turning it into an error would make a
    // database hiccup look like a harness failure.
    note_lifecycle(&store, &record.id, SessionLifecycle::Running);

    // Phase 18's "record session creation events", on the path that actually
    // creates one from the command line. The shell's own runtime publishes
    // the same event for a session started there; this is the other entry
    // point, and a log that only knew about one of them would be a log with a
    // hole in it exactly where a user was not using the interactive
    // interface.
    let events = EventRecorder::open(runtime);
    events.record(&record.id, LifecycleEvent::SessionStarted);

    let session = if headless {
        run_headless(&record.id, launch)
    } else {
        session::attach(launch)
    };
    let status = match session {
        Ok(status) => status,
        Err(err) => {
            note_lifecycle(&store, &record.id, SessionLifecycle::Failed);
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
    note_lifecycle(&store, &record.id, exit.session_state());

    if !status.success() {
        // The harness failing is not Glasshouse failing, so this is a plain
        // note on stderr rather than an error: the exit code below already
        // carries the outcome to whatever invoked Glasshouse.
        eprintln!("glasshouse: the harness {status}");
    }
    Ok(exit_code_for(&status))
}

/// Run a harness session that never takes this terminal — Phase 4's headless
/// presentation mode.
///
/// The mirror image of [`session::attach`]. The harness gets a real
/// pseudo-terminal in the project root exactly as it always does, but this
/// process's own terminal is never claimed: no raw mode, no alternate screen,
/// no output relayed to standard output. What the harness prints goes into
/// the session's own bounded scrollback, which is where an embedded session's
/// output goes too. That is the whole of "a PTY continues running without
/// occupying the visible session viewport" from the launch side; the shell
/// side is `shell::run`, which never makes a headless session the viewport's.
///
/// Glasshouse stays in the foreground for the session's whole life on
/// purpose. Returning early would drop the [`SessionRuntime`], and with it
/// the pseudo-terminal the harness is writing to — a detached session needs a
/// supervisor process, which is a different capability from this one.
///
/// **The terminal queries have to be answered here.** A headless session has
/// no emulator on the other end: on Windows nothing gets past ConPTY's
/// startup handshake without a reply, and on any platform a harness asking
/// `ESC[6n` waits forever for one. [`SessionRuntime`] knows how to answer but
/// cannot do it from its reader thread, so whoever owns the runtime must — in
/// the shell that is the tick, and here it is this loop.
///
/// # A signal here is a forced exit, and that is why the cleanup exists
///
/// [`shutdown::install_signal_handler`] ends the process immediately when the
/// terminal is not engaged, on the reasonable premise that a Glasshouse with
/// nothing to restore has nothing to wind down. **This path breaks that
/// premise**: it engages no terminal — that is what makes it headless — and
/// it owns a child process that stops receiving a hangup the moment Glasshouse
/// dies. Forced exit calls [`std::process::exit`], which runs no destructor,
/// so without the registration below a Ctrl-C would leave the harness running
/// with nothing left able to reach it.
///
/// Found by sending a real `SIGINT` to a real headless launch and looking for
/// the child afterwards; it was still there. `shutdown`'s own documentation
/// had already named this as the thing a second caller would have to get
/// right, which is exactly what this is.
///
/// Deliberately **not** solved by claiming the terminal is engaged. That flag
/// means "raw mode and the alternate screen are on", and `restore_terminal`
/// acts on it — setting it here would write escape sequences to a terminal
/// Glasshouse never touched.
fn run_headless(id: &SessionId, launch: HarnessLaunch<'_>) -> anyhow::Result<ExitStatus> {
    /// How often the loop wakes to answer queries and check on the child.
    const POLL: std::time::Duration = std::time::Duration::from_millis(20);

    let live = Arc::new(Mutex::new(SessionRuntime::new()));
    lock(&live).start(id.clone(), SessionPresentation::Headless, &launch)?;

    // Best effort by construction, exactly as `session::attach`'s is:
    // `try_lock` gives up rather than risk blocking the one path whose whole
    // purpose is to always work. The loop below holds the lock only for the
    // moment it takes to poll, and never across its sleep. The guard
    // unregisters on the way out, so the callback never outlives the session
    // it refers to.
    let _forced_exit = {
        let live = Arc::clone(&live);
        let id = id.clone();
        shutdown::on_forced_exit(move || {
            close_before_forced_exit(&live, &id, FORCED_EXIT_BOUND);
        })
    };

    // A blocking process that prints nothing is indistinguishable from a hung
    // one. On stderr, because standard output belongs to whatever the caller
    // is piping this into.
    eprintln!("glasshouse: session {id} is running headless; nothing is drawn here");

    loop {
        {
            let mut live = lock(&live);
            live.answer_terminal_queries();
            for (ended, status) in live.poll_exits() {
                if &ended == id {
                    return Ok(status);
                }
            }
        }
        std::thread::sleep(POLL);
    }
}

/// Take the headless runtime's lock, ignoring poisoning.
///
/// A panicking thread must not strand a live harness: the process is still
/// running and still needs to be polled and eventually hung up, and refusing
/// to touch the runtime would guarantee the orphan the registration above
/// exists to prevent.
fn lock(live: &Mutex<SessionRuntime>) -> std::sync::MutexGuard<'_, SessionRuntime> {
    live.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// How long a forced-exit cleanup may spend trying to reach the runtime.
///
/// Far below any threshold a person would read as a hang, and far above the
/// microseconds the poll loop actually holds the lock for.
const FORCED_EXIT_BOUND: std::time::Duration = std::time::Duration::from_millis(500);

/// Close `id` on the way out of a forced exit, retrying briefly rather than
/// once.
///
/// [`glasshouse::shutdown`]'s rule is that a forced-exit callback must never
/// wait indefinitely: failing to clean up is survivable, failing to exit is
/// not. A **single** `try_lock` honours the letter of that rule and still
/// gets the wrong answer. The headless poll loop takes this same lock every
/// `POLL`, so one attempt is a coin flip, and losing it orphans a real
/// harness permanently with no second chance — there is no retry anywhere
/// above this.
///
/// That is not theoretical. It was **measured at 1 orphan in 100 runs under
/// 3x CPU load**, and it turned up first as an intermittent red
/// `test (macos-latest)` that passed on rerun against the identical commit.
///
/// A bound keeps the guarantee that actually matters — this returns, always,
/// and quickly — while removing the coin flip. Poisoning is treated as
/// ownership rather than as a reason to give up, for the same reason
/// [`lock`] does: a panicked thread must not strand a live child, and a
/// poisoned mutex would otherwise make `try_lock` fail for as long as we were
/// willing to retry.
///
/// Returns whether the runtime was reached.
fn close_before_forced_exit(
    live: &Mutex<SessionRuntime>,
    id: &SessionId,
    bound: std::time::Duration,
) -> bool {
    const RETRY: std::time::Duration = std::time::Duration::from_millis(1);
    let deadline = std::time::Instant::now() + bound;
    loop {
        match live.try_lock() {
            Ok(mut live) => {
                let _ = live.close(id);
                return true;
            }
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                let _ = poisoned.into_inner().close(id);
                return true;
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(RETRY);
            }
        }
    }
}

/// The provider the local Glasshouse gateway forwards to, with its
/// credential resolved.
///
/// The configuration lookup is the caller's job, exactly as it already is
/// for a direct-provider profile: `glasshouse::profile` never imports
/// `glasshouse::config`, so every configured provider is read here and
/// handed in as a value. Which of them the gateway uses — and why more than
/// one is a refusal rather than a choice — is
/// `glasshouse::profile::gateway_upstream`'s decision, not this function's.
fn gateway_upstream(
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    effective: &EffectiveConfig<'_>,
    secrets: &dyn glasshouse::secret::SecretStore,
) -> anyhow::Result<glasshouse::gateway::Upstream> {
    let mut providers = Vec::new();
    for name in effective.provider_names() {
        providers.push(effective.configured_provider(&name)?.value);
    }
    // Phase 9I line 532: a provider the user has marked at least one free
    // model on backs this launch with `Cost::Free` rather than the fail-closed
    // `Cost::Metered` every backend got before. Looked up the same way
    // `disposable_candidates` looks it up — project layer winning over user —
    // because `glasshouse::profile` may not import `glasshouse::config` to
    // answer this itself.
    let free = |name: &str| -> bool {
        project
            .and_then(|p| p.providers().get(name))
            .or_else(|| user.providers().get(name))
            .is_some_and(|config| !config.free_models().is_empty())
    };
    Ok(glasshouse::profile::gateway_upstream(
        &providers, secrets, &free,
    )?)
}

/// Generate one file that `exec`s `glasshouse run <harness> --profile
/// <name>`, forwarding its own arguments.
///
/// The generated file is the entire mechanism — see [`glasshouse::shim`]'s
/// module doc. This function only resolves *this* executable's own path and
/// the host platform; [`shim::generate`] is the only thing that writes
/// anything, and it writes exactly one file, inside `dir` and nowhere else.
fn run_shim(
    harness: &str,
    profile: &str,
    dir: &Path,
    name: Option<&str>,
    force: bool,
) -> anyhow::Result<ExitCode> {
    let glasshouse_exe = std::env::current_exe().map_err(|err| {
        anyhow::anyhow!("could not determine the Glasshouse executable's own path: {err}")
    })?;
    let request = ShimRequest {
        harness,
        profile,
        glasshouse_exe: &glasshouse_exe,
        dir,
        name,
        force,
    };

    match shim::generate(HostPlatform::detect(), &request) {
        Ok(path) => {
            println!("wrote {}", path.display());
            println!(
                "deleting that file is all it takes to remove the shim; Glasshouse writes \
                 nothing else on its behalf."
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => {
            eprintln!("glasshouse: {err}");
            Ok(ExitCode::FAILURE)
        }
    }
}

/// Re-resolve `profile_name`'s overlay for a resumed session — Phase 9A line
/// 368's resume half, production caller of `resume_session`.
///
/// Exactly [`launch_session`]'s own resolution: the same lookup, the same
/// secret store, the same gateway start. A resumed session's overlay is not a
/// smaller thing than a fresh one's, so there is no separate, weaker path
/// here for it to take.
///
/// # Errors here are never fatal to the resume
///
/// The caller treats any `Err` as "resume without the overlay, and say why" —
/// never as a reason to refuse the resume outright. `open_for_resume` has
/// already proven this session is safe to continue; a bypass acknowledgement
/// withdrawn since the original launch, or a provider since removed from
/// configuration, is a reason to fall back to a plain native resume, not a
/// reason to make an otherwise-healthy session unresumable.
/// The routing evidence ledger for this project — **only when a gateway will
/// actually be started** — or `None` with a warning.
///
/// # Why the gate, and what it cost to learn
///
/// The first version opened the ledger unconditionally, before
/// `start_if_required_with_telemetry` decided whether a gateway was needed at
/// all. On macOS and Linux that was merely wasted work. On Windows it **hung
/// six memory-extraction tests indefinitely** — a 37-minute stall with no
/// output, on a tree whose local gate was 13/13 green.
///
/// [`crate::routing::evidence::EvidenceLedger`] holds `Mutex<Connection>`: an
/// open SQLite handle for its whole lifetime. SQLite locks with advisory
/// POSIX locks on Unix and with mandatory `LockFileEx` on Windows, so a handle
/// this function opened on a launch that never needed it blocks a later writer
/// on Windows and is invisible on Unix. **Opening a database you may not use is
/// not free, and the platform that charges for it is not the one this project
/// develops on.**
///
/// Gating on [`glasshouse::gateway::gateway_is_required`] makes the open happen
/// exactly when the gateway that consumes it is started, which is also what
/// `start_if_required_with_telemetry` would have decided a moment later.
///
/// Phase 33A records an observation per forwarded gateway exchange. Opening
/// its store touches the project database, and both callers evaluate this
/// **before** `start_if_required_with_telemetry` decides whether a gateway is
/// needed at all — so this runs on every launch and every resume.
///
/// It therefore must not fail the caller. A launch that refused to start
/// because a telemetry table could not be opened would trade the user's whole
/// session for a row nobody is waiting on, and this project's own product
/// invariant is that Glasshouse orchestrates real harnesses rather than
/// standing between the user and one. The warning is `tracing::warn!` for the
/// same reason `set_lifecycle`'s is: it belongs in the log, not on the
/// terminal the harness is about to take over.
fn evidence_ledger(
    runtime: &glasshouse::Runtime,
    profiles: &[glasshouse::profile::LaunchProfile],
) -> Option<std::sync::Arc<glasshouse::routing::evidence::EvidenceLedger>> {
    if !glasshouse::gateway::gateway_is_required(profiles) {
        return None;
    }
    match glasshouse::routing::evidence::EvidenceLedger::open(runtime) {
        Ok(ledger) => Some(std::sync::Arc::new(ledger)),
        Err(err) => {
            tracing::warn!(
                error = %err,
                "routing evidence ledger unavailable; this session's turns will not be recorded"
            );
            None
        }
    }
}

fn resolve_resume_overlay(
    effective: &EffectiveConfig<'_>,
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    selection: &session::HarnessSelection,
    profile_name: &str,
    // The whole `Runtime`, not just its paths: `EvidenceLedger::open` reads
    // the project's database and its project id, and narrowing to `paths`
    // here would put the ledger out of reach on the resume path alone.
    runtime: &glasshouse::Runtime,
) -> anyhow::Result<(
    glasshouse::profile::LaunchProfile,
    glasshouse::profile::LaunchOverlay,
    Option<glasshouse::gateway::Gateway>,
)> {
    let launch_profile = effective
        .launch_profile(profile_name, selection.id())?
        .value;
    let acknowledged_bypass = effective.bypass_acknowledged(selection.id()).value;
    let provider = match &launch_profile.backend {
        glasshouse::profile::BackendResource::DirectProvider { provider } => {
            Some(effective.configured_provider(provider)?.value)
        }
        _ => None,
    };
    // Phase 9E: the same store `launch_session` prefers, resolved fresh — a
    // credential is never carried across processes, let alone across the gap
    // between the original launch and this resume.
    let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();
    let gateway = glasshouse::gateway::start_if_required_with_telemetry(
        std::slice::from_ref(&launch_profile),
        || gateway_upstream(user, project, effective, &secrets),
        Some(glasshouse::provider::telemetry::GatewayQuotaCache::new(
            runtime.paths(),
        )),
        evidence_ledger(runtime, std::slice::from_ref(&launch_profile)),
    )?;
    let resolution = glasshouse::profile::Resolution {
        adapter: selection.adapter(),
        acknowledged_bypass,
        provider: provider.as_ref(),
        secrets: &secrets,
    };
    // Phase 9J line 576 — see `launch_session`'s own call for why this is
    // resolved here rather than inside `profile/**`.
    let pairing = resolved_gateway_pairing(effective);
    let overlay = glasshouse::profile::resolve_with_gateway(
        &launch_profile,
        &resolution,
        gateway.as_ref(),
        &pairing,
    )?;
    Ok((launch_profile, overlay, gateway))
}

/// A one-line summary of a resolved overlay's mechanisms, for the "opening a
/// harness session" log line — category and detail only, exactly what
/// [`glasshouse::profile::LaunchOverlay::mechanisms`] exposes for rendering.
/// An environment *value* is never in here, because the overlay never puts
/// one in a `MechanismNote` to begin with.
fn mechanism_summary(overlay: &glasshouse::profile::LaunchOverlay) -> String {
    if overlay.mechanisms().is_empty() {
        return "none".to_owned();
    }
    overlay
        .mechanisms()
        .iter()
        .map(|note| format!("{}: {}", note.category, note.detail))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Install lifecycle hooks for a session that is about to start, returning
/// the arguments that make the harness read them.
///
/// Best effort by construction. A harness that reports nothing is a harness
/// Glasshouse knows less about, which is a smaller loss than refusing to start
/// a session the user asked for because a configuration file could not be
/// written.
fn install_session_document(
    runtime: &Runtime,
    selection: &session::HarnessSelection,
    id: &session::SessionId,
    project_hooks_consent: bool,
    response: &glasshouse::harness::response::Application,
) -> Vec<std::ffi::OsString> {
    let program = match std::env::current_exe() {
        Ok(program) => program,
        Err(err) => {
            tracing::warn!(error = %err, "could not find the Glasshouse executable for hooks");
            return Vec::new();
        }
    };
    let report = glasshouse::harness::HookCommand::new(
        program,
        id.as_str(),
        runtime.session_dir(id.as_str()),
        runtime.project().root(),
        runtime.paths().data_dir(),
        runtime.paths().config_dir(),
    );
    match selection.install_session_document(&report, project_hooks_consent, response) {
        Ok(document) => document.args,
        Err(err) => {
            tracing::warn!(
                session = %id,
                error = %err,
                "could not write the session's harness document"
            );
            Vec::new()
        }
    }
}

/// Record a lifecycle event a harness reported about one of its sessions.
///
/// # This function may never fail
///
/// It is run *by the harness*, inside the user's session, and Claude Code
/// treats a hook's non-zero exit as a veto: a `UserPromptSubmit` hook that
/// exits non-zero blocks the prompt outright, with the user's own words
/// echoed back at them and nothing sent. That was observed directly, not
/// assumed.
///
/// So every failure here is swallowed into the log. A database that cannot be
/// opened, a session that is not in it, an event nobody recognises — none of
/// them is worth costing the user a turn. Glasshouse's bookkeeping is never
/// more important than the session it is keeping books about.
fn report_hook(runtime: &Runtime, session: &str, event: &str) {
    report_hook_with(runtime, session, event, || {
        disposable_extraction_model(runtime)
    });
}

/// Phase 21/49: whether the automatic post-turn memory-extraction trigger may
/// run for this project — see
/// [`glasshouse::config::EffectiveConfig::memory_extraction_enabled`].
///
/// A configuration Glasshouse cannot read defaults to enabled, matching every
/// other read failure on this path: [`disposable_extraction_model`] falls
/// back the same way, for the same reason — a broken config file must not
/// silently and permanently turn off a working capability, and this trigger
/// already tolerates every other failure non-fatally (see
/// [`run_extraction_after_turn`]'s own doc comment).
fn memory_extraction_enabled(runtime: &Runtime) -> bool {
    let Ok(user) = UserConfig::load(runtime.paths()) else {
        return true;
    };
    let project = config::load_project_config(runtime.project()).unwrap_or(None);
    EffectiveConfig::new(&user, project.as_ref())
        .memory_extraction_enabled()
        .value
}

/// Phase 9I lines 530, 531 and 540's production caller: route this
/// extraction through `glasshouse::routing::disposable::DisposableRouting`
/// over the free models the user has actually configured, and report the
/// choice. Never actually calls a model — see
/// [`glasshouse::memory::extract::disposable`], which is what this returns.
///
/// Falls back to [`NoExtractionModel`] when the configuration cannot be
/// read at all — the same non-fatal-to-the-session posture
/// [`report_hook_with`]'s own doc comment describes for every other failure
/// on this path.
fn disposable_extraction_model(runtime: &Runtime) -> Box<dyn glasshouse::memory::ExtractionModel> {
    let user = match UserConfig::load(runtime.paths()) {
        Ok(user) => user,
        Err(err) => {
            tracing::debug!(error = %err, "could not read configuration for disposable routing");
            return Box::new(NoExtractionModel);
        }
    };
    let project = match config::load_project_config(runtime.project()) {
        Ok(project) => project,
        Err(err) => {
            tracing::debug!(error = %err, "could not read project configuration for disposable routing");
            return Box::new(NoExtractionModel);
        }
    };
    let effective = EffectiveConfig::new(&user, project.as_ref());
    let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let telemetry = glasshouse::provider::resources::GatheredTelemetry::new().gather_gateway_quota(
        &glasshouse::provider::telemetry::GatewayQuotaCache::new(runtime.paths()),
    );
    let candidates = disposable_candidates(
        &user,
        project.as_ref(),
        &effective,
        &secrets,
        &telemetry,
        now_unix,
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
    let routing = glasshouse::routing::disposable::DisposableRouting::for_support_work(
        effective.prefer_free_routing().value,
        free_preferences,
    );
    Box::new(glasshouse::memory::RoutedNoModel::new(
        glasshouse::routing::disposable::JobKind::MemoryExtraction,
        &candidates,
        &routing,
    ))
}

/// Every free resource Glasshouse's disposable-job routing may choose from,
/// built the same way `build_settings` builds a `ProviderRow`'s
/// configuration in `shell/mod.rs`: a provider's whole configuration comes
/// from whichever layer actually holds its name, project winning over user.
///
/// A provider that named no free models, or whose credential does not
/// currently resolve, contributes nothing — never a candidate with an
/// invented model name or a credential this process cannot actually use.
///
/// Each candidate carries whatever real capacity data `telemetry` has cached
/// for its provider — map lines 1536 and 1549, the same
/// [`glasshouse::provider::resources::observed_capacity`] `resources_report`
/// reads for `glasshouse resources`, applied here for the first time to a
/// candidate a routing *decision* actually ranks rather than only a report a
/// person reads.
fn disposable_candidates(
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    effective: &EffectiveConfig<'_>,
    secrets: &dyn glasshouse::secret::SecretStore,
    telemetry: &glasshouse::provider::resources::GatheredTelemetry,
    now_unix: i64,
) -> Vec<glasshouse::routing::disposable::DisposableCandidate> {
    use glasshouse::routing::Cost;
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
        if !provider_config.enabled() || provider_config.free_models().is_empty() {
            continue;
        }
        let capacity = disposable_candidate_capacity(&name, effective, telemetry, now_unix);
        for var in provider_config.credential_env() {
            let reference = SecretRef::Environment { var: var.clone() };
            if secrets.resolve(&reference).is_none() {
                continue;
            }
            let credential_id = CredentialId::new(name.clone(), reference);
            for model in provider_config.free_models() {
                candidates.push(
                    DisposableCandidate::new(
                        name.clone(),
                        model.clone(),
                        credential_id.clone(),
                        Cost::Free,
                    )
                    .with_capacity(capacity.clone()),
                );
            }
        }
    }
    candidates
}

/// What real telemetry says about `provider`'s remaining capacity right now
/// — map lines 1536, 1549 and 1550's inputs, read the same way
/// [`resources_report`] reads them for `glasshouse resources`, from the same
/// on-disk [`glasshouse::provider::telemetry::GatewayQuotaCache`] and no
/// network call of its own.
///
/// Every field defaults to "nothing known" when no reading has ever been
/// cached for this provider — a fresh install, or a provider only ever used
/// through a harness's own native subscription — which
/// `routing::disposable::DisposableRouting::score` renders as an honest
/// `0.0` contribution rather than a guess.
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

/// [`report_hook`] with the extraction model supplied.
///
/// The model is the one thing on this path that does not exist yet — Phase 39
/// owns the provider interface, and [`NoExtractionModel`] is what production
/// passes until it does. Everything else here *is* the production path:
/// the session lookup, the translation, the event record, the state change
/// and the extraction call are all the shipped code, which is why the seam is
/// here and not one level up.
///
/// A factory rather than a reference, because extraction runs on its own
/// thread and needs something it can own.
fn report_hook_with(
    runtime: &Runtime,
    session: &str,
    event: &str,
    model: impl Fn() -> Box<dyn glasshouse::memory::ExtractionModel>,
) {
    // Codex writes its payload to the hook's stdin, and a process that never
    // reads it can leave the harness writing into a closed pipe. Glasshouse
    // has the event name and the session identifier from its own argv, so
    // the payload is drained to EOF and thrown away, unread and unparsed —
    // never deserialized, logged, or stored. See
    // `the_hook_command_never_reads_its_payload` below, and the
    // `docs/product/design-decisions.md` section this function implements.
    let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());

    let outcome = (|| -> anyhow::Result<()> {
        let sessions = ProjectSessions::open(runtime)?;
        let store = sessions.store();
        let id = store.resolve_id(session)?;
        let record = store
            .get(&id)?
            .ok_or_else(|| anyhow::anyhow!("session `{id}` is not in this project"))?;

        // `observe`, not `lifecycle_for`. Two things follow from that and
        // both are capability lines:
        //
        // It preserves the raw observation in the debug log before
        // translating, so a harness that gained an event between releases
        // leaves a line naming what arrived — which is the difference between
        // a five-minute fix and a bisect, and is why the line is written
        // whether or not the event is recognised.
        //
        // And the observation is exactly two words: the integration slug from
        // this session's own record, and the event name from Glasshouse's own
        // argv. **The payload is not among them and cannot become one.** The
        // stream carrying the user's prompt and the model's last message was
        // drained into `io::sink()` above, unread; nothing downstream of here
        // has it to leak. See `the_hook_command_never_reads_its_payload`.
        let Some(translated) = session::lifecycle::observe(&record.harness, event) else {
            // An event this build does not recognise. Harnesses gain events
            // between releases, and guessing a state from an unfamiliar name
            // would be worse than ignoring it.
            tracing::debug!(event, "ignoring an unrecognised harness event");
            return Ok(());
        };

        // Phase 12's "record every translated lifecycle event with session ID
        // and timestamp", and Phase 18's "record lifecycle-hook events".
        // Recorded before the state change is decided, and independently of
        // whether one is applied at all: an event that arrived after the
        // session finished is still something that happened, and a log that
        // dropped it would be missing exactly the evidence somebody debugging
        // a late hook needs.
        EventRecorder::open(runtime).record_observed(
            &id,
            translated.clone(),
            Observation::new(&record.harness, event),
        );

        // Phase 21: *allow memory extraction to run after task completion.*
        //
        // This is the one place a harness tells Glasshouse that a task
        // finished, and `TurnEnded { Completed }` is the only event that
        // carries that claim — `session::lifecycle::event_for` is its single
        // construction site, and a source-scanning test fails if a second one
        // appears. So this is where the trigger belongs.
        //
        // Ordered **after** the event is recorded, on purpose: the log is the
        // material extraction reads, and a turn's own closing event should be
        // in it. Ordered **before** the state change for no reason at all
        // beyond it reading better; `run_extraction_after_turn` cannot fail
        // in a way the rest of this function could notice.
        if matches!(
            translated,
            LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Completed
            }
        ) && memory_extraction_enabled(runtime)
        {
            run_extraction_after_turn(runtime, &id, model());
        }

        let Some(next) = translated.implied_state() else {
            // A translated event that says nothing about the session's state
            // — it is in the log and that is all it was ever going to do.
            return Ok(());
        };

        if !session::may_apply(record.lifecycle, next) {
            tracing::debug!(
                session = %id,
                from = record.lifecycle.as_str(),
                to = next.as_str(),
                "not applying a harness event to a session in this state"
            );
            return Ok(());
        }
        store.set_lifecycle(&id, next)?;
        tracing::info!(session = %id, event, state = next.as_str(), "harness reported an event");
        Ok(())
    })();

    if let Err(err) = outcome {
        tracing::warn!(error = %err, event, "could not record a harness event");
    }
}

/// The extraction model Glasshouse has in production, which is none.
///
/// Phase 21 has two separate lines here and they are separate on purpose:
/// *"allow memory extraction to run after task completion"* is about the
/// **trigger**, and *"allow a configurable cheap or local model to perform
/// memory extraction"* is about the **model**. The trigger is built; the
/// model is Phase 39's disposable-job provider and does not exist.
///
/// So extraction really does run after every completed turn, and it really
/// does report `no extraction model is available` every time — which is
/// exactly the shape [`glasshouse::memory::ExtractionOutcome`] exists to
/// carry, and exactly the failure Phase 21's *"keep memory-extraction failure
/// non-fatal to the coding session"* is about. Naming itself plainly matters
/// as much as it does for `glasshouse memory extract`: a log line saying a
/// model ran when none did would be worse than no line.
struct NoExtractionModel;

impl glasshouse::memory::ExtractionModel for NoExtractionModel {
    fn describe(&self) -> String {
        "none configured (Phase 39 supplies the provider)".to_owned()
    }

    fn complete(
        &self,
        _prompt: &glasshouse::memory::extract::Prompt,
    ) -> Result<String, glasshouse::memory::ModelError> {
        Err(glasshouse::memory::ModelError::Unavailable)
    }
}

/// How long a hook process will wait for extraction before going on without
/// it.
///
/// The number is chosen against what is on the other side of it: this process
/// is run **by the harness, inside the user's session**, and Claude Code
/// treats a hook's exit as a gate on the turn. A model that hangs must
/// therefore cost the user a bounded pause and not an open-ended one.
///
/// Deliberately not "however long the model takes". Extraction is a support
/// job; a coding session waiting on one has the relationship backwards.
const EXTRACTION_BOUND: std::time::Duration = std::time::Duration::from_secs(5);

/// Run memory extraction over what this session has done, after a completed
/// turn.
///
/// # Nothing here can hurt the session, and that is the design
///
/// Phase 21: *"keep memory-extraction failure non-fatal to the coding
/// session."* Four different failures are absorbed here and none of them
/// reaches [`report_hook`]:
///
/// - the project database will not open, or the event log will not read —
///   logged, and the function returns;
/// - the model is unavailable, refuses, or answers rubbish —
///   [`glasshouse::memory::Extractor::run`] has no error channel at all and
///   describes it on the outcome;
/// - the model **panics** — caught inside `run`, reported as an outcome;
/// - the model **hangs** — the work is on its own thread and this waits
///   [`EXTRACTION_BOUND`], then leaves it behind. The thread dies when the
///   process exits moments later, having written nothing: the store is only
///   touched after the model answers.
///
/// # Why a thread and not just a call
///
/// The only thing that buys is the bound, and the bound is the whole point.
/// This codebase has no async runtime and [`glasshouse::memory::ExtractionModel`]
/// is deliberately synchronous, so a thread is the mechanism; `ExtractionModel`
/// is `Send + Sync` for precisely this reason.
///
/// Everything cheap happens before the thread starts — opening the database,
/// reading a bounded window of the log, scrubbing and bounding the chunk — so
/// what is on the far side of the bound is the model call and the insert, and
/// a timeout means the model, not Glasshouse.
fn run_extraction_after_turn(
    runtime: &Runtime,
    id: &SessionId,
    model: Box<dyn glasshouse::memory::ExtractionModel>,
) {
    use glasshouse::memory::extract::chunk::ChunkLimits;
    use glasshouse::memory::extract::lifecycle::{EVENT_WINDOW, chunk_for_session};
    use glasshouse::memory::{ExtractionTrigger, Extractor, ProjectMemory};

    let prepared = (|| -> anyhow::Result<_> {
        let log = EventLog::open(runtime)?;
        let events = log.recent_for_session(id, EVENT_WINDOW)?;
        let memory = ProjectMemory::open(runtime)?;
        Ok((memory, events))
    })();

    let (memory, events) = match prepared {
        Ok(prepared) => prepared,
        Err(err) => {
            tracing::warn!(
                session = %id,
                error = %format!("{err:#}"),
                "could not read this session's history for memory extraction"
            );
            return;
        }
    };

    // The commit is deliberately not read here. `checkpoint::git` knows how
    // to find one and this process does not need to: a memory's commit is
    // "where the project was when this was learned", and a hook process runs
    // while the user's tree is mid-edit. `glasshouse memory extract` takes
    // the session's activity from a person who knows; this path takes what
    // the log holds and claims nothing more.
    let chunk = chunk_for_session(id, &events, None, ChunkLimits::default());

    let (tx, rx) = std::sync::mpsc::channel();
    let session = id.clone();
    std::thread::spawn(move || {
        let store = memory.store();
        let outcome =
            Extractor::new(&store, model.as_ref()).run(&chunk, ExtractionTrigger::TaskCompleted);
        // A closed receiver means the bound expired and nobody is listening.
        // That is a normal outcome here, not an error.
        let _ = tx.send(outcome);
        drop(session);
    });

    match rx.recv_timeout(EXTRACTION_BOUND) {
        Ok(outcome) => match &outcome.failure {
            None => tracing::info!(
                session = %id,
                model = outcome.model,
                stored = outcome.stored(),
                duplicates = outcome.duplicates,
                speculative = outcome.speculative,
                rejected = outcome.rejected.len(),
                "memory extraction ran after a completed task"
            ),
            Some(failure) => tracing::info!(
                session = %id,
                model = outcome.model,
                reason = %failure,
                "memory extraction after a completed task produced nothing"
            ),
        },
        Err(_) => tracing::warn!(
            session = %id,
            bound_ms = EXTRACTION_BOUND.as_millis(),
            "memory extraction did not finish within its bound; the session is unaffected"
        ),
    }
}

/// Send panic information to the log instead of to the user's terminal.
///
/// # Why a process-global is the right call *here*
///
/// `memory::extract` records the caveat that it cannot fix: it catches a
/// panicking extraction model with `catch_unwind`, but the **default panic
/// hook has already printed to stderr** by then. Setting a global from a
/// library module would be that module deciding something about every program
/// that links it, which is why it did not.
///
/// This is not a library module. It is the `glasshouse hook` command, a
/// process the harness runs **inside the user's session**, and whose stderr
/// the harness may show them. A Rust backtrace appearing in the middle of
/// somebody's coding session because a support job fell over is the same
/// defect as the hook failing: Glasshouse's bookkeeping is never more
/// important than the session it keeps books about.
///
/// The panic is not swallowed — it is logged, with the payload and the
/// location, where `--log-file` will show it.
fn install_quiet_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|at| format!("{}:{}", at.file(), at.line()))
            .unwrap_or_else(|| "unknown location".to_owned());
        tracing::error!(location, panic = %info, "a glasshouse hook process panicked");
    }));
}

/// Records lifecycle events durably from a command that is about to exit.
///
/// # Why this is not the sink the shell uses
///
/// [`glasshouse::events::EventLogSink`] queues behind a writer thread,
/// because the shell publishes from a thread that is sometimes draining a
/// pseudo-terminal and must never wait. None of that applies here: a
/// `glasshouse hook` process lives for a few milliseconds and then exits, and
/// queueing behind a thread it is about to drop would lose the event it was
/// run to record. So this writes synchronously.
///
/// # Why there is a bus at all
///
/// [`glasshouse::events::RecordedEvent`] cannot be built without a session
/// identifier and a timestamp — that is a property of the type rather than a
/// habit of its callers, and [`EventBus::publish`] is what stamps both. Using
/// it as the minting authority is what keeps "record every translated
/// lifecycle event with session ID and timestamp" true on this path as well
/// as in the interactive one. No sink is attached to it, so nothing is
/// written twice.
///
/// # Every failure is swallowed into the log, deliberately
///
/// This runs inside the user's own session — see [`report_hook`], which may
/// never fail — and it is also on the launch path, where a bookkeeping
/// failure must not turn into what looks like a harness failure. A project
/// whose database cannot be opened loses event history and keeps its session.
struct EventRecorder {
    bus: EventBus,
    log: Option<EventLog>,
}

impl EventRecorder {
    fn open(runtime: &Runtime) -> Self {
        let log = match EventLog::open(runtime) {
            Ok(log) => Some(log),
            Err(err) => {
                tracing::warn!(error = %format!("{err:#}"), "could not open the project event log");
                None
            }
        };
        Self {
            bus: EventBus::new(),
            log,
        }
    }

    fn record(&self, id: &SessionId, event: LifecycleEvent) {
        self.append(id, event, None);
    }

    /// Record an event together with the harness report it was translated
    /// from — the harness's own two words, and nothing else.
    fn record_observed(&self, id: &SessionId, event: LifecycleEvent, observed: Observation) {
        self.append(id, event, Some(observed));
    }

    fn append(&self, id: &SessionId, event: LifecycleEvent, observed: Option<Observation>) {
        let recorded = self.bus.publish(id, event);
        let Some(log) = &self.log else {
            return;
        };
        if let Err(err) = log.append(&recorded, observed.as_ref()) {
            tracing::warn!(session = %id, error = %err, "could not record a lifecycle event");
        }
    }
}

/// `glasshouse checkpoint …`.
///
/// # What Glasshouse supplies, and what it refuses to
///
/// The session, the harness, the timestamp and the Git position are read
/// straight off the project and the repository. The objective, the state, the
/// decisions and the next actions are **arguments**, because they are things
/// only whoever did the work knows. Glasshouse could have filled them from a
/// session's terminal output and it deliberately does not: a checkpoint whose
/// objective was guessed from scrollback would be a confident fiction, and
/// this project already refuses to read state out of terminal output
/// everywhere else.
fn checkpoint_command(runtime: &Runtime, command: &CheckpointCommand) -> anyhow::Result<ExitCode> {
    let checkpoints = ProjectCheckpoints::open(runtime)?;
    let store = checkpoints.store();

    match command {
        CheckpointCommand::Save {
            objective,
            state,
            session,
            decisions,
            failed_approaches,
            files,
            tests,
            next_actions,
        } => {
            let sessions = ProjectSessions::open(runtime)?;
            let Some(record) = active_session(&sessions, session.as_deref())? else {
                eprintln!(
                    "glasshouse: this project has no recorded sessions to check point. \
                     Start one with `glasshouse launch`."
                );
                return Ok(ExitCode::FAILURE);
            };

            let stored = store.save(Checkpoint::capture(
                &record.id,
                &record.harness,
                CheckpointReason::Manual,
                store.now(),
                runtime.project().root(),
                Handoff {
                    objective: objective.clone(),
                    implementation_state: state.clone(),
                    decisions: decisions.clone(),
                    failed_approaches: failed_approaches.clone(),
                    files: files.clone(),
                    test_state: tests.clone(),
                    next_actions: next_actions.clone(),
                },
            ))?;

            println!("checkpoint {}", stored.id.short());
            println!("session    {}", short_id(&record.id));
            match &stored.checkpoint.git {
                Some(git) => match &git.branch {
                    Some(branch) => println!("git        {branch} at {}", git.commit),
                    None => println!("git        detached at {}", git.commit),
                },
                // Said out loud rather than left blank: "when available" is a
                // real condition, and a silent omission reads as a bug.
                None => println!("git        no repository position available"),
            }
            if stored.checkpoint.trimmed {
                println!(
                    "note       trimmed to fit {} bytes; the session has more",
                    glasshouse::checkpoint::MAX_BYTES
                );
            }
            println!(
                "\nStart a session anywhere from it with:\n  glasshouse launch <harness> \
                 --from-checkpoint {}",
                stored.id.short()
            );
        }
        CheckpointCommand::List => {
            print!("{}", checkpoint_listing(&store)?);
        }
        CheckpointCommand::Show {
            checkpoint,
            document,
        } => {
            let Some(stored) = resolve_checkpoint(&store, checkpoint.as_deref())? else {
                eprintln!("glasshouse: this project has no checkpoints yet.");
                return Ok(ExitCode::FAILURE);
            };
            if *document {
                println!("{}", stored.checkpoint.render());
            } else {
                print!("{}", stored.checkpoint.bootstrap_prompt());
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The session a checkpoint command means.
///
/// Named explicitly, or the project's most recently active one — which is what
/// "the active session" means outside the interactive interface, and is the
/// row `glasshouse sessions` already prints first.
fn active_session(
    sessions: &ProjectSessions,
    named: Option<&str>,
) -> anyhow::Result<Option<glasshouse::session::SessionRecord>> {
    let store = sessions.store();
    match named {
        Some(named) => {
            let id = store.resolve_id(named)?;
            Ok(Some(store.get(&id)?.ok_or_else(|| {
                anyhow::anyhow!("session `{id}` is not in this project")
            })?))
        }
        None => Ok(store.list()?.into_iter().next()),
    }
}

/// The checkpoint a command means: the one named, or the most recent.
fn resolve_checkpoint(
    store: &CheckpointStore<'_>,
    named: Option<&str>,
) -> anyhow::Result<Option<Stored>> {
    match named {
        Some("latest") | None => Ok(store.latest()?),
        Some(named) => {
            let id = store.resolve_id(named)?;
            Ok(Some(store.get(&id)?.ok_or_else(|| {
                anyhow::anyhow!("checkpoint `{id}` is not in this project")
            })?))
        }
    }
}

/// The handoff prompt a `--from-checkpoint` launch opens with, if one was
/// asked for.
///
/// A named checkpoint that does not exist is an error rather than an empty
/// prompt: starting a fresh session that silently lost its handoff is the
/// worst of the available outcomes, because it looks exactly like one that
/// worked.
fn resolve_bootstrap_prompt(
    runtime: &Runtime,
    named: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let Some(named) = named else {
        return Ok(None);
    };
    let checkpoints = ProjectCheckpoints::open(runtime)?;
    let store = checkpoints.store();
    let stored = resolve_checkpoint(&store, Some(named))?.ok_or_else(|| {
        anyhow::anyhow!("this project has no checkpoints yet, so there is nothing to start from")
    })?;

    tracing::info!(
        checkpoint = %stored.id,
        session = %stored.checkpoint.session,
        // The harness the checkpoint came *from*. Worth a line, because the
        // whole point is that it need not be the one about to start.
        recorded_by = %stored.checkpoint.harness,
        "starting a session from a checkpoint"
    );
    Ok(Some(stored.checkpoint.bootstrap_prompt()))
}

/// The `glasshouse checkpoint list` listing.
fn checkpoint_listing(store: &CheckpointStore<'_>) -> anyhow::Result<String> {
    use std::fmt::Write as _;

    let stored = store.list()?;
    if stored.is_empty() {
        return Ok("No checkpoints recorded for this project.\n\
                   Take one with `glasshouse checkpoint save --objective ... \
                   --state ...`.\n"
            .to_owned());
    }

    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}",
        checkpoint_row(
            "CHECKPOINT",
            "SESSION",
            "HARNESS",
            "WHY",
            "TAKEN",
            "OBJECTIVE"
        )
    );
    for entry in &stored {
        let _ = writeln!(
            out,
            "{}",
            checkpoint_row(
                &entry.id.short(),
                &short_id(&entry.checkpoint.session),
                &entry.checkpoint.harness,
                entry.checkpoint.reason.as_str(),
                &format_age(entry.checkpoint.created_at),
                &one_line(&entry.checkpoint.handoff.objective),
            )
        );
    }
    Ok(out)
}

/// One line of the checkpoint listing, header included.
///
/// The header and the rows go through one function so their columns cannot
/// drift apart, exactly as [`session_row`] does — the usual way a hand-aligned
/// table stops lining up is somebody widening a column in one of two format
/// strings.
fn checkpoint_row(
    checkpoint: &str,
    session: &str,
    harness: &str,
    reason: &str,
    taken: &str,
    objective: &str,
) -> String {
    format!(
        "{checkpoint:<12}  {session:<12}  {harness:<14}  {reason:<13}  {taken:<10}  {objective}"
    )
}

/// An objective as one table cell: first line only, and bounded.
///
/// A checkpoint's objective is free text a person wrote and may well be a
/// paragraph. A listing that let one row become forty would be unreadable, so
/// the table shows the first line and `checkpoint show` prints the rest.
fn one_line(text: &str) -> String {
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

/// Reopen a recorded session in its own harness.
///
/// The order here is the safety property. The store decides whether this
/// session may be resumed *at all* — it belongs to this project, it is not
/// still running, and it has a native identifier to resume to — before any
/// harness is selected and long before any process exists. A refusal costs
/// nothing; a session opened against the wrong project would be a breach of
/// the isolation the whole product rests on.
///
/// The harness is then whichever one the record names, not whichever one is
/// configured now: resuming a Codex conversation in Claude Code would be
/// nonsense, so a record's own harness is what gets selected.
fn resume_session(
    runtime: &Runtime,
    session: &str,
    harness_args: &[String],
) -> anyhow::Result<ExitCode> {
    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();

    // Both of these refuse rather than guess: an ambiguous prefix names its
    // candidates, and `open_for_resume` carries the project-isolation check.
    let id = store.resolve_id(session)?;
    let resumable = store.open_for_resume(&id)?;
    // Phase 9A line 368's resume half. `ResumableSession` carries only what
    // project isolation needs to check; the record's six facts — which
    // profile, which backend, which response profile — live on the full
    // `SessionRecord`, read with the store's existing `get`, not a new
    // column. `open_for_resume` already proved this session exists in this
    // project, so a `None` here would mean the store disagreed with itself
    // between those two calls.
    let record = store
        .get(&resumable.id)?
        .expect("open_for_resume already proved this session's record exists");

    let user = UserConfig::load(runtime.paths())?;
    let project = config::load_project_config(runtime.project())?;
    let effective = EffectiveConfig::new(&user, project.as_ref());
    let selection = session::select::select(Some(resumable.harness.as_str()), effective)?;

    let Some(mut args) = selection.resume_args(
        &resumable.native_session_id,
        harness_args.iter().map(String::as_str),
    ) else {
        anyhow::bail!(
            "{} has no resume mechanism Glasshouse has verified, so session `{}` cannot be \
             reopened. Start a new session instead.",
            selection.id().display_name(),
            short_id(&resumable.id)
        );
    };

    tracing::info!(
        session = %resumable.id,
        harness = selection.id().slug(),
        executable = %selection.executable().path().display(),
        source = %selection.source(),
        // The native identifier is not a secret — it names a conversation in
        // the user's own harness history, and it is the one fact that makes a
        // failed resume diagnosable.
        native_session = %resumable.native_session_id,
        "resuming a harness session"
    );

    // Phase 9A line 368's resume half, the sharper part: re-resolve the
    // launch profile the record names, so the overlay this resumed process
    // actually receives — its environment, its arguments, any generated
    // provider configuration — matches the profile `sessions show` still
    // reports for it. Before this, a resume applied none of the six facts it
    // displays.
    //
    // A profile that no longer resolves — deleted from configuration, its
    // harness executable now missing, a bypass acknowledgement withdrawn
    // since the original launch — is reported and skipped rather than
    // refused: `open_for_resume` has already established this session is
    // safe to continue, and a resume is not the moment to apply "refuse
    // rather than invent" as though it were a fresh launch. The user gets a
    // plain native resume and a line on stderr explaining why, not a session
    // that no longer opens at all.
    let overlay_resolution = record.launch_profile.as_deref().and_then(|name| {
        match resolve_resume_overlay(
            &effective,
            &user,
            project.as_ref(),
            &selection,
            name,
            runtime,
        ) {
            Ok(resolved) => Some(resolved),
            Err(err) => {
                eprintln!(
                    "glasshouse: resuming session `{}` without launch profile `{name}`'s overlay: \
                     {err:#}",
                    short_id(&resumable.id)
                );
                None
            }
        }
    });

    // The response profile a fresh session opened under this record's role
    // would get today. Not the five axes stored on the record — those are a
    // `Copy` snapshot with no precedence chain attached, so replaying them
    // verbatim could not tell a project's own configuration from a session
    // preset — but the same layered resolution `launch_session` performs,
    // asked with the role this session was recorded under.
    let response_request = ResponseRequest {
        role: Some(ResponseRequest::role_for(record.role)),
        ..ResponseRequest::default()
    };
    let response_profile = effective.response_profile(&response_request);
    for problem in response_profile.problems() {
        eprintln!("glasshouse: {problem}");
    }
    let response_application =
        glasshouse::harness::response::apply(selection.adapter(), response_profile.resolved());

    let project_hooks_consent = effective.project_hooks(selection.id()).value;
    args.splice(
        0..0,
        install_session_document(
            runtime,
            &selection,
            &resumable.id,
            project_hooks_consent,
            &response_application,
        ),
    );

    let launch = HarnessLaunch::new(selection.into_executable(), runtime.project()).args(args);
    // Both guards must outlive `session::attach` below — dropping either
    // early would delete the generated configuration file, or close the
    // gateway's loopback listener, out from under the harness that is about
    // to be pointed at them. Never read again, only held: see
    // `LaunchOverlay::install`'s own doc for why that is the whole contract.
    let (launch, _generated_guard, _gateway_guard) = match overlay_resolution {
        Some((_launch_profile, mut overlay, gateway)) => {
            let session_dir = runtime.session_dir(resumable.id.as_str());
            match overlay.install(glasshouse::harness::GeneratedConfigSite::new(&session_dir)) {
                Ok(generated) => (overlay.apply(launch), Some(generated), gateway),
                Err(err) => {
                    eprintln!(
                        "glasshouse: resuming session `{}` without its generated provider \
                         configuration: {err}",
                        short_id(&resumable.id)
                    );
                    (launch, None, None)
                }
            }
        }
        None => (launch, None, None),
    };

    note_lifecycle(&store, &resumable.id, SessionLifecycle::Running);

    // Phase 18's "record session resume events". A distinct event rather than
    // a second `SessionStarted`, because otherwise a reader has to infer a
    // resume from a session having started twice, and an inference is not a
    // recording.
    let events = EventRecorder::open(runtime);
    events.record(&resumable.id, LifecycleEvent::SessionResumed);

    let status = match session::attach(launch) {
        Ok(status) => status,
        Err(err) => {
            note_lifecycle(&store, &resumable.id, SessionLifecycle::Failed);
            return Err(err);
        }
    };

    let exit = ProcessExit::from_status(&status);
    events.record(
        &resumable.id,
        LifecycleEvent::ProcessExited { exit: exit.clone() },
    );
    note_lifecycle(&store, &resumable.id, exit.session_state());

    if !status.success() {
        // A harness that refuses the identifier — "No conversation found with
        // session ID: …" is Claude Code's answer — exits non-zero, and that
        // is the honest outcome to pass on rather than dress up.
        eprintln!("glasshouse: the harness {status}");
    }
    Ok(exit_code_for(&status))
}

/// Move a session to a new state, logging rather than failing.
///
/// See the call sites: once a harness is running, Glasshouse's own record
/// keeping is not worth failing the user's session over.
fn note_lifecycle(
    store: &glasshouse::session::SessionStore<'_>,
    id: &glasshouse::session::SessionId,
    lifecycle: SessionLifecycle,
) {
    if let Err(err) = store.set_lifecycle(id, lifecycle) {
        tracing::warn!(session = %id, %lifecycle, error = %err, "could not record a session state change");
    }
}

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
fn resources_report(
    runtime: &Runtime,
    verbose: bool,
    probe: &[String],
    no_harness: bool,
) -> anyhow::Result<String> {
    let user = UserConfig::load(runtime.paths())?;
    let project = config::load_project_config(runtime.project())?;
    let effective = EffectiveConfig::new(&user, project.as_ref());
    let now_unix = glasshouse::provider::cache::now_unix_seconds();

    let mut telemetry = glasshouse::provider::resources::GatheredTelemetry::new();
    telemetry = telemetry.gather_gateway_quota(
        &glasshouse::provider::telemetry::GatewayQuotaCache::new(runtime.paths()),
    );
    if !no_harness {
        telemetry = telemetry.gather_harness_status(now_unix);
    }

    let mut probes = String::new();
    if !probe.is_empty() {
        use std::fmt::Write as _;
        let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();
        let _ = writeln!(probes, "PROBES\n");
        for name in probe {
            let reading = glasshouse::provider::resources::probe_provider(
                &effective, &secrets, name, now_unix,
            );
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
    Ok(format!(
        "{probes}{}",
        glasshouse::provider::resources::report(&effective, &telemetry, options)
    ))
}

/// Render a memory search the way `session_report` renders sessions: the
/// provenance is part of the answer, because a memory a reader cannot trace
/// back to a session or a commit is one they have to take on trust.
///
/// The authority class is part of the answer for the same reason. Phase 21A's
/// fixed requirement is that retrieval preserve the distinction rather than
/// flatten every remembered statement into equally authoritative text, and
/// this is the one surface a person reaches.
fn memory_report(
    runtime: &Runtime,
    query: &str,
    history: bool,
    limit: usize,
) -> anyhow::Result<String> {
    use std::fmt::Write as _;

    use glasshouse::memory::ProjectMemory;
    use glasshouse::memory::search::SearchScope;

    let scope = if history {
        SearchScope::Historical
    } else {
        SearchScope::Current
    };

    let memory = ProjectMemory::open(runtime)?;
    let records = memory.store().search(query, scope, limit)?;

    let mut out = String::new();
    if records.is_empty() {
        // Say which of the two questions was asked. "No memories" after a
        // default search would otherwise read as "this project remembers
        // nothing", when the history was simply not looked at.
        if history {
            writeln!(out, "No memories match {query:?}, including history.")?;
        } else {
            writeln!(
                out,
                "No current memories match {query:?}. Use --history to include \
                 superseded and resolved ones."
            )?;
        }
        return Ok(out);
    }

    for record in &records {
        let subject = record.subject.as_deref().unwrap_or("(no subject)");
        // Phase 21A: retrieval must preserve the authority distinction rather
        // than flattening every memory into equally authoritative text. An
        // unclassified memory says so; it does not borrow a class.
        let authority = record.authority.map_or("unclassified", |a| a.as_str());
        // Phase 21B: *"treat a decision with missing rationale and missing
        // assumptions as lower-confidence."* The ranking already does it —
        // `memory::search::demote_thin_decisions` puts such a decision behind
        // a better-proven one of its own class. Saying so here is the other
        // half: a reader who cannot see *why* a decision sank has been given
        // a reordering and no reason for it.
        let confidence = if record.is_lower_confidence_decision() {
            "  lower-confidence"
        } else {
            ""
        };
        writeln!(
            out,
            "{}  {}  {authority}{confidence}  {subject}",
            record.kind, record.status
        )?;
        writeln!(out, "    {}", record.body)?;
        let provenance = provenance_lines(record);
        if !provenance.is_empty() {
            writeln!(out, "{provenance}")?;
        }
        let session = record.source_session_id.as_deref().unwrap_or("unknown");
        let commit = record.source_commit.as_deref().unwrap_or("unknown");
        let events = record
            .source_events
            .map_or_else(|| "no event range".to_owned(), |events| events.to_string());
        writeln!(out, "    from session {session}, commit {commit}, {events}")?;
    }
    Ok(out)
}

/// The Phase 21B provenance a search result carries, one labelled line per
/// field that has one.
///
/// Absent fields print nothing rather than `unknown`. There are nine of them
/// and a memory rarely has more than two; printing the absences would bury
/// the memory under a form. The one place absence *is* stated is the
/// `lower-confidence` marker beside the authority, which is where it changes
/// what the reader should do.
fn provenance_lines(record: &glasshouse::memory::MemoryRecord) -> String {
    use std::fmt::Write as _;

    let provenance = &record.provenance;
    let fields: [(&str, Option<&str>); 9] = [
        ("why", provenance.rationale.as_deref()),
        ("problem", provenance.problem.as_deref()),
        ("assumes", provenance.assumptions.as_deref()),
        ("scale", provenance.scale_assumptions.as_deref()),
        ("security", provenance.security_assumptions.as_deref()),
        ("compat", provenance.compatibility_assumptions.as_deref()),
        ("ops", provenance.operational_assumptions.as_deref()),
        ("evidence", provenance.evidence.as_deref()),
        ("quoted", provenance.source_excerpt.as_deref()),
    ];

    let mut out = String::new();
    if let Some(phase) = provenance.project_phase {
        let _ = writeln!(out, "    phase      {phase}");
    }
    for (label, value) in fields {
        if let Some(value) = value {
            let _ = writeln!(out, "    {label:<10} {value}");
        }
    }
    // The caller adds its own newline, so hand back a block without a
    // trailing blank line when there is nothing to say.
    out.pop();
    out
}

/// `glasshouse memory promote <id> <authority>` — Phase 21A's explicit
/// promotion. `Classifier::Reviewed`, because the person typing this is the
/// review the class requires.
fn memory_promote(runtime: &Runtime, id: &str, authority: &str) -> anyhow::Result<String> {
    use glasshouse::memory::{AuthorityChange, Classifier, MemoryAuthority, ProjectMemory};

    let wanted = match authority {
        "unclassified" | "none" => None,
        other => Some(MemoryAuthority::from_stored(other).ok_or_else(|| {
            anyhow::anyhow!(
                "`{other}` is not an authority class; use one of {} or `unclassified`",
                MemoryAuthority::ALL
                    .iter()
                    .map(|a| a.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?),
    };

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();
    let resolved = store.resolve_id(id)?;
    let (record, change) = store.set_authority(&resolved, wanted, Classifier::Reviewed)?;

    Ok(match change {
        AuthorityChange::Changed => format!(
            "{} is now {}\n",
            record.id,
            record.authority.map_or("unclassified", |a| a.as_str())
        ),
        AuthorityChange::Unchanged => format!(
            "{} was already {}\n",
            record.id,
            record.authority.map_or("unclassified", |a| a.as_str())
        ),
    })
}

/// A model's reply read from a file, for `glasshouse memory extract`.
///
/// [`describe`](glasshouse::memory::ExtractionModel::describe) says plainly
/// that nothing was called, and that string is stored on the outcome and
/// printed on every run. An evaluation run must never be mistaken later for
/// evidence that a model performed extraction — that capability is Phase 39's
/// and is not built.
struct ReplyFromFile(String);

impl glasshouse::memory::ExtractionModel for ReplyFromFile {
    fn describe(&self) -> String {
        "file (evaluation harness; no model was called)".to_owned()
    }

    fn complete(
        &self,
        _prompt: &glasshouse::memory::extract::Prompt,
    ) -> Result<String, glasshouse::memory::ModelError> {
        Ok(self.0.clone())
    }
}

/// `glasshouse memory extract` — Phase 21's manual run, for debugging and
/// evaluating extraction itself.
///
/// Everything except the model call is the production path: the chunk is
/// bounded and scrubbed by `SessionChunk::build`, the reply goes through the
/// same contract validation, credential screen, conservative classification
/// and duplicate check, and what survives is written to the project's real
/// memory store.
fn memory_extract(
    runtime: &Runtime,
    session: &str,
    activity: Option<&std::path::Path>,
    from_events: bool,
    reply_from: &std::path::Path,
) -> anyhow::Result<String> {
    use std::fmt::Write as _;

    use anyhow::Context as _;
    use glasshouse::memory::extract::chunk::{ChunkLimits, SessionChunk};
    use glasshouse::memory::extract::lifecycle::{EVENT_WINDOW, chunk_for_session};
    use glasshouse::memory::{ExtractionTrigger, Extractor, ProjectMemory};

    let reply = std::fs::read_to_string(reply_from)
        .with_context(|| format!("read the model reply from {}", reply_from.display()))?;

    // Two sources, and the difference between them is the provenance.
    //
    // A file of activity is text a person chose, and a memory extracted from
    // it can name the session but not which part of it — there is no event
    // range to name. The event log can: `chunk_for_session` narrows the range
    // to what actually reached the model, and every memory this run stores
    // carries it. That is Phase 21's *"store the originating session and
    // event references"* with a caller a person can actually run.
    let (chunk, source) = if from_events {
        let sessions = ProjectSessions::open(runtime)?;
        let id = sessions.store().resolve_id(session)?;
        let log = EventLog::open(runtime)?;
        let events = log.recent_for_session(&id, EVENT_WINDOW)?;
        let read = events.len();
        (
            chunk_for_session(&id, &events, None, ChunkLimits::default()),
            format!("{read} recorded events for session {id}"),
        )
    } else {
        let activity = activity.expect("clap requires --activity unless --from-events");
        let activity_text = std::fs::read_to_string(activity)
            .with_context(|| format!("read session activity from {}", activity.display()))?;
        (
            SessionChunk::build(
                session,
                None::<String>,
                activity_text.lines().map(str::to_owned),
                ChunkLimits::default(),
            ),
            format!("{}", activity.display()),
        )
    };

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();
    let model = ReplyFromFile(reply);
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);

    let mut out = String::new();
    writeln!(out, "trigger {}, model {}", outcome.trigger, outcome.model)?;
    writeln!(out, "source: {source}")?;
    writeln!(
        out,
        "activity: {} entries, {} dropped, {} truncated, {} credentials redacted",
        chunk.entries().len(),
        outcome.activity_dropped,
        outcome.activity_truncated,
        outcome.redactions
    )?;
    if let Some(events) = chunk.source_events() {
        writeln!(out, "provenance: {events} of this project's log")?;
    }

    if let Some(failure) = &outcome.failure {
        writeln!(out, "extraction produced nothing: {failure}")?;
        return Ok(out);
    }

    writeln!(
        out,
        "stored {}, {} duplicate, {} speculative, {} rejected",
        outcome.stored(),
        outcome.duplicates,
        outcome.speculative,
        outcome.rejected.len()
    )?;
    for id in &outcome.recorded {
        writeln!(out, "    stored    {id}")?;
    }
    for (id, classification) in &outcome.lowered {
        // Name the rule that bound, not just the outcome: the point of
        // reporting a lowering at all is that a reader can see *why* the
        // model's declared class was not the stored one.
        let reasons = classification
            .reasons
            .iter()
            .map(|r| r.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        writeln!(
            out,
            "    lowered   {id}  {} -> {} ({reasons})",
            classification.declared.as_str(),
            classification.stored.as_str()
        )?;
    }
    for rejection in &outcome.rejected {
        writeln!(out, "    rejected  {rejection}")?;
    }
    Ok(out)
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
fn session_pairing(
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

fn session_report(runtime: &Runtime) -> anyhow::Result<String> {
    use std::fmt::Write as _;

    let sessions = ProjectSessions::open(runtime)?;
    let records = sessions.store().list()?;

    if records.is_empty() {
        return Ok(format!(
            "No sessions recorded for {}.\nStart one with `glasshouse launch`.\n",
            runtime.project().name()
        ));
    }

    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}",
        session_row(
            "SESSION",
            "NAME",
            "PURPOSE",
            "HARNESS",
            "PROFILE",
            "STATE",
            "ROLE",
            "PRESENTED",
            "LAST ACTIVITY"
        )
    );
    for record in &records {
        let _ = writeln!(
            out,
            "{}",
            session_row(
                &short_id(&record.id),
                // A name and a purpose are the user's, and most sessions have
                // neither. A dash rather than a blank: an empty cell in a
                // fixed-width table reads as a rendering fault.
                record
                    .display_name
                    .as_ref()
                    .map_or("-", |name| name.as_str()),
                record
                    .purpose
                    .as_ref()
                    .map_or("-", |purpose| purpose.as_str()),
                &record.harness,
                // A dash, not the word "native": a session recorded before
                // Phase 9A ran under no profile at all, and that is a
                // different fact from having run the Native profile — see
                // `SessionRecord::launch_profile`'s doc.
                record.launch_profile.as_deref().unwrap_or("-"),
                disposition_word(record),
                &record.role.to_string(),
                &record.presentation.to_string(),
                &format_age(record.last_activity_at),
            )
        );
    }
    Ok(out)
}

/// A one-screen project and resource summary — capability map line 1779.
///
/// Composes what `doctor`, `sessions` and `resources` already compute —
/// [`Discovery::run`], [`ProjectSessions`], and
/// [`glasshouse::provider::registry::registry`] — into counts, rather than
/// re-deriving any of their own rendering. A reader who needs more than a
/// count already has the command that produces it.
fn status_report(runtime: &Runtime) -> anyhow::Result<String> {
    use std::fmt::Write as _;

    let project = runtime.project();
    let mut out = String::new();

    let _ = writeln!(out, "Glasshouse status");
    let _ = writeln!(out, "=================");
    let _ = writeln!(out);
    let _ = writeln!(out, "Project");
    let _ = writeln!(out, "  name: {}", project.name());
    let _ = writeln!(out, "  root: {}", project.display_root().display());
    let _ = writeln!(out, "  id:   {}", project.id());
    let _ = writeln!(out);

    let discovery = glasshouse::integrations::Discovery::run(project);
    let harnesses: Vec<_> = discovery.harnesses().collect();
    let usable = harnesses.iter().filter(|d| d.is_usable()).count();
    let problems: usize = harnesses.iter().map(|d| d.problems().len()).sum();
    let problem_note = if problems == 0 {
        String::new()
    } else {
        format!(
            " ({problems} problem{} — see `glasshouse doctor`)",
            if problems == 1 { "" } else { "s" }
        )
    };
    let _ = writeln!(
        out,
        "Harnesses    {usable}/{} usable{problem_note}",
        harnesses.len()
    );

    let sessions = ProjectSessions::open(runtime)?;
    let records = sessions.store().list()?;
    if records.is_empty() {
        let _ = writeln!(out, "Sessions     none recorded — see `glasshouse launch`");
    } else {
        let _ = writeln!(
            out,
            "Sessions     {} recorded, most recent {} ({}, {})",
            records.len(),
            short_id(&records[0].id),
            disposition_word(&records[0]),
            format_age(records[0].last_activity_at)
        );
    }

    let resources = glasshouse::provider::registry::registry();
    let _ = writeln!(
        out,
        "Resources    {} tracked — see `glasshouse resources` for quota detail",
        resources.len()
    );

    Ok(out)
}

/// One line of the session listing, header included.
///
/// The header and the rows go through the same function so their columns
/// cannot drift apart — the usual way a hand-aligned table stops lining up is
/// someone widening a column in one of the two format strings.
#[allow(clippy::too_many_arguments)]
fn session_row(
    session: &str,
    name: &str,
    purpose: &str,
    harness: &str,
    profile: &str,
    state: &str,
    role: &str,
    presented: &str,
    activity: &str,
) -> String {
    // Widths fit the longest value each column can hold: `resumable`,
    // `orchestrator`, `embedded`. `name` and `purpose` are the two the user
    // controls, and they are truncated by the format rather than bounded
    // here — the store already refuses anything longer than 64 and 32.
    format!(
        "{session:<12}  {name:<16}  {purpose:<10}  {harness:<14}  {profile:<12}  {state:<9}  \
         {role:<12}  {presented:<9}  {activity}"
    )
}

/// Enough of an identifier to name a session in conversation.
///
/// The full identifier stays available in `--log-level` output and is what any
/// command taking a session takes; this is only for the eye.
fn short_id(id: &glasshouse::session::SessionId) -> String {
    id.as_str().chars().take(12).collect()
}

/// Everything one session recorded, one fact per line.
///
/// # Seven answers, not one
///
/// The harness, the launch profile, the backend resource, the model, the
/// pairing class, the wire protocol and the response profile each get their
/// own line, printed from their own column, with no line derived from
/// another. That is the phase's second fixed architectural requirement made
/// visible: a reader can see that Glasshouse holds them apart, and a build
/// that started filling one in from another would show it here.
///
/// # A dash is not a value
///
/// `-` means *this build recorded nothing here*, which is what a session
/// started before these columns existed leaves behind. It is deliberately
/// different from `unknown` and from `the harness's own default`, both of
/// which are answers Glasshouse recorded on purpose.
fn session_detail(runtime: &Runtime, session: &str) -> anyhow::Result<String> {
    use std::fmt::Write as _;

    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    let id = store.resolve_id(session)?;
    let record = store
        .get(&id)?
        .ok_or_else(|| anyhow::anyhow!("session `{id}` is not in this project"))?;

    let mut out = String::new();
    let mut line = |label: &str, value: &str| {
        let _ = writeln!(out, "{label:<19}{value}");
    };

    line("session", record.id.as_str());
    line(
        "name",
        record.display_name.as_ref().map_or("-", |n| n.as_str()),
    );
    line(
        "purpose",
        record.purpose.as_ref().map_or("-", |p| p.as_str()),
    );
    line("project", &record.project_id);
    line("harness", &record.harness);
    line(
        "native session",
        record.native_session_id.as_deref().unwrap_or("-"),
    );
    line("state", disposition_word(&record));
    line("lifecycle", record.lifecycle.as_str());
    line("role", record.role.as_str());
    line("presented", record.presentation.as_str());
    line(
        "launch profile",
        record.launch_profile.as_deref().unwrap_or("-"),
    );
    line(
        "backend resource",
        record.backend_resource.as_deref().unwrap_or("-"),
    );
    line("model", record.model.as_ref().map_or("-", |m| m.label()));
    line(
        "pairing class",
        record
            .pairing_class
            .map_or("-", glasshouse::session::SessionPairingClass::as_str),
    );
    line(
        "protocol",
        record.protocol.map_or("-", SessionProtocol::as_str),
    );
    line("response profile", &response_profile_line(&record));
    line(
        "response mechanism",
        record
            .response_mechanism
            .map_or("-", glasshouse::session::ResponseMechanism::as_str),
    );
    line("created", &format_age(record.created_at));
    line("last activity", &format_age(record.last_activity_at));
    Ok(out)
}

/// A session's five response axes on one line, or `-` when none was recorded.
///
/// Rendered from `ResponseProfile::axes`, so the five names and the five
/// values come from `profile::response` rather than from a second list here.
fn response_profile_line(record: &SessionRecord) -> String {
    match &record.response_profile {
        Some(profile) => profile
            .axes()
            .iter()
            .map(|(dimension, value)| format!("{}={value}", dimension.slug()))
            .collect::<Vec<_>>()
            .join("  "),
        None => "-".to_owned(),
    }
}

/// Which of the four categories a session list has to separate.
///
/// One function, used by both the listing and the detail view, so the two can
/// never disagree about whether a session is resumable.
fn disposition_word(record: &SessionRecord) -> &'static str {
    match record.disposition() {
        SessionDisposition::Active => "active",
        SessionDisposition::Resumable => "resumable",
        SessionDisposition::Closed => "closed",
        SessionDisposition::Failed => "failed",
    }
}

/// Give a session a name, or take its name away — line 650.
///
/// The report says the native session identifier afterwards, and says it is
/// unchanged. That is the capability's own promise, and a promise a user
/// cannot see is one they have to take on trust.
fn rename_session(
    runtime: &Runtime,
    session: &str,
    name: Option<&str>,
    clear: bool,
) -> anyhow::Result<String> {
    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    let id = store.resolve_id(session)?;
    let before = store
        .get(&id)?
        .ok_or_else(|| anyhow::anyhow!("session `{id}` is not in this project"))?;

    let record = if clear {
        store.clear_name(&id)?
    } else {
        let name = name.expect("clap requires a name unless --clear was given");
        store.rename(&id, &SessionName::parse(name)?)?
    };

    // Read back from the row rather than from what was asked for: the point
    // of the line is that one column changed and another did not.
    let native = record
        .native_session_id
        .as_deref()
        .unwrap_or("none recorded");
    debug_assert_eq!(before.native_session_id, record.native_session_id);
    Ok(match &record.display_name {
        Some(name) => format!(
            "Session {} is now `{name}`.\nIts native session id is unchanged: {native}\n",
            short_id(&record.id)
        ),
        None => format!(
            "Session {} has no name.\nIts native session id is unchanged: {native}\n",
            short_id(&record.id)
        ),
    })
}

/// Tag a session with a lightweight purpose, or clear the tag — line 651.
fn tag_session(
    runtime: &Runtime,
    session: &str,
    purpose: Option<&str>,
    clear: bool,
) -> anyhow::Result<String> {
    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    let id = store.resolve_id(session)?;

    let record = if clear {
        store.clear_purpose(&id)?
    } else {
        let purpose = purpose.expect("clap requires a purpose unless --clear was given");
        store.set_purpose(&id, &SessionPurpose::parse(purpose)?)?
    };

    Ok(match &record.purpose {
        Some(purpose) => format!("Session {} is tagged `{purpose}`.\n", short_id(&record.id)),
        None => format!("Session {} has no purpose tag.\n", short_id(&record.id)),
    })
}

/// Retire Glasshouse's record of a session — line 654.
///
/// The second line is the whole of the capability's second half, said out
/// loud: Glasshouse closed its own record and touched nothing the harness
/// owns. The native session identifier is printed because it is what a person
/// would use to find that history afterwards, and printing it is the proof
/// that closing did not take it away.
fn close_session(runtime: &Runtime, session: &str) -> anyhow::Result<String> {
    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    let id = store.resolve_id(session)?;
    let record = store.close(&id)?;

    let mut out = format!(
        "Closed Glasshouse's record of session {}.\n",
        short_id(&record.id)
    );
    let kept = match &record.native_session_id {
        Some(native) => format!(
            "The {} session `{native}` was not touched: Glasshouse does not \
             own that history and did not delete it.\n",
            record.harness
        ),
        None => "No native session was ever recorded for it, so there was no \
                 harness history to keep or lose.\n"
            .to_owned(),
    };
    out.push_str(&kept);
    Ok(out)
}

/// A rough "how long ago", which is what a session list is actually read for.
fn format_age(timestamp: i64) -> String {
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

/// Translate a harness's exit into Glasshouse's own.
///
/// A session's exit status belongs to the harness, so scripts wrapping
/// Glasshouse see what they would have seen running the harness directly.
/// Two cases cannot be represented faithfully and are mapped rather than
/// faked: a process killed by a signal has no exit code of its own, and a
/// code outside a byte cannot be returned by this process at all. Both become
/// a plain failure instead of being truncated into some unrelated code — in
/// particular into a `0` that would report success.
fn exit_code_for(status: &ExitStatus) -> ExitCode {
    if status.success() {
        return ExitCode::SUCCESS;
    }
    if status.signal().is_some() {
        return ExitCode::FAILURE;
    }
    match u8::try_from(status.code()) {
        Ok(0) | Err(_) => ExitCode::FAILURE,
        Ok(code) => ExitCode::from(code),
    }
}

/// Why setup is being considered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetupTrigger {
    /// Glasshouse is starting normally and setup has never been completed.
    FirstRun,
    /// The user asked for it with `glasshouse setup`.
    Requested,
}

/// Run the setup wizard when it is wanted and possible.
///
/// Returns whether setup ended up completed. A first run that cannot show a
/// wizard is not an error: Glasshouse still works, it just has not recorded
/// the user's harness choices yet.
fn setup(runtime: &Runtime, trigger: SetupTrigger) -> anyhow::Result<bool> {
    let config = UserConfig::load(runtime.paths())?;

    if trigger == SetupTrigger::FirstRun && !onboarding::is_required(&config) {
        return Ok(true);
    }

    // The wizard needs a terminal it can take over. Piped or redirected output
    // means Glasshouse is being scripted, and silently blocking on a full
    // screen interface nobody can see would be worse than skipping it.
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        match trigger {
            SetupTrigger::FirstRun => {
                eprintln!(
                    "glasshouse: setup has not been completed. Run `glasshouse setup` \
                     in an interactive terminal to choose which harnesses to use."
                );
                return Ok(false);
            }
            SetupTrigger::Requested => {
                anyhow::bail!("`glasshouse setup` needs an interactive terminal");
            }
        }
    }

    // Discovery probes each harness for its version, so it is done once, here,
    // rather than inside the wizard: the wizard is a state machine over an
    // already-known result, which is what makes it testable without a terminal.
    let discovery = Discovery::run(runtime.project());

    match onboarding::run(runtime, &discovery, config)? {
        onboarding::Outcome::Completed(_) => Ok(true),
        onboarding::Outcome::Cancelled => {
            eprintln!("glasshouse: setup cancelled; nothing was saved.");
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Phase 9J line 576's §35 proof for `main.rs`'s own two lines: what a
    /// user actually configured reaches `GatewayPairing`, not
    /// `glasshouse::profile::GatewayPairing::default`'s `"strong"`. If
    /// `resolved_gateway_pairing` (or either of its two call sites) went back
    /// to constructing `GatewayPairing::default()` instead of resolving
    /// `effective.native_pairing_preference()`, this would still read
    /// `"strong"` and fail.
    #[test]
    fn resolved_gateway_pairing_reflects_the_users_configured_preference() {
        let mut user = UserConfig::default();
        user.pairing_mut().set_native_pairing_preference(Some(
            glasshouse::config::pairing::PairingPreference::Off,
        ));
        let effective = EffectiveConfig::new(&user, None);

        let pairing = resolved_gateway_pairing(&effective);

        assert_eq!(
            pairing.preference_slug, "off",
            "the user configured `off`; a default-valued `GatewayPairing` would read `strong` \
             instead"
        );
    }

    /// The out-of-the-box answer, for a user who has never configured this —
    /// matches `EffectiveConfig::native_pairing_preference`'s own documented
    /// default, and `GatewayPairing::default`'s.
    #[test]
    fn resolved_gateway_pairing_defaults_to_strong_when_nothing_is_configured() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);

        let pairing = resolved_gateway_pairing(&effective);

        assert_eq!(pairing.preference_slug, "strong");
    }

    /// Hold the runtime's lock for `held`, signalling once it is definitely
    /// taken so a test never races its own fixture.
    fn hold_lock_for(
        live: &Arc<Mutex<SessionRuntime>>,
        held: std::time::Duration,
    ) -> std::thread::JoinHandle<()> {
        let live = Arc::clone(live);
        let (taken, is_taken) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let guard = live
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            taken.send(()).expect("the test is still waiting");
            std::thread::sleep(held);
            drop(guard);
        });
        is_taken.recv().expect("the holder thread took the lock");
        holder
    }

    /// The regression for the orphan race, made deterministic.
    ///
    /// The end-to-end version of this lives in `pty_smoke` and is
    /// probabilistic — it caught the defect once in a hundred runs under
    /// load, and once on macOS CI. This one holds the lock on purpose, so it
    /// fails every time rather than one time in a hundred.
    #[test]
    fn a_forced_exit_cleanup_waits_out_a_briefly_held_lock() {
        let live = Arc::new(Mutex::new(SessionRuntime::new()));
        let holder = hold_lock_for(&live, std::time::Duration::from_millis(100));

        let reached = close_before_forced_exit(
            &live,
            &SessionId::new("headless"),
            std::time::Duration::from_secs(5),
        );

        assert!(
            reached,
            "the cleanup gave up while the lock was merely busy, which is how a \
             real harness gets orphaned"
        );
        holder.join().expect("holder thread");
    }

    /// The other direction, and the reason the bound exists at all: a lock
    /// that is never released must not keep the process from exiting.
    #[test]
    fn a_forced_exit_cleanup_gives_up_rather_than_hanging() {
        let live = Arc::new(Mutex::new(SessionRuntime::new()));
        let holder = hold_lock_for(&live, std::time::Duration::from_secs(3));

        let started = std::time::Instant::now();
        let reached = close_before_forced_exit(
            &live,
            &SessionId::new("headless"),
            std::time::Duration::from_millis(50),
        );
        let waited = started.elapsed();

        assert!(
            !reached,
            "nothing could have reached a lock held throughout"
        );
        assert!(
            waited < std::time::Duration::from_secs(2),
            "the bound was not honoured: waited {waited:?}, which on the real \
             forced-exit path is a process that will not die"
        );
        holder.join().expect("holder thread");
    }

    /// What the code did before the bound existed, kept as a test so the
    /// defect cannot quietly return: a single attempt against a busy lock
    /// simply loses, and a lost attempt is a permanently orphaned harness.
    #[test]
    fn a_single_attempt_loses_the_race_that_the_bound_wins() {
        let live = Arc::new(Mutex::new(SessionRuntime::new()));
        let holder = hold_lock_for(&live, std::time::Duration::from_millis(200));

        let one_shot = close_before_forced_exit(
            &live,
            &SessionId::new("headless"),
            std::time::Duration::ZERO,
        );

        assert!(
            !one_shot,
            "if a zero bound now succeeds, the retry loop stopped being what \
             makes this safe and this test is no longer measuring anything"
        );
        holder.join().expect("holder thread");
    }

    /// Every gateway this binary starts is handed the routing evidence
    /// ledger — Phase 33A's wiring, which no behavioural test can reach.
    ///
    /// **Why a source scan rather than a real assertion.** Both call sites are
    /// inside `launch_session` and `resolve_resume_overlay`, and reaching
    /// either needs a launch profile that actually requires a gateway plus a
    /// real harness process. The integrator removed the ledger from both sites
    /// and the entire suite stayed green — so the wiring was, to the tests,
    /// invisible. That is the gap this closes, and it is the same reason
    /// `a_single_attempt_loses_the_race_that_the_bound_wins` above exists:
    /// keep a defect from quietly returning when nothing else would notice.
    ///
    /// It deliberately proves *structure*, not behaviour, and the evidence
    /// ledger says so — Phase 33A's boxes do not close on this test. What it
    /// prevents is a future edit dropping the ledger back to `None` with
    /// nothing to object.
    ///
    /// Scans by `str::lines` via `production_code` (§14): `include_str!` reads
    /// the file as checked out, and a CRLF checkout would otherwise make a
    /// multi-line search silently find nothing.
    #[test]
    fn every_gateway_the_binary_starts_is_given_the_evidence_ledger() {
        let code = production_code(include_str!("main.rs"));

        let starts = code.matches("start_if_required_with_telemetry(").count();
        // Counts the *gated* form on purpose. An ungated `evidence_ledger(runtime)`
        // is the exact shape that hung six Windows tests for 37 minutes, so this
        // test must not accept it back.
        let ledgers = code
            .matches("evidence_ledger(runtime, std::slice::from_ref(&launch_profile))")
            .count();
        assert_eq!(
            starts, 2,
            "this binary should start a gateway at exactly two sites (launch and \
             resume); if that changed, this test needs to change with it"
        );
        assert_eq!(
            ledgers, starts,
            "a gateway is started somewhere without being handed the routing \
             evidence ledger: Phase 33A records nothing for that path, and no \
             behavioural test in this crate would notice"
        );
        assert!(
            !code.contains("start_if_required_with_quota_cache("),
            "a call site still uses the pre-Phase-33A entry point, which cannot \
             carry an evidence ledger at all"
        );
    }

    /// This file's own source, with its `#[cfg(test)]` block (and `//`
    /// comments) stripped — the same idiom as
    /// `harness::resolving_a_launch_profile_touches_no_files`'s
    /// `production_code` helper, used here to prove structure rather than to
    /// forbid a name.
    fn production_code(source: &str) -> String {
        source
            .split("#[cfg(test)]")
            .next()
            .expect("split always yields at least one part")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `glasshouse run` exists only so a generated shim has a stable name to
    /// `exec` into (see `glasshouse::shim`'s module doc); Phase 9B's
    /// guarantee is that it behaves exactly like `glasshouse launch`. The
    /// guarantee is structural, not merely observed: `run` and `launch`
    /// match together in one arm in `run()` above and call `launch_session`
    /// from there, so there is exactly one call site in production code for
    /// this test to find — a second one would mean the two commands had
    /// drifted onto separate paths.
    #[test]
    fn glasshouse_run_and_glasshouse_launch_take_the_same_path() {
        let code = production_code(include_str!("main.rs"));
        // `return launch_session(` matches only an actual call, never the
        // `fn launch_session(` definition line itself.
        let call_sites = code.matches("return launch_session(").count();
        assert_eq!(
            call_sites, 1,
            "`glasshouse run` and `glasshouse launch` must dispatch through exactly one call \
             to `launch_session` so they cannot diverge; found {call_sites} call sites"
        );
    }

    // --- a refused profile starts no process and records no session -------

    /// A harness enabled with a decoy executable, so `session::select::select`
    /// succeeds without a real install; the runtime it was bootstrapped
    /// against comes back too, so the caller can inspect state afterward.
    fn fixture_with_enabled_claude_code(tmp: &std::path::Path) -> Runtime {
        let root = tmp.join("project");
        std::fs::create_dir_all(root.join(".git")).unwrap();

        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            tmp.join("data").to_str().unwrap(),
            "--config-dir",
            tmp.join("config").to_str().unwrap(),
        ])
        .unwrap();
        let runtime = glasshouse::bootstrap(&cli, &root).unwrap();

        let decoy = tmp.join("fake-claude");
        std::fs::write(&decoy, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&decoy).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&decoy, perms).unwrap();
        }

        let mut user = UserConfig::load(runtime.paths()).unwrap();
        user.integrations_mut()
            .entry(glasshouse::integrations::IntegrationId::ClaudeCode)
            .set_enabled(true)
            .set_executable(Some(decoy));
        user.save(runtime.paths()).unwrap();

        runtime
    }

    #[test]
    fn a_refused_profile_starts_no_process_and_records_no_session() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = fixture_with_enabled_claude_code(tmp.path());

        // A provider-backed profile is always refused in Phase 9A (Phase
        // 9C/9D supply the provider configuration it would need).
        let mut user = UserConfig::load(runtime.paths()).unwrap();
        let mut profile = glasshouse::config::ProfileConfig::new(
            glasshouse::integrations::IntegrationId::ClaudeCode,
        );
        profile.set_backend(glasshouse::config::ProfileBackend::DirectProvider {
            provider: "openrouter".to_owned(),
        });
        user.profiles_mut().set("gateway", profile);
        user.save(runtime.paths()).unwrap();

        let status = launch_session(
            &runtime,
            Some("claude-code"),
            Some("gateway"),
            None,
            &ResponseRequest::default(),
            false,
            &[],
        )
        .unwrap();
        assert_eq!(status, ExitCode::FAILURE);

        let sessions = glasshouse::session::ProjectSessions::open(&runtime).unwrap();
        assert!(
            sessions.store().list().unwrap().is_empty(),
            "a refused profile must record no session"
        );
    }

    #[test]
    fn an_unacknowledged_bypass_also_starts_no_process_and_records_no_session() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = fixture_with_enabled_claude_code(tmp.path());

        let mut user = UserConfig::load(runtime.paths()).unwrap();
        let mut profile = glasshouse::config::ProfileConfig::new(
            glasshouse::integrations::IntegrationId::ClaudeCode,
        );
        profile.set_approval(glasshouse::config::ProfileApproval::Bypass);
        user.profiles_mut().set("yolo", profile);
        user.save(runtime.paths()).unwrap();

        let status = launch_session(
            &runtime,
            Some("claude-code"),
            Some("yolo"),
            None,
            &ResponseRequest::default(),
            false,
            &[],
        )
        .unwrap();
        assert_eq!(status, ExitCode::FAILURE);

        let sessions = glasshouse::session::ProjectSessions::open(&runtime).unwrap();
        assert!(sessions.store().list().unwrap().is_empty());
    }

    #[test]
    fn a_native_profile_launch_records_its_profile_name_and_backend() {
        // Not a full launch (that needs a real PTY-attachable harness); this
        // exercises everything `launch_session` does up to and including the
        // session record, by stopping the resolved profile one step short of
        // `HarnessLaunch` and checking what would have been recorded.
        let tmp = tempfile::tempdir().unwrap();
        let runtime = fixture_with_enabled_claude_code(tmp.path());
        let user = UserConfig::load(runtime.paths()).unwrap();
        let project = config::load_project_config(runtime.project()).unwrap();
        let effective = EffectiveConfig::new(&user, project.as_ref());
        let selection =
            glasshouse::session::select::select(Some("claude-code"), effective).unwrap();

        let resolved = effective
            .launch_profile(glasshouse::profile::NATIVE_PROFILE_NAME, selection.id())
            .unwrap()
            .value;
        assert_eq!(resolved.name, "native");
        assert_eq!(resolved.backend.slug(), "native");

        let secrets = glasshouse::secret::EnvironmentSecretStore::new();
        let overlay = glasshouse::profile::resolve(
            &resolved,
            &glasshouse::profile::Resolution {
                adapter: selection.adapter(),
                acknowledged_bypass: false,
                provider: None,
                secrets: &secrets,
            },
        )
        .unwrap();
        assert!(mechanism_summary(&overlay).contains("automatic review"));
    }

    // --- the hook handler never reads its payload -------------------------

    /// Every field a Codex hook payload can carry, per
    /// `docs/product/design-decisions.md`'s "Codex lifecycle hooks" section:
    /// the six every event carries, plus `SessionStart`'s `source`,
    /// `UserPromptSubmit`'s `turn_id`/`prompt`, and `Stop`'s
    /// `stop_hook_active`/`last_assistant_message`. `prompt` and
    /// `last_assistant_message` are the conversation itself.
    const HOOK_PAYLOAD_FIELDS: &[&str] = &[
        "session_id",
        "transcript_path",
        "hook_event_name",
        "permission_mode",
        "source",
        "turn_id",
        "prompt",
        "stop_hook_active",
        "last_assistant_message",
    ];

    /// The hook handler's own source, isolated from the rest of this file. A
    /// whole-file scan would trip on legitimate, unrelated code — this
    /// module's own `native_session_id` and `cwd` locals are not the Codex
    /// payload fields of the same or similar name — so this extracts just
    /// the one function the design decision is actually about.
    ///
    /// **`report_hook_with`, not `report_hook`.** The two were one function
    /// until extraction needed a model seam; `report_hook` is now a
    /// two-line wrapper and scanning *it* would pass trivially, which is
    /// this test's own stated failure mode — a scan over the wrong span
    /// passing for the wrong reason. The anchor assertion below is what
    /// caught the split when it happened.
    fn hook_handler_source() -> &'static str {
        let full = include_str!("main.rs");
        let start = full
            .find("fn report_hook_with(")
            .expect("report_hook_with must exist in this file");
        let after_start = &full[start..];
        // `"\n}"` rather than `"\n}\n"`: on Windows this file is checked out
        // with CRLF endings, so the closing brace reads `\r\n}\r\n` and a
        // pattern demanding `\n` on both sides never matches. Windows CI caught
        // exactly that. Matching only the newline *before* the brace works on
        // both, and a brace at column zero can only be this function's own.
        let end = after_start
            .find("\n}")
            .expect("report_hook must have a top-level closing brace");
        let body = &after_start[..end];
        // The slice must be the real function, not an empty or truncated one.
        // A scan over the wrong span passes for the wrong reason, which this
        // project has been caught by before — a `skip_while` that found a
        // harness *list* where an adapter *block* was meant. Anchor on
        // something the handler provably contains.
        assert!(
            body.contains("std::io::sink()"),
            "hook_handler_source() did not capture the real `report_hook` body; \
             the payload scan below would be checking nothing"
        );
        body
    }

    /// Strip `//` line comments, so a doc comment that merely *mentions* a
    /// forbidden name (as this file's own comments now do) cannot fail the
    /// scan below.
    fn strip_comments(source: &str) -> String {
        source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_hook_command_never_reads_its_payload() {
        let source = strip_comments(hook_handler_source());

        for forbidden in ["serde_json", "from_str", "from_reader"] {
            assert!(
                !source.contains(forbidden),
                "the hook handler names `{forbidden}`, so it might parse the payload it must \
                 only drain and discard"
            );
        }
        for field in HOOK_PAYLOAD_FIELDS {
            assert!(
                !source.contains(field),
                "the hook handler names the payload field `{field}`, which must never be read, \
                 logged, or stored"
            );
        }
    }

    #[test]
    fn the_payload_scan_would_catch_a_violation() {
        // The guard above is only worth having if it can fail.
        let violating = "fn report_hook(runtime: &Runtime, session: &str, event: &str) {\n    \
                          let payload: serde_json::Value = serde_json::from_str(\"{}\").unwrap();\n}\n";
        assert!(strip_comments(violating).contains("serde_json"));
        assert!(strip_comments(violating).contains("from_str"));

        let reading_a_field = "fn report_hook(runtime: &Runtime, session: &str, event: &str) {\n    \
                                tracing::debug!(prompt = \"x\");\n}\n";
        assert!(strip_comments(reading_a_field).contains("prompt"));
    }

    /// The listing's ages, including the case a review flagged: a timestamp in
    /// the future. `saturating_sub` saturates at `i64::MIN`, not at zero, so
    /// the value really can be negative and the first arm has to absorb it.
    #[test]
    fn ages_read_sensibly_including_a_clock_that_moved_backwards() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after the epoch")
            .as_secs() as i64;

        assert_eq!(format_age(now), "just now");
        assert_eq!(format_age(now - 30), "just now");
        assert_eq!(format_age(now - 120), "2m ago");
        assert_eq!(format_age(now - 7_200), "2h ago");
        assert_eq!(format_age(now - 3 * 86_400), "3d ago");

        // A future timestamp must not print a negative age.
        let ahead = format_age(now + 10_000);
        assert_eq!(
            ahead, "just now",
            "a future timestamp must not read as an age"
        );
        assert!(!ahead.contains('-'), "no negative ages: {ahead}");

        // Extremes must not panic or overflow. A row holding a nonsense
        // timestamp cannot come from Glasshouse's own writes — `system_clock`
        // never returns a negative — so the honest contract is only that the
        // output stays finite and non-negative. `i64::MIN` yields an absurdly
        // large age, which is the right kind of wrong: visibly broken rather
        // than plausibly incorrect.
        for extreme in [i64::MIN, i64::MAX, 0] {
            let text = format_age(extreme);
            assert!(!text.is_empty() && !text.contains('-'), "bad age: {text}");
        }
        assert_eq!(
            format_age(i64::MAX),
            "just now",
            "the far future reads as now"
        );
    }

    /// The header and every row go through `session_row`, so their columns
    /// cannot drift apart. Checked here rather than trusted.
    #[test]
    fn listing_columns_line_up_between_the_header_and_a_row() {
        let header = session_row(
            "SESSION",
            "NAME",
            "PURPOSE",
            "HARNESS",
            "PROFILE",
            "STATE",
            "ROLE",
            "PRESENTED",
            "LAST",
        );
        let row = session_row(
            "abc123",
            "the auth probe",
            "auth",
            "claude-code",
            "native",
            "resumable",
            "orchestrator",
            "embedded",
            "2h ago",
        );

        let starts = |line: &str| -> Vec<usize> {
            let mut out = vec![0];
            let bytes = line.as_bytes();
            for i in 1..bytes.len() {
                if bytes[i] != b' ' && bytes[i - 1] == b' ' && i >= 2 && bytes[i - 2] == b' ' {
                    out.push(i);
                }
            }
            out
        };
        assert_eq!(
            starts(&header),
            starts(&row),
            "columns must start at the same offsets:\n{header}\n{row}"
        );
    }

    // ---------------------------------------------------------------------
    // Phase 21 / 21A — the command surfaces, which is where these
    // capabilities become true of a program a person can run rather than of
    // a Rust API nothing calls.
    // ---------------------------------------------------------------------

    /// A bootstrapped project, with its temp directories kept alive.
    struct CliFixture {
        _workspace: tempfile::TempDir,
        _data: tempfile::TempDir,
        runtime: Runtime,
    }

    impl CliFixture {
        fn new() -> Self {
            let workspace = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(workspace.path().join(".git")).unwrap();
            let data = tempfile::tempdir().unwrap();
            let cli = Cli::try_parse_from([
                "glasshouse",
                "--data-dir",
                data.path().to_str().unwrap(),
                "--config-dir",
                data.path().to_str().unwrap(),
            ])
            .unwrap();
            let runtime = glasshouse::bootstrap(&cli, workspace.path()).unwrap();
            Self {
                _workspace: workspace,
                _data: data,
                runtime,
            }
        }
    }

    /// Phase 21A's fixed architectural requirement, at the only surface a
    /// person reaches: retrieval preserves the authority distinction rather
    /// than flattening every remembered statement into equally authoritative
    /// text.
    ///
    /// Drives all seven classes rather than a sample, from
    /// `MemoryAuthority::ALL`, so an eighth class fails here rather than
    /// being quietly unprintable.
    #[test]
    fn a_memory_search_names_the_authority_class_of_every_result() {
        use glasshouse::memory::{MemoryAuthority, MemoryKind, NewMemory, ProjectMemory};

        let fixture = CliFixture::new();
        let project = ProjectMemory::open(&fixture.runtime).unwrap();
        let store = project.store();

        for authority in MemoryAuthority::ALL {
            store
                .record(
                    NewMemory::new(
                        MemoryKind::Finding,
                        format!("The kestrel deploy is {}.", authority.as_str()),
                    )
                    .with_authority(Some(*authority)),
                )
                .unwrap();
        }
        // An unclassified memory says so. It must not borrow a neighbour's
        // class, and it must not be indistinguishable from a classified one.
        store
            .record(NewMemory::new(
                MemoryKind::Finding,
                "The kestrel deploy was never classified.",
            ))
            .unwrap();

        let report = memory_report(&fixture.runtime, "kestrel", false, 20).unwrap();

        for authority in MemoryAuthority::ALL {
            assert!(
                report.contains(authority.as_str()),
                "`{}` is missing from a search that returned it:\n{report}",
                authority.as_str()
            );
        }
        assert!(
            report.contains("unclassified"),
            "an unclassified memory must say so:\n{report}"
        );
    }

    /// Phase 21A — a person can promote and demote explicitly, and only a
    /// person can reach `invariant` at all.
    #[test]
    fn a_person_can_promote_a_memory_and_demote_it_again() {
        use glasshouse::memory::{MemoryKind, NewMemory, ProjectMemory};

        let fixture = CliFixture::new();
        let project = ProjectMemory::open(&fixture.runtime).unwrap();
        let id = project
            .store()
            .record(NewMemory::new(
                MemoryKind::Decision,
                "Sessions are keyed by project, not by directory.",
            ))
            .unwrap()
            .id;

        let promoted = memory_promote(&fixture.runtime, id.as_str(), "invariant").unwrap();
        assert!(promoted.contains("invariant"), "{promoted}");
        assert_eq!(
            project.store().get(&id).unwrap().unwrap().authority,
            Some(glasshouse::memory::MemoryAuthority::Invariant)
        );

        // Demotion is never refused: 21A's concern is memories becoming
        // binding without anyone deciding they should.
        let demoted = memory_promote(&fixture.runtime, id.as_str(), "preference").unwrap();
        assert!(demoted.contains("preference"), "{demoted}");

        let cleared = memory_promote(&fixture.runtime, id.as_str(), "unclassified").unwrap();
        assert!(cleared.contains("unclassified"), "{cleared}");
        assert_eq!(project.store().get(&id).unwrap().unwrap().authority, None);

        // A class that does not exist is refused by name rather than
        // silently storing nothing.
        let refused = memory_promote(&fixture.runtime, id.as_str(), "extremely-important");
        assert!(refused.is_err());
    }

    // ---------------------------------------------------------------------
    // Phase 21 — extraction after task completion, and the promise that its
    // failure never costs the coding session anything.
    //
    // These drive `report_hook_with`, which *is* `glasshouse hook`: the same
    // session lookup, the same translation, the same event record, the same
    // state change. Only the model is supplied, because the model is the one
    // piece Phase 39 owns and nothing has built.
    // ---------------------------------------------------------------------

    /// An extraction model whose reply is fixed, and which records that it
    /// was asked.
    struct Canned {
        reply: String,
        asked: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl glasshouse::memory::ExtractionModel for Canned {
        fn describe(&self) -> String {
            "test/canned".to_owned()
        }
        fn complete(
            &self,
            _prompt: &glasshouse::memory::extract::Prompt,
        ) -> Result<String, glasshouse::memory::ModelError> {
            self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self.reply.clone())
        }
    }

    /// A model that does the one thing a support job must never be able to do
    /// to the session that triggered it.
    struct Hostile(HostileKind);

    enum HostileKind {
        Refuses,
        Panics,
        Hangs,
    }

    impl glasshouse::memory::ExtractionModel for Hostile {
        fn describe(&self) -> String {
            "test/hostile".to_owned()
        }
        fn complete(
            &self,
            _prompt: &glasshouse::memory::extract::Prompt,
        ) -> Result<String, glasshouse::memory::ModelError> {
            match self.0 {
                HostileKind::Refuses => Err(glasshouse::memory::ModelError::Refused),
                HostileKind::Panics => panic!("the extraction model fell over"),
                // Far longer than `EXTRACTION_BOUND`, so the test measures the
                // bound rather than the sleep.
                HostileKind::Hangs => {
                    std::thread::sleep(std::time::Duration::from_secs(60));
                    Ok(String::new())
                }
            }
        }
    }

    const ONE_FINDING: &str = r#"{"memories":[{"kind":"finding","authority":"constraint",
         "disposition":"accepted","support":"established","confidence":"certain",
         "rationale":"the hook process is the only thing that sees a turn end",
         "project_phase":"alpha",
         "body":"Extraction after a task runs in the hook process."}]}"#;

    /// A session this project has recorded, ready to receive a harness event.
    fn recorded_session(runtime: &Runtime) -> glasshouse::session::SessionId {
        use glasshouse::session::NewSession;

        let sessions = ProjectSessions::open(runtime).unwrap();
        let record = sessions
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();
        record.id
    }

    fn stored_memories(runtime: &Runtime) -> Vec<glasshouse::memory::MemoryRecord> {
        use glasshouse::memory::ProjectMemory;
        use glasshouse::memory::search::SearchScope;

        ProjectMemory::open(runtime)
            .unwrap()
            .store()
            .search("extraction", SearchScope::Current, 10)
            .unwrap()
    }

    /// Line: *"Allow memory extraction to run after task completion."*
    ///
    /// The trigger is a harness saying `Stop`, which is the only report
    /// `session::lifecycle::event_for` turns into a completed turn. What the
    /// stored memory carries is the other half of the evidence: the session
    /// it came from, and the **range of this project's event log** the
    /// extractor was actually shown — Phase 21's *"store the originating
    /// session and event references so extracted memory retains
    /// provenance."*
    #[test]
    fn a_completed_task_runs_extraction_and_the_memory_names_where_it_came_from() {
        let fixture = CliFixture::new();
        let id = recorded_session(&fixture.runtime);
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        {
            let asked = std::sync::Arc::clone(&asked);
            report_hook_with(&fixture.runtime, id.as_str(), "Stop", move || {
                Box::new(Canned {
                    reply: ONE_FINDING.to_owned(),
                    asked: std::sync::Arc::clone(&asked),
                })
            });
        }

        assert_eq!(
            asked.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a completed task must ask the extraction model exactly once"
        );

        let stored = stored_memories(&fixture.runtime);
        assert_eq!(stored.len(), 1, "the memory reached the project's store");
        assert_eq!(stored[0].source_session_id.as_deref(), Some(id.as_str()));
        let events = stored[0]
            .source_events
            .expect("a memory extracted from the event log names the slice it came from");
        assert!(
            events.first >= 1 && events.last >= events.first,
            "the provenance range must name real log positions, got {events}"
        );
        assert_eq!(
            stored[0].provenance.project_phase,
            Some(glasshouse::memory::ProjectPhase::Alpha)
        );
    }

    /// The trigger is *task completion*, not *any harness event*.
    ///
    /// `StopFailure` is a turn that ended badly and `UserPromptSubmit` is a
    /// turn starting; neither is a completed task, and extraction that ran on
    /// them would be extraction running on a schedule rather than on the map's
    /// line. This is the discriminating half of the test above — without it,
    /// "runs after task completion" would be satisfied by "runs always".
    #[test]
    fn an_event_that_is_not_a_completed_task_asks_no_model() {
        for event in ["StopFailure", "UserPromptSubmit", "PermissionRequest"] {
            let fixture = CliFixture::new();
            let id = recorded_session(&fixture.runtime);
            let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

            {
                let asked = std::sync::Arc::clone(&asked);
                report_hook_with(&fixture.runtime, id.as_str(), event, move || {
                    Box::new(Canned {
                        reply: ONE_FINDING.to_owned(),
                        asked: std::sync::Arc::clone(&asked),
                    })
                });
            }

            assert_eq!(
                asked.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "`{event}` is not a completed task and must not run extraction"
            );
            assert!(stored_memories(&fixture.runtime).is_empty());
        }
    }

    /// Line: *"Keep memory-extraction failure non-fatal to the coding
    /// session."*
    ///
    /// This is the line's real setting: a `glasshouse hook` process running
    /// **inside the user's session**, where Claude Code treats a non-zero
    /// exit as a veto on the turn. Three failures a support job can produce,
    /// and after each one the session must be exactly as it would have been
    /// with no extraction at all — the event recorded, the lifecycle applied,
    /// nothing propagated.
    ///
    /// Note what is *not* asserted: that extraction succeeded. It did not,
    /// three times. That is the point.
    #[test]
    fn a_failing_extraction_model_costs_the_coding_session_nothing() {
        use glasshouse::session::SessionLifecycle;

        for kind in [HostileKind::Refuses, HostileKind::Panics] {
            let fixture = CliFixture::new();
            let id = recorded_session(&fixture.runtime);

            report_hook_with(&fixture.runtime, id.as_str(), "Stop", move || {
                Box::new(Hostile(match kind {
                    HostileKind::Refuses => HostileKind::Refuses,
                    HostileKind::Panics => HostileKind::Panics,
                    HostileKind::Hangs => HostileKind::Hangs,
                }))
            });

            // The session's own bookkeeping happened anyway.
            let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
            let record = sessions.store().get(&id).unwrap().unwrap();
            assert_eq!(
                record.lifecycle,
                SessionLifecycle::Idle,
                "a failed extraction must not stop the turn being recorded as ended"
            );

            let log = EventLog::open(&fixture.runtime).unwrap();
            assert_eq!(
                log.for_session(&id).unwrap().len(),
                1,
                "the lifecycle event is recorded whatever extraction did"
            );
            assert!(stored_memories(&fixture.runtime).is_empty());
        }
    }

    /// The fourth failure, and the only one a `Result` could never have
    /// absorbed: a model that never answers at all.
    ///
    /// `EXTRACTION_BOUND` is what stands between a hung provider and a user
    /// whose turn will not finish. The model here sleeps for a minute; the
    /// hook must be long gone.
    #[test]
    fn an_extraction_model_that_never_answers_is_abandoned_at_its_bound() {
        use glasshouse::session::SessionLifecycle;

        let fixture = CliFixture::new();
        let id = recorded_session(&fixture.runtime);

        let started = std::time::Instant::now();
        report_hook_with(&fixture.runtime, id.as_str(), "Stop", || {
            Box::new(Hostile(HostileKind::Hangs))
        });
        let waited = started.elapsed();

        assert!(
            waited < EXTRACTION_BOUND * 3,
            "the hook waited {waited:?} on a model that sleeps for a minute;              the bound is {EXTRACTION_BOUND:?}"
        );
        assert!(
            waited >= EXTRACTION_BOUND,
            "waiting {waited:?} means the bound was not what ended the wait"
        );

        let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
        assert_eq!(
            sessions.store().get(&id).unwrap().unwrap().lifecycle,
            SessionLifecycle::Idle
        );
        assert!(stored_memories(&fixture.runtime).is_empty());
    }

    /// What the shipped binary does today, stated as a test so it cannot
    /// quietly become something else.
    ///
    /// Phase 21 has two lines here and only one of them is built: the
    /// trigger is real and the **model is Phase 39's**. So a real
    /// `glasshouse hook` runs extraction after every completed task and
    /// reports that no model is available — and `NoExtractionModel::describe`
    /// says so in words, for the same reason `glasshouse memory extract`
    /// prints `no model was called`: an evaluation must never be mistakeable
    /// later for evidence a model did the work.
    #[test]
    fn the_shipped_binary_runs_extraction_after_a_task_and_reports_that_it_has_no_model() {
        use glasshouse::memory::ExtractionModel as _;

        let fixture = CliFixture::new();
        let id = recorded_session(&fixture.runtime);

        // The production entry point, with production's own model.
        report_hook(&fixture.runtime, id.as_str(), "Stop");

        assert!(stored_memories(&fixture.runtime).is_empty());
        let described = NoExtractionModel.describe();
        assert!(
            described.contains("none configured"),
            "the production model must name itself as absent: {described}"
        );
        assert_eq!(
            glasshouse::memory::ModelError::Unavailable.to_string(),
            "no extraction model is available"
        );
    }

    /// Phase 9I lines 530, 531 and 540, at the function `report_hook` itself
    /// calls to get its model. A user-configured free model, written to disk
    /// exactly as Settings would write it, is the one the disposable routing
    /// policy names, and the description says plainly that no model was
    /// called.
    ///
    /// **Not through `report_hook`'s own log line.** `run_extraction_after_turn`
    /// reports its outcome only via `tracing`, and capturing that reliably
    /// needs a thread-local subscriber — which this project's own
    /// `gateway::ingress::tests::recorded` uses successfully in isolation, but
    /// which proved to race `tracing`'s process-wide callsite interest cache
    /// under `scripts/ci-local.sh`'s real concurrent load here: the exact
    /// same assertion passed alone and failed, non-deterministically empty or
    /// partial, run beside this crate's other ~1050 tests. A flaky gate is
    /// worse than a narrower one, so this calls `disposable_extraction_model`
    /// directly instead — the paired test below,
    /// `report_hook_routes_extraction_through_disposable_extraction_model`,
    /// is what proves `report_hook` itself still calls it.
    #[test]
    fn disposable_extraction_model_prefers_a_configured_free_model_and_names_the_reason() {
        const VAR: &str = "GLASSHOUSE_TEST_ONLY_WIRE_DISPOSABLE_FREE_KEY";
        // SAFETY: `VAR` is unique to this test and removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }

        let fixture = CliFixture::new();
        let mut user = UserConfig::load(fixture.runtime.paths()).unwrap();
        let mut provider = glasshouse::config::ProviderConfig::new("openai-compatible");
        provider.set_credential_env(vec![VAR.to_owned()]);
        provider.set_free_models(vec!["nvidia/nemotron-nano-9b-v2:free".to_owned()]);
        user.providers_mut()
            .set("wire-disposable-test-provider", provider);
        user.save(fixture.runtime.paths()).unwrap();

        let model = disposable_extraction_model(&fixture.runtime);
        let described = model.describe();

        unsafe {
            std::env::remove_var(VAR);
        }

        assert!(
            described.contains("nvidia/nemotron-nano-9b-v2:free"),
            "the free model the user configured must be the one named: {described}"
        );
        assert!(
            described.contains("no model was called"),
            "an evaluation must never be mistakeable later for evidence a model did the work: \
             {described}"
        );
        // Map lines 1530 and 1554, at this same real production entry
        // point: the winning candidate's inspectable score travels all the
        // way to `describe()`, not only to a unit test that constructs
        // `DisposableRouting` directly. No telemetry was ever cached for
        // this provider, so the honest absence contributions are what must
        // appear — see the paired test below for the populated case.
        assert!(
            described.contains("normalized remaining capacity"),
            "the scorer's own contributions must reach the production caller's description: \
             {described}"
        );
        assert!(
            described.contains("no capacity telemetry cached"),
            "a provider nothing has been read about must say so rather than guess: {described}"
        );
    }

    /// The same production entry point as
    /// `disposable_extraction_model_prefers_a_configured_free_model_and_names_the_reason`,
    /// with one difference: a real [`glasshouse::provider::telemetry::GatewayQuotaCache`]
    /// entry planted on disk first — the same on-disk reading
    /// `glasshouse resources` already reads, and phase-32d's own report
    /// planted for the identical reason. Map lines 1536 and 1549 close only
    /// if this reading reaches `DisposableRouting::choose` through
    /// `disposable_extraction_model` itself, not through a test that
    /// constructs the routing policy by hand — so this calls no
    /// `routing::disposable` type directly.
    #[test]
    fn disposable_extraction_model_reflects_real_cached_capacity_telemetry() {
        const VAR: &str = "GLASSHOUSE_TEST_ONLY_WIRE_DISPOSABLE_CAPACITY_KEY";
        const PROVIDER: &str = "wire-disposable-capacity-test-provider";
        // SAFETY: `VAR` is unique to this test and removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }

        let fixture = CliFixture::new();
        let mut user = UserConfig::load(fixture.runtime.paths()).unwrap();
        let mut provider = glasshouse::config::ProviderConfig::new("openai-compatible");
        provider.set_credential_env(vec![VAR.to_owned()]);
        provider.set_free_models(vec!["a-free-model".to_owned()]);
        user.providers_mut().set(PROVIDER, provider);
        user.save(fixture.runtime.paths()).unwrap();

        let now_unix = glasshouse::provider::cache::now_unix_seconds();
        glasshouse::provider::telemetry::GatewayQuotaCache::new(fixture.runtime.paths()).store(
            PROVIDER,
            &glasshouse::provider::telemetry::RateLimitHeaders::read(vec![
                ("x-ratelimit-limit-requests", "7000"),
                ("x-ratelimit-limit-tokens", "6000"),
                ("x-ratelimit-remaining-requests", "6999"),
                ("x-ratelimit-remaining-tokens", "5991"),
                ("x-ratelimit-reset-requests", "300s"),
                ("x-ratelimit-reset-tokens", "300s"),
            ]),
            now_unix,
        );

        let model = disposable_extraction_model(&fixture.runtime);
        let described = model.describe();

        unsafe {
            std::env::remove_var(VAR);
        }

        assert!(
            !described.contains("no capacity telemetry cached"),
            "a real cached reading must displace the absence contribution: {described}"
        );
        assert!(
            described.contains("normalized remaining capacity"),
            "the capacity contribution must still be named: {described}"
        );
        assert!(
            !described.contains("no reset time known"),
            "a real cached reading carries a reset time too: {described}"
        );
    }

    /// `report_hook` — not `report_hook_with`, which every fixture above
    /// supplies its own fake model to — must itself ask
    /// `disposable_extraction_model` for its model, and never
    /// `NoExtractionModel` directly. A source scan, in the same style as
    /// `hook_handler_source`'s: the alternative is a runtime assertion that
    /// needs the model to actually run, and `report_hook`'s own body is two
    /// lines specifically so that reading it settles the question.
    #[test]
    fn report_hook_routes_extraction_through_disposable_extraction_model() {
        let full = include_str!("main.rs");
        let start = full
            .find("fn report_hook(runtime: &Runtime, session: &str, event: &str) {")
            .expect("report_hook must exist in this file");
        let after_start = &full[start..];
        let end = after_start
            .find("\n}")
            .expect("report_hook must have a top-level closing brace");
        let body = strip_comments(&after_start[..end]);

        assert!(
            body.contains("disposable_extraction_model"),
            "report_hook must ask disposable_extraction_model for its model: {body}"
        );
        assert!(
            !body.contains("NoExtractionModel"),
            "report_hook must not name NoExtractionModel itself — that is \
             disposable_extraction_model's own fallback for a configuration it could not read: \
             {body}"
        );
    }

    /// Phase 9I line 532, at the real production entry point — this file's
    /// own `gateway_upstream` wrapper, not `glasshouse::profile::gateway_upstream`
    /// directly. A provider the user marked a free model on, written to disk
    /// exactly as Settings would write it, backs the gateway at `Cost::Free`.
    #[test]
    fn a_configured_free_model_backs_the_gateway_at_no_cost() {
        const VAR: &str = "GLASSHOUSE_TEST_ONLY_WIRE_DISPOSABLE_GATEWAY_FREE_KEY";
        // SAFETY: `VAR` is unique to this test and removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }

        let fixture = CliFixture::new();
        let mut user = UserConfig::load(fixture.runtime.paths()).unwrap();
        let mut provider = glasshouse::config::ProviderConfig::new("anthropic-compatible");
        provider.set_base_url(Some("https://example.invalid/api".to_owned()));
        provider.set_credential_env(vec![VAR.to_owned()]);
        provider.set_free_models(vec!["a-free-model".to_owned()]);
        user.providers_mut()
            .set("wire-disposable-gateway-provider", provider);
        user.save(fixture.runtime.paths()).unwrap();

        let user = UserConfig::load(fixture.runtime.paths()).unwrap();
        let project = config::load_project_config(fixture.runtime.project()).unwrap();
        let effective = EffectiveConfig::new(&user, project.as_ref());
        let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();

        let upstream = gateway_upstream(&user, project.as_ref(), &effective, &secrets).unwrap();
        let rendered = format!("{upstream:?}");

        unsafe {
            std::env::remove_var(VAR);
        }

        assert!(
            rendered.contains("cost: \"free\""),
            "a provider the user marked a free model on must back the gateway at no cost: \
             {rendered}"
        );
    }

    /// Line: *"Treat a decision with missing rationale and missing
    /// assumptions as lower-confidence than a well-proven decision of the
    /// same authority class"*, at the surface a person reads.
    ///
    /// The ranking is the behaviour — `memory::search::demote_thin_decisions`
    /// puts such a decision behind a better-proven one of its own class, and
    /// four tests in `memory_provenance.rs` pin each clause of that. This is
    /// the other half, and it is not decoration: a reader handed a reordering
    /// with no reason for it has been given a mystery. The word `unclassified`
    /// earned its place in this output for the same reason.
    ///
    /// It also pins the negative case, which is where a label goes wrong: a
    /// decision that recorded *why* must not be marked, and neither must a
    /// finding that recorded nothing — the map's line is about decisions.
    #[test]
    fn a_search_marks_a_thinly_provenanced_decision_and_shows_the_provenance_of_the_others() {
        use glasshouse::memory::{
            DecisionProvenance, MemoryAuthority, MemoryKind, NewMemory, ProjectMemory, ProjectPhase,
        };

        let fixture = CliFixture::new();
        let project = ProjectMemory::open(&fixture.runtime).unwrap();
        let store = project.store();

        store
            .record(
                NewMemory::new(MemoryKind::Decision, "Kestrel runs on one instance.")
                    .with_subject(Some("kestrel topology"))
                    .with_authority(Some(MemoryAuthority::Decision))
                    .with_provenance(DecisionProvenance {
                        rationale: Some("the deploy target has one machine".to_owned()),
                        project_phase: Some(ProjectPhase::Beta),
                        operational_assumptions: Some("single instance, no daemon".to_owned()),
                        ..DecisionProvenance::default()
                    }),
            )
            .unwrap();
        store
            .record(
                NewMemory::new(MemoryKind::Decision, "Kestrel logs to stderr.")
                    .with_subject(Some("kestrel logging"))
                    .with_authority(Some(MemoryAuthority::Decision)),
            )
            .unwrap();
        store
            .record(
                NewMemory::new(MemoryKind::Finding, "Kestrel starts in under a second.")
                    .with_subject(Some("kestrel startup"))
                    .with_authority(Some(MemoryAuthority::Decision)),
            )
            .unwrap();

        let report = memory_report(&fixture.runtime, "kestrel", false, 10).unwrap();

        // The well-proven decision shows its reasoning, labelled.
        assert!(report.contains("phase      beta"), "{report}");
        assert!(
            report.contains("why        the deploy target has one machine"),
            "{report}"
        );
        assert!(
            report.contains("ops        single instance, no daemon"),
            "{report}"
        );

        // Exactly one line carries the marker, and it is the bare decision's.
        let marked: Vec<&str> = report
            .lines()
            .filter(|line| line.contains("lower-confidence"))
            .collect();
        assert_eq!(marked.len(), 1, "expected one marked line in:\n{report}");
        assert!(marked[0].contains("kestrel logging"), "{}", marked[0]);
    }

    /// Line: *"Store the originating session and event references so
    /// extracted memory retains provenance"* — at the surface a person
    /// reaches, which is what `glasshouse memory extract --from-events`
    /// exists for.
    ///
    /// The file-fed form of the same command cannot produce this: activity
    /// read out of a file has no position in the project's log to name.
    #[test]
    fn extracting_from_a_sessions_events_records_the_slice_of_the_log_it_read() {
        let fixture = CliFixture::new();
        let id = recorded_session(&fixture.runtime);

        // Give the session a history, the way a session gets one: through
        // the same hook path a harness drives.
        report_hook(&fixture.runtime, id.as_str(), "UserPromptSubmit");
        report_hook(&fixture.runtime, id.as_str(), "Stop");

        let dir = tempfile::tempdir().unwrap();
        let reply = dir.path().join("reply.json");
        std::fs::write(&reply, ONE_FINDING).unwrap();

        let report = memory_extract(&fixture.runtime, id.as_str(), None, true, &reply).unwrap();

        assert!(report.contains("recorded events for session"), "{report}");
        assert!(report.contains("provenance: event"), "{report}");
        assert!(report.contains("stored 1"), "{report}");

        let stored = stored_memories(&fixture.runtime);
        assert_eq!(stored.len(), 1);
        let events = stored[0].source_events.expect("an event range");
        assert_eq!(events.first, 1);
        assert_eq!(events.last, 2, "both recorded events reached the model");
    }

    /// Phase 21 — extraction runs manually, for debugging and evaluation.
    ///
    /// The model half is supplied from a file, which is what makes this
    /// runnable before Phase 39 exists. Everything else is the production
    /// path, and the assertions below are on that: the reply is validated,
    /// classified conservatively, screened and stored.
    #[test]
    fn a_manual_extraction_runs_the_whole_pipeline_and_says_no_model_was_called() {
        use glasshouse::memory::ProjectMemory;
        use glasshouse::memory::search::SearchScope;

        let fixture = CliFixture::new();
        let dir = tempfile::tempdir().unwrap();
        let activity = dir.path().join("activity.txt");
        let reply = dir.path().join("reply.json");
        std::fs::write(&activity, "the kestrel migration ran twice\n").unwrap();
        std::fs::write(
            &reply,
            r#"{"memories":[{"kind":"finding","authority":"constraint",
                 "disposition":"accepted","support":"established",
                 "confidence":"certain",
                 "rationale":"the runner resumes from MAX(version)",
                 "body":"A migration rollback must delete a contiguous range."}]}"#,
        )
        .unwrap();

        let report =
            memory_extract(&fixture.runtime, "s-1", Some(&activity), false, &reply).unwrap();

        assert!(report.contains("stored 1"), "{report}");
        // The output must never let an evaluation run be mistaken later for
        // evidence that a model performed extraction.
        assert!(
            report.contains("no model was called"),
            "the run must say a model was not called:\n{report}"
        );

        let stored = ProjectMemory::open(&fixture.runtime)
            .unwrap()
            .store()
            .search("migration", SearchScope::Current, 10)
            .unwrap();
        assert_eq!(stored.len(), 1, "the memory reached the real store");
        assert_eq!(stored[0].source_session_id.as_deref(), Some("s-1"));
    }
}
