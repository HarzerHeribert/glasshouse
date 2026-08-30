use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use std::io::IsTerminal;

use glasshouse::checkpoint::git::GitPosition;
use glasshouse::checkpoint::{
    Checkpoint, CheckpointReason, CheckpointStore, Handoff, ProjectCheckpoints, Stored,
    WorkingTreeStatus,
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
            let request = text.join(" ");
            // A model failure degrades to the heuristic and says so, rather
            // than failing the command: the classification is still produced,
            // and Phase 35's own fallback is what produces it. The exit code
            // is unchanged — this command has never had a failure mode, and a
            // routing model the user configured being unreachable is not one
            // it should acquire.
            let model_output = match classify_with_routing_model(&runtime, &request) {
                ClassificationAttempt::NotConfigured => None,
                ClassificationAttempt::Answered(classification) => Some(classification),
                ClassificationAttempt::Failed(why) => {
                    eprintln!("glasshouse: {why}; deterministic heuristics answered instead");
                    None
                }
            };
            print!(
                "{}",
                glasshouse::routing::classify::report(&request, model_output)
            );
        }
        Some(Command::Route {
            moment,
            to,
            fresh,
            now,
            task,
        }) => match route_report(
            &runtime,
            moment,
            to.as_deref(),
            *fresh,
            *now,
            task.as_deref(),
        ) {
            Ok(report) => print!("{report}"),
            Err(err) => {
                eprintln!("glasshouse: {err:#}");
                return Ok(ExitCode::FAILURE);
            }
        },
        Some(Command::RoutingCost { hours }) => match routing_cost_report(&runtime, *hours) {
            Ok(report) => print!("{report}"),
            Err(err) => {
                eprintln!("glasshouse: {err:#}");
                return Ok(ExitCode::FAILURE);
            }
        },
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
            Some(SessionCommand::Reserve { session, clear }) => {
                match reserve_override_session(&runtime, session, *clear) {
                    Ok(report) => print!("{report}"),
                    Err(err) => {
                        eprintln!("glasshouse: {err:#}");
                        return Ok(ExitCode::FAILURE);
                    }
                }
            }
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
            to,
            fresh,
            headless,
            harness_args,
        })
        | Some(Command::Run {
            harness,
            response_profile,
            response_role,
            profile,
            from_checkpoint,
            to,
            fresh,
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
                LaunchDestination {
                    profile: profile.as_deref(),
                    from_checkpoint: from_checkpoint.as_deref(),
                    to: to.as_deref(),
                    fresh: *fresh,
                },
                &response,
                *headless,
                harness_args,
            );
        }
        Some(Command::Resume {
            session,
            harness_args,
        }) => {
            return resume_session(
                &runtime,
                session,
                harness_args,
                false,
                RouteOnResume::AtTaskBoundary,
            );
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
            MemoryCommand::Challenge { id, reason } => {
                print!("{}", memory_challenge(&runtime, id, reason)?);
            }
            MemoryCommand::Revalidate {
                id,
                outcome,
                by,
                reason,
                automatic,
                list,
                limit,
            } => {
                if *list {
                    print!("{}", memory_revalidate_list(&runtime, *limit)?);
                } else {
                    let id = id.as_deref().ok_or_else(|| {
                        anyhow::anyhow!("an id is required unless --list is given")
                    })?;
                    let outcome = outcome.as_deref().ok_or_else(|| {
                        anyhow::anyhow!("an outcome is required unless --list is given")
                    })?;
                    print!(
                        "{}",
                        memory_revalidate(
                            &runtime,
                            id,
                            outcome,
                            by.as_deref(),
                            reason.as_deref(),
                            *automatic
                        )?
                    );
                }
            }
            MemoryCommand::Conflicts { limit } => {
                print!("{}", memory_conflicts_list(&runtime, *limit)?);
            }
            MemoryCommand::Resolve { id, outcome } => {
                print!("{}", memory_resolve_conflict(&runtime, id, outcome)?);
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
            // The two client verbs. They resolve this project's socket from
            // `runtime` — the same `--scope`-or-Git-root resolution every
            // other subcommand here performs — and take no path of their
            // own; see `cli::ApiCommand::Send` for why that is the security
            // property rather than an omission.
            ApiCommand::Send { session, text } => {
                api::send_message(&runtime, session, text)?;
            }
            ApiCommand::Interrupt { session } => {
                api::interrupt(&runtime, session)?;
            }
            ApiCommand::Read { session, max_bytes } => {
                api::read_output(&runtime, session, *max_bytes)?;
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

// ---------------------------------------------------------------------------
// Phase 37 — the session router's production callers, map lines 1592–1602.
//
// `glasshouse::routing::session` ranks *destinations*, and a destination is
// something only this file can assemble: it needs this project's session
// records, this user's launch profiles, the provider table and the quota
// cache, none of which that module is allowed to reach (its own
// `the_session_router_cannot_look_a_session_or_a_checkpoint_up` fails the
// build if it ever tries). So the five inputs are read here, once, and every
// caller below goes through the same two functions.
// ---------------------------------------------------------------------------

/// Everything a person typed about **where** this session goes and what it
/// boots from — the four arguments `launch_session` reads before it resolves
/// anything.
///
/// One type rather than four parameters because they are one statement, and
/// the router reads all four together: `to` and `fresh` are line 1602's
/// override outright, and `profile` and `from_checkpoint` are the two ways of
/// saying "a new session" without using that word (see the override built in
/// `launch_session`). Separating them would let a caller pass this decision's
/// profile with last decision's override, which is the same reason
/// `routing::session::RouterInputs` is one struct.
#[derive(Debug, Clone, Copy, Default)]
struct LaunchDestination<'a> {
    /// `--profile`: the launch profile a **new** session runs under.
    profile: Option<&'a str>,
    /// `--from-checkpoint`: the handoff a new session opens with.
    from_checkpoint: Option<&'a str>,
    /// `--to`: this destination, whatever the ranking says.
    to: Option<&'a str>,
    /// `--fresh`: a new session, whatever the ranking says.
    fresh: bool,
}

/// The identifier `--to` takes for "a new session under this profile".
///
/// Three parts, and each one is load-bearing. The `fresh:` prefix keeps a
/// destination that does not exist yet out of the namespace of recorded
/// session identifiers, which is what `--to` and `RoutingOverride::to`
/// compare against. The harness slug is there because `glasshouse route`
/// ranks across every enabled harness and **every one of them has a `native`
/// profile** — without it, `fresh:native` names between one and ten different
/// destinations and an override lands on whichever was built first.
fn fresh_destination_id(harness: glasshouse::integrations::IntegrationId, profile: &str) -> String {
    format!("fresh:{}:{profile}", harness.slug())
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
enum DestinationScope<'a> {
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
}

/// Every place this project's next piece of work could go, and the current
/// destination when the caller is standing in one.
///
/// Ordered sessions-first, most recently active first, then one fresh
/// destination per configured launch profile; `SessionRouter::choose` uses the
/// caller's order as its tiebreaker, and "what you were most recently doing"
/// is the honest tiebreaker for equal scores.
fn routing_destinations(
    runtime: &Runtime,
    effective: &EffectiveConfig<'_>,
    harness: glasshouse::integrations::IntegrationId,
    scope: DestinationScope<'_>,
) -> anyhow::Result<Vec<glasshouse::routing::session::Destination>> {
    use glasshouse::routing::session::Destination;

    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let telemetry = glasshouse::provider::resources::GatheredTelemetry::new().gather_gateway_quota(
        &glasshouse::provider::telemetry::GatewayQuotaCache::new(runtime.paths()),
    );

    let mut destinations = Vec::new();

    // 1. The sessions this project already has.
    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    for record in store.list()? {
        // A session on another harness is not a destination for a launch that
        // has already selected this one, and `resume` reads the harness off
        // the record rather than ranking across them.
        if record.harness != harness.slug() {
            continue;
        }
        let Some(warm) = warm_session(&record, now_unix, scope) else {
            continue;
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
        let (backend, protocols) = destination_backend(effective, &profile, record.model.clone());
        destinations.push(
            with_provider_protocols(
                Destination::existing(
                    record.id.as_str(),
                    harness,
                    profile.name.clone(),
                    backend,
                    warm,
                ),
                protocols,
            )
            .with_capacity(destination_capacity(
                &profile, effective, &telemetry, now_unix,
            )),
        );
    }

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
    let offered: Vec<String> = match scope {
        DestinationScope::Everything => effective
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
        let (backend, protocols) = destination_backend(effective, &profile, None);
        destinations.push(
            with_provider_protocols(
                Destination::fresh(
                    fresh_destination_id(harness, &name),
                    harness,
                    profile.name.clone(),
                    backend,
                    checkpoint,
                ),
                protocols,
            )
            .with_capacity(destination_capacity(
                &profile, effective, &telemetry, now_unix,
            )),
        );
    }

    Ok(destinations)
}

/// Whether a launch under `profile` would get past `glasshouse::profile::resolve`'s
/// protocol check, which refuses with `Refusal::ProtocolMismatch`.
///
/// # Why the launch path has to ask this and the diagnostic does not
///
/// The two are asking different questions and both answers are right. The
/// router's `ProtocolFit::Compatible` means *"not this protocol, but the
/// provider serves another one the harness does speak"* — a true statement
/// about whether that provider and that harness can work together at all, and
/// the reason `Destination::with_provider_protocols` exists. `profile::resolve`
/// asks something narrower: whether the harness can serve the protocol **this
/// profile declared**, and it refuses rather than quietly picking a different
/// one, which is that module's whole discipline.
///
/// So a profile can be `Compatible` to the router and refused by the launch,
/// and offering the launch path a destination it will then refuse would turn a
/// routing decision into a failed command. The diagnostic keeps ranking it,
/// because "this provider could serve this harness, but not over the protocol
/// you configured" is exactly what a person needs to read.
fn launch_can_resolve_protocol(profile: &glasshouse::profile::LaunchProfile) -> bool {
    let Some(expected) = profile.expected_protocol else {
        return true;
    };
    glasshouse::harness::adapter_for(profile.harness)
        .map(|adapter| adapter.describe().backends.protocols)
        .and_then(|declared| {
            declared
                .value()
                .map(|protocols| protocols.contains(&expected))
        })
        .unwrap_or(false)
}

/// The profile a `fresh:<harness>:<profile>` identifier names, when it names
/// one for `harness`.
///
/// `None` for a recorded session's identifier, and `None` for a fresh
/// identifier belonging to a different harness — which then reaches the router
/// as an override naming a destination that was not offered, and is refused
/// out loud rather than silently reinterpreted.
fn fresh_destination_profile(
    id: &str,
    harness: glasshouse::integrations::IntegrationId,
) -> Option<&str> {
    id.strip_prefix("fresh:")?
        .strip_prefix(harness.slug())?
        .strip_prefix(':')
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
/// `Cost` is `Metered` for everything here, and that is not a shortcut: the
/// session router reads a backend's provider, credential, model and tool
/// semantics and never its cost, and `Cost::Metered` is the fail-closed value
/// the rest of this project uses when nobody has marked a model free.
fn destination_backend(
    effective: &EffectiveConfig<'_>,
    profile: &glasshouse::profile::LaunchProfile,
    recorded_model: Option<glasshouse::routing::AssignedModel>,
) -> (
    glasshouse::routing::Backend,
    Vec<glasshouse::harness::WireProtocol>,
) {
    use glasshouse::profile::BackendResource;
    use glasshouse::routing::{Backend, Cost, CredentialId};
    use glasshouse::secret::SecretRef;

    let pairing = session_pairing(effective, profile);
    let model = recorded_model.unwrap_or_else(|| pairing.model().clone());
    let protocol = pairing
        .route()
        .protocol
        .map(|protocol| protocol.slug().to_owned())
        .unwrap_or_default();

    let (provider, credential, protocols) = match &profile.backend {
        BackendResource::DirectProvider { provider } => {
            match effective.configured_provider(provider) {
                Ok(resolved) => {
                    let resolved = resolved.value;
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
        ),
    };

    (
        Backend::new(
            provider,
            protocol,
            model,
            credential,
            Cost::Metered,
            pairing.tool_semantics(),
        ),
        protocols,
    )
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
/// `glasshouse resources` reads and with no request of its own.
fn destination_capacity(
    profile: &glasshouse::profile::LaunchProfile,
    effective: &EffectiveConfig<'_>,
    telemetry: &glasshouse::provider::resources::GatheredTelemetry,
    now_unix: i64,
) -> Option<glasshouse::provider::quota::RemainingCapacityScore> {
    use glasshouse::profile::BackendResource;
    use glasshouse::provider::registry::ResourceKind;

    let kind = match &profile.backend {
        BackendResource::Native => ResourceKind::NativeSubscription {
            harness: profile.harness,
        },
        BackendResource::DirectProvider { provider } => {
            ResourceKind::from_direct_provider(provider.clone())
        }
        BackendResource::GlasshouseGateway => ResourceKind::GlasshouseGateway,
    };
    glasshouse::provider::resources::observed_capacity(&kind, effective, telemetry, now_unix)
        .remaining_capacity_score()
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
fn observed_provider_health(
    runtime: &Runtime,
    destinations: &[glasshouse::routing::session::Destination],
) -> glasshouse::routing::free::FreePool {
    use glasshouse::routing::free::FreeResource;

    observed_health_of(
        runtime,
        destinations.iter().map(|destination| {
            FreeResource::new(
                destination.backend().credential().clone(),
                destination.backend().model().label(),
            )
        }),
    )
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
fn observed_health_of(
    runtime: &Runtime,
    resources: impl IntoIterator<Item = glasshouse::routing::free::FreeResource>,
) -> glasshouse::routing::free::FreePool {
    use glasshouse::provider::telemetry::{GatewayHealthCache, GatewayHealthReading};
    use glasshouse::routing::free::FreePool;

    let mut pool = FreePool::new();
    let stored = GatewayHealthCache::new(runtime.paths()).load_all();
    if stored.is_empty() {
        return pool;
    }

    // Hazard 2: one pair, read together, for every reading below.
    let now = std::time::Instant::now();
    let now_unix = glasshouse::provider::cache::now_unix_seconds();

    for resource in resources {
        let credential = resource.credential();
        let label = credential.label();
        let model = resource.model().to_owned();

        let mut named: Option<&GatewayHealthReading> = None;
        let mut contradicted = false;
        for reading in stored
            .iter()
            .filter(|(provider, _)| provider == credential.provider())
            .flat_map(|(_, readings)| readings.iter())
            .filter(|reading| reading.credential_label == label && reading.model == model)
        {
            match named {
                None => named = Some(reading),
                // Two entries saying the same thing are one reading written
                // twice, not a disagreement.
                Some(first) if first == reading => {}
                Some(_) => {
                    contradicted = true;
                    break;
                }
            }
        }
        let Some(reading) = named.filter(|_| !contradicted) else {
            continue;
        };

        pool.adopt_observed(
            &resource,
            reading.consecutive_failures,
            reading.cooling_down_until(now, now_unix),
            reading.credential_rejected,
        );
    }

    pool
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
fn routing_override(
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

/// `glasshouse route` — map lines 1601 and 1602: the command, which is
/// [`route_recommendation`] asked and [`render_route_recommendation`]
/// printed, and nothing else.
///
/// The moment is parsed here rather than inside the recommendation because
/// this is where a person's typed spelling arrives, and the message they get
/// back quotes it.
fn route_report(
    runtime: &Runtime,
    moment: &str,
    to: Option<&str>,
    fresh: bool,
    now: bool,
    task: Option<&str>,
) -> anyhow::Result<String> {
    let Some(parsed) = routing_moment_from_str(moment) else {
        anyhow::bail!(
            "`{moment}` is not a routing moment; use `session-start`, `task-boundary` or \
             `mid-turn`"
        )
    };

    let user = UserConfig::load(runtime.paths())?;
    let project = config::load_project_config(runtime.project())?;
    let effective = EffectiveConfig::new(&user, project.as_ref());

    let recommendation = route_recommendation(runtime, &effective, parsed, to, fresh, now, task)?;
    Ok(render_route_recommendation(&recommendation))
}

/// `glasshouse routing-cost` — capability map line 1464: what Glasshouse's
/// own routing model has spent, in tokens and requests, apart from every
/// other row this project's evidence ledger holds.
///
/// # Why the ledger is opened here, and nowhere earlier (practice §65)
///
/// The same reasoning [`record_classification_observation`]'s own header
/// gives: an open [`glasshouse::routing::evidence::EvidenceLedger`] holds a
/// SQLite handle for its whole lifetime, and a handle opened for work that
/// never happens blocks a later writer under Windows while staying invisible
/// under POSIX advisory locks. This command's handler is the one path that
/// actually reads the ledger, so it is opened here and nowhere upstream of
/// it.
fn routing_cost_report(runtime: &Runtime, hours: u32) -> anyhow::Result<String> {
    let ledger = glasshouse::routing::evidence::EvidenceLedger::open(runtime)?;
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let window_seconds = i64::from(hours) * 3600;
    let groups = ledger.consumption_by_purpose(now_unix, window_seconds)?;
    Ok(render_routing_cost(
        runtime.project().id().as_str(),
        hours,
        &groups,
    ))
}

/// Render [`routing_cost_report`]'s per-`(purpose, harness_recorded)` groups
/// as `glasshouse routing-cost` prints them.
///
/// **The one rule this function exists to hold:** a token figure nobody
/// counted prints as the words *not counted*, never as a digit and never as
/// `0` — the hazard this whole package was built to avoid, because "nothing
/// was spent" and "nobody counted it" are different facts and a reader who
/// cannot tell them apart has been handed a fabrication. It is the
/// coding-agent group, below, that this bites hardest: it has a real request
/// count and no token count at all, and the two must never be allowed to
/// look like the same kind of absence.
///
/// Capability map line 1331's gateway half applies the same rule to a
/// different pair of columns: `first-byte samples` is a real count (honestly
/// `0` when nothing timed), and `time to first byte` is `render_time_to_first_byte`'s
/// own *not recorded* — never `0ms` — for exactly that case. Unlike the token
/// columns above, the coding-agent group is the one group this build **can**
/// honestly time, because a first-byte instant is a clock reading rather than
/// a read of the response body the relay never parses.
fn render_routing_cost(
    project_id: &str,
    hours: u32,
    groups: &[glasshouse::routing::evidence::PurposeConsumption],
) -> String {
    let mut out = format!("Routing consumption for project {project_id}, last {hours}h\n");
    if groups.is_empty() {
        out.push_str("\n  no routing observations recorded in this window\n");
    } else {
        for group in groups {
            let label = purpose_group_label(group);
            out.push_str(&format!("\n  {label}\n"));
            out.push_str(&format!(
                "    requests            : {}\n",
                group.sample_count
            ));
            out.push_str(&format!(
                "    input tokens        : {}\n",
                render_token_count(group.input_tokens)
            ));
            out.push_str(&format!(
                "    output tokens       : {}\n",
                render_token_count(group.output_tokens)
            ));
            out.push_str(&format!(
                "    cached input tokens : {}\n",
                render_token_count(group.cached_input_tokens)
            ));
            out.push_str(&format!(
                "    first-byte samples  : {}\n",
                group.first_byte_sample_count
            ));
            out.push_str(&format!(
                "    time to first byte  : {}\n",
                render_time_to_first_byte(group.mean_time_to_first_byte_ms)
            ));
        }
    }
    out.push_str(
        "\ncoding-agent consumption relayed through the gateway is never counted in this \
         build (the relay never parses a reply body), so the coding-agent group above always \
         has its tokens print as \"not counted\" even though its request count is real; \
         \"not counted\" always means nobody read a count, never that nothing was spent.\n",
    );
    out
}

/// The label one [`PurposeConsumption`][glasshouse::routing::evidence::PurposeConsumption]
/// group prints under.
///
/// `purpose` alone cannot tell coding-agent consumption apart from every
/// other unstamped producer — both leave it `NULL` — so a `None` purpose is
/// read alongside `harness_recorded`, exactly as
/// [`glasshouse::routing::evidence::PurposeConsumption`]'s own doc comment
/// explains: only the gateway relay names a harness on every row it writes.
fn purpose_group_label(group: &glasshouse::routing::evidence::PurposeConsumption) -> &str {
    match (group.purpose.as_deref(), group.harness_recorded) {
        (Some(purpose), _) => purpose,
        (None, true) => "coding-agent (gateway relay)",
        (None, false) => "(no purpose or harness recorded)",
    }
}

/// `Some(n)` as a digit, `None` as the phrase [`render_routing_cost`]'s own
/// doc comment names — never `0` for a count this build never read.
fn render_token_count(value: Option<i64>) -> String {
    match value {
        Some(count) => count.to_string(),
        None => "not counted".to_owned(),
    }
}

/// Capability map line 1331's gateway half, rendered — `render_token_count`'s
/// own rule applied to a timing column rather than a token count: a group
/// with no timed rows prints the words *not recorded*, never a digit and
/// never `0ms`, because "the mean was zero" and "nothing was timed" are
/// different facts and this build must never let them look the same.
///
/// `mean_ms` is [`None`] exactly when
/// [`glasshouse::routing::evidence::PurposeConsumption::first_byte_sample_count`]
/// is `0` — see that field's own doc comment — so there is nothing else this
/// function needs to check.
fn render_time_to_first_byte(mean_ms: Option<f64>) -> String {
    match mean_ms {
        Some(ms) => format!("{}ms (mean)", ms.round() as i64),
        None => "not recorded".to_owned(),
    }
}

/// The three spellings `glasshouse route --moment` accepts and the control
/// door's `recommend_route` answers in, written down exactly once.
///
/// These are **not** `RoutingMoment::as_str`, which is prose for a person
/// (`"session start"`, with a space) and is what a rendered report prints.
/// A wire vocabulary a caller sends and gets back has to round-trip, and a
/// table read in both directions is how the sending spelling and the
/// answering spelling are kept from drifting apart.
const ROUTING_MOMENTS: [(&str, glasshouse::routing::session::RoutingMoment); 3] = [
    (
        "session-start",
        glasshouse::routing::session::RoutingMoment::SessionStart,
    ),
    (
        "task-boundary",
        glasshouse::routing::session::RoutingMoment::TaskBoundary,
    ),
    (
        "mid-turn",
        glasshouse::routing::session::RoutingMoment::MidTurn,
    ),
];

/// [`ROUTING_MOMENTS`], read as a parser.
///
/// Answers an `Option` rather than an error because the two callers must
/// phrase the refusal differently: the command echoes back what the person
/// typed at their own terminal, and the door — where the string arrived over
/// a socket — names the three valid spellings without repeating the one it
/// was handed.
fn routing_moment_from_str(moment: &str) -> Option<glasshouse::routing::session::RoutingMoment> {
    ROUTING_MOMENTS
        .iter()
        .find(|(spelling, _)| *spelling == moment)
        .map(|(_, moment)| *moment)
}

/// [`ROUTING_MOMENTS`], read the other way: the spelling a caller may send
/// back, for the control door's answer.
///
/// Gated to match its only consumer, `api::unix`, for the reason
/// `api/mod.rs` states about `protocol`: an item reached only from a
/// platform-gated module is dead code everywhere else, and `-D warnings`
/// makes that a hard error rather than a warning.
#[cfg(unix)]
fn routing_moment_slug(moment: glasshouse::routing::session::RoutingMoment) -> &'static str {
    ROUTING_MOMENTS
        .iter()
        .find(|(_, candidate)| *candidate == moment)
        .map(|(spelling, _)| *spelling)
        // Unreachable while the table covers the enum, and an honest fallback
        // rather than a panic if a variant is ever added without it.
        .unwrap_or_else(|| moment.as_str())
}

/// What a routing question was answered with, before anything renders it —
/// the structured half of [`route_report`], and the whole of what the
/// control door's `recommend_route` reports (map line 1681).
///
/// This exists so there is exactly **one** ranking. `memory_search_grouped`
/// and `render_memory_report` already have this shape for the memory door:
/// one function computes, another renders, and the door reads the computed
/// form rather than parsing the rendered one. A door that asked the router
/// its own question would be a second implementation of the same policy, and
/// the two could disagree about where work should go without anything
/// failing.
enum RouteRecommendation {
    /// `SessionRouter::choose` answered. Boxed because the ranking carries
    /// every candidate it weighed and the other variant carries a word;
    /// `clippy::large_enum_variant` is right that the difference should not
    /// be paid by every value of this type.
    Ranked(Box<RankedRoute>),
    /// It did not, and [`NoRoute`] says which of its two situations applies.
    Nowhere(NoRoute),
}

/// A routing decision together with the two things [`routing_caveats`] needs
/// in order to say what the ranking could not see.
struct RankedRoute {
    routed: glasshouse::routing::session::Routed,
    /// Every candidate the router was offered, kept because a caveat is
    /// about the candidate set rather than about the winner.
    destinations: Vec<glasshouse::routing::session::Destination>,
    /// The `Destination::id` of every fresh candidate `glasshouse launch`
    /// would itself refuse — see [`launch_can_resolve_protocol`].
    refused_by_launch: Vec<String>,
    /// Every resource `observed_provider_health` could attribute a persisted
    /// reading to. Kept rather than recomputed because a caveat about what
    /// the ranking could not see has to be answered from the pool the ranking
    /// was actually given.
    health_observed: Vec<String>,
}

/// Why there is no recommendation.
///
/// `SessionRouter::choose` answers `None` in exactly two situations, and they
/// are different facts about this project rather than one error — which is
/// why this is an enum a caller can match on rather than a sentence it would
/// have to parse.
enum NoRoute {
    /// No session to continue, and no launch profile to start one under.
    NoDestination,
    /// The moment does not take routing (line 1592), and there is no session
    /// for the work to stay on either.
    MomentDoesNotRoute(glasshouse::routing::session::RoutingMoment),
}

/// `glasshouse route`'s decision, and the control door's — map lines 1601,
/// 1602 and 1681.
///
/// **Decides nothing and starts nothing.** It assembles exactly the inputs
/// `launch_session` assembles, asks the same `SessionRouter` the same
/// question, and hands back what it answered. That is what makes it a
/// diagnostic worth reading rather than a second implementation that could
/// drift: if this and a launch ever disagreed, one of them would be lying,
/// and there is one function each of them calls.
///
/// Nothing on this path writes. It opens the project's session store and
/// checkpoint store to *read* candidates (`routing_destinations`,
/// `latest_checkpoint_quality`), and records no session, no event, and no
/// routing observation — which is the whole of line 1681's *"without
/// executing it"*, and is asserted over the shipped binary in
/// `tests/routing_api.rs`.
///
/// Two differences from the launch path, both stated in the rendered output
/// rather than hidden:
///
/// 1. it ranks across **every enabled harness**, because a caller asking
///    where work should go has not yet chosen one, whereas a launch has;
/// 2. it includes sessions that are still **running** (`DestinationScope`),
///    because "switch to that terminal" is an answer a person can act on and
///    is not one a second process can carry out.
fn route_recommendation(
    runtime: &Runtime,
    effective: &EffectiveConfig<'_>,
    moment: glasshouse::routing::session::RoutingMoment,
    to: Option<&str>,
    fresh: bool,
    now: bool,
    task: Option<&str>,
) -> anyhow::Result<RouteRecommendation> {
    use glasshouse::integrations::{IntegrationId, IntegrationKind};
    use glasshouse::routing::session::{RouterInputs, RoutingMoment, SessionRouter};

    let mut destinations = Vec::new();
    // Which of them `glasshouse launch` would refuse, so the report can say so
    // about the one it recommends — see `launch_can_resolve_protocol`.
    let mut refused_by_launch: Vec<String> = Vec::new();
    for harness in IntegrationId::ALL
        .iter()
        .copied()
        .filter(|id| id.kind() == IntegrationKind::Harness)
        .filter(|id| effective.enabled(*id, false).value)
    {
        let everything =
            routing_destinations(runtime, effective, harness, DestinationScope::Everything)?;
        refused_by_launch.extend(
            everything
                .iter()
                .filter(|destination| destination.is_fresh())
                .filter(|destination| {
                    effective
                        .launch_profile(destination.launch_profile(), harness)
                        .is_ok_and(|profile| !launch_can_resolve_protocol(&profile.value))
                })
                .map(|destination| destination.id().to_owned()),
        );
        destinations.extend(everything);
    }

    let overrides = effective.pairing_overrides();
    // Line 1599's bridge, on the path that *reports*. The live pool still
    // belongs to a running gateway's session lock and this diagnostic has no
    // gateway — but `glasshouse launch` weighs what a previous gateway
    // persisted, so a report that skipped it would explain a different
    // ranking from the one the acting path produces, which is the one defect
    // a routing explanation cannot have. `routing_caveats` below says which
    // of the two happened rather than asserting the empty case.
    let health = observed_provider_health(runtime, &destinations);
    let requirements = task_requirements_from_text(task);
    let inputs = RouterInputs {
        overrides: &overrides,
        health: &health,
        now: std::time::Instant::now(),
        requirements,
    };

    let mut user_override = routing_override(to, fresh);
    if now {
        user_override = user_override.and_route_now();
    }

    // `current` is what the work is on, and the moment decides whether there
    // is one. `RoutingMoment::SessionStart`'s own doc is explicit — *"no
    // session exists yet for this work"* — so `None` there is the type's
    // answer, not a shortcut. At a task boundary and mid-turn the work is
    // somewhere, and the most recently active session is the honest reading
    // of where.
    //
    // Load-bearing for line 1597: `prompt_cache_state` is defined as a
    // comparison `CacheLocality::between(from, to)`, so a caller that never
    // supplies a `from` has an inert term and an explanation that cannot say
    // why. This is the `from`.
    let current = match moment {
        RoutingMoment::SessionStart => None,
        RoutingMoment::TaskBoundary | RoutingMoment::MidTurn => destinations
            .iter()
            .find(|destination| !destination.is_fresh())
            .cloned(),
    };

    let Some(routed) = SessionRouter::with_override(user_override).choose(
        moment,
        current.as_ref(),
        &destinations,
        &inputs,
    ) else {
        // `choose` answers `None` in exactly two situations, and they are
        // different facts about this project rather than one error.
        return Ok(RouteRecommendation::Nowhere(if moment.permits_routing() {
            NoRoute::NoDestination
        } else {
            NoRoute::MomentDoesNotRoute(moment)
        }));
    };

    Ok(RouteRecommendation::Ranked(Box::new(RankedRoute {
        routed,
        destinations,
        refused_by_launch,
        health_observed: health
            .observed()
            .into_iter()
            .map(|(resource, _)| resource.label())
            .collect(),
    })))
}

/// [`RouteRecommendation`] as `glasshouse route` prints it — the rendering
/// half, layered on top of the decision rather than computed beside it.
fn render_route_recommendation(recommendation: &RouteRecommendation) -> String {
    match recommendation {
        RouteRecommendation::Nowhere(NoRoute::NoDestination) => {
            "There is nowhere for this work to go: this project has no session to continue \
             and no launch profile to start one under. `glasshouse doctor` reports which \
             harnesses are installed.\n"
                .to_owned()
        }
        RouteRecommendation::Nowhere(NoRoute::MomentDoesNotRoute(moment)) => format!(
            "Nothing is routed at a {moment} moment (line 1592), and this project has no \
             session for the work to stay on either. Ask at a session start or a task \
             boundary, or pass --now to decide here anyway.\n"
        ),
        RouteRecommendation::Ranked(ranked) => {
            let mut out = ranked.routed.render_overview();
            out.push('\n');
            out.push_str(&routing_caveats(
                &ranked.routed,
                &ranked.destinations,
                &ranked.refused_by_launch,
                &ranked.health_observed,
            ));
            out
        }
    }
}

/// Classify `task`'s free-form description of the work into the
/// `TaskRequirements` `RouterInputs` carries — the wire that makes the
/// capability registry (`routing::capability`) reachable from a production
/// caller (map lines 1382–1391). Absent or blank text reproduces today's
/// `TaskRequirements::default()` behaviour byte for byte (ruling 1).
///
/// Classification happens here, once, at the entry point — never inside
/// `SessionRouter` itself (ruling 2), so the router stays something a
/// classification is handed to and tested against, rather than something
/// that computes its own input.
///
/// `needs_tool_calls` is derived from the same signal fields
/// `TaskClassification::hard_capabilities` already reads, rather than left
/// hardcoded `false` (ruling 3): a task this heuristic marks as needing
/// repository access, shell execution, or browser interaction needs the
/// harness to act through its tool-call protocol, not only answer in words.
fn task_requirements_from_text(
    task: Option<&str>,
) -> glasshouse::routing::session::TaskRequirements {
    use glasshouse::routing::classify::classify_heuristically;
    use glasshouse::routing::session::TaskRequirements;

    let Some(text) = task.map(str::trim).filter(|text| !text.is_empty()) else {
        return TaskRequirements::default();
    };
    let hard_capabilities = classify_heuristically(text).hard_capabilities();
    TaskRequirements {
        needs_tool_calls: !hard_capabilities.is_empty(),
        hard_capabilities,
    }
}

/// What the ranking above could not see, said out loud.
///
/// A routing explanation whose silent terms are invisible is worse than one
/// that is short: a reader cannot tell "provider health was equal" from
/// "provider health was never read". Every line here names an input that
/// contributed nothing and why.
fn routing_caveats(
    routed: &glasshouse::routing::session::Routed,
    destinations: &[glasshouse::routing::session::Destination],
    refused_by_launch: &[String],
    health_observed: &[String],
) -> String {
    use glasshouse::routing::session::Continuation;
    use std::fmt::Write as _;

    let mut out = String::from("what this ranking could not see\n");
    if health_observed.is_empty() {
        let _ = writeln!(
            out,
            "  provider health   nothing observed — no gateway has yet persisted a health \
             reading for any of these credentials, so the term is 0.0 for every destination"
        );
    }
    if destinations
        .iter()
        .all(|destination| destination.is_fresh())
    {
        let _ = writeln!(
            out,
            "  session affinity  this project has recorded no session that is still warm, so \
             every candidate is a fresh start"
        );
    }
    if destinations
        .iter()
        .all(|destination| destination.capacity().is_none())
    {
        let _ = writeln!(
            out,
            "  quota pressure    no quota reading has been cached for any of these providers; \
             `glasshouse resources --probe <provider>` takes one"
        );
    }
    if refused_by_launch.contains(&routed.chosen().id().to_owned()) {
        let _ = writeln!(
            out,
            "  not launchable    profile `{}` declares a protocol its harness does not speak, \
             so `glasshouse launch` refuses it — this ranking answers whether the provider \
             could serve that harness at all, which is a different question",
            routed.chosen().launch_profile()
        );
    }
    if matches!(
        routed.chosen().continuation(),
        Continuation::Existing(warm)
            if warm.state == glasshouse::config::pairing::WarmSessionState::Live
    ) {
        let _ = writeln!(
            out,
            "  still running     `{}` is live, so it is the best place for this work and not \
             one `glasshouse run` can enter — switch to that terminal, or pass `--fresh`",
            routed.chosen().id()
        );
    }
    out
}

fn launch_session(
    runtime: &Runtime,
    harness: Option<&str>,
    destination: LaunchDestination<'_>,
    response: &ResponseRequest,
    headless: bool,
    harness_args: &[String],
) -> anyhow::Result<ExitCode> {
    let LaunchDestination {
        profile: profile_name,
        from_checkpoint,
        to,
        fresh,
    } = destination;
    let user = UserConfig::load(runtime.paths())?;
    let project = config::load_project_config(runtime.project())?;
    let effective = EffectiveConfig::new(&user, project.as_ref());
    let selection = session::select::select(harness, effective)?;
    // -----------------------------------------------------------------------
    // Phase 37 lines 1592, 1593 and 1595–1600: **where** this work goes is
    // decided here, at a session boundary, before a launch profile is
    // resolved — because the destination is what chooses the profile and not
    // the other way round.
    //
    // This is the production call the router was built for. Everything below
    // it already worked; what it did not do was ask whether this project
    // already had a session worth continuing, which is line 1593 in one
    // sentence. Deleting the `choose` call below must break
    // `tests/route_command.rs`, and that is the point (practice §35): the
    // router's own eleven mutations prove its scoring and none of them can
    // prove that anything calls it.
    // -----------------------------------------------------------------------
    // Which profile a *new* session would run under, from the same three
    // sources it has always come from — with `--to fresh:<harness>:<profile>`
    // added as a fourth, because an identifier a person pasted out of
    // `glasshouse route` has to mean the same thing here as it did there.
    let named_profile = to
        .and_then(|id| fresh_destination_profile(id, selection.id()))
        .or(profile_name);
    // A profile the user disabled is not a profile Glasshouse may start,
    // and being asked for it by name is the one case where saying nothing
    // would be worst: the routing filter above simply stops offering it, so
    // without this a `--profile` naming it would launch it anyway and
    // `enabled` would mean nothing on the path that actually starts a
    // session.
    //
    // Refused *here*, before `routing_destinations` and before any
    // pre-flight check, so a refusal costs nothing — no probe, no session
    // record, no process — matching the harness-not-installed refusal below
    // it in `session::select`.
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
    let fresh_profile = named_profile.unwrap_or(glasshouse::profile::NATIVE_PROFILE_NAME);
    let destinations = routing_destinations(
        runtime,
        &effective,
        selection.id(),
        DestinationScope::Launchable {
            profile: fresh_profile,
        },
    )?;
    let overrides = effective.pairing_overrides();
    // **Map line 1599's bridge, on the path that acts.** The live pool a
    // gateway fills still does not exist here — that gateway is started
    // further down, and only for a profile that needs one — but what a
    // gateway *exports* does: `provider::telemetry::GatewayHealthReading`s,
    // persisted to `GatewayHealthCache` under this run's own data directory,
    // by whichever earlier `glasshouse run` or `glasshouse launch` served the
    // work. `observed_provider_health` reads them into the pool
    // `provider_health` looks in, and its own doc has the two hazards that
    // make it a design rather than a wiring — the rendered `credential_label`
    // against a `CredentialId`, and unix seconds against an epoch-less
    // `Instant`. Neither is guessed at; a reading that cannot be attributed
    // without guessing is not attributed, which leaves exactly the inert
    // `0.0` this line had before the bridge.
    //
    // The reading comes from a *previous* process. That is the whole point:
    // the health of a provider is not a fact this launch can observe about a
    // session it has not started yet.
    let health = observed_provider_health(runtime, &destinations);
    let inputs = glasshouse::routing::session::RouterInputs {
        overrides: &overrides,
        health: &health,
        now: std::time::Instant::now(),
        requirements: glasshouse::routing::session::TaskRequirements::default(),
    };
    // Line 1602 on the path that acts, not only on the one that reports.
    //
    // Two of these are the user's flags. The other two are statements they
    // already made by typing something else, and reading them as anything but
    // "this launch is a fresh one" would be a router overruling a person:
    // `--profile` names the profile a *new* session should run under, and
    // `--from-checkpoint` hands a new session its opening prompt. Neither is
    // a thing to do to a session that is already going.
    let user_override = if to.is_some() || fresh {
        routing_override(to, fresh)
    } else if from_checkpoint.is_some() {
        glasshouse::routing::session::RoutingOverride::fresh()
    } else if let Some(name) = profile_name {
        glasshouse::routing::session::RoutingOverride::to(fresh_destination_id(
            selection.id(),
            name,
        ))
    } else {
        glasshouse::routing::session::RoutingOverride::none()
    };
    let routed = glasshouse::routing::session::SessionRouter::with_override(user_override).choose(
        glasshouse::routing::session::RoutingMoment::SessionStart,
        None,
        &destinations,
        &inputs,
    );

    // A destination the router chose is announced before anything happens,
    // never after: a person who did not want their previous session continued
    // needs to read that on the way in, while `--fresh` is still an answer.
    if let Some(routed) = &routed {
        // Map lines 1829 and 1830: this is the one moment both facts are
        // known, and the one the two `eprintln!`s below already render for a
        // person without either being counted anywhere. `glasshouse route`
        // (main.rs:1462) reaches the same router but never this branch, so
        // it never reaches this call either — it reports without acting.
        glasshouse::evaluation::record_routing_decision(
            runtime,
            routed.chosen().id(),
            routed.chosen().is_fresh(),
            routed.overrode(),
            glasshouse::evaluation::now_unix(),
        );
        if let Some(refusal) = routed.override_refused() {
            eprintln!("glasshouse: {refusal}");
        }
        if let glasshouse::routing::session::Continuation::Existing(warm) =
            routed.chosen().continuation()
        {
            eprintln!(
                "glasshouse: continuing session {} ({}, idle {}) rather than starting a new one; \
                 pass --fresh to start one anyway.",
                routed.chosen().id(),
                warm.state,
                format_age(glasshouse::provider::cache::now_unix_seconds() - warm.idle_seconds)
            );
            return resume_session(
                runtime,
                routed.chosen().id(),
                harness_args,
                headless,
                RouteOnResume::AlreadyRouted,
            );
        }
    }

    // The chosen fresh destination names the profile this launch resolves.
    // `routed` is `None` only when there was nowhere at all for the work to
    // go, which for a fresh launch means no profile resolved for this
    // harness; the implied Native profile always does, so the fallback below
    // is unreachable in practice and is written as the same answer this
    // function gave before the router existed rather than as a panic.
    let requested_profile = routed
        .as_ref()
        .map(|routed| routed.chosen().launch_profile().to_owned())
        .unwrap_or_else(|| {
            profile_name
                .unwrap_or(glasshouse::profile::NATIVE_PROFILE_NAME)
                .to_owned()
        });

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
    //
    // Map line 1735: the relay is built here, before the gateway, because the
    // sink has to exist before the thing it writes into does — see
    // `DegradeRelay`. It is installed below, once the session record and the
    // event recorder are both real.
    let degrade_relay = DegradeRelay::new();
    let gateway = match glasshouse::gateway::start_if_required_with_degrade_sink(
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
        // Capability map lines 1311/1321/1322/1324: the durable resource-
        // health cache, the same additive shape as the quota cache above and
        // read back by exactly the same `glasshouse resources` invocation.
        Some(glasshouse::provider::telemetry::GatewayHealthCache::new(
            runtime.paths(),
        )),
        Some(degrade_relay.sink()),
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
            )))
            // Phase 40 line 1646: the session this one was bootstrapped from,
            // if this launch is a `--from-checkpoint` handoff. `None` for
            // every other launch — a session not started from a checkpoint
            // must never record an invented source.
            .with_source_session(bootstrap.as_ref().map(|(_, source)| source.clone())),
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
        Some((prompt, _)) => launch.args(std::iter::once(prompt.as_str())),
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
    let events = Arc::new(EventRecorder::open(runtime));
    events.record(&record.id, LifecycleEvent::SessionStarted);

    // Map line 1735, the other half of `DegradeRelay`: from here on a failed
    // gateway upstream is recorded against this session, by the gateway's own
    // thread, while the harness below keeps running. The record is the one
    // this process owns and its `backend_resource` was written above, so
    // `degrade_resource` can already tell whether this session was on the
    // resource that failed.
    degrade_relay.install(Arc::clone(&events), vec![record.clone()]);

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
    // Map line 1735. Built by `resume_session`, which is where the recorder
    // this eventually writes into is opened; this function only starts the
    // gateway, so it is a parameter rather than something resolved here.
    degrade_sink: glasshouse::gateway::DegradeSink,
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
    let gateway = glasshouse::gateway::start_if_required_with_degrade_sink(
        std::slice::from_ref(&launch_profile),
        || gateway_upstream(user, project, effective, &secrets),
        Some(glasshouse::provider::telemetry::GatewayQuotaCache::new(
            runtime.paths(),
        )),
        evidence_ledger(runtime, std::slice::from_ref(&launch_profile)),
        Some(glasshouse::provider::telemetry::GatewayHealthCache::new(
            runtime.paths(),
        )),
        Some(degrade_sink),
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
    report_hook_with(runtime, session, event, |id| {
        disposable_extraction_model(runtime, id)
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
/// [`run_extraction`]'s own doc comment).
fn memory_extraction_enabled(runtime: &Runtime) -> bool {
    let Ok(user) = UserConfig::load(runtime.paths()) else {
        return true;
    };
    let project = config::load_project_config(runtime.project()).unwrap_or(None);
    EffectiveConfig::new(&user, project.as_ref())
        .memory_extraction_enabled()
        .value
}

/// Phase 19: whether Glasshouse may take a checkpoint automatically at a
/// task boundary — see
/// [`glasshouse::config::EffectiveConfig::automatic_checkpoint_enabled`].
///
/// A configuration Glasshouse cannot read defaults to enabled, matching
/// [`memory_extraction_enabled`]'s own fallback and for the same reason: a
/// broken config file must not silently and permanently turn off a working
/// capability, and this trigger already tolerates every other failure
/// non-fatally (see [`checkpoint_after_turn`]'s own doc comment).
fn automatic_checkpoint_enabled(runtime: &Runtime) -> bool {
    let Ok(user) = UserConfig::load(runtime.paths()) else {
        return true;
    };
    let project = config::load_project_config(runtime.project()).unwrap_or(None);
    EffectiveConfig::new(&user, project.as_ref())
        .automatic_checkpoint_enabled()
        .value
}

/// Take an automatic checkpoint for `id` at a task boundary, after a
/// completed turn.
///
/// # Nothing here can hurt the session
///
/// Matching [`run_extraction`]'s own policy for its neighbour: a
/// checkpoint that cannot be taken is logged and this returns. It never
/// propagates an error to [`report_hook_with`] and never blocks past a
/// synchronous read of a couple of small files and one write — there is no
/// model call here, so there is nothing to bound with a thread and a
/// timeout the way extraction needs.
///
/// # What it carries forward
///
/// A checkpoint's objective, state and next actions are authored —
/// Glasshouse does not know them and will not guess them from a session's
/// terminal output, for the same reason nothing else in this codebase reads
/// state out of scrollback. So this carries forward the handoff from the
/// session's most recent checkpoint, restamped with the current time and the
/// repository's current position — the same shape
/// `shell::checkpoint_task_boundaries` already uses in the interactive shell,
/// for the same reason. A session that has never had a checkpoint taken gets
/// nothing here, silently: there is no handoff to carry forward and nothing
/// honest to invent.
fn checkpoint_after_turn(runtime: &Runtime, id: &SessionId, harness: &str) {
    let outcome = (|| -> anyhow::Result<()> {
        let checkpoints = ProjectCheckpoints::open(runtime)?;
        let store = checkpoints.store();
        let Some(previous) = store.latest_for(id)? else {
            return Ok(());
        };
        let refreshed = Checkpoint::capture(
            id,
            harness,
            CheckpointReason::TaskBoundary,
            store.now(),
            runtime.project().root(),
            previous.checkpoint.handoff.clone(),
        );
        store.save(refreshed)?;
        Ok(())
    })();

    if let Err(err) = outcome {
        tracing::warn!(
            session = %id,
            error = %format!("{err:#}"),
            "could not take an automatic checkpoint"
        );
    }
}

/// Map line 1171: refresh a session's portable checkpoint just before the
/// harness compacts its own context, so the handoff a fresh window would
/// bootstrap from reflects where the repository actually stands rather than
/// wherever it stood at the last completed turn.
///
/// # Refresh, not a new kind of checkpoint
///
/// This mirrors [`checkpoint_after_turn`] in every respect but one: it
/// preserves `previous.checkpoint.reason` instead of stamping
/// [`CheckpointReason::TaskBoundary`]. A compaction is not a turn ending, so
/// stamping `TaskBoundary` would misdescribe why the checkpoint exists — and
/// `CheckpointReason` has exactly two variants, both pinned by a SQL `CHECK`,
/// so there is no third value honest enough to invent instead. What moves is
/// `created_at` and the Git position; the reason a person or agent already
/// gave the checkpoint does not change because the harness is about to
/// compact.
///
/// # `store.latest_for(id)?` returning `None` is the whole of "when practical"
///
/// A session that has never had a checkpoint taken gets nothing here,
/// silently — there is no previous handoff to carry forward and nothing
/// honest to invent, exactly as [`checkpoint_after_turn`] already declines.
///
/// # Nothing here can hurt the session
///
/// Same stance as its neighbour: a checkpoint that cannot be refreshed is
/// logged and this returns, never propagating an error back to the hook that
/// is running inside somebody's coding session.
fn checkpoint_before_compaction(runtime: &Runtime, id: &SessionId, harness: &str) {
    let outcome = (|| -> anyhow::Result<()> {
        let checkpoints = ProjectCheckpoints::open(runtime)?;
        let store = checkpoints.store();
        let Some(previous) = store.latest_for(id)? else {
            return Ok(());
        };
        let refreshed = Checkpoint::capture(
            id,
            harness,
            previous.checkpoint.reason,
            store.now(),
            runtime.project().root(),
            previous.checkpoint.handoff.clone(),
        );
        store.save(refreshed)?;
        Ok(())
    })();

    if let Err(err) = outcome {
        tracing::warn!(
            session = %id,
            error = %format!("{err:#}"),
            "could not refresh the checkpoint before compaction"
        );
    }
}

/// Phase 21 line 834's production caller: the cheap or local model the user
/// actually chose, ready to be asked.
///
/// # `None` is the whole of the consent, and it is the default
///
/// This returns `Some` only when
/// [`glasshouse::config::EffectiveConfig::memory_extraction_model`] names a
/// provider and model — a field that is `None` until a person writes it. A
/// user who has configured providers, free models, routing preferences and
/// nothing else gets `None` here and therefore exactly today's behaviour:
/// [`disposable_extraction_model`] falls through to
/// `glasshouse::memory::RoutedNoModel`, which chooses a resource, says so,
/// and calls nothing.
///
/// That is deliberately stricter than "the user has configured a free
/// model". A free-model list is a statement about cost; it is not a request
/// that a hook running **inside a coding session** start making outbound
/// requests. Line 834 says *configurable*, and this is the configuration.
///
/// # Every failure below is `None`, logged once
///
/// An unreadable configuration, a provider that is not in the table, a
/// template that does not resolve, a protocol this build does not speak, a
/// credential that is named and unset — each is a choice that cannot produce
/// a call, and each returns `None` after one log line. Never a guess at a
/// correction, and never a silent one: the resulting outcome still says in
/// words that no model was called, which is what stops an evaluation reading
/// later as evidence that one did.
fn configured_extraction_model(
    runtime: &Runtime,
) -> Option<Box<dyn glasshouse::memory::ExtractionModel>> {
    use glasshouse::memory::ConfiguredModel;
    use glasshouse::secret::{SecretRef, SecretStore as _};

    let user = UserConfig::load(runtime.paths())
        .inspect_err(
            |err| tracing::debug!(error = %err, "could not read configuration for the extraction model"),
        )
        .ok()?;
    let project = config::load_project_config(runtime.project())
        .inspect_err(
            |err| tracing::debug!(error = %err, "could not read project configuration for the extraction model"),
        )
        .ok()
        .flatten();
    let effective = EffectiveConfig::new(&user, project.as_ref());
    let chosen = effective.memory_extraction_model().value?;

    // The provider's whole configuration comes from whichever layer actually
    // holds its name, project winning over user — the same rule
    // `disposable_candidates` applies, and for the same reason.
    let Some(provider_config) = project
        .as_ref()
        .and_then(|p| p.providers().get(chosen.provider()))
        .or_else(|| user.providers().get(chosen.provider()))
    else {
        tracing::warn!(
            provider = chosen.provider(),
            "the configured extraction model names a provider this project has not configured"
        );
        return None;
    };
    if !provider_config.enabled() {
        tracing::warn!(
            provider = chosen.provider(),
            "the configured extraction model names a disabled provider"
        );
        return None;
    }
    let provider = match provider_config.to_provider(chosen.provider()) {
        Ok(provider) => provider,
        Err(err) => {
            tracing::warn!(error = %err, "the configured extraction model's provider does not resolve");
            return None;
        }
    };

    // A provider that names no credential variable is the local case — a
    // runner on loopback needs none, and `ConfiguredModel::new` builds it
    // without one. A provider that names several and has one set resolves to
    // the first that does, the same order `disposable_candidates` walks.
    let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();
    let credential = provider
        .credential_env
        .iter()
        .find_map(|var| secrets.resolve(&SecretRef::Environment { var: var.clone() }));

    match ConfiguredModel::new(&provider, chosen.model(), credential) {
        Ok(model) => Some(Box::new(model)),
        Err(err) => {
            tracing::warn!(error = %err, "the configured extraction model cannot be used");
            None
        }
    }
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
fn disposable_extraction_model(
    runtime: &Runtime,
    session: &glasshouse::session::SessionId,
) -> Box<dyn glasshouse::memory::ExtractionModel> {
    if let Some(chosen) = configured_extraction_model(runtime) {
        return chosen;
    }
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
    let routing = glasshouse::routing::disposable::DisposableRouting::for_support_work(
        effective.prefer_free_routing().value,
        free_preferences,
    )
    .with_reserve_override(reserve_override);
    let job = glasshouse::routing::disposable::JobKind::MemoryExtraction;
    let routed = glasshouse::memory::RoutedNoModel::new(job, &candidates, &routing);

    // The decision is made above and, until this line existed, died in a
    // `tracing::info!` a few frames later. `describe()` is the string
    // production already renders — the chosen model, its provider, its cost,
    // the `UseReason`, and every named contribution behind it, or the reason
    // no resource could serve — so what reaches the ledger is the rationale
    // that was *used*, not a second decision made for the ledger's benefit.
    // Asking `routing.choose` again here would produce a different `Instant`
    // and could produce a different answer.
    //
    // # Which thread, and why it is safe (practice §65)
    //
    // This one, the hook process's main thread, and *before* anything is
    // spawned. `report_hook_with` evaluates `model(&id)` — this function — as
    // an argument to `run_extraction`, so Rust has finished here before
    // `run_extraction` opens the event log, opens `ProjectMemory`, or starts
    // the extraction thread that owns that memory handle for as long as the
    // bound allows. The ledger handle is therefore opened, used and dropped
    // while this process holds exactly one other connection to the project
    // database — the `ProjectSessions` handle `report_hook_with` is sitting
    // on, which is idle and holds no lock — and that is the same shape
    // `EventRecorder::open(runtime).record_observed(..)` on this very path
    // already has. Nothing here outlives the turn, and no handle is kept.
    //
    // Only on this branch, deliberately. The early return above is a model
    // the user configured by name, where no disposable routing decision is
    // made at all; recording a rationale for it would be recording something
    // that did not happen.
    glasshouse::evaluation::record_disposable_route(
        runtime,
        job,
        session.as_str(),
        &glasshouse::memory::ExtractionModel::describe(&routed),
        glasshouse::evaluation::now_unix(),
    );

    Box::new(routed)
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
/// # Where the metered half comes from, and why it is not a permission gate
///
/// `docs/product/design-decisions.md`'s *"Metered capacity for background
/// jobs"* records that ordinary support work may spend metered quota as a
/// last resort. [`ProviderConfig::metered_models`] **is** that decision
/// applied per provider, not a switch that sits above it: an empty list is
/// the coherent off state (this function then builds only free candidates
/// for that provider, exactly as before this batch), and naming a model
/// there is the user's decision already made — nothing here asks again.
/// Whether a candidate this loop builds is actually *usable* is
/// [`glasshouse::routing::disposable::DisposableRouting::choose`]'s job:
/// free capacity still wins whenever any can serve (line 533), and Phase
/// 32F's protected-reserve policy still gates every metered one.
///
/// A model named in both lists resolves through
/// [`ProviderConfig::cost_of`] — `Free` wins, and it is added once, not
/// twice.
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
                candidates.push(
                    DisposableCandidate::new(
                        name.clone(),
                        model.clone(),
                        credential_id.clone(),
                        provider_config.cost_of(model),
                    )
                    .with_capacity(capacity.clone()),
                );
            }
        }
    }
    candidates
}

/// What `routing_observations.purpose` records for a call `glasshouse
/// classify` made.
///
/// Spelled once. `purpose` is a `TEXT` column with no `CHECK`
/// (`database.rs`'s migration 11), so the only thing keeping two producers
/// from writing two spellings of one word is that each has exactly one.
const CLASSIFICATION_PURPOSE: &str = "classification";

/// What happened when `glasshouse classify` tried to have a model classify a
/// request.
///
/// Three outcomes rather than an `Option`, because "the user configured no
/// routing model" and "the routing model they configured could not answer"
/// are different facts that a caller must say differently: the first is
/// Phase 35's ordinary state and deserves no message at all, and the second
/// is a degrade the user is entitled to be told about. Collapsing them would
/// make a broken configuration look like an absent one.
enum ClassificationAttempt {
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
        ModelError::Unavailable => "the routing model could not be reached",
        ModelError::Refused => "the routing model declined the request",
        ModelError::TimedOut => "the routing model did not answer within its bound",
        ModelError::Failed { .. } => "the routing model's call produced no usable answer",
    }
    .to_owned()
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
/// and build the model it named.
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
///
/// Where the inputs come from — including the health pool, which this path
/// no longer leaves empty — is [`automatic_classification_choice`]'s header.
/// This function is the half that turns the choice into a model.
fn automatic_classification_model(
    runtime: &Runtime,
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    effective: &EffectiveConfig<'_>,
    request_text: &str,
) -> Result<glasshouse::memory::ConfiguredModel, String> {
    // The tier this job's own demand implies, from the request itself. This
    // is `RoutedNoModel::new_for_request`'s fifth link, made by the one
    // `JobKind` its doc comment says the constructor was waiting for — a
    // request, not a transcript of a finished turn.
    let requirement = glasshouse::routing::classify::classify_heuristically(request_text);
    let choice =
        automatic_classification_choice(runtime, user, project, effective, Some(&requirement))
            .map_err(|reason| {
                format!("no resource is available to classify this request: {reason}")
            })?;

    classification_model(
        user,
        project,
        choice.provider(),
        choice.model(),
        Some(choice.credential().reference()),
    )
}

/// Which configured resource automatic routing-model selection picks right
/// now — the decision itself, separated from building the model so that a
/// diagnostic can name the same pick without asking anything to classify.
///
/// # Why the diagnostic must share this function rather than repeat it
///
/// `glasshouse resources` reports the model this would choose (map line
/// 1443). A report that rebuilt the candidate list and the policy beside this
/// one would be a second implementation of the decision, free to drift from
/// the one that actually runs — and a diagnostic that names a different model
/// than the classifier uses is worse than none. So there is one function, and
/// the report and the classifier differ only in what they do with its answer.
///
/// `classification` is `None` for a caller with no request in hand, which is
/// exactly what [`DisposableRouting::choose`] documents that value as meaning
/// — the fixed [`WorkloadTier::Leaf`] the policy used before a classification
/// existed to ask. The report says so rather than implying a request was
/// classified.
///
/// # The health pool is read, not empty
///
/// The gateway writes what it learned from real request outcomes to
/// [`glasshouse::provider::telemetry::GatewayHealthCache`], and
/// [`observed_health_of`] reads that back for exactly the candidates this
/// call is about. Passing `FreePool::new()` here — which this path did until
/// this batch — meant every candidate was treated as available, so
/// `choose`'s health and allowance filter could never exclude anything on the
/// production path (map line 1433, practice §36).
///
/// **No `ReserveOverride`.** That input is scoped to sessions the user named
/// by hand with `glasshouse sessions reserve`, and this decision is made for
/// no session at all — there is no identity here for the override to apply
/// to, and inventing one would grant a reserve exemption nobody asked for.
fn automatic_classification_choice(
    runtime: &Runtime,
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    effective: &EffectiveConfig<'_>,
    classification: Option<&glasshouse::routing::classify::TaskClassification>,
) -> Result<
    glasshouse::routing::disposable::DisposableChoice,
    glasshouse::routing::disposable::NoResource,
> {
    use glasshouse::routing::disposable::{DisposableRouting, JobKind};

    let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let telemetry = glasshouse::provider::resources::GatheredTelemetry::new().gather_gateway_quota(
        &glasshouse::provider::telemetry::GatewayQuotaCache::new(runtime.paths()),
    );
    let candidates =
        disposable_candidates(user, project, effective, &secrets, &telemetry, now_unix);
    let health = observed_health_of(
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
    let routing = DisposableRouting::for_support_work(
        effective.prefer_free_routing().value,
        free_preferences,
    );

    routing.choose(
        JobKind::Classification,
        &candidates,
        &health,
        std::time::Instant::now(),
        classification,
    )
}

/// Ask the configured routing model to classify `request_text`.
///
/// This is the caller `routing::classify::classify`'s `Some(..)` arm was
/// written for and never had: the module is downstream of the decision about
/// *which* model classifies, and this is where that decision is made and
/// carried out.
///
/// # The three resolutions, and which one changes nothing
///
/// `RoutingModelResolution::Heuristics` returns before anything is read,
/// built, opened or sent. A build with no routing model configured — which is
/// every build until somebody configures one — asks nothing, opens no
/// database, and prints exactly what it printed before this function existed.
/// `tests/classification_call.rs` holds that byte-for-byte against the
/// heuristic's own output.
fn classify_with_routing_model(runtime: &Runtime, request_text: &str) -> ClassificationAttempt {
    use glasshouse::config::RoutingModelResolution;
    use glasshouse::memory::ExtractionModel as _;

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

    let built = match effective.routing_model_resolution().value {
        RoutingModelResolution::Heuristics(_) => return ClassificationAttempt::NotConfigured,
        RoutingModelResolution::Pinned { provider, model } => {
            classification_model(&user, project.as_ref(), &provider, &model, None)
        }
        RoutingModelResolution::Automatic => automatic_classification_model(
            runtime,
            &user,
            project.as_ref(),
            &effective,
            request_text,
        ),
    };
    let model = match built {
        Ok(model) => model,
        Err(why) => return ClassificationAttempt::Failed(why),
    };

    // `describe()` names the provider, the model and the route, and neither
    // the base URL nor the credential — see `memory::extract::model`'s header
    // for why the base URL is excluded even though it looks harmless. This is
    // the label the classification is attributed to, and it comes from the
    // model this process built, never from anything the reply said.
    let label = model.describe();
    let prompt = glasshouse::memory::extract::Prompt::for_request(
        glasshouse::routing::classify::CLASSIFICATION_PROMPT_CONTRACT,
        glasshouse::routing::classify::CLASSIFICATION_RESPONSE_SCHEMA,
        request_text,
    );

    let reply = match model.complete_observed(&prompt) {
        Ok(reply) => reply,
        Err(err) => return ClassificationAttempt::Failed(routing_model_failure(&err)),
    };
    // Recorded before the reply is read, and whether or not it parses: this
    // row is what the call *cost*, and a call that came back in the wrong
    // shape cost exactly as much as one that came back in the right one.
    if let Some(call) = &reply.call {
        record_classification_observation(runtime, call);
    }

    match glasshouse::routing::classify::parse_classification(&reply.reply, label) {
        Ok(classification) => ClassificationAttempt::Answered(classification),
        Err(err) => ClassificationAttempt::Failed(err.to_string()),
    }
}

/// Append what one classification call cost to the routing evidence ledger,
/// under `purpose = "classification"`.
///
/// # Why the handle is opened here and dropped here (practice §65)
///
/// `EvidenceLedger` holds an open SQLite connection for its whole lifetime,
/// and a handle opened for work that never happens blocks a later writer on
/// Windows while being invisible on Unix. So it is opened at the one point
/// its consumer exists — after a provider has actually answered and there is
/// a `ModelCall` to record — and not on the path where `glasshouse classify`
/// asks no model at all, which is every run until somebody configures one.
///
/// No error channel, for the same reason [`record_extraction_observation`]
/// has none: a classification a person asked for is not made worse by the
/// bookkeeping failing, and Glasshouse's books are never more important than
/// the answer they are about.
fn record_classification_observation(
    runtime: &Runtime,
    call: &glasshouse::memory::extract::ModelCall,
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
        .with_purpose(Some(CLASSIFICATION_PURPOSE));
    if let Err(err) = ledger.record(observation, glasshouse::provider::cache::now_unix_seconds()) {
        tracing::warn!(error = %err, "could not record what classification cost");
    }
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

/// How long a hook process will wait for the harness to finish writing the
/// payload it is about to throw away.
///
/// # Why draining the payload needs a bound at all
///
/// [`report_hook_with`] drains its standard input so that a harness writing a
/// payload is not left writing into a closed pipe. Copying *to end of input*
/// is an **unbounded** wait, and the harness is the thing that decides when
/// that end arrives. A harness that writes nothing and never closes the pipe
/// parks this process there for as long as it lives — inside the user's
/// session, on the event Claude Code treats as a gate on the turn. That is
/// exactly what [`report_hook`]'s own doc comment says may never happen here.
///
/// Not hypothetical, and not Windows-specific either, though Windows is where
/// it was found: reached over an `ssh` channel whose far end never sees end of
/// input — which is how the local gate's Windows leg runs the suite, and which
/// its macOS leg avoids only because that one redirects from `/dev/null` — the
/// six tests that call this function block for ever, and every other test in
/// the target passes. Measured on both batch 50 and its own base commit, so
/// the wait is older than the batch that surfaced it.
///
/// # Why one second
///
/// Shorter than [`EXTRACTION_BOUND`] because there is far less on the other
/// side of it. The harness writes the payload as it starts this process, so a
/// live harness is finished before the first database is even open and the
/// normal cost of this wait is nothing at all. Any wait that reaches the bound
/// is already the pathological case, and the answer to it is to get on with
/// the bookkeeping rather than to keep waiting.
const PAYLOAD_DRAIN_BOUND: std::time::Duration = std::time::Duration::from_secs(1);

/// Run `work` on its own thread and stop waiting for it after `bound`,
/// reporting whether it finished in time.
///
/// # The abandoned thread is deliberate
///
/// Nothing here can stop a thread parked in a blocking read, and stopping it
/// is not the point: the point is that *this* thread may go on without it. The
/// work is left running, the process finishes what it was doing and exits, and
/// the operating system closes whatever handle the thread was waiting on.
///
/// [`run_extraction`] does the same thing by hand rather than
/// through this, because it needs the extraction's *outcome* back and not
/// merely the fact that it arrived.
fn abandon_after(bound: std::time::Duration, work: impl FnOnce() + Send + 'static) -> bool {
    let (finished, waiter) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        work();
        // A closed receiver means the bound expired and nobody is listening.
        // That is a normal outcome here, not an error.
        let _ = finished.send(());
    });
    waiter.recv_timeout(bound).is_ok()
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
///
/// It takes the session's *resolved* identifier because the routing decision
/// behind the model depends on it: capability map line 1290 lets the user
/// override protected-reserve protection for one named session, and
/// [`disposable_extraction_model`] can only honour that for the session it is
/// deciding for. `session` above is whatever the harness put on the command
/// line; the resolved id is what the user's configuration records.
fn report_hook_with(
    runtime: &Runtime,
    session: &str,
    event: &str,
    model: impl Fn(&glasshouse::session::SessionId) -> Box<dyn glasshouse::memory::ExtractionModel>,
) {
    // Codex writes its payload to the hook's stdin, and a process that never
    // reads it can leave the harness writing into a closed pipe. Glasshouse
    // has the event name and the session identifier from its own argv, so
    // the payload is drained to EOF and thrown away, unread and unparsed —
    // never deserialized, logged, or stored. See
    // `the_hook_command_never_reads_its_payload` below, and the
    // `docs/product/design-decisions.md` section this function implements.
    //
    // On its own thread, and abandoned at `PAYLOAD_DRAIN_BOUND`, because the
    // end of that input is the harness's decision and this process may not
    // wait on it for ever. See the constant for what the unbounded version
    // did.
    let drained = abandon_after(PAYLOAD_DRAIN_BOUND, || {
        let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
    });
    if !drained {
        tracing::debug!(
            bound_ms = PAYLOAD_DRAIN_BOUND.as_millis(),
            "the harness had not closed this hook's input; going on without it"
        );
    }

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
            // Phase 21: *allow memory extraction to run before or around
            // native prompt compaction.*
            //
            // A compaction is not a `SessionLifecycle` state and has no
            // `LifecycleEvent`, so it lands here — in the arm for events that
            // translate to nothing — rather than beside the completed-turn
            // trigger below. No `lifecycle_events` row is written for it and
            // none can be: its `kind` is a SQL `CHECK` and `database`'s house
            // rule refuses to widen one. See
            // `session::lifecycle::precedes_native_compaction`.
            //
            // Gated by the same `memory_extraction` switch as the post-turn
            // trigger, and deliberately so: a user who turned automatic
            // extraction off turned it off, not "off except when the harness
            // compacts".
            if session::lifecycle::precedes_native_compaction(event) {
                // Capability map line 1159 — *"track the number of observed
                // compactions for a session when known"* — and this is the
                // only place in the shipped binary that knows one is coming.
                //
                // **Outside the `memory_extraction` gate, deliberately.**
                // That switch decides whether Glasshouse *does* something
                // about a compaction; the compaction happened either way, and
                // a count that silently stopped when a user turned extraction
                // off would be a number no reader could trust. It is also
                // ordered first, so a count is recorded even if extraction
                // takes the full `EXTRACTION_BOUND` and this process is torn
                // down by the harness while waiting.
                //
                // Best-effort: a compaction is the harness's business and a
                // hook that failed to write a counter must not fail the turn
                // over it, which is the same stance every other write on this
                // path takes.
                if let Err(err) = store.record_observed_compaction(&id) {
                    tracing::debug!(
                        error = %err,
                        session = %id,
                        "could not count an observed compaction"
                    );
                }
                if memory_extraction_enabled(runtime) {
                    run_extraction(
                        runtime,
                        &id,
                        model(&id),
                        glasshouse::memory::ExtractionTrigger::BeforeCompaction,
                    );
                }
                // Map line 1171 — *"prefer creating or refreshing a portable
                // checkpoint before intentional compaction when practical"*.
                // Gated by `automatic_checkpoint`, the same independent
                // switch `checkpoint_after_turn` answers to below, and
                // deliberately **not** `memory_extraction`: checkpoints and
                // extraction are separate capabilities and turning one off
                // must leave the other exactly as it was.
                if automatic_checkpoint_enabled(runtime) {
                    checkpoint_before_compaction(runtime, &id, &record.harness);
                }
                return Ok(());
            }
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
        // Phase 19: *allow Glasshouse to request a checkpoint automatically
        // at selected task boundaries.*
        //
        // This is the one place a harness tells Glasshouse that a task
        // finished, and `TurnEnded { Completed }` is the only event that
        // carries that claim — `session::lifecycle::event_for` is its single
        // construction site, and a source-scanning test fails if a second one
        // appears. So this is where both triggers belong.
        //
        // Ordered **after** the event is recorded, on purpose: the log is the
        // material extraction reads, and a turn's own closing event should be
        // in it. Ordered **before** the state change for no reason at all
        // beyond it reading better; neither `run_extraction` nor
        // `checkpoint_after_turn` can fail in a way the rest of this function
        // could notice.
        //
        // The two triggers are gated independently — `memory_extraction` and
        // `automatic_checkpoint` are separate config fields, read by separate
        // `EffectiveConfig` methods — so turning one off leaves the other
        // exactly as it was.
        if matches!(
            translated,
            LifecycleEvent::TurnEnded {
                outcome: TurnOutcome::Completed
            }
        ) {
            if memory_extraction_enabled(runtime) {
                run_extraction(
                    runtime,
                    &id,
                    model(&id),
                    glasshouse::memory::ExtractionTrigger::TaskCompleted,
                );
            }
            if automatic_checkpoint_enabled(runtime) {
                checkpoint_after_turn(runtime, &id, &record.harness);
            }
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
fn run_extraction(
    runtime: &Runtime,
    id: &SessionId,
    model: Box<dyn glasshouse::memory::ExtractionModel>,
    trigger: glasshouse::memory::ExtractionTrigger,
) {
    use glasshouse::memory::extract::chunk::ChunkLimits;
    use glasshouse::memory::extract::lifecycle::{EVENT_WINDOW, chunk_for_session};
    use glasshouse::memory::{Extractor, ProjectMemory};

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

    // The **working tree**, though, is read here, and mid-edit is exactly why.
    //
    // The refusal above is about a *commit*: "where the project was when this
    // was learned" is a poor answer while somebody is still editing. The same
    // sentence argues the other way for the dirty set — mid-edit is the state
    // in which what differs from the index is most informative and a commit
    // least — so this reads it and records it under its own name, `observed`,
    // rather than claiming the memory referenced anything.
    //
    // Read here, before the thread starts, for the reason this function's own
    // doc gives about everything else cheap: the model call can take seconds,
    // and the set this associates memories with should be the one that was
    // true when extraction began rather than whatever the user has typed
    // since. `WorkingTreeStatus::detect` opens two small files and no
    // database.
    let observed_files = WorkingTreeStatus::detect(runtime.project().root())
        .map(|status| status.changed_files)
        .unwrap_or_default();

    let (tx, rx) = std::sync::mpsc::channel();
    let session = id.clone();
    std::thread::spawn(move || {
        let store = memory.store();
        let outcome = Extractor::new(&store, model.as_ref()).run(&chunk, trigger);
        // A closed receiver means the bound expired and nobody is listening.
        // That is a normal outcome here, not an error.
        let _ = tx.send(outcome);
        drop(session);
    });

    match rx.recv_timeout(EXTRACTION_BOUND) {
        Ok(outcome) => {
            match &outcome.failure {
                None => tracing::info!(
                    session = %id,
                    trigger = %trigger,
                    model = outcome.model,
                    stored = outcome.stored(),
                    duplicates = outcome.duplicates,
                    speculative = outcome.speculative,
                    rejected = outcome.rejected.len(),
                    "memory extraction ran"
                ),
                Some(failure) => tracing::info!(
                    session = %id,
                    trigger = %trigger,
                    model = outcome.model,
                    reason = %failure,
                    "memory extraction produced nothing"
                ),
            }
            // After the log line and outside the `failure` match on purpose:
            // a reply that failed the extraction contract still cost whatever
            // the provider says it cost, and a ledger that recorded only the
            // runs that worked would under-report exactly the calls worth
            // knowing about.
            record_extraction_observation(runtime, &outcome);
            record_observed_files(runtime, &outcome.recorded, &observed_files);
        }
        Err(_) => tracing::warn!(
            session = %id,
            trigger = %trigger,
            bound_ms = EXTRACTION_BOUND.as_millis(),
            "memory extraction did not finish within its bound; the session is unaffected"
        ),
    }
}

/// What the extraction model reported the call cost, into this project's
/// routing evidence ledger.
///
/// # This is the first thing in this build that counts tokens
///
/// `routing_observations` has carried `input_tokens`, `output_tokens` and
/// `cached_input_tokens` since migration 11 and nothing has ever written
/// one: `crate::gateway::ingress` relays a response body it is designed
/// never to parse, so the gateway producer leaves all three `NULL` and says
/// so in its own module header. Memory extraction is the other path —
/// Glasshouse builds the request itself and already deserializes the whole
/// reply — so the counts come from a document that was parsed anyway. See
/// [`glasshouse::memory::extract::ModelCall::observation`] for exactly what
/// one row carries and what it deliberately leaves empty.
///
/// # Why the ledger is opened here and not beside the event log
///
/// The same finding [`evidence_ledger`] carries, one path over.
/// [`glasshouse::routing::evidence::EvidenceLedger`] holds `Mutex<Connection>`
/// — an open SQLite handle for its whole lifetime — and a handle opened on a
/// path that turns out to have nothing to write blocks a later writer under
/// Windows' mandatory `LockFileEx` while being invisible under POSIX advisory
/// locks. So nothing is opened until `observation()` has already said there
/// is a row: that is [`None`] for every run that reached no provider, which
/// is every run under the default configuration, where extraction chooses a
/// resource and calls nothing at all.
///
/// # A failure here is one log line
///
/// [`run_extraction`]'s own posture, for its own reason: this is a hook
/// process running inside somebody's coding session, and Glasshouse's
/// bookkeeping is never more important than the session it keeps books
/// about. There is no error channel out of this function because no caller
/// should have one.
fn record_extraction_observation(
    runtime: &Runtime,
    outcome: &glasshouse::memory::ExtractionOutcome,
) {
    let Some(observation) = outcome.observation() else {
        return;
    };
    let ledger = match glasshouse::routing::evidence::EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "routing evidence ledger unavailable; what this extraction cost is not recorded"
            );
            return;
        }
    };
    if let Err(err) = ledger.record(observation, glasshouse::provider::cache::now_unix_seconds()) {
        tracing::warn!(
            error = %err,
            "could not record what memory extraction cost"
        );
    }
}

/// Which files were being worked on when these memories were learned, into
/// this project's `memory_files` — migration 17.
///
/// # This records an observation and not a reference, deliberately
///
/// `paths` is what the git index said differed from the working tree when
/// extraction began. It says *"this was learned while that file was being
/// worked on"*, which is a fact about the **session**: three memories out of a
/// session that dirtied twenty files get all sixty pairs, and each pair is
/// true. It is emphatically not capability-map line 1139's *"the files a
/// memory explicitly references"* — on this path the model's input carries no
/// prose at all, so a model asked to name files here would be fabricating from
/// an empty input, and line 1294's rule is that a fabricated value inverts the
/// policy rather than degrading it. Every row therefore carries
/// [`glasshouse::memory::FileAssociation::Observed`].
///
/// # Why the store is opened here and not beside the event log
///
/// [`record_extraction_observation`]'s finding, one function over, for the
/// same reason: an open SQLite handle on a path that turns out to have
/// nothing to write blocks a later writer under Windows' mandatory
/// `LockFileEx` while being invisible under POSIX advisory locks (practice
/// §65). So the guard comes first and nothing is opened at all when there is
/// no row — which is every extraction that stored nothing, and every one run
/// against a clean tree.
///
/// This deliberately runs on the calling thread rather than inside the
/// extraction thread: the thread outlives its bound, and a write started
/// there after the process has already decided to move on would be a second
/// writable handle appearing at an unpredictable moment.
///
/// # A failure here is one log line
///
/// [`run_extraction`]'s posture, and the path is not named in it: a file path
/// is the user's own data, so the log says how many associations were lost
/// and never which files they were about.
fn record_observed_files(
    runtime: &Runtime,
    recorded: &[glasshouse::memory::MemoryId],
    paths: &[String],
) {
    if recorded.is_empty() || paths.is_empty() {
        return;
    }
    let memory = match glasshouse::memory::ProjectMemory::open(runtime) {
        Ok(memory) => memory,
        Err(err) => {
            tracing::warn!(
                error = %format!("{err:#}"),
                "project memory unavailable; which files this session was \
                 working on is not recorded"
            );
            return;
        }
    };
    match memory.store().record_observed_files(recorded, paths) {
        Ok(written) => tracing::debug!(
            memories = recorded.len(),
            files = paths.len(),
            rows = written,
            "recorded which files were being worked on"
        ),
        Err(err) => tracing::warn!(
            error = %err,
            "could not record which files were being worked on"
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
/// # Why the log is behind a `Mutex`
///
/// [`EventLog`] owns a `rusqlite::Connection`, which is `Send` and **not**
/// `Sync`. Since [`DegradeRelay`], a recorder is no longer touched only by
/// the thread that built it: the gateway's own connection thread reports a
/// failed upstream through it, so `&EventRecorder` crosses a thread boundary
/// and the type has to be `Sync` to be shared at all. The lock is what makes
/// it so, and it is uncontended in practice — the two writers are a launch
/// path making one bookkeeping call at a time and a gateway thread that only
/// speaks when its upstream has just failed.
struct EventRecorder {
    bus: EventBus,
    log: Option<Mutex<EventLog>>,
}

impl EventRecorder {
    fn open(runtime: &Runtime) -> Self {
        let log = match EventLog::open(runtime) {
            Ok(log) => Some(Mutex::new(log)),
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

    /// Record that one backend resource stopped serving — map line 1735's
    /// durable half, on the path the shipped binary actually takes.
    ///
    /// # Why `degrade_resource` is called rather than reimplemented
    ///
    /// Which sessions a failing resource affects is one rule, and it lives in
    /// [`glasshouse::events::degrade_resource`]: *a session is affected if,
    /// and only if, its own record says it resolved to this backend
    /// resource.* Selecting the sessions here instead would be a second copy
    /// of that rule, and it would leave `degrade_resource` with no production
    /// caller again — the exact state the evidence ledger refused this line
    /// in.
    ///
    /// # Why it publishes on a bus that keeps nothing
    ///
    /// `degrade_resource` publishes each `GatewayUnhealthy` on the bus it is
    /// given, and the durable write on this path is [`Self::append`], which
    /// publishes on *this* recorder's bus to mint the record. Handing it
    /// `self.bus` would mint every event twice. A history of zero makes the
    /// bus purely the question-asking apparatus: nothing is kept, nothing is
    /// dropped, and the returned [`glasshouse::events::Degradation`] is the
    /// answer this method acts on.
    fn degrade(
        &self,
        records: &[SessionRecord],
        resource: &str,
        reason: glasshouse::events::GatewayFailure,
    ) {
        let selection = EventBus::with_history(0);
        let degradation =
            glasshouse::events::degrade_resource(&selection, records, resource, reason);
        for id in &degradation.affected {
            self.record(
                id,
                LifecycleEvent::GatewayUnhealthy {
                    resource: degradation.resource.clone(),
                    reason,
                },
            );
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
        let log = log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Err(err) = log.append(&recorded, observed.as_ref()) {
            tracing::warn!(session = %id, error = %err, "could not record a lifecycle event");
        }
    }
}

/// The most gateway failures held while there is still nowhere to record
/// them.
///
/// A bound rather than an unbounded queue for the reason every other buffer
/// in this crate is bounded: the window is meant to be empty, and a window
/// that is not empty is a defect, not a workload. Anything past this is
/// counted and reported rather than kept — see [`DegradeRelay::report`].
const EARLY_GATEWAY_FAILURES: usize = 32;

/// Where a gateway failure is recorded, given that the recorder does not
/// exist yet when the gateway starts.
///
/// # The ownership problem, stated exactly
///
/// [`glasshouse::gateway::DegradeSink`] has to be handed to the gateway at
/// `start_if_required_with_degrade_sink`, and **both** of this binary's
/// gateway starts happen before anything the sink needs exists:
/// `launch_session` starts the gateway 184 lines before it opens its
/// [`EventRecorder`], and it has no `SessionRecord` at all until the store
/// has created one. So the sink cannot close over a bus and a session list;
/// there is nothing to close over. This is the handle it closes over
/// instead, created before the gateway and filled by [`Self::install`] once
/// both halves exist.
///
/// # Why the session records are a snapshot, and whose sessions they are
///
/// [`glasshouse::events::degrade_resource`] takes the records it should
/// consider. This relay is given **the sessions this process owns** — one, on
/// either path — and not a fresh read of the project's whole session table.
/// Two reasons, and the second is the load-bearing one:
///
/// - reading fresh would mean a `SessionStore` on the gateway's thread, which
///   means a second open connection held for the life of the session for a
///   read that fires only when an upstream has failed. §65's Windows hang was
///   exactly that shape;
/// - and a gateway is **per instance**. Another Glasshouse process's session
///   is served by *its* gateway, which does its own detecting. Degrading it
///   from here would report a failure this process never observed on that
///   session's behalf. The narrower snapshot is the honest claim.
///
/// # Lifetime
///
/// The sink holds an `Arc<DegradeRelay>` and the relay holds an
/// `Arc<EventRecorder>`; neither points back, so there is no cycle to leak.
/// No thread is started here and none is kept alive: the relay is inert
/// between calls, and the gateway's own guard is what stops the threads that
/// call it.
struct DegradeRelay {
    state: Mutex<RelayState>,
}

/// [`DegradeRelay`]'s two lives, in one lock so that a failure arriving
/// during [`DegradeRelay::install`] cannot be pushed onto a queue that has
/// just been drained.
enum RelayState {
    /// Before the recorder exists. Failures accumulate, with their count.
    Waiting {
        held: Vec<(String, glasshouse::events::GatewayFailure)>,
        /// How many were dropped for exceeding [`EARLY_GATEWAY_FAILURES`].
        dropped: usize,
    },
    /// After it does. Failures are written straight through.
    Ready {
        events: Arc<EventRecorder>,
        records: Vec<SessionRecord>,
    },
}

impl DegradeRelay {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(RelayState::Waiting {
                held: Vec::new(),
                dropped: 0,
            }),
        })
    }

    fn own(&self) -> std::sync::MutexGuard<'_, RelayState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The sink to start a gateway with.
    fn sink(self: &Arc<Self>) -> glasshouse::gateway::DegradeSink {
        let relay = Arc::clone(self);
        Arc::new(move |resource: &str, reason| relay.report(resource, reason))
    }

    /// Called by the gateway's own connection thread, once per exchange whose
    /// outcome says the upstream failed.
    ///
    /// A failure that arrives before [`Self::install`] is **held, not
    /// dropped and not fatal**. Panicking would take down the user's session
    /// over a telemetry ordering problem, and discarding would make the one
    /// window this line exists to observe the one window it cannot see. Past
    /// the bound the failure is counted and named in the log, so even the
    /// discard leaves a trace.
    fn report(&self, resource: &str, reason: glasshouse::events::GatewayFailure) {
        match &mut *self.own() {
            RelayState::Waiting { held, dropped } => {
                if held.len() >= EARLY_GATEWAY_FAILURES {
                    *dropped += 1;
                    tracing::warn!(
                        resource,
                        %reason,
                        dropped = *dropped,
                        "a gateway failure arrived before this launch had anywhere to record \
                         it, and more than {EARLY_GATEWAY_FAILURES} are already waiting"
                    );
                    return;
                }
                held.push((resource.to_owned(), reason));
            }
            RelayState::Ready { events, records } => {
                events.degrade(records, resource, reason);
            }
        }
    }

    /// Give the relay somewhere to write, and replay anything that arrived
    /// first.
    ///
    /// Called on the launch and resume paths at the first point where both
    /// the recorder and the session record exist. Nothing before this point
    /// waits on it: the gateway is already serving.
    fn install(&self, events: Arc<EventRecorder>, records: Vec<SessionRecord>) {
        let mut state = self.own();
        let ready = RelayState::Ready {
            events: Arc::clone(&events),
            records: records.clone(),
        };
        let RelayState::Waiting { held, dropped } = std::mem::replace(&mut *state, ready) else {
            // Each path builds its own relay and installs once. A second
            // install would mean two owners of one gateway's failures.
            tracing::warn!("a degrade relay was installed twice");
            return;
        };
        if dropped > 0 {
            tracing::warn!(
                dropped,
                "gateway failures were discarded before this launch could record them"
            );
        }
        // Still under the lock: a failure arriving now waits rather than
        // overtaking the replay, so the log keeps the order the gateway saw.
        for (resource, reason) in held {
            events.degrade(&records, &resource, reason);
        }
    }
}

impl Drop for DegradeRelay {
    /// The last resort against a silent loss: a relay dropped while still
    /// holding failures is a launch that ended before it could record them —
    /// a database that would not open, or an early return between the gateway
    /// and the recorder. Nothing can be written at that point; saying so in
    /// the log is what stops it from being invisible.
    fn drop(&mut self) {
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let RelayState::Waiting { held, dropped } = state
            && (!held.is_empty() || *dropped > 0)
        {
            tracing::warn!(
                held = held.len(),
                dropped,
                "this launch ended without ever being able to record a gateway failure"
            );
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
                    memory: binding_memory_lines(runtime),
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

/// The project's current binding memory, rendered for a checkpoint's
/// `Handoff::memory` — line 1641.
///
/// Opening the project's memory database or reading its binding records is
/// never allowed to fail a checkpoint: a checkpoint with no memory section is
/// strictly better than no checkpoint at all, so either failure degrades to
/// an empty list rather than propagating with `?`. `api/unix.rs::request_checkpoint`
/// carries the identical addition rather than calling through this one — see
/// its own comment on why that duplication stands.
fn binding_memory_lines(runtime: &Runtime) -> Vec<String> {
    use glasshouse::memory::ProjectMemory;

    let Ok(memory) = ProjectMemory::open(runtime) else {
        return Vec::new();
    };
    let Ok(records) = memory.store().binding(20) else {
        return Vec::new();
    };
    records
        .into_iter()
        .map(|record| match record.subject {
            // Phase 20 allows an absent subject; rendering an empty one would
            // print a heading nobody wrote.
            Some(subject) => format!("{subject}: {}", record.body),
            None => record.body,
        })
        .collect()
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
/// asked for, alongside the session the checkpoint was recorded from.
///
/// A named checkpoint that does not exist is an error rather than an empty
/// prompt: starting a fresh session that silently lost its handoff is the
/// worst of the available outcomes, because it looks exactly like one that
/// worked.
fn resolve_bootstrap_prompt(
    runtime: &Runtime,
    named: Option<&str>,
) -> anyhow::Result<Option<(String, SessionId)>> {
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
    Ok(Some((
        stored.checkpoint.bootstrap_prompt(),
        stored.checkpoint.session.clone(),
    )))
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
/// Line 1592's task-boundary caller, and line 1601's explanation on it.
///
/// Prints where the router would have sent this work and what the named
/// session displaced. Never changes the destination — see `RouteOnResume`.
/// Everything it needs can fail (the session store, a deleted profile, a quota
/// cache that will not open), and none of those may cost a person their
/// resume, so the whole thing is best effort and silent when it has nothing to
/// say.
/// **It explains; it does not move the work.** The session was named on the
/// command line, and a router that answered "somewhere else" would overrule
/// the most explicit statement a person can make — so the named session goes
/// in as `RoutingOverride::to`, which is what line 1602 calls a user override,
/// and the ranking it displaced is printed beside it. Stated as a limit rather
/// than left to be discovered: **line 1593 is earned on the launch path**,
/// where the choice is genuinely open, and not here.
fn report_task_boundary_routing(runtime: &Runtime, session: &str) {
    use glasshouse::routing::session::{
        RouterInputs, RoutingMoment, RoutingOverride, SessionRouter, TaskRequirements,
    };

    // Its own scope, and everything it opened is closed before it returns —
    // see the call site.
    let Some((id, harness)) = ({
        let Ok(sessions) = ProjectSessions::open(runtime) else {
            return;
        };
        let store = sessions.store();
        store
            .resolve_id(session)
            .ok()
            .and_then(|id| store.get(&id).ok().flatten())
            .map(|record| (record.id.clone(), record.harness.clone()))
    }) else {
        return;
    };
    let Some(harness) = glasshouse::integrations::IntegrationId::ALL
        .iter()
        .copied()
        .find(|candidate| candidate.slug() == harness)
    else {
        return;
    };

    let Ok(user) = UserConfig::load(runtime.paths()) else {
        return;
    };
    let Ok(project) = config::load_project_config(runtime.project()) else {
        return;
    };
    let effective = EffectiveConfig::new(&user, project.as_ref());

    let Ok(destinations) =
        routing_destinations(runtime, &effective, harness, DestinationScope::Everything)
    else {
        return;
    };
    let current = destinations
        .iter()
        .find(|destination| destination.id() == id.as_str())
        .cloned();
    let overrides = effective.pairing_overrides();
    // Line 1599's bridge again — see `observed_provider_health`. This report
    // is read beside the launch path's own decision, so it weighs the same
    // persisted readings that path does.
    let health = observed_provider_health(runtime, &destinations);
    let inputs = RouterInputs {
        overrides: &overrides,
        health: &health,
        now: std::time::Instant::now(),
        requirements: TaskRequirements::default(),
    };
    let Some(routed) = SessionRouter::with_override(RoutingOverride::to(id.as_str())).choose(
        RoutingMoment::TaskBoundary,
        current.as_ref(),
        &destinations,
        &inputs,
    ) else {
        return;
    };
    // A ranking that agreed with the user says nothing worth a line on their
    // terminal; one that would have chosen differently is the whole reason
    // line 1601 exists.
    if let Some(automatic) = routed.overrode() {
        eprintln!(
            "glasshouse: resuming {} because you named it; the ranking would have chosen `{}` \
             at this task boundary. `glasshouse route --moment task-boundary` says why.",
            short_id(&id),
            automatic
        );
    }
}

/// Whether a resume is the moment a routing decision is taken, or the tail of
/// one that already was.
///
/// `glasshouse resume` is line 1592's **task boundary**: one piece of work
/// finished and another is beginning, which is exactly when the map allows the
/// work to move. `launch_session` reaches the same code after having already
/// decided at a *session* boundary, and routing twice for one launch would
/// re-decide something nobody asked to have re-decided — the failure mode line
/// 1592 is written against, one layer up from the per-turn one it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteOnResume {
    /// Take the task-boundary decision here.
    AtTaskBoundary,
    /// The caller already routed; this is the tail of its decision.
    AlreadyRouted,
}

fn resume_session(
    runtime: &Runtime,
    session: &str,
    harness_args: &[String],
    headless: bool,
    routing: RouteOnResume,
) -> anyhow::Result<ExitCode> {
    // Line 1592's other moment, and it runs **before** this function's own
    // session store is opened.
    //
    // Not a stylistic choice. `routing_destinations` opens a connection to
    // this project's session database, and practice §65 is the record of what
    // an extra open handle costs on a path nobody asserts about: SQLite takes
    // advisory locks on Unix and mandatory `LockFileEx` locks on Windows, so
    // two live connections to one database in one process is invisible on the
    // machine this is developed on and a hang on the one it ships to. Running
    // the routing report to completion first means exactly one connection is
    // ever live.
    if routing == RouteOnResume::AtTaskBoundary {
        report_task_boundary_routing(runtime, session);
    }

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
    // Map line 1735: built before the gateway `resolve_resume_overlay` starts,
    // installed below beside the recorder — see `DegradeRelay`.
    let degrade_relay = DegradeRelay::new();
    let overlay_resolution = record.launch_profile.as_deref().and_then(|name| {
        match resolve_resume_overlay(
            &effective,
            &user,
            project.as_ref(),
            &selection,
            name,
            runtime,
            degrade_relay.sink(),
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

    note_resume(&store, &resumable);

    // Phase 18's "record session resume events". A distinct event rather than
    // a second `SessionStarted`, because otherwise a reader has to infer a
    // resume from a session having started twice, and an inference is not a
    // recording.
    let events = Arc::new(EventRecorder::open(runtime));
    events.record(&resumable.id, LifecycleEvent::SessionResumed);

    // Map line 1735's resume half. `record` was read from the store above, so
    // its `backend_resource` is whatever the original launch resolved — which
    // is exactly the question `degrade_resource` asks of it.
    degrade_relay.install(Arc::clone(&events), vec![record.clone()]);

    // Headless is the caller's statement about *this* process, so it survives
    // a launch that the router turned into a resume — `glasshouse run
    // --headless` must not quietly take over the terminal because the best
    // destination happened to be a session that already existed.
    let attached = if headless {
        run_headless(&resumable.id, launch)
    } else {
        session::attach(launch)
    };
    let status = match attached {
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

/// Move a resumed session back to `Running`, logging rather than failing.
///
/// [`note_lifecycle`]'s sibling, and **not** a call to it. `set_lifecycle`
/// declines to move a finished record back to a live state, because a hook
/// process outliving its harness must not resurrect a stopped session — and
/// this write went through that same door and was refused by that same rule,
/// so a session Glasshouse had just reopened kept reading `stopped`. Every
/// hook the resumed harness then sent was discarded for arriving at a
/// session the store believed was over.
///
/// `SessionStore::begin_resume` is the door for the case where Glasshouse is
/// the one acting. It re-checks the disposition under the write lock, so a
/// record another process closed between `open_for_resume` and here is
/// refused rather than revived.
///
/// Best effort, for `note_lifecycle`'s reason: the harness is about to be
/// handed the user's conversation, and Glasshouse's own record keeping is not
/// worth failing that over. A refusal is logged at `warn` rather than
/// swallowed, because a resume that could not be recorded is the exact
/// condition this function exists to make visible.
fn note_resume(
    store: &glasshouse::session::SessionStore<'_>,
    resumable: &glasshouse::session::ResumableSession,
) {
    if let Err(err) = store.begin_resume(resumable) {
        tracing::warn!(
            session = %resumable.id,
            error = %err,
            "could not record a session resume; the session will keep reading as finished"
        );
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
    telemetry = telemetry.gather_gateway_health(
        &glasshouse::provider::telemetry::GatewayHealthCache::new(runtime.paths()),
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
    let mut out = format!(
        "{probes}{}",
        glasshouse::provider::resources::report(&effective, &telemetry, options)
    );
    out.push('\n');
    render_routing_model(
        &mut out,
        runtime,
        &user,
        project.as_ref(),
        &effective,
        verbose,
    );
    Ok(out)
}

/// Capability map line 1443 — *"show the currently selected routing model in
/// resource diagnostics"* — as the last block of `glasshouse resources`.
///
/// # Why this surface, and why it is not the settings screen
///
/// The Settings overlay already renders the configured
/// [`glasshouse::config::RoutingModelChoice`], and `docs/product/evidence/phase-34c.md`
/// ruled that showing a value on the screen where you set it is
/// configuration, not diagnosis. This is the diagnostic surface: the routing
/// model is named next to the capacity, health and quota of the very
/// resources it would be chosen from, which is where the question *"why did
/// routing behave that way"* is actually asked.
///
/// # The honesty constraint, and it is the point of the block
///
/// `Automatic` is an intent — the word the Settings overlay shows — and
/// naming only that would answer a different question than a person reading
/// `glasshouse resources` is asking. So the block runs the real decision
/// ([`automatic_classification_choice`], the same function `glasshouse
/// classify` calls) and names the resource it picked.
///
/// **And it says `would`, in every arm.** Nothing in this build classifies
/// anything on its own: `routing::classify::classify`'s only production
/// caller is the `glasshouse classify` diagnostic, and nothing else asks a
/// routing model a question. Rendering a "currently selected routing model"
/// beside live capacity numbers with no signal that it classifies nothing is
/// the spectacle Phase 47 exists to prevent, so the `in use` row says so in
/// as many words and is not conditional on anything.
///
/// # No credential, ever
///
/// [`glasshouse::routing::disposable::DisposableChoice`] carries a
/// [`glasshouse::routing::CredentialId`], and nothing below reads it. A
/// provider name, a model name and the policy's own explanation are what this
/// block prints — the same rule `memory::extract::model`'s header states for
/// the label a classification is attributed to.
fn render_routing_model(
    out: &mut String,
    runtime: &Runtime,
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    effective: &EffectiveConfig<'_>,
    verbose: bool,
) {
    use glasshouse::config::{RoutingFallback, RoutingModelResolution};
    use std::fmt::Write as _;

    let resolution = effective.routing_model_resolution();
    out.push_str("ROUTING MODEL\n");

    let configured = match &resolution.value {
        RoutingModelResolution::Automatic => "automatic".to_owned(),
        RoutingModelResolution::Pinned { provider, model } => format!("{provider}/{model}"),
        RoutingModelResolution::Heuristics(RoutingFallback::ProviderNotConfigured {
            provider,
            ..
        }) => format!("deterministic heuristics (`{provider}` is no longer configured)"),
        RoutingModelResolution::Heuristics(_) => "deterministic heuristics".to_owned(),
    };
    let _ = writeln!(
        out,
        "  {:<16}{configured} ({})",
        "configured",
        resolution.layer.describe_source()
    );

    match &resolution.value {
        RoutingModelResolution::Automatic => {
            // `None`: this report has no request in hand, and `choose`
            // documents that value as the fixed `WorkloadTier::Leaf` the
            // policy used before a classification existed to ask. A request
            // invented here to fill the argument would make the reported pick
            // depend on words nobody typed.
            match automatic_classification_choice(runtime, user, project, effective, None) {
                Ok(choice) => {
                    let _ = writeln!(
                        out,
                        "  {:<16}{} on {} — {}, {}",
                        "would select",
                        choice.model(),
                        choice.provider(),
                        choice.cost().as_str(),
                        choice.reason()
                    );
                    let _ = writeln!(
                        out,
                        "  {:<16}for a request of unknown demand; a classified request can \
                         select another",
                        ""
                    );
                    if verbose {
                        for line in choice.explanation().render().lines() {
                            let _ = writeln!(out, "  {:<16}{line}", "");
                        }
                    }
                }
                Err(reason) => {
                    let _ = writeln!(out, "  {:<16}nothing — {reason}", "would select");
                }
            }
        }
        RoutingModelResolution::Pinned { provider, model } => {
            let _ = writeln!(
                out,
                "  {:<16}{model} on {provider} — pinned, so no ranking runs",
                "would select"
            );
        }
        RoutingModelResolution::Heuristics(_) => {
            let _ = writeln!(
                out,
                "  {:<16}no model — deterministic heuristics classify without asking one",
                "would select"
            );
        }
    }

    let _ = writeln!(
        out,
        "  {:<16}nothing yet. `glasshouse classify` is the only command that asks a routing \
         model;\n  {:<16}no other Glasshouse decision calls one, so this names a choice rather \
         than a habit.",
        "in use", ""
    );
}

/// The one search this project's memory retrieval goes through — Phase 21F
/// line 929's grouping, and the shared core `memory_report` (the CLI's
/// `glasshouse memory search`) and `api::unix::query_memory` (the machine
/// door) both render from, so the two can never disagree about what a query
/// finds or how it is grouped.
fn memory_search_grouped(
    runtime: &Runtime,
    query: &str,
    history: bool,
    limit: usize,
) -> anyhow::Result<glasshouse::memory::search::RetrievalResult> {
    use glasshouse::memory::ProjectMemory;
    use glasshouse::memory::search::SearchScope;

    let scope = if history {
        SearchScope::Historical
    } else {
        SearchScope::Current
    };

    // The memory connection is opened, used and dropped before the evaluation
    // ledger opens its own. Two SQLite handles held over each other on one
    // file is practice §65's Windows hang, and there is no reason to hold both
    // here: the search is finished before the observation is written.
    let grouped = {
        let memory = ProjectMemory::open(runtime)?;
        memory.store().search_grouped(query, scope, limit)?
    };

    // Phase 51 lines 1822 and 1826: a retrieval is an ephemeral decision that
    // changes what the user gets and otherwise leaves no trace, so this is the
    // one place it becomes countable. One row per returned memory, carrying
    // `memory_id` and nothing of the memory itself; whether a memory was stale
    // is read later by joining `memories`, not judged here. This records and
    // never fails: bookkeeping does not get to break a search.
    glasshouse::evaluation::record_memory_retrieval(
        runtime,
        glasshouse::evaluation::RetrievalScope::from_history_flag(history),
        grouped
            .invariants_and_constraints
            .iter()
            .chain(grouped.other.iter())
            .map(|record| record.id.as_str()),
        glasshouse::evaluation::now_unix(),
    );

    Ok(grouped)
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
    let grouped = memory_search_grouped(runtime, query, history, limit)?;
    render_memory_report(&grouped, query, history)
}

/// Pure formatting half of [`memory_report`], separated so
/// `api::unix::query_memory` can render the identical text from a
/// [`glasshouse::memory::search::RetrievalResult`] it already has, without a
/// second trip through the database.
fn render_memory_report(
    grouped: &glasshouse::memory::search::RetrievalResult,
    query: &str,
    history: bool,
) -> anyhow::Result<String> {
    use std::fmt::Write as _;

    let mut out = String::new();
    if grouped.invariants_and_constraints.is_empty() && grouped.other.is_empty() {
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

    // Phase 21F line 929: current invariants and constraints are printed as
    // their own group, ahead of and apart from everything else a search
    // matched, rather than left for a reader to tell apart from a rendered
    // string.
    if !grouped.invariants_and_constraints.is_empty() {
        writeln!(out, "-- current invariants & constraints --")?;
        for record in &grouped.invariants_and_constraints {
            write_memory_record(&mut out, record)?;
        }
    }
    if !grouped.other.is_empty() {
        if !grouped.invariants_and_constraints.is_empty() {
            writeln!(out, "-- other results --")?;
        }
        for record in &grouped.other {
            write_memory_record(&mut out, record)?;
        }
    }
    Ok(out)
}

/// One memory, rendered the way [`memory_report`] prints every result.
fn write_memory_record(
    out: &mut String,
    record: &glasshouse::memory::MemoryRecord,
) -> anyhow::Result<()> {
    use std::fmt::Write as _;

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
    // Phase 21F line 936: when this memory's authority means it may
    // constrain implementation, carry its validity and invalidation
    // conditions into the answer as well as its rationale — already printed
    // above, as `provenance_lines`'s "why" field.
    let constraint = constraint_lines(record);
    if !constraint.is_empty() {
        writeln!(out, "{constraint}")?;
    }
    // Phase 21F lines 937/938: a challenged memory must not read as settled.
    // Gated on `status`, not on `review_reason` alone, because a memory whose
    // review was resolved keeps its last `review_reason` on the record —
    // `MemoryStore::set_status` never clears it — so status is the only
    // field that says whether the challenge is still open.
    if record.status == glasshouse::memory::MemoryStatus::NeedsReview
        && let Some(reason) = record.review_reason
    {
        writeln!(
            out,
            "    challenged    {reason} — not returned as settled until resolved"
        )?;
    }
    // Map line 925: *"record why a decision was superseded so future agents do
    // not resurrect it without context."* `memory search --history` is where a
    // superseded memory is read at all, so it is where the context has to
    // arrive; printing the successor's identifier without the reason is
    // exactly the resurrection risk the line names.
    //
    // Gated on `superseded_reason` alone rather than on the status as well.
    // `MemoryStore::set_status` clears the column whenever a memory leaves
    // `Superseded`, so a reason present *is* a supersession in force — unlike
    // `review_reason` above, which survives its review being resolved and
    // therefore needs the status to disambiguate it.
    if let Some(reason) = &record.superseded_reason {
        writeln!(out, "    superseded    {reason}")?;
    }
    let session = record.source_session_id.as_deref().unwrap_or("unknown");
    let commit = record.source_commit.as_deref().unwrap_or("unknown");
    let events = record
        .source_events
        .map_or_else(|| "no event range".to_owned(), |events| events.to_string());
    writeln!(out, "    from session {session}, commit {commit}, {events}")?;
    Ok(())
}

/// Phase 21F line 936's conditional half: a memory's validity and
/// invalidation conditions are worth carrying only when its authority means
/// it may constrain implementation — an [`glasshouse::memory::MemoryAuthority::Invariant`],
/// a [`glasshouse::memory::MemoryAuthority::Constraint`], or an accepted
/// [`glasshouse::memory::MemoryAuthority::Decision`] (exactly
/// [`glasshouse::memory::MemoryAuthority::is_binding`]).
///
/// Explicit on `is_binding()` rather than "whichever fields happen to be
/// populated": an idea that recorded an invalidation condition anyway —
/// nothing in the schema stops it — must not read as though it could still
/// constrain anything.
fn constraint_lines(record: &glasshouse::memory::MemoryRecord) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    if !record
        .authority
        .is_some_and(glasshouse::memory::MemoryAuthority::is_binding)
    {
        return out;
    }
    if let Some(validity) = record.validity_conditions.as_deref() {
        let _ = writeln!(out, "    valid while  {validity}");
    }
    if let Some(invalidation) = record.invalidation_conditions.as_deref() {
        let _ = writeln!(out, "    invalid if   {invalidation}");
    }
    // The caller adds its own trailing newline, matching `provenance_lines`.
    out.pop();
    out
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

/// `glasshouse memory challenge <id> <reason>` — Phase 21F lines 937/938:
/// let the receiving agent say, explicitly, that current evidence
/// contradicts a memory, rather than silently distrusting it in a way
/// nothing records.
///
/// Reuses Phase 21C's `mark_for_review` and its six reasons rather than
/// inventing a seventh state: a challenge *is* "something changed that may
/// invalidate this; a person or a stronger agent has to look" — the review
/// mechanism already built for that. The retrieval half of 937/938 is true
/// the moment this returns: `SearchScope::Current` only ever returns
/// `Active` memories (see `memory/search.rs`'s own documentation), so the
/// challenged memory drops out of every default search immediately and
/// stays reachable only as history — `glasshouse memory search --history`.
///
/// 938's "before further automatic injection into the same task" has no
/// consumer in this build: Phase 27 (automatic injection) does not exist, so
/// there is nothing that injects a memory for this to gate. Closed on the
/// retrieval half only — see the packet's own reasoning, echoing §33's rule
/// of asking the capability as a question a user would ask: *can Glasshouse
/// stop presenting a challenged memory as settled?* Yes. *Can it stop an
/// automatic injection from using it?* There is no automatic injection to
/// stop.
fn memory_challenge(runtime: &Runtime, id: &str, reason: &str) -> anyhow::Result<String> {
    use glasshouse::memory::{ProjectMemory, ReviewReason};

    let parsed = ReviewReason::from_stored(reason).ok_or_else(|| {
        anyhow::anyhow!(
            "`{reason}` is not a review reason; use one of {}",
            ReviewReason::ALL
                .iter()
                .map(|r| r.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();
    let resolved = store.resolve_id(id)?;
    let record = store.mark_for_review(&resolved, parsed)?;

    Ok(format!(
        "{} is now {} ({}); it will not be returned as current until the challenge is \
         resolved. It remains searchable as history with --history.\n",
        record.id,
        glasshouse::memory::MemoryStatus::NeedsReview,
        parsed.as_str()
    ))
}

/// `glasshouse memory revalidate --list` — Phase 21G line 950's selection
/// half: the bounded queue of memories actually waiting for review, so
/// revalidation never becomes a sweep over the project's whole history.
/// Wires `MemoryStore::with_status`, which had no production caller before
/// this.
fn memory_revalidate_list(runtime: &Runtime, limit: usize) -> anyhow::Result<String> {
    use glasshouse::memory::{MemoryStatus, ProjectMemory};

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();
    let waiting = store.with_status(MemoryStatus::NeedsReview, limit)?;

    if waiting.is_empty() {
        return Ok("no memory is waiting for review\n".to_owned());
    }

    let mut out = String::new();
    for record in &waiting {
        out.push_str(&format!(
            "{} {} ({})\n",
            record.id,
            record.subject.as_deref().unwrap_or(&record.body),
            record
                .review_reason
                .map_or("no reason recorded", |reason| reason.as_str())
        ));
    }
    Ok(out)
}

/// `glasshouse memory revalidate <id> <outcome>` — Phase 21G line 949: the
/// resolution `memory challenge` has always promised
/// (`main.rs::memory_challenge` prints *"it will not be returned as current
/// until the challenge is resolved"*) and this build has never shipped.
/// `<outcome>` is exactly the four words the line names.
///
/// Defaults to the reviewed actor: a person typing this command by hand is
/// the human review Phase 22's gate asks for. `--automatic` invokes the
/// automatic actor instead, purely so the refusal on a high-impact memory
/// (line 948) is reachable and testable — nothing in this build calls it
/// that way itself.
fn memory_revalidate(
    runtime: &Runtime,
    id: &str,
    outcome: &str,
    by: Option<&str>,
    reason: Option<&str>,
    automatic: bool,
) -> anyhow::Result<String> {
    use glasshouse::memory::{ConflictResolver, ProjectMemory, ReviewReason};

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();
    let resolved = store.resolve_id(id)?;
    let actor = if automatic {
        ConflictResolver::Automatic
    } else {
        ConflictResolver::Reviewed
    };

    let record = match outcome {
        "reaffirmed" => {
            if by.is_some() || reason.is_some() {
                anyhow::bail!("`reaffirmed` takes neither --by nor --reason");
            }
            store.revalidate_reaffirmed(&resolved, actor)?
        }
        "needs-review" => {
            if by.is_some() {
                anyhow::bail!("`needs-review` does not take --by");
            }
            let reason = reason
                .ok_or_else(|| anyhow::anyhow!("`needs-review` requires --reason <REASON>"))?;
            let parsed_reason = ReviewReason::from_stored(reason).ok_or_else(|| {
                anyhow::anyhow!(
                    "`{reason}` is not a review reason; use one of {}",
                    ReviewReason::ALL
                        .iter()
                        .map(|r| r.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            store.revalidate_needs_review(&resolved, parsed_reason, actor)?
        }
        "superseded" => {
            // Map line 925. `--reason` here is the operator's own sentence
            // about why this decision went, not `needs-review`'s six-value
            // vocabulary above — a different question with a different answer
            // type, which is why it is stored in its own column. Optional: a
            // supersession with nothing to say is still a supersession.
            let by = by.ok_or_else(|| anyhow::anyhow!("`superseded` requires --by <ID>"))?;
            let successor = store.resolve_id(by)?;
            store.revalidate_superseded(&resolved, &successor, reason, actor)?
        }
        "invalidated" => {
            if by.is_some() || reason.is_some() {
                anyhow::bail!("`invalidated` takes neither --by nor --reason");
            }
            store.revalidate_invalidated(&resolved, actor)?
        }
        other => anyhow::bail!(
            "`{other}` is not a revalidation outcome; use one of reaffirmed, needs-review, \
             superseded, invalidated"
        ),
    };

    Ok(format!("{} is now {}\n", record.id, record.status))
}

/// `glasshouse memory conflicts` — map line 922's surfacing half.
///
/// An ordinary `glasshouse memory search` can move two memories to
/// [`glasshouse::memory::MemoryStatus::Conflicted`]
/// (`memory::search::flag_contradictions` → `MemoryStore::mark_conflicted`),
/// which drops both out of every default search immediately —
/// `MemoryStatus::is_current` answers `false` for `Conflicted`, same as every
/// other non-`Active` status. Wires [`glasshouse::memory::MemoryStore::with_status`]
/// again, this time against `Conflicted` rather than `NeedsReview`: that
/// method already selects by the `status` column alone and never consulted
/// `is_current`, so listing a conflict needed no new store query, only a
/// second production call to the one that already exists.
fn memory_conflicts_list(runtime: &Runtime, limit: usize) -> anyhow::Result<String> {
    use glasshouse::memory::{MemoryStatus, ProjectMemory};

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();
    let conflicted = store.with_status(MemoryStatus::Conflicted, limit)?;

    if conflicted.is_empty() {
        return Ok("no memory is conflicted\n".to_owned());
    }

    let mut out = String::new();
    for record in &conflicted {
        out.push_str(&format!(
            "{} {} ({})\n",
            record.id,
            record.subject.as_deref().unwrap_or(&record.body),
            record.authority.map_or("unclassified", |a| a.as_str())
        ));
    }
    Ok(out)
}

/// `glasshouse memory resolve <id> <outcome>` — map line 922's resolution
/// half: [`glasshouse::memory::MemoryStore::resolve_conflict`] is fully
/// implemented and tested and, before this, reachable only from `cargo test`.
///
/// Always calls it with [`glasshouse::memory::ConflictResolver::Reviewed`],
/// never `::Automatic`: a person typing this command by hand already is the
/// review Phase 22's gate asks for, and `::Automatic` would refuse every
/// binding-authority and every unclassified memory
/// (`MemoryStore::require_reviewed_for_high_impact`'s own documentation) —
/// the majority of them — which would make this command look broken rather
/// than working as designed. There is no `--automatic` flag here the way
/// `memory revalidate` has one: nothing in this build calls conflict
/// resolution automatically, so there is no refusal path this command needs
/// to make reachable.
fn memory_resolve_conflict(runtime: &Runtime, id: &str, outcome: &str) -> anyhow::Result<String> {
    use glasshouse::memory::{ConflictResolver, MemoryStatus, ProjectMemory};

    let outcome = match outcome {
        "active" => MemoryStatus::Active,
        "superseded" => MemoryStatus::Superseded,
        other => anyhow::bail!(
            "`{other}` is not a conflict outcome; use `active` to keep this memory as current \
             knowledge or `superseded` to record it as replaced"
        ),
    };

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();
    let resolved = store.resolve_id(id)?;
    let record = store.resolve_conflict(&resolved, outcome, ConflictResolver::Reviewed)?;

    Ok(format!("{} is now {}\n", record.id, record.status))
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

    // Run from a person's own shell at a moment they chose, unlike the
    // post-turn hook path: the project's current commit is exactly "where the
    // project was when this was learned", and cheap to read — see
    // `GitPosition::detect`.
    let commit = GitPosition::detect(runtime.project().root()).map(|position| position.commit);

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
            chunk_for_session(&id, &events, commit.as_deref(), ChunkLimits::default()),
            format!("{read} recorded events for session {id}"),
        )
    } else {
        let activity = activity.expect("clap requires --activity unless --from-events");
        let activity_text = std::fs::read_to_string(activity)
            .with_context(|| format!("read session activity from {}", activity.display()))?;
        (
            SessionChunk::build(
                session,
                commit,
                activity_text.lines().map(str::to_owned),
                ChunkLimits::default(),
            ),
            format!("{}", activity.display()),
        )
    };

    // The same reading `run_extraction` takes, from the same producer, before
    // the model is asked. Here it is unambiguously cheap: this command is one
    // synchronous pass on the main thread, and the store below is the same
    // connection that is about to write the memories, so nothing opens a
    // second handle.
    let observed_files = WorkingTreeStatus::detect(runtime.project().root())
        .map(|status| status.changed_files)
        .unwrap_or_default();

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();
    let model = ReplyFromFile(reply);
    let outcome = Extractor::new(&store, &model).run(&chunk, ExtractionTrigger::Manual);

    // Observed, never referenced — see `record_observed_files` for the whole
    // argument. A clean tree, or an extraction that stored nothing, writes no
    // rows rather than an empty one. A failure is reported and does not fail
    // the command: the memories are already stored.
    if let Err(err) = store.record_observed_files(&outcome.recorded, &observed_files) {
        tracing::warn!(
            error = %err,
            "could not record which files were being worked on"
        );
    }

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

    // Phase 30, lines 1159 and 1161-1165. `store.context` is the sole
    // producer of these facts (`session/store.rs::SessionStore::context`);
    // before this call it had no caller outside that module's own tests, so
    // every value it computes was correct and unreachable. A read failure
    // collapses to the same "-" the fields above use for nothing recorded,
    // exactly like `context()`'s own `Ok(None)` case — a session detail
    // report must finish even when this extra context cannot be read.
    let context = store.context(&id).ok().flatten();
    line(
        "compactions",
        &context
            .as_ref()
            .and_then(|c| c.observed_compactions)
            .map_or_else(|| "-".to_string(), |n| n.to_string()),
    );
    line(
        "prompt cache",
        &context
            .as_ref()
            .map_or_else(|| "-".to_string(), |c| c.prompt_cache.to_string()),
    );
    line(
        "checkpoint",
        &context
            .as_ref()
            .map_or_else(|| "-".to_string(), |c| c.checkpoint.to_string()),
    );
    line(
        "task continuity",
        &context
            .as_ref()
            .map_or_else(|| "-".to_string(), |c| c.task_continuity.to_string()),
    );
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

/// Capability map line 1290: *"allow the user to override reserve protection
/// for a specific task or session"* — the user-facing half.
///
/// # The scope is the whole point
///
/// The override is recorded as a **session identifier**, never as a flag.
/// There is no argument here that means "every session", and
/// [`glasshouse::routing::disposable::ReserveOverride`] has no constructor
/// that could express one: an override covering everything would be the
/// protected reserve disabled, which is a different capability from the one
/// this line asks for and a worse one, because the reserve exists to stop
/// background jobs exhausting the quota an interactive session needs.
///
/// The identifier is resolved through the session store first, so what lands
/// in the configuration is the canonical id rather than whatever prefix was
/// typed — the hook path that later reads it has resolved its own id the same
/// way, and two spellings of one session must not fail to match.
///
/// # Why the user layer
///
/// Writes go to the user-level configuration, like every other write outside
/// the settings UI: [`glasshouse::config::write_project_config_with_consent`]
/// puts a file inside the user's repository and its own doc comment reserves
/// that for a caller that has obtained explicit confirmation. Typing this
/// command is consent to record a preference, not consent to add a file to a
/// checked-out tree.
fn reserve_override_session(
    runtime: &Runtime,
    session: &str,
    clear: bool,
) -> anyhow::Result<String> {
    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    let id = store.resolve_id(session)?;
    let id = id.to_string();

    let mut user = UserConfig::load(runtime.paths())?;
    let mut granted: Vec<String> = user
        .routing()
        .reserve_override_sessions()
        .map(<[String]>::to_vec)
        .unwrap_or_default();
    granted.retain(|recorded| recorded != &id);
    if !clear {
        granted.push(id.clone());
    }
    // `Some(vec![])` rather than `None` once the user has touched this: an
    // empty list is "this layer says no sessions", which is a decision, and
    // `None` is "this layer never decided", which would defer to a project
    // layer the user has just tried to overrule. See the field's own doc.
    user.routing_mut()
        .set_reserve_override_sessions(Some(granted));
    user.save(runtime.paths())?;

    let short = &id[..id.len().min(8)];
    Ok(if clear {
        format!(
            "Session {short} no longer overrides reserve protection; its background jobs are \
             subject to the protected reserve again.\n"
        )
    } else {
        format!(
            "Session {short} may now spend protected quota reserve. No other session is \
             affected, and `glasshouse sessions reserve {short} --clear` withdraws it.\n"
        )
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

        // Both doors, counted together. `start_if_required_with_degrade_sink`
        // is `start_if_required_with_telemetry` plus map line 1735's sink and
        // is what both sites call today; counting only the older name would
        // have made this test pass with *zero* gateways found, which is the
        // §68 shape — a filter that matches nothing looks exactly like a pass.
        let starts = code.matches("start_if_required_with_telemetry(").count()
            + code.matches("start_if_required_with_degrade_sink(").count();
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

    /// Map line 1735's structural half: every gateway this binary starts is
    /// also given somewhere to report a failed upstream.
    ///
    /// The same standing and the same limits as the evidence-ledger scan
    /// above — it proves *presence*, not behaviour, and line 1735 does not
    /// close on it. `gateway_degrade::the_shipped_binary_records_a_gateway_\
    /// failure_against_the_session_it_launched` is what closes the line; this
    /// exists so that a future edit dropping one of the two sites back to
    /// `None` has something to object, since only one of the two paths has a
    /// behavioural test.
    #[test]
    fn every_gateway_the_binary_starts_is_given_somewhere_to_report_a_failure() {
        let code = production_code(include_str!("main.rs"));

        let starts = code.matches("start_if_required_with_degrade_sink(").count();
        // The argument each start is actually given. `launch_session` hands
        // its relay's sink in directly; the resume path builds the relay in
        // `resume_session` and passes it down, so the argument at the start
        // itself is the forwarded parameter.
        let sinks = code.matches("Some(degrade_relay.sink()),").count()
            + code.matches("Some(degrade_sink),").count();
        assert_eq!(
            starts, 2,
            "this binary should start a gateway at exactly two sites (launch and \
             resume); if that changed, this test needs to change with it"
        );
        assert_eq!(
            sinks, starts,
            "a gateway is started somewhere without a degrade sink, so its \
             upstream failing would be recorded nowhere — which is the state \
             map line 1735 was refused in"
        );
        assert_eq!(
            code.matches("DegradeRelay::new()").count(),
            starts,
            "each gateway start needs its own relay: two paths sharing one \
             would report a failure against the other's session"
        );
        assert_eq!(
            code.matches("degrade_relay.install(").count(),
            starts,
            "a relay is built and never installed: its sink would hold every \
             failure and write none of them"
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
            LaunchDestination {
                profile: Some("gateway"),
                ..LaunchDestination::default()
            },
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
            LaunchDestination {
                profile: Some("yolo"),
                ..LaunchDestination::default()
            },
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

    /// The mechanism that stops the payload drain being an open-ended wait —
    /// see [`PAYLOAD_DRAIN_BOUND`].
    ///
    /// The drain itself cannot be driven from in here: it reads the *process's*
    /// standard input, and a test that redirected that would redirect it for
    /// every other test in this binary. What is testable is the bound, and the
    /// bound is the whole of the fix. The end-to-end observation is recorded
    /// where it was made: with this process's input held open, the six tests
    /// that call `report_hook_with` block for ever, on this tree and on its
    /// base commit alike.
    ///
    /// Asserts *both* halves. Waiting is not evidence on its own — a version
    /// that reported `false` without ever running the work would satisfy the
    /// first assertion and be useless, which is what the second one is for.
    #[test]
    fn work_that_never_finishes_is_abandoned_at_its_bound() {
        let bound = std::time::Duration::from_millis(200);
        let started = std::time::Instant::now();
        let finished = abandon_after(bound, || {
            std::thread::sleep(std::time::Duration::from_secs(30));
        });
        let waited = started.elapsed();

        assert!(
            !finished,
            "work that sleeps for thirty seconds cannot have finished inside a {bound:?} bound"
        );
        assert!(
            waited < std::time::Duration::from_secs(5),
            "the caller waited {waited:?} on a {bound:?} bound, so the bound is not what ended \
             the wait"
        );
        assert!(
            waited >= bound,
            "waiting {waited:?} means the bound was not what ended the wait either"
        );
    }

    /// The other half: work that does finish inside its bound is waited for
    /// and reported as finished, so the bound never becomes an excuse to skip
    /// the drain a live harness is in the middle of.
    #[test]
    fn work_that_finishes_inside_its_bound_is_waited_for() {
        let done = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = std::sync::Arc::clone(&done);
        assert!(
            abandon_after(std::time::Duration::from_secs(30), move || {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }),
            "work that returns immediately must be reported as finished"
        );
        assert_eq!(
            done.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the work must actually have run"
        );
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

    /// Phase 21F lines 937/938, acceptance test 5: a challenged memory is
    /// not returned as settled, and its reason is recorded and readable.
    /// Enters through `memory_challenge` and `memory_report`, exactly what
    /// `glasshouse memory challenge` and `glasshouse memory search` run.
    #[test]
    fn a_challenged_memory_drops_out_of_current_search_and_names_why() {
        use glasshouse::memory::{MemoryAuthority, MemoryKind, NewMemory, ProjectMemory};

        let fixture = CliFixture::new();
        let project = ProjectMemory::open(&fixture.runtime).unwrap();
        let id = project
            .store()
            .record(
                NewMemory::new(
                    MemoryKind::Decision,
                    "The egret worker retries indefinitely.",
                )
                .with_authority(Some(MemoryAuthority::Decision)),
            )
            .unwrap()
            .id;

        const BODY: &str = "retries indefinitely";

        let before = memory_report(&fixture.runtime, "egret", false, 10).unwrap();
        assert!(before.contains(BODY), "{before}");

        let challenged =
            memory_challenge(&fixture.runtime, id.as_str(), "production_incident").unwrap();
        assert!(challenged.contains("needs_review"), "{challenged}");
        assert!(challenged.contains("production_incident"), "{challenged}");

        // No longer returned as current, settled knowledge.
        let after = memory_report(&fixture.runtime, "egret", false, 10).unwrap();
        assert!(
            !after.contains(BODY),
            "a challenged memory must not appear in a default search:\n{after}"
        );

        // Still reachable as history, with the reason recorded and readable.
        let history = memory_report(&fixture.runtime, "egret", true, 10).unwrap();
        assert!(history.contains(BODY), "{history}");
        assert!(history.contains("needs_review"), "{history}");
        assert!(
            history.contains("production_incident"),
            "the challenge reason must be readable in the history report:\n{history}"
        );

        // A reason that is not one of the six is refused, and nothing is
        // written.
        let refused = memory_challenge(&fixture.runtime, id.as_str(), "vibes");
        assert!(refused.is_err());
    }

    /// Phase 21G line 949, acceptance test 1 — the round trip the binary
    /// currently promises and cannot deliver: challenging a memory moves it
    /// to `needs-review` and out of every default search, and until this
    /// batch nothing could move it back. Enters through `memory_challenge`,
    /// `memory_report` and `memory_revalidate`, exactly what `glasshouse
    /// memory challenge`, `glasshouse memory search` and `glasshouse memory
    /// revalidate` run.
    #[test]
    fn a_challenged_memory_is_reaffirmed_back_into_default_search_with_a_fresh_validation() {
        use glasshouse::memory::{
            MemoryAuthority, MemoryKind, MemoryStatus, NewMemory, ProjectMemory,
        };

        let fixture = CliFixture::new();
        let project = ProjectMemory::open(&fixture.runtime).unwrap();
        let id = project
            .store()
            .record(
                NewMemory::new(
                    MemoryKind::Decision,
                    "The heron worker retries at most three times.",
                )
                .with_authority(Some(MemoryAuthority::Decision)),
            )
            .unwrap()
            .id;

        const BODY: &str = "retries at most three times";

        assert!(
            project
                .store()
                .get(&id)
                .unwrap()
                .unwrap()
                .last_validated_at
                .is_none(),
            "a freshly recorded memory has never been validated"
        );

        let before = memory_report(&fixture.runtime, "heron", false, 10).unwrap();
        assert!(before.contains(BODY), "{before}");

        memory_challenge(&fixture.runtime, id.as_str(), "project_state").unwrap();
        let after_challenge = memory_report(&fixture.runtime, "heron", false, 10).unwrap();
        assert!(
            !after_challenge.contains(BODY),
            "a challenged memory must drop out of a default search:\n{after_challenge}"
        );

        let revalidated = memory_revalidate(
            &fixture.runtime,
            id.as_str(),
            "reaffirmed",
            None,
            None,
            false,
        )
        .unwrap();
        assert!(revalidated.contains("active"), "{revalidated}");

        // Back in a default search.
        let after_revalidate = memory_report(&fixture.runtime, "heron", false, 10).unwrap();
        assert!(
            after_revalidate.contains(BODY),
            "a reaffirmed memory must return to a default search:\n{after_revalidate}"
        );

        // ... with a fresh validation timestamp and the matching status.
        let record = project.store().get(&id).unwrap().unwrap();
        assert_eq!(record.status, MemoryStatus::Active);
        assert!(
            record.last_validated_at.is_some(),
            "reaffirming must record a validation timestamp"
        );

        // An outcome that is not one of the four is refused.
        let refused = memory_revalidate(&fixture.runtime, id.as_str(), "vibes", None, None, false);
        assert!(refused.is_err());
    }

    /// Phase 21G line 950, acceptance test 4 — `--list` is bounded to
    /// `NeedsReview` memories and touches nothing. Enters through
    /// `memory_revalidate_list`, exactly what `glasshouse memory revalidate
    /// --list` runs. Wires `MemoryStore::with_status`, which had no
    /// production caller before this.
    #[test]
    fn revalidate_list_is_bounded_to_needs_review_memories_and_touches_nothing() {
        use glasshouse::memory::{
            MemoryKind, MemoryStatus, NewMemory, ProjectMemory, ReviewReason,
        };

        let fixture = CliFixture::new();
        let project = ProjectMemory::open(&fixture.runtime).unwrap();
        let store = project.store();

        let mut needing_review = Vec::new();
        for i in 0..3 {
            let record = store
                .record(NewMemory::new(
                    MemoryKind::Finding,
                    format!("egret finding {i}"),
                ))
                .unwrap();
            store
                .mark_for_review(&record.id, ReviewReason::ProjectState)
                .unwrap();
            needing_review.push(record.id);
        }
        // An active memory and an invalidated one: neither is waiting for
        // review, and neither may ever appear in the listing.
        store
            .record(NewMemory::new(MemoryKind::Finding, "an active finding"))
            .unwrap();
        let invalidated = store
            .record(NewMemory::new(
                MemoryKind::Finding,
                "an invalidated finding",
            ))
            .unwrap();
        store
            .set_status(&invalidated.id, MemoryStatus::Invalidated)
            .unwrap();

        let listing = memory_revalidate_list(&fixture.runtime, 2).unwrap();
        let lines: Vec<&str> = listing.lines().collect();
        assert_eq!(
            lines.len(),
            2,
            "the listing must not exceed --limit:\n{listing}"
        );
        for line in &lines {
            assert!(
                needing_review
                    .iter()
                    .any(|id| line.starts_with(id.as_str())),
                "every listed entry must be one of the needs-review memories: {line}"
            );
        }

        // Nothing was touched: every needs-review memory is still
        // needs-review, and the untouched active/invalidated rows are
        // unchanged.
        for id in &needing_review {
            assert_eq!(
                store.get(id).unwrap().unwrap().status,
                MemoryStatus::NeedsReview
            );
        }
        assert_eq!(
            store.get(&invalidated.id).unwrap().unwrap().status,
            MemoryStatus::Invalidated
        );
    }

    /// Phase 21F line 936, on the CLI's own text report — the machine door's
    /// half is `tests/memory_query_api.rs`, and this is the surface a person
    /// reads. A binding memory's validity and invalidation conditions are
    /// printed; a non-binding one's are not, even when the row carries them.
    #[test]
    fn the_report_prints_validity_and_invalidation_conditions_only_for_binding_memories() {
        use glasshouse::memory::{
            DecisionProvenance, MemoryAuthority, MemoryKind, NewMemory, ProjectMemory,
        };

        let fixture = CliFixture::new();
        let project = ProjectMemory::open(&fixture.runtime).unwrap();
        let store = project.store();
        store
            .record(
                NewMemory::new(
                    MemoryKind::Constraint,
                    "The kite export must be single-writer.",
                )
                .with_authority(Some(MemoryAuthority::Constraint))
                .with_provenance(DecisionProvenance {
                    rationale: Some("a partial file broke a downstream job".to_owned()),
                    ..DecisionProvenance::default()
                })
                .with_validity_conditions(Some("the export stays single-writer"))
                .with_invalidation_conditions(Some("the export gains concurrent writers")),
            )
            .unwrap();
        store
            .record(
                NewMemory::new(
                    MemoryKind::Finding,
                    "The kite export could maybe batch writes.",
                )
                .with_authority(Some(MemoryAuthority::Idea))
                .with_validity_conditions(Some("nobody has decided this yet")),
            )
            .unwrap();

        let report = memory_report(&fixture.runtime, "kite", false, 10).unwrap();
        assert!(
            report.contains("valid while  the export stays single-writer"),
            "{report}"
        );
        assert!(
            report.contains("invalid if   the export gains concurrent writers"),
            "{report}"
        );
        assert!(
            !report.contains("nobody has decided this yet"),
            "a non-binding memory's validity condition must not be printed:\n{report}"
        );
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
            report_hook_with(&fixture.runtime, id.as_str(), "Stop", move |_| {
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
                report_hook_with(&fixture.runtime, id.as_str(), event, move |_| {
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

            report_hook_with(&fixture.runtime, id.as_str(), "Stop", move |_| {
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
        report_hook_with(&fixture.runtime, id.as_str(), "Stop", |_| {
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

    /// Capability map line 1290, end to end inside the shipped binary: the
    /// command a person types records the override, and
    /// `disposable_extraction_model` — the function `report_hook` calls to
    /// build its extraction model — honours it for that session and denies
    /// the identical candidate for another.
    ///
    /// **Through `disposable_extraction_model` itself, not through a helper
    /// that reads the same configuration.** The first version of this test
    /// did the latter, and a mutation replacing the production wiring with
    /// `ReserveOverride::none()` SURVIVED all 44 tests in this binary:
    /// practice §35's exact shape, where the helper that makes a test
    /// convenient reproduces the production step the test claims to prove.
    /// The mutation is killed now, at the two assertions below.
    ///
    /// The candidate setup is
    /// `disposable_extraction_model_lets_the_protected_reserve_policy_deny_a_metered_candidate`'s,
    /// deliberately: a 10% remaining balance is inside the `Reserve` band and
    /// a 7200s reset is at or past `RESET_DISTANT_SECONDS`, so this is the
    /// combination the policy denies. Only the override differs between the
    /// two halves.
    #[test]
    fn the_reserve_override_a_user_records_reaches_the_routing_decision() {
        const VAR: &str = "GLASSHOUSE_TEST_ONLY_RESERVE_OVERRIDE_KEY";
        const PROVIDER: &str = "wire-reserve-override-test-provider";
        // SAFETY: `VAR` is unique to this test and removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }

        let fixture = CliFixture::new();
        let granted = recorded_session(&fixture.runtime);
        let other = recorded_session(&fixture.runtime);
        assert_ne!(granted, other);

        let mut user = UserConfig::load(fixture.runtime.paths()).unwrap();
        let mut provider = glasshouse::config::ProviderConfig::new("openai-compatible");
        provider.set_credential_env(vec![VAR.to_owned()]);
        provider.set_metered_models(vec!["a-reserved-model".to_owned()]);
        user.providers_mut().set(PROVIDER, provider);
        user.save(fixture.runtime.paths()).unwrap();

        let now_unix = glasshouse::provider::cache::now_unix_seconds();
        glasshouse::provider::telemetry::GatewayQuotaCache::new(fixture.runtime.paths()).store(
            PROVIDER,
            &glasshouse::provider::telemetry::RateLimitHeaders::read(vec![
                ("x-ratelimit-limit-requests", "1000"),
                ("x-ratelimit-limit-tokens", "1000"),
                ("x-ratelimit-remaining-requests", "100"),
                ("x-ratelimit-remaining-tokens", "100"),
                ("x-ratelimit-reset-requests", "7200s"),
                ("x-ratelimit-reset-tokens", "7200s"),
            ]),
            now_unix,
        );

        // Before the user says anything, this candidate is denied — which is
        // what makes the two assertions after the grant attributable.
        let before = disposable_extraction_model(&fixture.runtime, &granted).describe();

        let report = reserve_override_session(&fixture.runtime, granted.as_str(), false).unwrap();
        let after_granted = disposable_extraction_model(&fixture.runtime, &granted).describe();
        let after_other = disposable_extraction_model(&fixture.runtime, &other).describe();

        reserve_override_session(&fixture.runtime, granted.as_str(), true).unwrap();
        let after_clear = disposable_extraction_model(&fixture.runtime, &granted).describe();

        let recorded = UserConfig::load(fixture.runtime.paths()).unwrap();
        let project_file = fixture
            .runtime
            .project()
            .root()
            .join(".glasshouse")
            .join("config.toml");

        unsafe {
            std::env::remove_var(VAR);
        }

        assert!(
            before.contains("protected-reserve policy denied"),
            "the control case must be denied, or nothing below is attributable: {before}"
        );
        assert!(
            report.contains("No other session is affected"),
            "the command must say what it did not do, too: {report}"
        );

        assert!(
            !after_granted.contains("protected-reserve policy denied"),
            "the session the user named must be allowed to spend the reserve: {after_granted}"
        );
        assert!(
            after_granted.contains(granted.as_str()),
            "the explanation must name the session the override was granted for: {after_granted}"
        );

        // The assertion that separates line 1290 from "the reserve is off".
        assert!(
            after_other.contains("protected-reserve policy denied"),
            "a session the user never named must not inherit another session's override: \
             {after_other}"
        );

        assert!(
            after_clear.contains("protected-reserve policy denied"),
            "`--clear` must actually withdraw the override: {after_clear}"
        );

        // The write went to the user layer and nowhere near the repository.
        assert_eq!(
            recorded.routing().reserve_override_sessions(),
            Some([].as_slice())
        );
        assert!(
            !project_file.exists(),
            "recording a preference must not put a file in the user's repository"
        );
    }

    /// A session identifier no test here has recorded a reserve override
    /// for — capability map line 1290's *negative* case, and the state every
    /// one of these tests was in before that line existed.
    ///
    /// Named rather than inlined so that a test which ever *does* want the
    /// override has to say so, and so a reader of these tests can see at a
    /// glance that the reserve policy below is deciding without one.
    fn a_session_not_overridden() -> glasshouse::session::SessionId {
        glasshouse::session::SessionId::new("a-session-with-no-reserve-override")
    }

    /// Phase 9I lines 530, 531 and 540, at the function `report_hook` itself
    /// calls to get its model. A user-configured free model, written to disk
    /// exactly as Settings would write it, is the one the disposable routing
    /// policy names, and the description says plainly that no model was
    /// called.
    ///
    /// **Not through `report_hook`'s own log line.** `run_extraction`
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

        let model = disposable_extraction_model(&fixture.runtime, &a_session_not_overridden());
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

        let model = disposable_extraction_model(&fixture.runtime, &a_session_not_overridden());
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

    /// Map lines 1293 and 1550, at `disposable_extraction_model` itself —
    /// the same production entry point the two tests above use, not a
    /// `routing::disposable` type constructed by hand (§35). A provider that
    /// named only a [`glasshouse::config::ProviderConfig::metered_models`]
    /// entry, with no telemetry cached (so the reserve gate sees the least
    /// protective band, [`glasshouse::provider::quota::CapacityBand::Plenty`]),
    /// is reachable and chosen: `disposable_candidates` built a real metered
    /// candidate, and Phase 32F's `evaluate_reserve_spend` ran and allowed it.
    #[test]
    fn disposable_extraction_model_falls_back_to_a_configured_metered_model_when_permitted() {
        const VAR: &str = "GLASSHOUSE_TEST_ONLY_WIRE_DISPOSABLE_METERED_KEY";
        // SAFETY: `VAR` is unique to this test and removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }

        let fixture = CliFixture::new();
        let mut user = UserConfig::load(fixture.runtime.paths()).unwrap();
        let mut provider = glasshouse::config::ProviderConfig::new("openai-compatible");
        provider.set_credential_env(vec![VAR.to_owned()]);
        provider.set_metered_models(vec!["an-expensive-model".to_owned()]);
        user.providers_mut()
            .set("wire-disposable-metered-test-provider", provider);
        user.save(fixture.runtime.paths()).unwrap();

        let model = disposable_extraction_model(&fixture.runtime, &a_session_not_overridden());
        let described = model.describe();

        unsafe {
            std::env::remove_var(VAR);
        }

        assert!(
            described.contains("an-expensive-model"),
            "the metered model the user named must be the one chosen: {described}"
        );
        assert!(
            described.contains("metered"),
            "the candidate must be reported as metered, never as free: {described}"
        );
        assert!(
            described.contains("protected-reserve policy"),
            "Phase 32F's gate must have actually run and left its reasoning in the \
             explanation: {described}"
        );
        assert!(
            described.contains("no model was called"),
            "Phase 39 does not exist yet: {described}"
        );
    }

    /// Line 533's load-bearing half, through the real production caller
    /// rather than a hand-built `routing::disposable` policy: a provider
    /// that named both a free and a metered model still yields the free one,
    /// however the metered candidate would otherwise have scored.
    #[test]
    fn disposable_extraction_model_prefers_a_free_model_over_a_configured_metered_one() {
        const VAR: &str = "GLASSHOUSE_TEST_ONLY_WIRE_DISPOSABLE_BOTH_KEY";
        // SAFETY: `VAR` is unique to this test and removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }

        let fixture = CliFixture::new();
        let mut user = UserConfig::load(fixture.runtime.paths()).unwrap();
        let mut provider = glasshouse::config::ProviderConfig::new("openai-compatible");
        provider.set_credential_env(vec![VAR.to_owned()]);
        provider.set_free_models(vec!["nvidia/nemotron-nano-9b-v2:free".to_owned()]);
        provider.set_metered_models(vec!["an-expensive-model".to_owned()]);
        user.providers_mut()
            .set("wire-disposable-both-test-provider", provider);
        user.save(fixture.runtime.paths()).unwrap();

        let model = disposable_extraction_model(&fixture.runtime, &a_session_not_overridden());
        let described = model.describe();

        unsafe {
            std::env::remove_var(VAR);
        }

        assert!(
            described.contains("nvidia/nemotron-nano-9b-v2:free"),
            "free capacity must win whenever any can serve, however plentiful the reserve: \
             {described}"
        );
        assert!(
            !described.contains("an-expensive-model"),
            "the metered candidate must not be the one chosen while a free one can serve: \
             {described}"
        );
    }

    /// Map line 1550, denying rather than allowing, through the real capacity
    /// telemetry `disposable_candidate_capacity` reads — not a hand-built
    /// `CandidateCapacity` (that proof already exists in
    /// `routing::disposable::tests`; this one proves the real reading reaches
    /// the gate). A remaining balance of 10% falls inside
    /// `CapacityBandThresholds::DEFAULT`'s `Reserve` band (above 2%, at or
    /// below 15%), and a reset 7200s away is `RESET_DISTANT_SECONDS` or
    /// further — the same combination `routing::disposable::tests::the_protected_reserve_policy_gates_the_metered_fallback`
    /// denies, reached here through `disposable_extraction_model` end to end.
    #[test]
    fn disposable_extraction_model_lets_the_protected_reserve_policy_deny_a_metered_candidate() {
        const VAR: &str = "GLASSHOUSE_TEST_ONLY_WIRE_DISPOSABLE_DENIED_KEY";
        const PROVIDER: &str = "wire-disposable-denied-test-provider";
        // SAFETY: `VAR` is unique to this test and removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }

        let fixture = CliFixture::new();
        let mut user = UserConfig::load(fixture.runtime.paths()).unwrap();
        let mut provider = glasshouse::config::ProviderConfig::new("openai-compatible");
        provider.set_credential_env(vec![VAR.to_owned()]);
        provider.set_metered_models(vec!["a-reserved-model".to_owned()]);
        user.providers_mut().set(PROVIDER, provider);
        user.save(fixture.runtime.paths()).unwrap();

        let now_unix = glasshouse::provider::cache::now_unix_seconds();
        glasshouse::provider::telemetry::GatewayQuotaCache::new(fixture.runtime.paths()).store(
            PROVIDER,
            &glasshouse::provider::telemetry::RateLimitHeaders::read(vec![
                ("x-ratelimit-limit-requests", "1000"),
                ("x-ratelimit-limit-tokens", "1000"),
                ("x-ratelimit-remaining-requests", "100"),
                ("x-ratelimit-remaining-tokens", "100"),
                ("x-ratelimit-reset-requests", "7200s"),
                ("x-ratelimit-reset-tokens", "7200s"),
            ]),
            now_unix,
        );

        let model = disposable_extraction_model(&fixture.runtime, &a_session_not_overridden());
        let described = model.describe();

        unsafe {
            std::env::remove_var(VAR);
        }

        assert!(
            described.contains("protected-reserve policy denied"),
            "a Reserve-band candidate with a distant reset and no cheaper alternative must be \
             denied: {described}"
        );
        assert!(
            described.contains("a-reserved-model"),
            "the denial must name the candidate it refused: {described}"
        );
        assert!(
            described.contains("no model was called"),
            "a refusal is still not a call: {described}"
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

    /// Map line 779, at the surface a person actually runs: `glasshouse
    /// memory extract` in a real Git repository records the commit the
    /// project was standing at, resolved with the same
    /// `GitPosition::detect` reading that a checkpoint uses. The fixture
    /// hand-writes `HEAD`/`refs` the way `checkpoint/git.rs`'s own tests do,
    /// rather than shelling out to `git`, and a sanity assertion proves that
    /// setup actually produces the commit before the extraction runs at all.
    #[test]
    fn manual_extraction_in_a_git_repository_records_the_head_commit() {
        use glasshouse::memory::ProjectMemory;
        use glasshouse::memory::search::SearchScope;

        const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

        let fixture = CliFixture::new();
        let git_dir = fixture._workspace.path().join(".git");
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        std::fs::write(git_dir.join("refs/heads/main"), format!("{COMMIT}\n")).unwrap();
        assert_eq!(
            GitPosition::detect(fixture.runtime.project().root()).map(|position| position.commit),
            Some(COMMIT.to_owned()),
            "fixture setup must produce the commit this test checks for"
        );

        let dir = tempfile::tempdir().unwrap();
        let activity = dir.path().join("activity.txt");
        let reply = dir.path().join("reply.json");
        std::fs::write(&activity, "we settled on a checkpoint format\n").unwrap();
        std::fs::write(
            &reply,
            r#"{"memories":[{"kind":"finding","authority":"historical",
                 "disposition":"accepted","support":"established",
                 "confidence":"certain",
                 "body":"A checkpoint fixture proved the commit round-trips."}]}"#,
        )
        .unwrap();

        let report =
            memory_extract(&fixture.runtime, "s-1", Some(&activity), false, &reply).unwrap();
        assert!(report.contains("stored 1"), "{report}");

        let stored = ProjectMemory::open(&fixture.runtime)
            .unwrap()
            .store()
            .search("round-trips", SearchScope::Current, 10)
            .unwrap();
        assert_eq!(stored.len(), 1, "the memory reached the real store");
        assert_eq!(
            stored[0].source_commit.as_deref(),
            Some(COMMIT),
            "the stored memory must carry the repository's head commit"
        );
    }

    /// The other half of map line 779: a project that is not a Git
    /// repository still extracts normally, with no commit recorded rather
    /// than an error. `CliFixture` gives an empty `.git` directory with no
    /// `HEAD` — the "unreadable HEAD" case `GitPosition::detect` folds into
    /// the same `None` as "no repository at all".
    #[test]
    fn manual_extraction_outside_a_repository_stores_no_commit_and_does_not_error() {
        use glasshouse::memory::ProjectMemory;
        use glasshouse::memory::search::SearchScope;

        let fixture = CliFixture::new();
        assert_eq!(
            GitPosition::detect(fixture.runtime.project().root()),
            None,
            "fixture setup sanity: an empty .git directory has no readable HEAD"
        );

        let dir = tempfile::tempdir().unwrap();
        let activity = dir.path().join("activity.txt");
        let reply = dir.path().join("reply.json");
        std::fs::write(&activity, "we settled on a checkpoint format\n").unwrap();
        std::fs::write(
            &reply,
            r#"{"memories":[{"kind":"finding","authority":"historical",
                 "disposition":"accepted","support":"established",
                 "confidence":"certain",
                 "body":"A non-repository extraction stores no commit."}]}"#,
        )
        .unwrap();

        let report = memory_extract(&fixture.runtime, "s-1", Some(&activity), false, &reply);
        assert!(
            report.is_ok(),
            "extraction outside a repository must not error: {report:?}"
        );
        assert!(report.unwrap().contains("stored 1"));

        let stored = ProjectMemory::open(&fixture.runtime)
            .unwrap()
            .store()
            .search("non-repository", SearchScope::Current, 10)
            .unwrap();
        assert_eq!(stored.len(), 1, "the memory reached the real store");
        assert_eq!(stored[0].source_commit, None);
    }

    /// Line 1641, exercised at the surface a person actually runs. Against
    /// `checkpoint_command` itself, not a hand-built `Handoff` — a
    /// `skip-state-update` mutation that quietly replaced
    /// `binding_memory_lines(runtime)` with `Vec::new()` would be invisible to
    /// any test that only exercises `Checkpoint`/`Handoff` directly, because
    /// those never call `ProjectMemory` at all. This is the caller §35 asks
    /// for: the one the shipped binary actually reaches.
    #[test]
    fn checkpoint_save_carries_binding_project_memory_into_the_handoff() {
        use glasshouse::memory::{MemoryAuthority, MemoryKind, NewMemory, ProjectMemory};

        let fixture = CliFixture::new();
        let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
        let session = sessions
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();

        ProjectMemory::open(&fixture.runtime)
            .unwrap()
            .store()
            .record(
                NewMemory::new(
                    MemoryKind::Constraint,
                    "never store secrets in a checkpoint",
                )
                .with_authority(Some(MemoryAuthority::Constraint)),
            )
            .unwrap();
        // Present in the project, but never binding — must not leak in.
        ProjectMemory::open(&fixture.runtime)
            .unwrap()
            .store()
            .record(NewMemory::new(
                MemoryKind::Finding,
                "the CI runner is slow on Mondays",
            ))
            .unwrap();

        let command = CheckpointCommand::Save {
            objective: "prove project memory reaches the handoff".to_owned(),
            state: "wiring checkpoint_command to ProjectMemory".to_owned(),
            session: Some(session.id.as_str().to_owned()),
            decisions: Vec::new(),
            failed_approaches: Vec::new(),
            files: Vec::new(),
            tests: None,
            next_actions: Vec::new(),
        };
        let status = checkpoint_command(&fixture.runtime, &command).unwrap();
        assert_eq!(status, ExitCode::SUCCESS);

        let checkpoints = ProjectCheckpoints::open(&fixture.runtime).unwrap();
        let stored = checkpoints.store().list().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(
            stored[0].checkpoint.handoff.memory,
            vec!["never store secrets in a checkpoint".to_owned()],
            "the binding memory must reach the checkpoint's handoff, and the \
             unclassified one must not"
        );
        assert!(
            stored[0]
                .checkpoint
                .bootstrap_prompt()
                .contains("RELEVANT MEMORY"),
            "the bootstrap prompt must carry it forward"
        );
    }

    /// A checkpoint with no memory section is strictly better than a
    /// checkpoint that never happened: `binding_memory_lines` degrades to an
    /// empty list rather than propagating, even when the project's memory
    /// database cannot be opened at all. Unix-only because the failure is
    /// forced through a permission bit; the guard itself is not platform
    /// specific.
    #[cfg(unix)]
    #[test]
    fn binding_memory_lines_degrades_to_empty_when_the_database_cannot_be_opened() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = CliFixture::new();
        // Force the project database open, so the file exists, then take
        // away every permission on it — `ProjectMemory::open` must now fail.
        glasshouse::session::ProjectSessions::open(&fixture.runtime).unwrap();
        std::fs::set_permissions(
            fixture.runtime.database_path(),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();

        let lines = binding_memory_lines(&fixture.runtime);

        assert_eq!(
            lines,
            Vec::<String>::new(),
            "an unopenable database must degrade to no memory, not panic"
        );

        // Restore permissions so the fixture's own directories can still be
        // cleaned up on drop.
        std::fs::set_permissions(
            fixture.runtime.database_path(),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }

    // -----------------------------------------------------------------------
    // GH-ROUTER-TASK-INPUT — `task_requirements_from_text` at its real call
    // site.
    //
    // No built-in provider template in this codebase ever declares a
    // protocol's tool calls `Declared::verified(false, ..)` — every one of
    // `provider::templates()`'s entries (including the two generic
    // `openai-compatible`/`anthropic-compatible` templates) uses
    // `unverified_support`, and `config::pairing::pairing_for_profile` leaves
    // `tool_calls` at its `Declared::Unverified` default for `Native` and
    // `GlasshouseGateway` profiles too. So `Backend::tools() ==
    // ToolSemantics::KnownAbsent` is unreachable through `glasshouse route`'s
    // compiled-binary path today, and acceptance test 4 cannot be written as
    // a `tests/route_command.rs` black-box run the way tests 1, 2, 3 and 5
    // are — see `packet_errors` in this packet's report.
    //
    // This proves the same claim the way `tests/session_router.rs` proves
    // every other hard-constraint gate: by calling `SessionRouter::choose`
    // directly, through `task_requirements_from_text`, the actual function
    // `route_report` calls at its `RouterInputs` construction site. Mutation
    // (b) — hardcoding `needs_tool_calls: false` back into that function —
    // fails this test, because the `KnownAbsent` destination would stop being
    // rejected for a task that plainly asks for shell execution.
    #[test]
    fn a_task_implying_tool_use_reaches_the_hard_constraint_gate_through_the_real_call_site() {
        use glasshouse::integrations::IntegrationId;
        use glasshouse::routing::free::FreePool;
        use glasshouse::routing::session::{
            Destination, RouterInputs, RoutingMoment, SessionRouter,
        };
        use glasshouse::routing::{AssignedModel, Backend, Cost, CredentialId, ToolSemantics};
        use glasshouse::secret::SecretRef;
        use std::time::Instant;

        fn backend_with_tools(tools: ToolSemantics) -> Backend {
            Backend::new(
                "anthropic",
                "anthropic-messages",
                AssignedModel::named("claude-opus-4"),
                CredentialId::new(
                    "anthropic",
                    SecretRef::Environment {
                        var: "ANTHROPIC_API_KEY".to_owned(),
                    },
                ),
                Cost::Metered,
                tools,
            )
        }

        let overrides = glasshouse::harness::pairing::PairingOverrides::from_parts(
            "no configuration",
            std::collections::BTreeMap::new(),
            std::collections::BTreeMap::new(),
        );
        let health = FreePool::new();
        let now = Instant::now();

        let known_absent = Destination::fresh(
            "known-absent",
            IntegrationId::ClaudeCode,
            "default",
            backend_with_tools(ToolSemantics::KnownAbsent),
            None,
        );
        let unverified = Destination::fresh(
            "unverified",
            IntegrationId::ClaudeCode,
            "default",
            backend_with_tools(ToolSemantics::Unverified),
            None,
        );

        // A task this heuristic reads as needing shell execution: the same
        // call `route_report` makes on real `--task` text.
        let tool_use_requirements =
            task_requirements_from_text(Some("run cargo test and fix whatever fails"));
        assert!(
            tool_use_requirements.needs_tool_calls,
            "a task naming shell execution must derive `needs_tool_calls: true`"
        );
        let inputs = RouterInputs {
            overrides: &overrides,
            health: &health,
            now,
            requirements: tool_use_requirements,
        };
        let routed = SessionRouter::new()
            .choose(
                RoutingMoment::SessionStart,
                None,
                &[known_absent.clone(), unverified.clone()],
                &inputs,
            )
            .expect("destinations were offered");
        assert_eq!(
            routed.chosen().id(),
            "unverified",
            "a task needing tool calls must not be sent where they are established absent"
        );
        assert_eq!(routed.rejected().len(), 1);
        assert_eq!(
            routed.rejected()[0].1,
            glasshouse::routing::HardConstraint::ToolSemantics
        );

        // The absent-`--task` behaviour: `needs_tool_calls` stays `false`,
        // and the same `known-absent` destination is no longer rejected.
        let no_task_requirements = task_requirements_from_text(None);
        assert!(!no_task_requirements.needs_tool_calls);
        let inputs = RouterInputs {
            overrides: &overrides,
            health: &health,
            now,
            requirements: no_task_requirements,
        };
        let routed = SessionRouter::new()
            .choose(
                RoutingMoment::SessionStart,
                None,
                &[known_absent, unverified],
                &inputs,
            )
            .expect("destinations were offered");
        assert!(
            routed.rejected().is_empty(),
            "with no task text, nothing needs tool calls, so nothing is rejected on that \
             ground: {:?}",
            routed.rejected()
        );
    }
}
