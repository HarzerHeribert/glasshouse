use std::ffi::OsString;
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use std::io::IsTerminal;

use glasshouse::checkpoint::git::GitPosition;
use glasshouse::checkpoint::{
    Checkpoint, CheckpointReason, CheckpointStore, Handoff, ProjectCheckpoints, Stored,
    WorkingTreeStatus,
};
use glasshouse::cli::{ApiCommand, CheckpointCommand, ContextFirewallCommand, McpCommand};
use glasshouse::config::response::{ResponseProfileEntry, ResponseRequest};
use glasshouse::config::{self, EffectiveConfig, ProjectConfig, UserConfig};
use glasshouse::events::{
    EventBus, EventLog, LifecycleEvent, Observation, ProcessExit, TurnOutcome,
};
use glasshouse::guardrails::{AssumptionStore, GuardrailOverride};
use glasshouse::integrations::{Discovery, cmux};
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
        Some(Command::Entitlements) => {
            print!("{}", entitlements_report(&runtime)?);
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
            force,
        }) => {
            print!(
                "{}",
                resources_report(&runtime, *verbose, probe, *no_harness, *force)?
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
            //
            // Line 1469: the same text-keyed cache `classify_for_routing`
            // consults, best-effort on a configuration re-read exactly like
            // `forbidden_providers` does — this command has no `EffectiveConfig`
            // of its own to reuse.
            let text_cache =
                ClassificationTextCache::new(runtime.paths(), runtime.project().id().as_str());
            let text_key = glasshouse::routing::request::normalised_task_key(&request);
            let resolution_tag = match (
                UserConfig::load(runtime.paths()),
                config::load_project_config(runtime.project()),
            ) {
                (Ok(user), Ok(project)) => {
                    let effective = EffectiveConfig::new(&user, project.as_ref());
                    classification_cache_resolution_tag(&effective.routing_model_resolution().value)
                }
                _ => {
                    tracing::debug!(
                        "could not re-read configuration for the classification text cache"
                    );
                    None
                }
            };
            let no_fingerprint = glasshouse::routing::request::RoutingFingerprint::new(
                None,
                &[],
                std::iter::empty::<String>(),
            );
            let cached = resolution_tag.as_deref().and_then(|tag| {
                let record = text_cache.lookup(&text_key)?;
                let now = glasshouse::provider::cache::now_unix_seconds();
                record
                    .is_reusable_for(now, &no_fingerprint, tag)
                    .then(|| record.classification())
                    .flatten()
            });
            let model_output = match cached {
                Some(classification) => Some(classification),
                None => match classify_with_routing_model(
                    &runtime,
                    &glasshouse::routing::request::RouterRequest::for_text(&request),
                ) {
                    ClassificationAttempt::NotConfigured => None,
                    ClassificationAttempt::Answered(classification) => {
                        if let Some(tag) = resolution_tag.as_deref() {
                            text_cache.store(
                                glasshouse::routing::request::CachedClassification::new(
                                    text_key.clone(),
                                    no_fingerprint.clone(),
                                    tag,
                                    &classification,
                                    glasshouse::provider::cache::now_unix_seconds(),
                                ),
                            );
                        }
                        Some(classification)
                    }
                    ClassificationAttempt::Failed(why) => {
                        eprintln!("glasshouse: {why}; deterministic heuristics answered instead");
                        None
                    }
                },
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
        Some(Command::ContextFirewall { command }) => match command {
            ContextFirewallCommand::Hook {
                passthrough_tokens,
                min_semantic_tokens,
                task,
                tools,
                emit_updated_output,
                mode,
            } => {
                let mode = match glasshouse::config::firewall::FirewallMode::from_stored(
                    mode.trim(),
                ) {
                    Some(mode) => mode,
                    None => {
                        eprintln!(
                            "glasshouse: `{mode}` is not a context-firewall mode; use one of {}",
                            glasshouse::config::firewall::FirewallMode::spellings()
                        );
                        return Ok(ExitCode::FAILURE);
                    }
                };
                context_firewall_hook(
                    &runtime,
                    *passthrough_tokens,
                    *min_semantic_tokens,
                    task,
                    tools,
                    *emit_updated_output,
                    mode,
                )?;
            }
            ContextFirewallCommand::Show {
                id,
                candidate,
                file,
                range,
                stats,
            } => {
                if *stats {
                    match context_firewall_show_stats(&runtime, id)? {
                        Some(summary) => print!("{summary}"),
                        None => {
                            eprintln!(
                                "glasshouse: no context-firewall raw result stored under `{id}`"
                            );
                            return Ok(ExitCode::FAILURE);
                        }
                    }
                } else {
                    let request = if let Some(candidate_id) = candidate {
                        ExpansionRequest::Candidate(*candidate_id)
                    } else if let Some(file) = file {
                        ExpansionRequest::File(file.clone())
                    } else if let Some(range) = range {
                        match parse_line_range(range) {
                            Ok(bounds) => ExpansionRequest::Range(bounds),
                            Err(reason) => {
                                eprintln!("glasshouse: {reason}");
                                return Ok(ExitCode::FAILURE);
                            }
                        }
                    } else {
                        ExpansionRequest::Whole
                    };
                    match context_firewall_show(&runtime, id, request)? {
                        ExpansionOutcome::Content(content) => print!("{content}"),
                        ExpansionOutcome::NotFound => {
                            eprintln!(
                                "glasshouse: no context-firewall raw result stored under `{id}`"
                            );
                            return Ok(ExitCode::FAILURE);
                        }
                        ExpansionOutcome::Refused(reason) => {
                            eprintln!("glasshouse: {reason}");
                            return Ok(ExitCode::FAILURE);
                        }
                    }
                }
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
            Some(SessionCommand::Focus { session }) => match focus_session(&runtime, session) {
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
            Some(SessionCommand::Restyle {
                session,
                profile,
                accept_loss,
            }) => {
                if let Err(err) = restyle_session(&runtime, session, profile, *accept_loss) {
                    eprintln!("glasshouse: {err:#}");
                    return Ok(ExitCode::FAILURE);
                }
            }
            Some(SessionCommand::Tell {
                session,
                instruction,
            }) => {
                if let Err(err) = tell_session(&runtime, session, instruction) {
                    eprintln!("glasshouse: {err:#}");
                    return Ok(ExitCode::FAILURE);
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
            no_routing,
            checkpoint_first,
            headless,
            no_memory,
            task,
            guardrail,
            presentation,
            presentation_ref,
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
            no_routing,
            checkpoint_first,
            headless,
            no_memory,
            task,
            guardrail,
            presentation,
            presentation_ref,
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
            // Phase 21K line 1008: refused here, before anything is
            // resolved or recorded, so a misspelt override costs nothing.
            let guardrail = match guardrail.as_deref().map(parse_guardrail_override) {
                Some(Ok(kind)) => Some(kind),
                Some(Err(err)) => {
                    eprintln!("glasshouse: {err}");
                    return Ok(ExitCode::FAILURE);
                }
                None => None,
            };
            // Phase 17: the pane's command is assembled here, from the raw
            // flags, because this is the one place that has all of them —
            // `launch_session` receives the launch already interpreted.
            let external = match external_presentation(
                presentation.as_deref(),
                presentation_ref.as_deref(),
                || {
                    let executable = std::env::current_exe()?;
                    let launch = pane_launch_args(PaneLaunch {
                        harness: harness.as_deref(),
                        response_profile: response_profile.as_deref(),
                        response_role: response_role.as_deref(),
                        profile: profile.as_deref(),
                        from_checkpoint: from_checkpoint.as_deref(),
                        to: to.as_deref(),
                        fresh: *fresh,
                        headless: *headless,
                        harness_args,
                    });
                    Ok(cmux::pane_command(
                        &executable,
                        &pane_global_args(cli, &runtime),
                        &launch,
                    ))
                },
            ) {
                Ok(external) => external,
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
                    task: task.as_deref(),
                    no_routing: *no_routing,
                    checkpoint_first: *checkpoint_first,
                },
                &response,
                *headless,
                *no_memory,
                external,
                harness_args,
                guardrail,
            );
        }
        Some(Command::Resume {
            session,
            checkpoint_first,
            harness_args,
        }) => {
            // Line 1716's resume half. Taken here, before `resume_session`
            // opens anything: the session being left is whichever this
            // project was most recently in, and that is a fact about the
            // store rather than about the resume, so it is established and
            // its handle closed before the resume's own is opened (practice
            // §65).
            if *checkpoint_first {
                checkpoint_before_moving(&runtime, Some(session))?;
            }
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
            MemoryCommand::Commit { session } => {
                print!("{}", memory_commit(&runtime, session.as_deref())?);
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
            MemoryCommand::Export {
                tracked,
                include_findings,
                dry_run,
            } => {
                print!(
                    "{}",
                    memory_export_tracked(&runtime, *tracked, *include_findings, *dry_run)?
                );
            }
            MemoryCommand::ExportLocal {
                harness,
                limit,
                no_exclude,
            } => {
                print!(
                    "{}",
                    memory_export_local(&runtime, harness.as_deref(), *limit, !*no_exclude)?
                );
            }
            MemoryCommand::Rate {
                id,
                verdict,
                session,
                note,
            } => {
                print!(
                    "{}",
                    memory_rate(&runtime, id, *verdict, session.as_deref(), note.as_deref())?
                );
            }
            MemoryCommand::Retrievals { hours } => {
                print!("{}", memory_retrievals_report(&runtime, *hours)?);
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
        // Phase 21K line 1048: what agents have stated, and what became of
        // it — read from the ledger, never inferred from a transcript.
        Some(Command::Assumptions { session, limit }) => {
            match assumptions_report(&runtime, session.as_deref(), *limit) {
                Ok(report) => print!("{report}"),
                Err(err) => {
                    eprintln!("glasshouse: {err:#}");
                    return Ok(ExitCode::FAILURE);
                }
            }
        }
        // Constant text: no runtime, no store, no configuration. See
        // `glasshouse::policy` for why the policy is text Glasshouse carries
        // rather than a check Glasshouse runs, and `cli::Command::Policy` for
        // why this prints the delivered form rather than a prettier one.
        Some(Command::Policy { part }) => {
            println!("{}", glasshouse::policy::render(*part));
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
            // Line 1717's two verbs. Client-side they are the same shape as
            // the three above: one connection, one request, the door's own
            // sentence back.
            ApiCommand::Mute { session, seconds } => {
                api::mute(&runtime, session, *seconds)?;
            }
            ApiCommand::Unmute { session } => {
                api::unmute(&runtime, session)?;
            }
        },
        // The MCP door — the same handlers `api serve` answers with, over
        // stdio, bound to the project `runtime` was resolved for exactly as
        // every other arm here is. See `api::mcp` for the ruling it
        // implements.
        Some(Command::Mcp { command }) => match command {
            McpCommand::Serve => api::serve_mcp(&runtime)?,
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
    /// `--task`: what the work is, which decides what the destination must
    /// be able to do (Phase 34D). `None` classifies nothing and reproduces
    /// the launch exactly as it was before classification existed.
    task: Option<&'a str>,
    /// `--no-routing`: take no routing decision for this launch at all —
    /// capability map line 1712.
    ///
    /// Not a fifth way of naming a destination, which is why it sits here
    /// beside them rather than being folded into one: the four fields above
    /// say *where*, and this one says *stop deciding*. With it set, the four
    /// above are still read and still obeyed — a person who says both "do
    /// not rank" and "go here" has said two compatible things.
    no_routing: bool,
    /// `--checkpoint-first`: check point the session this work is leaving
    /// before it moves — capability map line 1716.
    checkpoint_first: bool,
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
) -> glasshouse::routing::Entitlement {
    use glasshouse::config::{EntitlementModels, TelemetryScope};
    use glasshouse::routing::{EntitlementModelsFacet, EntitlementThrottleFacet};

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

/// 56A line 1969's binding half, beside line 1973's child-env scrub: the
/// launch's secret store with every **foreign** entitlement's credential
/// reference refused. `profile::resolve` binds a direct-provider launch to
/// "the first credential reference that currently resolves" out of the
/// provider's declared pool — a rule written before the broker existed —
/// so with the pool brokered, the resolution the overlay sees must only be
/// able to answer with the serving account's own reference, or the process
/// would authenticate as whichever account is listed first while the
/// announcement names another. Same filter as
/// `EffectiveConfig::foreign_entitlement_credential_refs`, wrapped rather
/// than re-derived; everything that is not an entitlement credential
/// resolves exactly as before.
struct EntitlementScopedSecrets<'a> {
    inner: &'a dyn glasshouse::secret::SecretStore,
    foreign: Vec<glasshouse::secret::SecretRef>,
}

impl glasshouse::secret::SecretStore for EntitlementScopedSecrets<'_> {
    fn resolve(
        &self,
        reference: &glasshouse::secret::SecretRef,
    ) -> Option<glasshouse::secret::Secret> {
        if self.foreign.contains(reference) {
            return None;
        }
        self.inner.resolve(reference)
    }

    fn is_present(&self, reference: &glasshouse::secret::SecretRef) -> bool {
        // The same answer `resolve` gives, without producing a value: a
        // foreign account's credential is not present *to this launch*.
        !self.foreign.contains(reference) && self.inner.is_present(reference)
    }

    fn describe(&self) -> &'static str {
        // The underlying store's own label: this wrapper narrows which
        // references answer, not where values come from, and a diagnostic
        // naming a store the user has never heard of would mislead.
        self.inner.describe()
    }
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
fn announce_entitlement(
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
fn entitlement_refusal_message(
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
fn routing_destinations(
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
    let sticky =
        ClassificationStickyCache::new(runtime.paths(), runtime.project().id().as_str()).load();
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
        // Phase 56 line 1954's producer: the entitlement this session's
        // profile charges, so a rule the user has since written applies to
        // continuing it exactly as it applies to starting a fresh one. When
        // SEVERAL pool entries back the session's provider, no record says
        // which account actually served it — the serving account of an
        // existing session is 56A-4's rebinding ground — so the destination
        // honestly carries none rather than a guess.
        let matches = pool_entitlements_for(&pool, harness, &profile.backend);
        let entitlement = match matches.as_slice() {
            [only] => Some(routing_entitlement(only, &band_thresholds)),
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
            .with_pairing_prior_evidence(pairing_prior_evidence),
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
                    .with_entitlement(Some(routing_entitlement(resolved, &band_thresholds)))
                    .with_estimated_input_size(fresh_estimated_size)
                    .with_resource_facts(resource_facts)
                    .with_pairing_prior_evidence(pairing_prior_evidence),
                );
            }
        } else {
            let ceiling = destination_tier_ceiling(effective, &backend, &query);
            let pairing_prior_evidence = pairing_prior_evidence_count(consumption, &backend);
            let fresh_entitlement = matches
                .first()
                .map(|resolved| routing_entitlement(resolved, &band_thresholds));
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
                .with_pairing_prior_evidence(pairing_prior_evidence),
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
fn session_checkpoint_tokens(
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
fn estimated_project_memory_tokens(runtime: &Runtime, task: &str) -> Option<u64> {
    use glasshouse::memory::ProjectMemory;
    use glasshouse::memory::inject::BriefingOutcome;

    let project = ProjectMemory::open(runtime).ok()?;
    let outcome = glasshouse::memory::inject::briefing(
        &project.store(),
        task,
        &std::collections::HashSet::new(),
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
        Some(BriefingOutcome::NothingNew) | None => None,
    }
}

/// Map line 1304's checkpoint component of a fresh-session cost estimate:
/// the rendered size of the project's own latest checkpoint — the same
/// document [`latest_checkpoint_quality`] reads its quality facts from,
/// measured rather than modeled. `None` when this project has no checkpoint
/// at all.
fn latest_checkpoint_tokens(runtime: &Runtime) -> Option<u64> {
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

/// The cost class of the destination a launch actually routed to — map line
/// 1835's *"low-cost or free route"* versus *"the premium route it
/// displaced"*, as a fact rather than a guess.
///
/// # Why this is not `destination.backend().cost()`
///
/// [`destination_backend`] hardcodes `Cost::Metered` for every destination it
/// builds, and says so: the session router reads a backend's provider,
/// credential, model and tool semantics and never its cost, so the field is
/// the fail-closed constant rather than a measurement. Recording *that* as a
/// route's class would give line 1835 one bucket for ever and report a
/// tautology.
///
/// So the class is read where the fact actually lives:
/// [`ProviderConfig::cost_of`], the same one lookup `disposable_candidates`
/// and `gateway_upstream` use, applied to the destination's own provider and
/// model with the project layer winning over the user layer. `glasshouse::
/// profile` and `glasshouse::routing` may not import `glasshouse::config`, so
/// main.rs is where this can be answered at all.
///
/// # `None` is the third answer, and it is honest
///
/// A destination on a harness's own sign-in names no configured provider, and
/// a gateway-backed one assigns its model when the session starts. Neither
/// has a marked cost, and Glasshouse does not know what a subscription costs
/// at the margin. That is recorded as
/// [`glasshouse::evaluation::UNKNOWN_COST_CLASS`] and counted in its own
/// bucket — never folded into `metered`, which would be a number nobody
/// measured.
fn routed_cost_class(
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    destination: &glasshouse::routing::session::Destination,
) -> Option<glasshouse::routing::Cost> {
    let model = destination.backend().model().name()?;
    let provider = destination.backend().provider();
    let config = project
        .and_then(|project| project.providers().get(provider))
        .or_else(|| user.providers().get(provider))?;
    Some(config.cost_of(model))
}

/// Whether the pool this launch handed the router held any observed health
/// reading for the destination it chose — map line 1854's *sparse* half.
///
/// The key is built exactly as [`observed_provider_health`] builds it, from
/// the destination's own credential and model label, so a hit here means the
/// same resource and not a resource that merely renders the same.
///
/// **Two of line 1854's three words now, not one.** `routing::evidence`'s
/// `Confidence` belongs to the gateway's aggregate ledger, which
/// `SessionRouter` never reads, and a
/// [`glasshouse::routing::free::FreePool`] health entry carries no
/// observation time — but the cache the pool was filled from does, per
/// provider file, and [`ObservedHealth`] carries it here. So *sparse* is
/// answered by whether the pool held this destination and *stale* by how old
/// the file that supplied it was, against
/// [`glasshouse::evaluation::HEALTH_EVIDENCE_HORIZON_SECONDS`].
///
/// *Incorrectly segmented*, line 1854's third, still has no producer
/// anywhere on this path and is not invented: nothing in this build compares
/// a health reading's segmentation against the resource it was attributed
/// to, and the line stays open on that word alone.
fn routing_evidence_for(
    health: &ObservedHealth,
    destination: &glasshouse::routing::session::Destination,
    now_unix: i64,
) -> glasshouse::evaluation::RoutingEvidence {
    use glasshouse::routing::free::FreeResource;

    let chosen = FreeResource::new(
        destination.backend().credential().clone(),
        destination.backend().model().label(),
    );
    let held = health
        .pool()
        .observed()
        .iter()
        .any(|(resource, _)| *resource == chosen);
    // A pool hit whose date is somehow missing answers `absent`, not fresh —
    // `and_then` rather than `unwrap_or(now_unix)`, which is the one
    // substitution that would turn an unknown into a favourable fact.
    let observed_at = held.then(|| health.observed_at(&chosen)).flatten();
    glasshouse::evaluation::RoutingEvidence::from_observation(observed_at, now_unix)
}

/// Capability map line 1564's caller half: how the most recent exchange on
/// `current`'s backend ended, from the evidence ledger, keyed exactly as the
/// gateway writes it (`exchange.provider`, `assignment.backend().model().label()`).
/// `None` when the ledger cannot be opened or holds nothing for the pair — a
/// missing history is not a failure, and the router's explanation says so.
fn latest_failure_class(
    runtime: &Runtime,
    current: &glasshouse::routing::session::Destination,
) -> Option<glasshouse::routing::evidence::FailureClass> {
    use glasshouse::routing::evidence::{CLASSIFICATION_EVIDENCE_WINDOW_SECONDS, EvidenceLedger};

    let ledger = match EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::debug!(error = %err, "evidence ledger unavailable; no retry promotion");
            return None;
        }
    };
    ledger
        .latest_failure_class_for_model(
            current.backend().provider(),
            current.backend().model().label(),
            glasshouse::provider::cache::now_unix_seconds(),
            CLASSIFICATION_EVIDENCE_WINDOW_SECONDS,
        )
        .unwrap_or_else(|err| {
            tracing::debug!(error = %err, "could not read the last failure class");
            None
        })
}

/// Capability map line 1566: one ledger row per tier movement the launch
/// path acted on, under [`glasshouse::routing::evidence::TIER_ESCALATION_PURPOSE`]
/// or [`glasshouse::routing::evidence::TIER_DOWNGRADE_PURPOSE`], so a later
/// evaluation can count how often the router moved a tier and which way.
///
/// The same `glasshouse`/`session-router` identity and the same
/// open-write-drop shape as [`record_routing_latency`], for the same reasons
/// — and a `Held` movement writes nothing, because "the tier stood" is the
/// row's absence, exactly as a launch that classified nothing leaves no
/// latency row.
fn record_tier_movement(
    runtime: &Runtime,
    harness: glasshouse::integrations::IntegrationId,
    movement: &glasshouse::routing::session::TierMovement,
) {
    use glasshouse::routing::evidence::{
        EvidenceLedger, NewObservation, TIER_DOWNGRADE_PURPOSE, TIER_ESCALATION_PURPOSE,
    };
    use glasshouse::routing::session::TierMovement;

    let purpose = match movement {
        TierMovement::Escalated { .. } => TIER_ESCALATION_PURPOSE,
        TierMovement::Downgraded { .. } => TIER_DOWNGRADE_PURPOSE,
        TierMovement::Held { .. } => return,
    };
    let ledger = match EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "routing evidence ledger unavailable; the tier movement is not recorded"
            );
            return;
        }
    };
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let observation = NewObservation::new("glasshouse", "session-router")
        .with_harness(Some(harness.slug()))
        .with_purpose(Some(purpose))
        .with_timing(Some(now_unix), Some(now_unix));
    if let Err(err) = ledger.record(observation, now_unix) {
        tracing::warn!(error = %err, "could not record the tier movement");
    }
}

/// Capability map line 1970: one ledger row per pool fallback the launch
/// path acted on. The same open-write-drop shape as
/// [`record_tier_movement`], for the same reasons — and **a decision that
/// made no fallback writes nothing**, because "the broker stayed put" is
/// the row's absence, exactly as a held tier is.
///
/// The row carries the fallback whole **without a migration**: `purpose` is
/// the trigger, `quota_context` is the account the work LEFT (so the
/// entitlements view's own per-account reader finds it), and the account
/// the work went TO is the `sessions.entitlement` column migration 22
/// added, written by this same launch from this same decision. `provider`
/// and `model` are the chosen destination's.
///
/// Map line 1307's own producer: `cost`, when given, is
/// [`glasshouse::routing::session::Routed::cost`] — the value **that
/// decision itself computed**, carried in rather than recomputed here from a
/// `PriceTable` that may since have changed on disk. This is the only launch
/// writer with a `Destination` in scope
/// (`record_tier_movement`'s `TierMovement` carries none), so it is the only
/// production caller `cost_micro_usd` has today; most rows still leave it
/// `NULL`, on every decision that made no fallback at all.
fn record_entitlement_fallback(
    runtime: &Runtime,
    harness: glasshouse::integrations::IntegrationId,
    destination: &glasshouse::routing::session::Destination,
    fallback: &glasshouse::routing::session::EntitlementFallback,
    cost: Option<glasshouse::routing::evidence::ObservedCost>,
) {
    use glasshouse::routing::evidence::{
        ENTITLEMENT_FALLBACK_EXHAUSTED_PURPOSE, ENTITLEMENT_FALLBACK_THROTTLED_PURPOSE,
        EvidenceLedger, NewObservation,
    };
    use glasshouse::routing::session::FallbackReason;

    let fallback_purpose = match fallback.reason() {
        FallbackReason::Exhausted => ENTITLEMENT_FALLBACK_EXHAUSTED_PURPOSE,
        FallbackReason::Throttled => ENTITLEMENT_FALLBACK_THROTTLED_PURPOSE,
    };
    let ledger = match EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "routing evidence ledger unavailable; the entitlement fallback is not recorded"
            );
            return;
        }
    };
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let observation = NewObservation::new(
        destination.backend().provider(),
        destination.backend().model().label(),
    )
    .with_harness(Some(harness.slug()))
    .with_purpose(Some(fallback_purpose))
    .with_quota_context(Some(fallback.from().to_owned()))
    .with_timing(Some(now_unix), Some(now_unix))
    .with_cost(cost);
    if let Err(err) = ledger.record(observation, now_unix) {
        tracing::warn!(error = %err, "could not record the entitlement fallback");
    }
}

/// Handle `context-firewall hook` — the production caller map lines
/// 1980-1990 need. Reads one `PostToolUse` event on stdin, runs it through
/// [`glasshouse::firewall::process`], records telemetry, and writes the
/// hook response on stdout.
///
/// Fails open at every internal step: a stdin document this build cannot
/// parse, a raw-store write that fails, or a ledger that cannot be opened
/// all end in the same no-op response a passthrough result gets, never a
/// nonzero exit — `docs/product/evidence/phase-57.md`'s "fail open, never
/// empty" applies to the hook process itself, not only to the reduction.
fn context_firewall_hook(
    runtime: &Runtime,
    passthrough_tokens: u64,
    min_semantic_tokens: u64,
    task: &str,
    tools: &[String],
    emit_updated_output: bool,
    mode: glasshouse::config::firewall::FirewallMode,
) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use std::io::Read;

    let mut input = Vec::new();
    std::io::stdin()
        .read_to_end(&mut input)
        .context("could not read the PostToolUse event from stdin")?;

    let event = match glasshouse::firewall::adapter::parse_event(&input) {
        Ok(event) => event,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "context firewall: could not parse the PostToolUse event; answering with a \
                 no-op response"
            );
            return print_context_firewall_response(None);
        }
    };

    let normalized = glasshouse::firewall::adapter::normalize(&event);
    let config = glasshouse::firewall::FirewallConfig::new(passthrough_tokens, tools.to_vec());
    let store = glasshouse::firewall::RawStore::open(runtime.state_dir().join("context-firewall"));
    let now_unix = glasshouse::provider::cache::now_unix_seconds();

    // Phase 57B, map lines 1997-2003: resolved once, from configuration and
    // disposable routing, and handed to `process` as a trait object — the
    // core itself never touches `DisposableRouting`, a `JobKind`, or a
    // provider (see `firewall::mod`'s own header). A configuration this
    // build cannot read degrades to "no reducer" — the same fail-open
    // posture every other step of this hook already has.
    let user = UserConfig::load(runtime.paths()).ok();
    let project = config::load_project_config(runtime.project())
        .ok()
        .flatten();
    let aggressive_drops_uncertain = user.as_ref().is_some_and(|user| {
        EffectiveConfig::new(user, project.as_ref())
            .context_firewall_aggressive_drops_uncertain()
            .value
    });
    let active_reducer = match &user {
        Some(user) => disposable_reducer(runtime, user, project.as_ref(), &event.session_id),
        None => None,
    };
    let tool_query = glasshouse::firewall::adapter::tool_query(&event.tool_input);
    let file_paths = glasshouse::firewall::adapter::tool_input_paths(&event.tool_input);
    let semantic = glasshouse::firewall::SemanticContext {
        mode,
        reducer: active_reducer.as_deref(),
        task,
        tool_query: tool_query.as_deref(),
        file_paths: &file_paths,
        min_semantic_tokens,
        aggressive_drops_uncertain,
    };

    let outcome = glasshouse::firewall::process(
        &store,
        &config,
        &event.session_id,
        &event.tool_use_id,
        now_unix,
        &event.tool_name,
        normalized,
        &semantic,
    );

    record_context_firewall_telemetry(runtime, &outcome, now_unix);

    // Map line 1991's mode decision, enforced here rather than trusted to
    // whatever registered the command line: `shadow` never emits
    // `updatedToolOutput`, whatever `--emit-updated-output` says, because
    // shadow's whole point is a session that sees only originals while the
    // pipeline still runs in full for storage, telemetry and provenance.
    let effective_emit =
        emit_updated_output && mode != glasshouse::config::firewall::FirewallMode::Shadow;
    let updated_output = match &outcome {
        glasshouse::firewall::Outcome::Reduced { forwarded_text, .. } if effective_emit => {
            Some(forwarded_text.as_str())
        }
        _ => None,
    };
    print_context_firewall_response(updated_output)
}

/// Write the `PostToolUse` hook response JSON to stdout — the protocol
/// channel here exactly as it is for `glasshouse mcp serve`.
fn print_context_firewall_response(updated_output: Option<&str>) -> anyhow::Result<()> {
    let response = glasshouse::firewall::adapter::hook_response(updated_output);
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

/// Map line 1987: one telemetry row per reduced result and one per bypass —
/// never for a passthrough result, which line 1981 already defines as
/// carrying nothing beyond the harness's own original output.
fn record_context_firewall_telemetry(
    runtime: &Runtime,
    outcome: &glasshouse::firewall::Outcome,
    now_unix: i64,
) {
    use glasshouse::routing::evidence::{
        CONTEXT_FIREWALL_BYPASS_PURPOSE, CONTEXT_FIREWALL_REDUCTION_PURPOSE, EvidenceLedger,
        NewObservation,
    };

    let (purpose, route) = match outcome {
        glasshouse::firewall::Outcome::Passthrough { .. } => return,
        glasshouse::firewall::Outcome::Reduced { .. } => (CONTEXT_FIREWALL_REDUCTION_PURPOSE, None),
        glasshouse::firewall::Outcome::Bypass { reason, .. } => {
            (CONTEXT_FIREWALL_BYPASS_PURPOSE, Some(reason.as_str()))
        }
    };

    let ledger = match EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "routing evidence ledger unavailable; a context-firewall event is not recorded"
            );
            return;
        }
    };
    // `provider`/`model` have no real backend here — this is not a model
    // call — so a fixed, self-describing placeholder stands in, exactly as
    // `CORRELATION_PURPOSE`'s rows use the displaced route's identity for a
    // row that is "about" something rather than an exchange. No reader
    // filters on this pair, so it cannot be mistaken for real spend; the
    // `purpose` column is what keeps it out of every such reader, per its
    // own doc comment.
    let observation = NewObservation::new("glasshouse", "context-firewall")
        .with_harness(Some(
            glasshouse::integrations::IntegrationId::ClaudeCode.slug(),
        ))
        .with_purpose(Some(purpose))
        .with_route(route)
        .with_quota_context(Some(outcome.tool_name().to_owned()))
        .with_timing(Some(now_unix), Some(now_unix));
    if let Err(err) = ledger.record(observation, now_unix) {
        tracing::warn!(error = %err, "could not record a context-firewall event");
    }

    // Map line 1987's second half (the 1987 ruling in
    // `docs/product/evidence/phase-57.md`): a reducer call is a REAL model
    // call, so its own row carries the real provider/model identity and the
    // provider-reported token counts in the ledger's token columns —
    // distinct from the bookkeeping row above, which is not a model call
    // and therefore never carries tokens. Recorded whenever a call actually
    // completed with a parseable reply, applied or not (map line 1987: the
    // cost was real either way).
    if let glasshouse::firewall::Outcome::Reduced {
        semantic: Some(semantic),
        ..
    } = outcome
        && let Some(call) = &semantic.call
    {
        let call_observation = NewObservation::new(call.provider.clone(), call.model.clone())
            .with_harness(Some(
                glasshouse::integrations::IntegrationId::ClaudeCode.slug(),
            ))
            .with_purpose(Some(CONTEXT_FIREWALL_REDUCTION_PURPOSE))
            .with_route(call.route.clone())
            .with_quota_context(Some(outcome.tool_name().to_owned()))
            .with_timing(Some(now_unix), Some(now_unix))
            .with_tokens(
                call.input_tokens,
                call.output_tokens,
                call.cached_input_tokens,
            );
        if let Err(err) = ledger.record(call_observation, now_unix) {
            tracing::warn!(error = %err, "could not record a context-firewall reducer call");
        }
    }
}

/// Map line 2004's four granularities. `Whole` is the pre-existing
/// behaviour every earlier package relied on; the other three are this
/// package's own, and are reached only through this same subcommand — no
/// invented side channel.
enum ExpansionRequest {
    Whole,
    Candidate(usize),
    File(String),
    Range((usize, usize)),
}

/// What `context_firewall_show` decided, once the reference itself
/// resolved (or did not).
enum ExpansionOutcome {
    Content(String),
    /// The reference itself does not name a stored entry — the pre-existing
    /// refusal `Show`'s bare form already had.
    NotFound,
    /// The reference resolved, but the requested slice of it does not exist
    /// (an out-of-range candidate id, a file the result never names, or a
    /// reversed/out-of-bounds range). Kept distinct from `NotFound`: the
    /// expansion-request telemetry below already counted this reference as
    /// found, because it was — the refusal is about the *granularity*, not
    /// the reference.
    Refused(String),
}

/// `context-firewall show <id>`: expand a previously stored raw tool
/// result at the granularity `request` names, and record the map line 1988
/// expansion-request telemetry either way — a miss is still a request, and
/// still part of the recall signal. `request = Whole` reproduces map line
/// 1985's exact byte-identical round-trip; the other three variants are map
/// line 2004's.
fn context_firewall_show(
    runtime: &Runtime,
    id: &str,
    request: ExpansionRequest,
) -> anyhow::Result<ExpansionOutcome> {
    use anyhow::Context as _;

    let store = glasshouse::firewall::RawStore::open(runtime.state_dir().join("context-firewall"));
    let entry = store
        .read(id)
        .with_context(|| format!("could not read the context-firewall raw store for `{id}`"))?;
    record_context_firewall_expansion(runtime, entry.as_ref().map(|entry| entry.tool.as_str()));
    let Some(entry) = entry else {
        return Ok(ExpansionOutcome::NotFound);
    };

    Ok(match request {
        ExpansionRequest::Whole => ExpansionOutcome::Content(entry.content),
        ExpansionRequest::Candidate(candidate_id) => {
            // Recomputed rather than stored: `reduce` is a pure function of
            // `entry.content`, which is the exact original this entry has
            // held since it was written — the same id therefore always
            // names the same candidate, with nothing new to persist.
            let reduction = glasshouse::firewall::reduce::reduce(&entry.content);
            match reduction
                .candidates
                .iter()
                .find(|candidate| candidate.id == candidate_id)
            {
                Some(candidate) => ExpansionOutcome::Content(candidate.text.clone()),
                None => ExpansionOutcome::Refused(format!(
                    "`{id}` has no candidate `{candidate_id}` (0..{})",
                    reduction.candidates.len()
                )),
            }
        }
        ExpansionRequest::File(file) => {
            let matches: Vec<&str> = entry
                .content
                .lines()
                .filter(|line| line_names_file(line, &file))
                .collect();
            if matches.is_empty() {
                ExpansionOutcome::Refused(format!("`{id}` names no file `{file}`"))
            } else {
                ExpansionOutcome::Content(format!("{}\n", matches.join("\n")))
            }
        }
        ExpansionRequest::Range((start, end)) => {
            if start == 0 {
                ExpansionOutcome::Refused(
                    "line ranges are 1-indexed; `0` is not a line".to_string(),
                )
            } else if start > end {
                ExpansionOutcome::Refused(format!(
                    "range `{start}-{end}` is reversed; start must not exceed end"
                ))
            } else {
                let lines: Vec<&str> = entry.content.lines().collect();
                if end > lines.len() {
                    ExpansionOutcome::Refused(format!(
                        "range `{start}-{end}` is out of bounds; `{id}` has {} line{}",
                        lines.len(),
                        if lines.len() == 1 { "" } else { "s" }
                    ))
                } else {
                    ExpansionOutcome::Content(format!("{}\n", lines[start - 1..end].join("\n")))
                }
            }
        }
    })
}

/// Map line 2004's file granularity: a line naming `file` is either the
/// bare path on its own (Glob-shaped output) or a `path:...` prefix
/// (ripgrep-shaped search-hit output) — the two file-per-line shapes this
/// build's own eligible tools (Grep, Glob) actually produce. An exact
/// prefix only: a line about a different file that merely contains `file`
/// as a substring must never match.
fn line_names_file(line: &str, file: &str) -> bool {
    let trimmed = line.trim();
    trimmed == file || trimmed.starts_with(&format!("{file}:"))
}

/// Map line 2004's range granularity: `START-END`, 1-indexed and
/// inclusive. Malformed input (non-numeric, no separator) is refused with
/// the same clear-error posture as a reversed or out-of-bounds range —
/// `context_firewall_show` never sees anything but a validated pair.
fn parse_line_range(spec: &str) -> Result<(usize, usize), String> {
    let (start, end) = spec
        .split_once('-')
        .ok_or_else(|| format!("`{spec}` is not a `START-END` line range"))?;
    let start: usize = start
        .trim()
        .parse()
        .map_err(|_| format!("`{spec}` is not a `START-END` line range"))?;
    let end: usize = end
        .trim()
        .parse()
        .map_err(|_| format!("`{spec}` is not a `START-END` line range"))?;
    Ok((start, end))
}

/// `context-firewall show <id> --stats`: the entry's own recorded map line
/// 2005 comparison — original/forwarded token estimates and
/// retained/total candidate counts — never its content. This is the
/// "check for yourself" surface a savings claim needs, kept separate from
/// content expansion rather than folded into it, so a caller can always
/// tell which one it asked for.
fn context_firewall_show_stats(runtime: &Runtime, id: &str) -> anyhow::Result<Option<String>> {
    use anyhow::Context as _;
    use std::fmt::Write as _;

    let store = glasshouse::firewall::RawStore::open(runtime.state_dir().join("context-firewall"));
    let entry = store
        .read(id)
        .with_context(|| format!("could not read the context-firewall raw store for `{id}`"))?;
    record_context_firewall_expansion(runtime, entry.as_ref().map(|entry| entry.tool.as_str()));
    let Some(entry) = entry else {
        return Ok(None);
    };

    let mut out = String::new();
    let _ = writeln!(out, "tool: {}", entry.tool);
    let _ = writeln!(out, "original_tokens: {}", entry.original_token_estimate);
    match entry.forwarded_token_estimate {
        Some(tokens) => {
            let _ = writeln!(out, "forwarded_tokens: {tokens}");
        }
        None => {
            let _ = writeln!(
                out,
                "forwarded_tokens: unknown (recorded before map line 2005)"
            );
        }
    }
    match (entry.retained_candidates, entry.total_candidates) {
        (Some(retained), Some(total)) => {
            let _ = writeln!(out, "retained_candidates: {retained}");
            let _ = writeln!(out, "total_candidates: {total}");
        }
        _ => {
            let _ = writeln!(
                out,
                "retained_candidates/total_candidates: unknown (recorded before map line 2005)"
            );
        }
    }
    Ok(Some(out))
}

/// Map line 1988: track raw-expansion requests as their own telemetry
/// rows, independent of map line 1987's reduction/bypass rows — a recall
/// regression must be measurable before any savings claim from those rows
/// is believed.
fn record_context_firewall_expansion(runtime: &Runtime, found_tool: Option<&str>) {
    use glasshouse::routing::evidence::{
        CONTEXT_FIREWALL_EXPANSION_PURPOSE, EvidenceLedger, NewObservation,
    };

    let ledger = match EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "routing evidence ledger unavailable; a context-firewall expansion request is \
                 not recorded"
            );
            return;
        }
    };
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let observation = NewObservation::new("glasshouse", "context-firewall")
        .with_purpose(Some(CONTEXT_FIREWALL_EXPANSION_PURPOSE))
        .with_route(Some(if found_tool.is_some() {
            "found"
        } else {
            "not-found"
        }))
        .with_quota_context(found_tool)
        .with_timing(Some(now_unix), Some(now_unix));
    if let Err(err) = ledger.record(observation, now_unix) {
        tracing::warn!(
            error = %err,
            "could not record a context-firewall expansion request"
        );
    }
}

/// The workload tier a launch's routing decision used, and whether line
/// 1459's conservative rule moved it — **capability map line 1834**'s
/// producer input, from the classification that decision actually acted on.
///
/// `None` — no `--task`, so nothing classified — is
/// [`glasshouse::evaluation::RoutingTier::Unclassified`], **its own bucket
/// and never nothing**: a launch that states no task still made a routing
/// decision, and omitting its row would make *"this project never states its
/// tasks"* read as *"this project never launches"*.
///
/// *Escalated* is whether the tier the decision used differs from the tier
/// the classifier stated, which is not the same as whether the conservative
/// rule fired — see [`glasshouse::evaluation::RoutingTier::Classified`]'s own
/// doc comment for the case at the top of the scale where the two part.
fn routed_tier(classified: Option<&ClassifiedRouting>) -> glasshouse::evaluation::RoutingTier {
    use glasshouse::evaluation::RoutingTier;

    let Some(classified) = classified else {
        return RoutingTier::Unclassified;
    };
    let answer = &classified.answer;
    RoutingTier::Classified {
        tier: answer.required_tier(),
        escalated: answer.required_tier() != answer.stated_tier(),
    }
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
fn failover_prevention_sink(
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
fn record_correlation_steer(
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
fn observed_provider_health(
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
            health.pool.record_pool(
                credential,
                &PoolReading {
                    limit,
                    remaining,
                    resets_in,
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
struct ObservedHealth {
    pool: glasshouse::routing::free::FreePool,
    /// Every resource adopted into `pool`, with the unix second its file was
    /// written. A `Vec` rather than a map because it is walked once per
    /// routed destination and holds one entry per configured destination.
    observed_at: Vec<(glasshouse::routing::free::FreeResource, i64)>,
}

impl ObservedHealth {
    fn pool(&self) -> &glasshouse::routing::free::FreePool {
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
    fn observed_at(&self, resource: &glasshouse::routing::free::FreeResource) -> Option<i64> {
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
fn observed_health_of(
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
    let mut report = render_route_recommendation(&recommendation);
    report.push('\n');
    report.push_str(&route_outcomes_section(runtime));
    report.push_str(&tier_outcome_section(runtime));
    report.push_str(&capability_suggestions_section(runtime, &effective));
    report.push_str(&harness_efficiency_section(runtime));
    report.push_str(&support_work_section(runtime));
    report.push_str(&route_correlations_section(runtime));
    report.push_str(&throttle_scope_section(runtime));
    Ok(report)
}

/// Capability map lines 1370, 1373, 1374, 1376 and 1852, printed for a
/// person: every pair of routes this project's ledger has observed at the
/// same moments, with the sample size **before** any confidence — line
/// 1376's rule, that a correlation is presented as meaningful only once
/// enough overlapping observations exist, and otherwise as the count that
/// fell short — and beneath them how many gateway failovers the correlation
/// term actually steered.
///
/// Reads the routing ledger over
/// [`glasshouse::routing::evidence::CLASSIFICATION_EVIDENCE_WINDOW_SECONDS`]
/// — the same seven days the failover itself reads — so what this prints is
/// what the router weighed. Practice §65: opened here, dropped on return,
/// and a ledger that cannot be opened costs this section and never the
/// recommendation above it.
fn route_correlations_section(runtime: &Runtime) -> String {
    use glasshouse::routing::evidence::{
        CLASSIFICATION_EVIDENCE_WINDOW_SECONDS, CORRELATION_PURPOSE, CorrelationVerdict,
        EvidenceLedger,
    };

    let header =
        "\nRoute correlations in this project, last 7 days (map lines 1370-1376)\n".to_owned();
    let ledger = match EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::debug!(
                error = %err,
                "could not open the routing evidence ledger for the route-correlation section"
            );
            return format!("{header}\n  the routing evidence ledger could not be opened\n");
        }
    };
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let correlations =
        match ledger.route_correlations(now_unix, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS) {
            Ok(correlations) => correlations,
            Err(err) => return format!("{header}\n  {err}\n"),
        };
    // Line 1852, counted back by purpose: every row the failover-prevention
    // sink wrote for a steered failover, and nothing else.
    let steered: usize =
        match ledger.consumption_by_purpose(now_unix, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS) {
            Ok(groups) => groups
                .iter()
                .filter(|group| group.purpose.as_deref() == Some(CORRELATION_PURPOSE))
                .map(|group| group.sample_count)
                .sum(),
            Err(err) => return format!("{header}\n  {err}\n"),
        };

    let mut out = header;
    out.push('\n');
    if correlations.is_empty() {
        out.push_str(
            "  no two routes have been observed at the same moment, so nothing is known about \
             whether any pair fails together\n",
        );
    }
    for correlation in correlations.iter() {
        let (a, b) = correlation.routes();
        match correlation.verdict() {
            CorrelationVerdict::InsufficientEvidence {
                sample_size,
                required,
            } => {
                let _ = writeln_str(
                    &mut out,
                    format!(
                        "  {a} and {b}: insufficient evidence — {sample_size} of the {required} \
                         overlapping observations a correlation needs; treated as no correlation"
                    ),
                );
            }
            CorrelationVerdict::Measured {
                confidence,
                sample_size,
            } => {
                let _ = writeln_str(
                    &mut out,
                    format!(
                        "  {a} and {b}: failed the same way at the same moment in {} of \
                         {sample_size} overlapping observations — correlation {confidence:.2}",
                        correlation.overlaps()
                    ),
                );
            }
        }
    }
    out.push('\n');
    if steered == 0 {
        out.push_str(
            "  no gateway failover in this window was steered by a measured route \
             correlation (map line 1852)\n",
        );
    } else {
        let _ = writeln_str(
            &mut out,
            format!(
                "  {steered} gateway failover(s) in this window went somewhere other than a route \
                 whose failures overlap the failed backend's — separate quota, not separate \
                 failure resilience (map line 1852)"
            ),
        );
    }
    out
}

/// Capability map line 1317, printed for a person: every route this
/// project's ledger has seen throttled, and whether that throttle reads as
/// this provider's own cadence limiter firing everywhere or as one model's
/// own limit — line 1317's "track", not "act": nothing here changes a
/// failover, only what a person reading `glasshouse route` is told about a
/// throttle that already happened.
///
/// Two of the map line's four scopes are never printed here, on purpose:
/// **account-specific** and **request-pool-specific** have no producer in
/// this build — see [`glasshouse::routing::evidence::ThrottleScope`]'s own
/// doc comment and refusal register row 531.
///
/// Reads the same
/// [`glasshouse::routing::evidence::CLASSIFICATION_EVIDENCE_WINDOW_SECONDS`]
/// window as [`route_correlations_section`], for the same reason: what this
/// prints should be what a real failover, reading the ledger at the same
/// moment, would see.
fn throttle_scope_section(runtime: &Runtime) -> String {
    use glasshouse::routing::evidence::{
        CLASSIFICATION_EVIDENCE_WINDOW_SECONDS, EvidenceLedger, ThrottleScope,
    };

    let header = "\nThrottle scope in this project, last 7 days (map line 1317)\n".to_owned();
    let ledger = match EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::debug!(
                error = %err,
                "could not open the routing evidence ledger for the throttle-scope section"
            );
            return format!("{header}\n  the routing evidence ledger could not be opened\n");
        }
    };
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let scopes = match ledger.throttle_scopes(now_unix, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS) {
        Ok(scopes) => scopes,
        Err(err) => return format!("{header}\n  {err}\n"),
    };

    let mut out = header;
    out.push('\n');
    if scopes.is_empty() {
        out.push_str("  no throttle has been observed on any route in this window\n");
    }
    for (route, scope) in scopes.iter() {
        match scope {
            ThrottleScope::ProviderWide => {
                let _ = writeln_str(
                    &mut out,
                    format!(
                        "  {route}: provider-wide — a throttle on this route overlapped a \
                         throttle on another model of the same provider"
                    ),
                );
            }
            ThrottleScope::ModelSpecific => {
                let _ = writeln_str(
                    &mut out,
                    format!(
                        "  {route}: model-specific — every observed throttle on this route \
                         overlapped a sibling model that was not throttled"
                    ),
                );
            }
            ThrottleScope::AccountSpecific => {
                let _ = writeln_str(
                    &mut out,
                    format!(
                        "  {route}: account-specific — this account's sibling models were \
                         throttled together while another account of the same provider kept \
                         serving"
                    ),
                );
            }
            ThrottleScope::Unknown {
                sample_size,
                required,
            } => {
                let _ = writeln_str(
                    &mut out,
                    format!(
                        "  {route}: insufficient evidence — {sample_size} of the {required} \
                         informative throttle events a scope needs; treated as unknown"
                    ),
                );
            }
        }
    }
    out
}

/// `writeln!` without the `use std::fmt::Write` every caller would need.
fn writeln_str(out: &mut String, line: String) -> std::fmt::Result {
    use std::fmt::Write as _;
    writeln!(out, "{line}")
}

/// How far back `glasshouse route`'s outcome section looks.
///
/// Thirty days, comfortably inside
/// [`glasshouse::evaluation::Retention`]'s ninety, so the window is one a
/// ledger that has been pruning can still answer. A person reading a
/// recommendation wants recent behaviour; a longer window would average this
/// month's routes with a configuration two months dead.
const ROUTE_OUTCOME_WINDOW_DAYS: i64 = 30;

/// Map lines 1834, 1835, 1845 and 1854, printed for a person — the consumer
/// half of this package, and the reason its producers are not Cluster B.
///
/// # Why it lives in `glasshouse route` rather than in a command of its own
///
/// `route` is the report about routing: it already prints the ranking, the
/// override, and what the ranking could not see. *How the routes this project
/// took actually turned out* is the same subject and the same reader, and a
/// second command would have meant a new `Command` variant in `cli.rs` —
/// a file two other workers are editing this round (practice §77). Nothing
/// here decides anything; the recommendation above is computed without it.
///
/// # The rules this rendering exists to hold
///
/// Every ratio prints its denominator and no ratio prints a percentage — a
/// bare `60%` cannot be told from a `3 of 5` that is one lucky afternoon.
/// `unknown` is its own bucket in every table, never folded into a
/// neighbour, and a session whose harness never reported a turn end is
/// counted as exactly that: not a failure, and not a success.
///
/// # Practice §65
///
/// The ledger is opened here, in the one function that reads it, and dropped
/// when this returns. `route` is a command a person types; a ledger that
/// cannot be opened costs this section and never the report.
fn route_outcomes_section(runtime: &Runtime) -> String {
    use glasshouse::evaluation::{EvaluationKind, EvaluationObservations};

    let header = format!("Past routes in this project, last {ROUTE_OUTCOME_WINDOW_DAYS} days\n");
    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::debug!(
                error = %format!("{err:#}"),
                "could not open the evaluation ledger for the routing-outcome section"
            );
            return format!("{header}\n  the evaluation ledger could not be opened\n");
        }
    };

    let to = glasshouse::evaluation::now_unix();
    let from = to - ROUTE_OUTCOME_WINDOW_DAYS * 24 * 60 * 60;
    let by_class = ledger.route_outcomes_by(EvaluationKind::RoutingCostClassObserved, from, to);
    let by_pairing = ledger.route_outcomes_by_pairing_class(from, to);
    let by_evidence = ledger.route_outcomes_by(EvaluationKind::RoutingEvidenceObserved, from, to);
    // Map line 1834. The bucket is the tier *with* its escalation, which is
    // what makes "did this tier succeed without escalation" a comparison
    // between two rows of one table rather than a join.
    let by_tier = ledger.route_outcomes_by(EvaluationKind::RoutingTierObserved, from, to);
    // Map line 1851. Counted rather than joined: these rows carry no
    // session, because the gateway that ranks a failover holds no Glasshouse
    // session id — see `EvaluationKind::FailoverPrevented`.
    let preventions = ledger.counts_by_subject(EvaluationKind::FailoverPrevented, from, to);

    let (by_class, by_pairing, by_evidence, by_tier, preventions) =
        match (by_class, by_pairing, by_evidence, by_tier, preventions) {
            (Ok(class), Ok(pairing), Ok(evidence), Ok(tier), Ok(preventions)) => {
                (class, pairing, evidence, tier, preventions)
            }
            (Err(err), ..)
            | (_, Err(err), ..)
            | (_, _, Err(err), ..)
            | (_, _, _, Err(err), _)
            | (_, _, _, _, Err(err)) => {
                // `WindowNotRetained` is the honest one: retention trimmed rows
                // this window reaches back past, and a smaller number would be a
                // fabrication. It is reported, not rounded away.
                return format!("{header}\n  {err}\n");
            }
        };

    if by_class.is_empty() {
        // Map line 1851 is still printed here. A prevention row carries no
        // session — the gateway that ranks a failover holds no Glasshouse
        // session id — so a project that has failed over without ever
        // launching under a build that attributes routes has a real count and
        // no routed sessions, and returning early would hide it behind an
        // unrelated emptiness.
        return format!(
            "{header}\n  no routed sessions recorded in this window\n{}",
            render_failover_preventions(&preventions)
        );
    }

    let mut out = header;
    out.push_str("\n  by cost class\n");
    out.push_str(&render_route_outcome_rows(&by_class));
    out.push_str(
        "\n  by pairing class (task success only; line 1845's other five \
                  quantities have no producer in this build)\n",
    );
    out.push_str(&render_route_outcome_rows(&by_pairing));
    out.push_str(
        "\n  by evidence held about the destination when it was chosen (`observed` is a row \
         written before staleness was measured, never re-labelled)\n",
    );
    out.push_str(&render_route_outcome_rows(&by_evidence));
    out.push_str(
        "\n  by workload tier the decision used, and whether the conservative rule escalated \
         it (map line 1834)\n",
    );
    out.push_str(&render_route_outcome_rows(&by_tier));
    out.push_str(&render_failover_preventions(&preventions));
    out.push_str(
        "\nA session whose harness never reported a turn end is counted as neither a success \
         nor a failure; a quiet or exited process is never read as either.\n",
    );
    out
}

/// Map line 1480, printed for a person: whether each workload tier has
/// enough evidence to say how its routed sessions turned out, and what that
/// evidence says when it does.
///
/// A section of its own, beside [`route_correlations_section`] and
/// [`throttle_scope_section`] — not a change to [`route_outcomes_section`]'s
/// own "by workload tier" table, which map line 1834 already closed and
/// whose regression asserts raw, un-gated counts. Line 1834 asks what was
/// recorded; line 1480 asks whether enough of it exists to summarize, which
/// is [`glasshouse::evaluation::EvaluationObservations::outcomes_by_tier`]'s
/// own [`glasshouse::evaluation::TierOutcomeVerdict`] gate.
///
/// Same window as [`route_outcomes_section`] and the same practice §65
/// reasoning: the ledger is opened here, in the one function that reads it
/// for this section, and dropped when this returns.
fn tier_outcome_section(runtime: &Runtime) -> String {
    use glasshouse::evaluation::{EvaluationObservations, TierOutcomeVerdict};

    let header =
        "\nWorkload-tier outcomes in this project, last 30 days (map line 1480)\n".to_owned();
    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::debug!(
                error = %format!("{err:#}"),
                "could not open the evaluation ledger for the tier-outcome section"
            );
            return format!("{header}\n  the evaluation ledger could not be opened\n");
        }
    };
    let to = glasshouse::evaluation::now_unix();
    let from = to - ROUTE_OUTCOME_WINDOW_DAYS * 24 * 60 * 60;
    let outcomes = match ledger.outcomes_by_tier(from, to) {
        Ok(outcomes) => outcomes,
        Err(err) => return format!("{header}\n  {err}\n"),
    };

    let mut out = header;
    out.push('\n');
    if outcomes.is_empty() {
        out.push_str("  no routed sessions recorded in this window\n");
        return out;
    }
    for outcome in outcomes.iter() {
        match outcome.verdict {
            TierOutcomeVerdict::InsufficientEvidence {
                sample_size,
                required,
            } => {
                let _ = writeln_str(
                    &mut out,
                    format!(
                        "  {}: insufficient evidence — {sample_size} of the {required} reported \
                         turns a tier summary needs; treated as no summary",
                        outcome.bucket
                    ),
                );
            }
            TierOutcomeVerdict::Measured {
                successful,
                failed,
                sample_size,
            } => {
                let _ = writeln_str(
                    &mut out,
                    format!(
                        "  {}: {successful} of {sample_size} reported turns succeeded, {failed} \
                         failed",
                        outcome.bucket
                    ),
                );
            }
        }
        if outcome.undecided > 0 {
            let _ = writeln_str(
                &mut out,
                format!(
                    "    {} session(s) with no turn end reported yet — undecided, never a \
                     failure",
                    outcome.undecided
                ),
            );
        }
    }
    out
}

/// Map line 1481, printed for a person: where a tier's observed outcomes
/// disagree with what a configured capability record says about a model at
/// that tier — a suggestion, never a rewrite.
///
/// **The evidence gate is [`glasshouse::evaluation::TierOutcomeVerdict::Measured`]
/// itself** — the same [`glasshouse::routing::evidence::MIN_SAMPLE_FOR_SUMMARY`]
/// floor [`tier_outcome_section`] already gates its own summary on, reused
/// rather than a second threshold invented for this section: a tier with too
/// few reported turns to summarize has too few to suggest a calibration
/// change from either. `TierOutcomeVerdict::InsufficientEvidence` — which
/// includes the empty-window and zero-sample case — is skipped outright.
///
/// **Read-only by construction.** This reads
/// [`glasshouse::config::EffectiveConfig::calibrated_model_ceilings`] and the
/// evaluation ledger and writes a string; nothing here holds a `&mut`
/// `ProviderConfig`, opens a config file for writing, or calls any `set_*`
/// method. The rendered line names the config key a person would edit
/// themselves — the suggestion stops at naming it.
fn capability_suggestions_section(runtime: &Runtime, effective: &EffectiveConfig<'_>) -> String {
    use glasshouse::config::ConfiguredWorkloadTier;
    use glasshouse::config::capability::CeilingResolution;
    use glasshouse::evaluation::{EvaluationObservations, TierOutcomeVerdict};

    let header = "\nCalibration suggestions from observed outcomes, last 30 days (map line \
                   1481)\n"
        .to_owned();
    let ledger = match EvaluationObservations::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::debug!(
                error = %format!("{err:#}"),
                "could not open the evaluation ledger for the calibration-suggestions section"
            );
            return format!("{header}\n  the evaluation ledger could not be opened\n");
        }
    };
    let to = glasshouse::evaluation::now_unix();
    let from = to - ROUTE_OUTCOME_WINDOW_DAYS * 24 * 60 * 60;
    let outcomes = match ledger.outcomes_by_tier(from, to) {
        Ok(outcomes) => outcomes,
        Err(err) => return format!("{header}\n  {err}\n"),
    };

    let calibrated = effective.calibrated_model_ceilings();
    let mut out = header;
    out.push('\n');
    let mut suggested = false;
    for outcome in &outcomes {
        // The gate: below MIN_SAMPLE_FOR_SUMMARY, `outcomes_by_tier` itself
        // has already declined to summarize this tier, and a suggestion
        // built from fewer reported turns than the section beside it trusts
        // for a summary would be a second, weaker threshold nobody asked for.
        let TierOutcomeVerdict::Measured {
            successful,
            failed,
            sample_size,
        } = outcome.verdict
        else {
            continue;
        };
        // The bucket is a display word (`RoutingTier::as_str`'s vocabulary,
        // e.g. `standard-escalated`); the escalation suffix is stripped
        // before parsing because a model's configured ceiling names the
        // tier itself, never whether a session was escalated into it.
        let Some(bucket_tier) =
            ConfiguredWorkloadTier::parse(outcome.bucket.trim_end_matches("-escalated"))
                .map(ConfiguredWorkloadTier::tier)
        else {
            continue;
        };
        for (provider, model, resolution) in &calibrated {
            let (configured_tier, provenance, config_key) = match resolution {
                CeilingResolution::UserCapabilityRecord(tier) => (
                    *tier,
                    "the user's own capability assignment",
                    format!("providers.{provider}.model_capabilities.{model}.ceiling"),
                ),
                CeilingResolution::Prior(Some(tier)) => (
                    *tier,
                    "a benchmark-derived prior",
                    format!("providers.{provider}.model_capabilities.{model}.ceiling"),
                ),
                _ => continue,
            };
            if configured_tier != bucket_tier {
                continue;
            }
            // The disagreement this section names: a majority of reported
            // turns at the model's own configured tier failed. Never acted
            // on — only rendered, with the exact key a person would edit.
            if failed > successful {
                suggested = true;
                let _ = writeln_str(
                    &mut out,
                    format!(
                        "  {provider}/{model} is configured at `{configured_tier}` by \
                         {provenance}, but {failed} of {sample_size} reported turns at this \
                         tier failed — consider lowering `{config_key}` (a suggestion; nothing \
                         was changed)"
                    ),
                );
            }
        }
    }
    if !suggested {
        out.push_str(
            "  no disagreement between observed outcomes and configured calibration met the \
             evidence gate\n",
        );
    }
    out
}

/// How many of [`support_work_section`]'s rows to print — the packet's own
/// "most recent N (say 10)".
const SUPPORT_WORK_RECENT_LIMIT: usize = 10;

/// Map line 1951, printed for a person: per-harness task efficiency —
/// tokens, wall-clock, request count, and outcome by task class — so that
/// harness choice can rest on evidence rather than on which vendor bills
/// for it.
///
/// Two ledgers, joined in Rust rather than in SQL, because they hold
/// different rows for different reasons. The outcome-and-task-class half
/// comes from
/// [`glasshouse::evaluation::EvaluationObservations::outcomes_by_tier_and_harness`]
/// — `evaluation_observations` joined to `sessions.harness`, exactly
/// [`tier_outcome_section`]'s own join with a harness dimension added, one
/// row per (harness, task class). The token/wall-clock/request-count half
/// comes from
/// [`glasshouse::routing::evidence::EvidenceLedger::request_stats_by_harness`]
/// — `routing_observations.harness`, written directly at record time — and
/// is computed once per harness rather than per task class, because
/// `routing_observations` carries no tier at all; the same per-harness
/// figures are printed on every task-class row for that harness. That is
/// the box line's own split: *"tokens, wall-clock, request count"*
/// unqualified, *"outcome … by task class"* qualified.
///
/// A harness with no `routing_observations` rows in the window (every
/// session routed but nothing dispatched yet) prints `0 request(s)` and
/// *"not exposed on 0 of 0 exchanges"* rather than being silently dropped
/// from the token/wall-clock figures.
fn harness_efficiency_section(runtime: &Runtime) -> String {
    use glasshouse::evaluation::{EvaluationObservations, TierOutcomeVerdict};
    use glasshouse::routing::evidence::EvidenceLedger;

    let header =
        "\nPer-harness task efficiency in this project, last 30 days (map line 1951)\n".to_owned();

    let to = glasshouse::evaluation::now_unix();
    let from = to - ROUTE_OUTCOME_WINDOW_DAYS * 24 * 60 * 60;

    let outcomes = match EvaluationObservations::open(runtime) {
        Ok(ledger) => match ledger.outcomes_by_tier_and_harness(from, to) {
            Ok(outcomes) => outcomes,
            Err(err) => return format!("{header}\n  {err}\n"),
        },
        Err(err) => {
            tracing::debug!(
                error = %format!("{err:#}"),
                "could not open the evaluation ledger for the harness-efficiency section"
            );
            return format!("{header}\n  the evaluation ledger could not be opened\n");
        }
    };

    let stats = match EvidenceLedger::open(runtime) {
        Ok(ledger) => match ledger.request_stats_by_harness(from, to) {
            Ok(stats) => stats,
            Err(err) => return format!("{header}\n  {err}\n"),
        },
        Err(err) => {
            tracing::debug!(
                error = %err,
                "could not open the routing evidence ledger for the harness-efficiency section"
            );
            return format!("{header}\n  the routing evidence ledger could not be opened\n");
        }
    };

    let mut out = header;
    out.push('\n');
    if outcomes.is_empty() {
        out.push_str("  no routed sessions recorded in this window\n");
        return out;
    }

    for row in outcomes.iter() {
        let request_stats = stats.iter().find(|s| s.harness == row.harness);

        let outcome_clause = match row.outcome.verdict {
            TierOutcomeVerdict::InsufficientEvidence {
                sample_size,
                required,
            } => format!(
                "insufficient evidence — {sample_size} of the {required} reported turns a tier \
                 summary needs; treated as no summary"
            ),
            TierOutcomeVerdict::Measured {
                successful,
                failed,
                sample_size,
            } => {
                format!("{successful} of {sample_size} reported turns succeeded, {failed} failed")
            }
        };
        let undecided_clause = if row.outcome.undecided > 0 {
            format!("; {} undecided", row.outcome.undecided)
        } else {
            String::new()
        };

        let requests = request_stats.map_or(0, |stats| stats.requests);
        let wall_clock_clause = match request_stats.and_then(|stats| stats.wall_clock) {
            Some(wall_clock) => format!(
                ", median wall-clock {}ms across {} timed exchange(s)",
                wall_clock.median_ms, wall_clock.sample_count
            ),
            None => String::new(),
        };
        // Below, `token_rows_present == 0` must render as an absence, never
        // as a printed `0` — the mutation this line is written to catch is
        // exactly `input_tokens_sum`/`output_tokens_sum` printed unguarded
        // when every row in the group left both `NULL`.
        let tokens_clause = match request_stats {
            None => "tokens: not exposed on 0 of 0 exchanges".to_owned(),
            Some(stats) if stats.token_rows_present == 0 => format!(
                "tokens: not exposed on {} of {} exchanges",
                stats.requests, stats.requests
            ),
            Some(stats) if stats.token_rows_present == stats.requests => format!(
                "tokens: {} in / {} out",
                stats.input_tokens_sum, stats.output_tokens_sum
            ),
            Some(stats) => format!(
                "tokens: {} in / {} out on {} of {} exchanges; not exposed on {}",
                stats.input_tokens_sum,
                stats.output_tokens_sum,
                stats.token_rows_present,
                stats.requests,
                stats.requests - stats.token_rows_present
            ),
        };

        let _ = writeln_str(
            &mut out,
            format!(
                "  {} — {}: {outcome_clause}{undecided_clause}; {requests} request(s)\
                 {wall_clock_clause}; {tokens_clause}",
                row.harness, row.outcome.bucket
            ),
        );
    }
    out
}

/// Map line 1629, printed for a person: which resource performed important
/// support work — classification or memory extraction — most recently, so
/// a person debugging *"which model classified this task?"* finds the
/// answer here rather than in the raw ledger.
///
/// Reads
/// [`glasshouse::routing::evidence::EvidenceLedger::recent_support_work`],
/// the purpose-filtered sibling of `Self::recent` that this line needs:
/// `recent` requires the caller to already name one `(provider, model,
/// route, harness)` identity, and the whole point of this section is that
/// the identity is unknown in advance — it is the question a person is
/// asking.
fn support_work_section(runtime: &Runtime) -> String {
    use glasshouse::routing::evidence::EvidenceLedger;

    let header = "\nRecent support work in this project (map line 1629) — which resource \
                   classified or extracted, for debugging\n"
        .to_owned();
    let ledger = match EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::debug!(
                error = %err,
                "could not open the routing evidence ledger for the support-work section"
            );
            return format!("{header}\n  the routing evidence ledger could not be opened\n");
        }
    };
    let recent = match ledger.recent_support_work(SUPPORT_WORK_RECENT_LIMIT) {
        Ok(rows) => rows,
        Err(err) => return format!("{header}\n  {err}\n"),
    };

    let mut out = header;
    out.push('\n');
    if recent.is_empty() {
        out.push_str("  no classification or memory-extraction call recorded yet\n");
        return out;
    }
    for observation in recent.iter() {
        let purpose = observation
            .purpose
            .as_deref()
            .unwrap_or("(no purpose recorded)");
        let route = observation
            .route
            .as_deref()
            .unwrap_or("(no route recorded)");
        let outcome = observation
            .outcome
            .map(glasshouse::routing::evidence::Outcome::as_str)
            .unwrap_or("unknown");
        let wall_clock = observation
            .duration_ms()
            .map(|ms| format!("{ms}ms"))
            .unwrap_or_else(|| "(not timed)".to_owned());
        let _ = writeln_str(
            &mut out,
            format!(
                "  at {}: {purpose} by {} / {} via {route} — {outcome}, {wall_clock}",
                observation.observed_at_unix, observation.provider, observation.model
            ),
        );
    }
    out
}

/// Map line 1851, as one sentence with its denominator — *"3 of 7 failovers
/// were steered off a shared upstream"*.
///
/// # Why this is a count and not a table
///
/// A prevention row carries no session and no turn outcome — the gateway
/// that ranks a failover holds no Glasshouse session id — so there is
/// nothing to bucket a success rate by. What line 1851 asks is *how often*,
/// and the honest shape of that is a numerator over the total of every
/// bucket the ledger holds, printed together.
///
/// A window with no failover prints so, and prints nothing that could be
/// read as a rate: `0 of 0` is not `0%`, and an absent measurement is never
/// rendered as a measured zero.
fn render_failover_preventions(counts: &[(String, i64)]) -> String {
    use glasshouse::evaluation::FailoverPrevention;

    let total: i64 = counts.iter().map(|(_, count)| *count).sum();
    let prevented = counts
        .iter()
        .find(|(subject, _)| subject == FailoverPrevention::Prevented.as_str())
        .map(|(_, count)| *count)
        .unwrap_or_default();
    if total == 0 {
        return "\n  no gateway failover was ranked in this window, so nothing is recorded \
                about what failure-domain evidence did to one (map line 1851)\n"
            .to_owned();
    }
    format!(
        "\n  {prevented} of {total} gateway failovers went somewhere the failure-domain term \
         changed the winner to — that many were steered off a candidate sharing the failed \
         backend's provider (map line 1851)\n"
    )
}

/// One table of [`RouteOutcomeCounts`], aligned on the bucket name.
fn render_route_outcome_rows(counts: &[glasshouse::evaluation::RouteOutcomeCounts]) -> String {
    if counts.is_empty() {
        return "    (nothing recorded)\n".to_owned();
    }
    let width = counts
        .iter()
        .map(|row| row.bucket.chars().count())
        .max()
        .unwrap_or_default();
    counts
        .iter()
        .map(|row| {
            format!(
                "    {:width$} : {}\n",
                row.bucket,
                render_route_outcome_line(row)
            )
        })
        .collect()
}

/// One bucket's counts, as a sentence with both its denominators in it.
///
/// **Never a percentage, and never a completion count without the number of
/// turns it is out of.** The two denominators are different quantities —
/// turns a harness reported on, and sessions that were routed — and a
/// rendering that dropped either would let a reader divide the wrong pair.
fn render_route_outcome_line(counts: &glasshouse::evaluation::RouteOutcomeCounts) -> String {
    let reported = counts.reported_turns();
    let verdicts = if reported == 0 {
        "no reported turns".to_owned()
    } else {
        format!(
            "{} of {reported} reported turns completed",
            counts.completed
        )
    };
    let sessions = format!(
        "{} session{} routed",
        counts.sessions,
        if counts.sessions == 1 { "" } else { "s" }
    );
    if counts.sessions_without_outcome > 0 {
        format!(
            "{verdicts}; {sessions}, {} with no turn end reported",
            counts.sessions_without_outcome
        )
    } else {
        format!("{verdicts}; {sessions}")
    }
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
    let earliest_unix = now_unix.saturating_sub(window_seconds);
    let groups = ledger.consumption_by_purpose(now_unix, window_seconds)?;
    let translation = ledger.translation_cache_savings(now_unix, window_seconds)?;
    // Fail-soft, the same posture `context_firewall_savings_summary` already
    // takes for `status`: a raw store this build cannot read yet renders as
    // "not counted", never as a hard error for a readout command.
    let store = glasshouse::firewall::RawStore::open(runtime.state_dir().join("context-firewall"));
    let firewall_savings = store.savings_in_window(earliest_unix, now_unix).ok();
    let bypass_count: usize = groups
        .iter()
        .filter(|group| {
            group.purpose.as_deref()
                == Some(glasshouse::routing::evidence::CONTEXT_FIREWALL_BYPASS_PURPOSE)
        })
        .map(|group| group.sample_count)
        .sum();
    Ok(render_routing_cost(
        runtime.project().id().as_str(),
        hours,
        &groups,
        firewall_savings,
        bypass_count,
        &translation,
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
    firewall_savings: Option<glasshouse::firewall::WindowSavings>,
    firewall_bypasses: usize,
    translation: &[glasshouse::routing::evidence::TranslationSavings],
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
    out.push_str(&render_savings_section(
        firewall_savings,
        firewall_bypasses,
        translation,
    ));
    out
}

/// Map line 2034: what was saved, by purpose, each figure with its own
/// denominator — Phase 58's ingestion of Headroom's *"savings readout that
/// is a query over the ledger"* (design-decisions.md, *"Headroom,
/// compared"*, Taken item 4). Three facets, and the same rule
/// [`render_routing_cost`]'s own doc comment states for every other figure
/// in this report: a quantity nobody recorded prints as words, never as a
/// digit and never as `0`.
fn render_savings_section(
    firewall_savings: Option<glasshouse::firewall::WindowSavings>,
    firewall_bypasses: usize,
    translation: &[glasshouse::routing::evidence::TranslationSavings],
) -> String {
    let mut out = String::from("\nSAVINGS\n");

    out.push_str("\n  context firewall\n");
    match firewall_savings {
        Some(savings) if savings.results > 0 || firewall_bypasses > 0 => {
            let total = savings.results + firewall_bypasses;
            let unestimated_note = if savings.unestimated > 0 {
                format!(" ({} without a recorded estimate)", savings.unestimated)
            } else {
                String::new()
            };
            out.push_str(&format!(
                "    kept local (estimated) {} tokens across {} reductions of {total} results \
                 above threshold{unestimated_note}\n",
                savings.kept_local, savings.results
            ));
        }
        _ => {
            out.push_str("    not counted: no context-firewall activity recorded in this window\n")
        }
    }

    if translation.is_empty() {
        out.push_str("\n  translation\n    not counted: no translated exchange recorded\n");
    } else {
        for row in translation {
            let route = row.route.as_deref().unwrap_or("(no route recorded)");
            let quota_context = row
                .quota_context
                .as_deref()
                .unwrap_or("(no credential recorded)");
            out.push_str(&format!("\n  translation {route} / {quota_context}\n"));
            let denominator = row.input_tokens + row.cached_input_tokens;
            let ratio = row
                .cache_read_ratio()
                .map(|fraction| format!("{:.1}%", fraction * 100.0))
                .unwrap_or_else(|| "not counted".to_owned());
            out.push_str(&format!(
                "    prompt-cache reads {} of {denominator} translated input tokens ({ratio})\n",
                row.cached_input_tokens
            ));
        }
    }

    out.push_str(
        "\n  response profile\n    not counted: no exchange row carries a response profile \
         (no producer stamps one on a routing-observation row, and there is no session column \
         to join `sessions.response_profile` through — a schema decision, not this package's)\n",
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
///
/// Capability map line 1330 began stamping that same relay traffic with
/// [`glasshouse::routing::evidence::HARNESS_TURN_PURPOSE`], so the stamped and
/// the unstamped rows are **one fact across a build boundary**, not two — the
/// identical treatment `RoutingOverhead::from_consumption` gives them. Without
/// the first arm below a stamped row falls through to the general case and the
/// report prints the raw constant `harness-turn` where a person used to read
/// this label; `tests/routing_cost.rs` caught exactly that.
fn purpose_group_label(group: &glasshouse::routing::evidence::PurposeConsumption) -> &str {
    match (group.purpose.as_deref(), group.harness_recorded) {
        (Some(glasshouse::routing::evidence::HARNESS_TURN_PURPOSE), _) | (None, true) => {
            "coding-agent (gateway relay)"
        }
        (Some(purpose), _) => purpose,
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
/// Not gated, since Phase 43. It used to be `#[cfg(unix)]` to match its
/// only consumer, `api::unix`, which was itself gated; the handlers in that
/// module now compile on every platform because the MCP door reaches them
/// over stdio, so this is live everywhere too — and a gate left here would
/// be a Windows build error, which the cross-check caught.
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

/// The session router with the user's reserve configuration attached —
/// lines 1571 and 1577 (`routing.reserve.interactive`) and line 1290
/// (`routing.reserve_override_sessions`) on every path that ranks, so the
/// path that acts and the path that reports cannot disagree about either.
fn session_router(
    runtime: &Runtime,
    effective: &EffectiveConfig<'_>,
    user_override: glasshouse::routing::session::RoutingOverride,
) -> glasshouse::routing::session::SessionRouter {
    glasshouse::routing::session::SessionRouter::with_override(user_override)
        .with_reserve_policies(effective.reserve_policies())
        .with_reserve_override_sessions(effective.reserve_override_sessions().value)
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
/// One qualification since Phase 34D: with a routing model **configured** and
/// a `--task` stated, this asks that model exactly as a launch would — so the
/// two cannot disagree about the classification — and the *cost* of that one
/// call is recorded under `purpose = "classification"`, as every routing-model
/// call is. That is a fact about what the diagnostic spent, not about the
/// work, which is still not executed. It never writes the sticky
/// classification record a launch leaves behind (`remember_classification`),
/// because a preview is not a decision.
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
    use glasshouse::routing::session::{RouterInputs, RoutingMoment};

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
        let everything = routing_destinations(
            runtime,
            effective,
            harness,
            DestinationScope::Everything,
            task,
        )?;
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
    let health = observed_provider_health(runtime, effective, &destinations);
    // Phase 34D on the path that reports: the same classifier the launch
    // path calls, over the same destinations, so the explanation printed
    // here is the one a launch would act on. No sticky record is consulted
    // — this path ranks across every harness and decides nothing, so the
    // one difference from a launch is that it always asks rather than
    // reusing, and `classify_for_routing`'s doc says so.
    let classified = classify_for_routing(
        runtime,
        effective,
        RoutingClassificationSite {
            task,
            moment,
            harness: None,
            harness_named: false,
            to,
            fresh,
            destinations: &destinations,
            health: health.pool(),
            sticky: None,
            text_cache: None,
        },
    );
    let inputs = RouterInputs {
        overrides: &overrides,
        health: health.pool(),
        now: std::time::Instant::now(),
        requirements: classified
            .as_ref()
            .map(|classified| classified.answer.requirements())
            .unwrap_or_default(),
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

    // Line 1564's caller: at a task boundary the work is somewhere, and how
    // its last exchange there ended is a fact the ledger holds. Read only
    // when there is a current destination — a session start has no last
    // attempt to promote after — and handed to the router, which promotes
    // on a model-capability class and names any other.
    let retry_after = current
        .as_ref()
        .and_then(|current| latest_failure_class(runtime, current));
    let Some(routed) = session_router(runtime, effective, user_override)
        .with_retry_after(retry_after)
        .choose(moment, current.as_ref(), &destinations, &inputs)
    else {
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
            .pool()
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

/// What one routing decision classified the work as, and the facts that
/// answer was conditioned on — Phase 34D's answer beside Phase 34E's
/// fingerprint. `None` from [`classify_for_routing`] when no task was
/// stated, which is every launch and every `route` that reproduces the
/// pre-classification behaviour byte for byte.
struct ClassifiedRouting {
    answer: glasshouse::routing::request::RouterAnswer,
    fingerprint: glasshouse::routing::request::RoutingFingerprint,
}

/// Everything [`classify_for_routing`] needs to build the router request
/// from what its caller already holds — never a file, a transcript, an
/// environment variable or a credential, which is map lines 1425, 1426,
/// 1455 and 1456 made structural (see `routing::request`'s header).
struct RoutingClassificationSite<'a> {
    /// `--task`. Absent or blank means "classify nothing".
    task: Option<&'a str>,
    moment: glasshouse::routing::session::RoutingMoment,
    /// The harness this decision is for. `None` for `glasshouse route`,
    /// which ranks across every enabled harness.
    harness: Option<glasshouse::integrations::IntegrationId>,
    /// Whether the person named the harness on the command line (line
    /// 1450's "pinned harness") rather than letting the one enabled harness
    /// be selected.
    harness_named: bool,
    to: Option<&'a str>,
    fresh: bool,
    destinations: &'a [glasshouse::routing::session::Destination],
    health: &'a glasshouse::routing::free::FreePool,
    /// The sticky record to consult for line 1467. `Some` on the path that
    /// acts; `None` on the path that reports, which never reuses.
    sticky: Option<&'a ClassificationStickyCache>,
    /// Line 1469's text-keyed cache. `Some` on the path that acts; `None` on
    /// the path that reports — the same reason `sticky` is `None` there:
    /// `route`'s own comment says it always asks rather than reusing, and a
    /// diagnostic that answers from yesterday's cache is not a diagnostic.
    text_cache: Option<&'a ClassificationTextCache>,
}

/// Deterministic heuristics' answer for `text`, with the reason they answered.
///
/// The one producer of a heuristic [`RouterAnswer`] in this binary, called
/// on every path that ends up not asking a model — no routing model
/// configured (line 1471), an explicit destination (line 1470), or a model
/// that did not answer — so those three paths cannot classify differently.
fn heuristic_answer(
    text: &str,
    reason: glasshouse::routing::request::HeuristicReason,
) -> glasshouse::routing::request::RouterAnswer {
    use glasshouse::routing::request::{AnswerProvenance, RouterAnswer};

    RouterAnswer::new(
        glasshouse::routing::classify::classify_heuristically(text),
        AnswerProvenance::Heuristic(reason),
    )
}

/// Classify the work a routing decision is about — Phase 34D's producer,
/// and the one place the four Phase 34E economy rules this package closes
/// are decided.
///
/// Returns `None` when no task was stated: no request is built, no model is
/// asked, no ledger is opened, and the caller hands the router
/// `TaskRequirements::default()` exactly as it did before this existed.
///
/// With a task, in this order:
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
///
/// Line 1459 is applied by every reader of the answer rather than here:
/// [`RouterAnswer::requirements`] carries the *conservative* tier, and
/// [`RouterAnswer::explain`] says when confidence was low and what it did.
fn classify_for_routing(
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
                            None => match classify_with_routing_model(runtime, &request) {
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
fn classification_cache_resolution_tag(
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
struct ClassificationStickyCache {
    path: std::path::PathBuf,
}

impl ClassificationStickyCache {
    fn new(paths: &glasshouse::paths::RuntimePaths, project_id: &str) -> Self {
        Self {
            path: paths
                .project_state_dir(project_id)
                .join("routing-classification.json"),
        }
    }

    fn load(&self) -> Option<glasshouse::routing::request::StickyClassification> {
        let bytes = std::fs::read(&self.path).ok()?;
        glasshouse::routing::request::StickyClassification::from_json(&bytes)
    }

    fn store(&self, record: &glasshouse::routing::request::StickyClassification) {
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
const CLASSIFICATION_TEXT_CACHE_CAPACITY: usize = 64;

/// Where line 1469's text-keyed cache is kept — the same project-scoped
/// directory as [`ClassificationStickyCache`] and
/// [`glasshouse::provider::telemetry::RoutingStickyCache`], and the same
/// file shape, except the record is a map keyed by
/// [`glasshouse::routing::request::normalised_task_key`] rather than a
/// single value: one JSON file, written to a temporary name and renamed,
/// every read failure answering an empty cache rather than an error.
struct ClassificationTextCache {
    path: std::path::PathBuf,
}

impl ClassificationTextCache {
    fn new(paths: &glasshouse::paths::RuntimePaths, project_id: &str) -> Self {
        Self {
            path: paths
                .project_state_dir(project_id)
                .join("routing-classification-cache.json"),
        }
    }

    fn load(
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
    fn lookup(&self, key: &str) -> Option<glasshouse::routing::request::CachedClassification> {
        self.load().remove(key)
    }

    fn store(&self, record: glasshouse::routing::request::CachedClassification) {
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

/// Leave this decision's classification behind for the next one — line
/// 1467's write side, called once the session the work landed on has an
/// identifier. Only an answer a model actually gave is worth remembering:
/// heuristics are free to re-run, and a reused answer is already on disk.
fn remember_classification(
    cache: &ClassificationStickyCache,
    classified: Option<&ClassifiedRouting>,
    session: &str,
) {
    let Some(classified) = classified else {
        return;
    };
    if !classified.answer.provenance().asked_a_model() {
        return;
    }
    cache.store(&glasshouse::routing::request::StickyClassification::new(
        session,
        classified.fingerprint.clone(),
        classified.answer.classification(),
        glasshouse::provider::cache::now_unix_seconds(),
    ));
}

/// What `routing_observations.purpose` records for map line 1849's
/// measurement. Spelled once, like [`CLASSIFICATION_PURPOSE`], and now in
/// `routing::evidence` beside it, because `RoutingOverhead` reads this word
/// back and a second spelling would split the only producer from the only
/// reader.
const ROUTING_LATENCY_PURPOSE: &str = glasshouse::routing::evidence::ROUTING_LATENCY_PURPOSE;

/// What `routing_observations.purpose` records for a memory-extraction call
/// — capability map line 1832. Aliased from the ledger's own constant for
/// [`CLASSIFICATION_PURPOSE`]'s reason.
const EXTRACTION_PURPOSE: &str = glasshouse::routing::evidence::EXTRACTION_PURPOSE;

/// Map line 1849: record what routing added to this launch, from the start
/// of the decision (`started`) to its end — the point after which profile
/// resolution, the gateway and the process spawn happen identically whether
/// or not a task was stated, and are therefore the launch's own cost rather
/// than routing's.
///
/// Called only when a classification ran, so a launch that states no task
/// opens no ledger (practice §65) and leaves no row: the row's absence is
/// the honest reading of "nothing was added". Opened, written and dropped
/// here, before any gateway holds its own handle.
///
/// The ledger's timing columns are unix **seconds** (migration 11), so a
/// sub-second decision reads back as `0` through `duration_ms()`; the
/// millisecond figure goes to the log beside it. A finer column is a schema
/// decision this package does not take.
fn record_routing_latency(
    runtime: &Runtime,
    started: std::time::Instant,
    started_at_unix: i64,
    harness: glasshouse::integrations::IntegrationId,
    answer: &glasshouse::routing::request::RouterAnswer,
) {
    let elapsed = started.elapsed();
    let completed_at_unix = glasshouse::provider::cache::now_unix_seconds();
    tracing::info!(
        elapsed_ms = elapsed.as_millis() as u64,
        asked_a_model = answer.provenance().asked_a_model(),
        "routing decision latency before the harness starts"
    );
    let ledger = match glasshouse::routing::evidence::EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "routing evidence ledger unavailable; routing latency is not recorded"
            );
            return;
        }
    };
    let observation =
        glasshouse::routing::evidence::NewObservation::new("glasshouse", "session-router")
            .with_harness(Some(harness.slug()))
            .with_purpose(Some(ROUTING_LATENCY_PURPOSE))
            // Map line 1276's missing link, and the reason migration 23
            // exists: `answer` has carried a `TaskClass` since Phase 34C and
            // this row — the one row every routed request produces — has
            // never written it down. `glasshouse::routing::burn` reads it
            // back.
            .with_task_class(Some(answer.task_class()))
            .with_timing(Some(started_at_unix), Some(completed_at_unix));
    if let Err(err) = ledger.record(observation, completed_at_unix) {
        tracing::warn!(error = %err, "could not record routing latency");
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

// Eight, and the eighth arrived at integration: `external` is Phase 17's and
// `guardrail` is Phase 21K's, written by two packages that never shared a
// tree. Neither belongs in `LaunchDestination` -- that bundle answers *where
// the work goes*, and one of these says where the session is *shown* while
// the other says how hard its premises are *gated*. Folding either in to
// satisfy a lint would put an unrelated fact in a named type.
#[allow(clippy::too_many_arguments)]
fn launch_session(
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
        fresh,
        task,
        no_routing,
        checkpoint_first,
    } = destination;
    // Map line 1849: the routing decision is timed from here. Whether the
    // figure is ever recorded is decided by whether a task was stated.
    let routing_started = std::time::Instant::now();
    let routing_started_at_unix = glasshouse::provider::cache::now_unix_seconds();
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
    // Map line 372's remaining clause: with no profile named, the router is
    // asked to rank every *enabled* profile rather than the one implied
    // fallback below picks for it. `--to`, `--fresh` and `--from-checkpoint`
    // all leave `named_profile` unset too, and none of them names a profile
    // either — the ranking still gets to pick which one a fresh session
    // would run under; only `--to fresh:<harness>:<profile>` and `--profile`
    // count as "the user pinned one," because those are the only two that
    // said so by name.
    let profile_selection = named_profile.is_none();
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

    // -----------------------------------------------------------------------
    // Line 1712: the off switch, and it is taken **here** — before
    // `routing_destinations` opens this project's session store, its quota
    // cache and its health cache, and before anything classifies this
    // launch's task.
    //
    let sticky_cache =
        ClassificationStickyCache::new(runtime.paths(), runtime.project().id().as_str());
    let text_cache = ClassificationTextCache::new(runtime.paths(), runtime.project().id().as_str());
    // # Why routing off does not report what it would have done
    //
    // The obvious courtesy is to rank anyway and print *"routing is off; it
    // would have continued session X"*. That is exactly the work the person
    // turned off. The ranking's inputs are not free: three on-disk stores are
    // opened to build the destinations — practice §65 is this project's
    // record of what an unnecessary open handle costs on the platform it does
    // not develop on, where SQLite's locks are mandatory rather than advisory
    // — and the task classification on this path can reach a routing model.
    // Doing all of it to render one sentence would make "off" mean "the same
    // work, silently, and then a message about it".
    //
    // So the line says routing is off and where that was decided, and points
    // at `glasshouse route`, which answers *"what would have happened"* on
    // demand and starts nothing. Asking is a thing a person does
    // deliberately; being charged for the answer is not.
    // -----------------------------------------------------------------------
    let automatic = effective.automatic_routing();
    let routing_off = no_routing || !automatic.value;
    if routing_off {
        if no_routing {
            eprintln!(
                "glasshouse: automatic routing is off for this launch (--no-routing), so no \
                 ranking was taken. `glasshouse route` shows what it would have chosen, \
                 without starting anything."
            );
        } else {
            eprintln!(
                "glasshouse: automatic routing is off {}, so no ranking was taken. \
                 `glasshouse route` shows what it would have chosen, without starting \
                 anything.",
                automatic.layer.describe_source()
            );
        }
        // A `--to` naming a session this project already has is the one thing
        // that still moves work into an existing session with the ranking
        // off. It is not the ranking deciding — it is the person, and turning
        // the ranking off was never a statement about their own flags.
        //
        // A `fresh:<harness>:<profile>` identifier falls through instead: it
        // names a session that does not exist yet, and starting it is what
        // the rest of this function already does under `fresh_profile`, which
        // `named_profile` has already read that identifier's profile out of.
        if let Some(id) = to
            && fresh_destination_profile(id, selection.id()).is_none()
        {
            eprintln!(
                "glasshouse: continuing session `{id}` because you named it; with routing off, \
                 nothing else was considered."
            );
            if checkpoint_first {
                checkpoint_before_moving(runtime, Some(id))?;
            }
            return resume_session(
                runtime,
                id,
                harness_args,
                headless,
                RouteOnResume::AlreadyRouted,
            );
        }
    }

    // Line 1712 again: with the ranking off, none of this runs at all —
    // not the three stores `routing_destinations` opens, not the health
    // bridge, not `choose`. `routed` is `None`, which the tail below already
    // handles as "there was no routing decision", and the fresh destination
    // is the profile this launch resolved on its own.
    let (routed, classified, health) = if routing_off {
        (
            None,
            None,
            ObservedHealth {
                pool: glasshouse::routing::free::FreePool::new(),
                observed_at: Vec::new(),
            },
        )
    } else {
        // Map line 372: automatic routing is on here (`routing_off` is
        // false only when it is), so the fresh side of the candidate set
        // widens to every enabled profile exactly when the person did not
        // pin one — `Launchable` unchanged for a pin, `Launchable` unchanged
        // for automatic off (that branch never reaches this arm at all).
        let scope = if profile_selection {
            DestinationScope::LaunchableAcrossProfiles
        } else {
            DestinationScope::Launchable {
                profile: fresh_profile,
            }
        };
        let destinations = routing_destinations(runtime, &effective, selection.id(), scope, task)?;
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
        let health = observed_provider_health(runtime, &effective, &destinations);
        // Phase 34D, on the path that acts: what the work *is* decides what the
        // destination must be able to do. `None` — no `--task` — hands the
        // router `TaskRequirements::default()` and asks nothing, which is this
        // launch exactly as it was before classification existed.
        let classified = classify_for_routing(
            runtime,
            &effective,
            RoutingClassificationSite {
                task,
                moment: glasshouse::routing::session::RoutingMoment::SessionStart,
                harness: Some(selection.id()),
                harness_named: harness.is_some(),
                to,
                fresh,
                destinations: &destinations,
                health: health.pool(),
                sticky: Some(&sticky_cache),
                text_cache: Some(&text_cache),
            },
        );
        let inputs = glasshouse::routing::session::RouterInputs {
            overrides: &overrides,
            health: health.pool(),
            now: std::time::Instant::now(),
            requirements: classified
                .as_ref()
                .map(|classified| classified.answer.requirements())
                .unwrap_or_default(),
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
        let router = session_router(runtime, &effective, user_override);
        let routed = router.choose(
            glasshouse::routing::session::RoutingMoment::SessionStart,
            None,
            &destinations,
            &inputs,
        );
        // Phase 56 line 1954, the half a ranking cannot express. `choose`
        // answers `None` when every destination failed a hard constraint and
        // there is no current session to hold — and until now this launch read
        // that as "nowhere to go" and started `fresh_profile` anyway, the
        // silence Phase 35D's decision 3 recorded. A destination the user's
        // own entitlement rule refused must not be started by that fallback,
        // so the same gate is asked what it refused, and the launch stops by
        // name. Only the entitlement constraint is read here: a protocol or
        // tool-semantics refusal of the sole destination keeps the behaviour it
        // had, which `profile::resolve` already refuses on its own terms.
        let nowhere_to_go = routed.is_none();
        if nowhere_to_go
            && let Some(refused) = router.refused(&destinations, &inputs).into_iter().find(
                |(destination, constraint)| {
                    destination.is_fresh()
                        && destination.launch_profile() == fresh_profile
                        && matches!(
                            constraint,
                            glasshouse::routing::HardConstraint::Entitlement { .. }
                        )
                },
            )
        {
            let (_, constraint) = refused;
            let name = match &constraint {
                glasshouse::routing::HardConstraint::Entitlement { entitlement, .. } => {
                    entitlement.clone()
                }
                _ => unreachable!("filtered to the entitlement constraint above"),
            };
            eprintln!(
                "glasshouse: not starting this session — {}, and launch profile `{fresh_profile}` \
                 would charge it. Change the rule under `[entitlements.{name}]`, or launch \
                 under a profile whose entitlement serves this work.",
                constraint.reason().unwrap_or_default()
            );
            return Ok(ExitCode::FAILURE);
        }
        if let Some(classified) = &classified {
            // The classification the decision just acted on, in the same words
            // `glasshouse route --task` prints — including whether line 1459's
            // conservative rules fired. And the end of what routing added to
            // this launch (line 1849), recorded before anything below opens a
            // ledger handle of its own.
            eprintln!("glasshouse: {}", classified.answer.explain());
            // Lines 1565 and 1566, on the path that acts: a moved tier is
            // said before the destination it produced is announced below,
            // and recorded so it can be counted. `glasshouse route` renders
            // the same movement in its report and records nothing.
            if let Some(routed) = &routed
                && let Some(movement) = routed.movement().filter(|movement| movement.fired())
            {
                eprintln!(
                    "glasshouse: tier {}. `glasshouse route --task ...` says why; `--to <id>` \
                     overrules it.",
                    movement.describe()
                );
                record_tier_movement(runtime, selection.id(), movement);
            }
            record_routing_latency(
                runtime,
                routing_started,
                routing_started_at_unix,
                selection.id(),
                &classified.answer,
            );
        }
        // Line 1970, on the path that acts — and OUTSIDE the classified
        // guard, because a fallback is not a classification and a launch
        // that states no task can still make one. The account the broker
        // left is said before the destination it produced is announced
        // below, and recorded so it can be counted. `glasshouse route`
        // renders the same fallback in its report and records nothing.
        if let Some(routed) = &routed
            && let Some(fallback) = routed.fallback()
        {
            eprintln!(
                "glasshouse: {}. `glasshouse route` says why.",
                fallback.describe()
            );
            record_entitlement_fallback(
                runtime,
                selection.id(),
                routed.chosen(),
                fallback,
                routed.cost(),
            );
        }
        (routed, classified, health)
    };

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
        // -------------------------------------------------------------------
        // Line 1720: *"surface automation decisions instead of silently
        // moving work between sessions."* Every automated outcome this
        // function can reach says so here, before it happens — an override
        // that was refused, an override that was honoured, a continuation, or
        // a fresh session the ranking chose over destinations it could have
        // continued. The one case with nothing to announce is a project with
        // no alternative: a ranking of one destination moved nothing.
        // -------------------------------------------------------------------
        if let Some(refusal) = routed.override_refused() {
            eprintln!(
                "glasshouse: your routing override was not applied — {refusal}. The ranking's \
                 own choice was used instead."
            );
        }
        if let Some(automatic) = routed.overrode() {
            eprintln!(
                "glasshouse: going to `{}` because you named it; the ranking would have chosen \
                 `{automatic}`. `glasshouse route` says why.",
                routed.chosen().id()
            );
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
            // Map lines 1835 and 1854, on the branch where no session has to
            // be minted for the route to have somewhere to land: the
            // destination *is* the session this work continues, so its id is
            // already the session id. The fresh branch records the same two
            // rows once `store.create` below has produced one.
            let observed_at = glasshouse::evaluation::now_unix();
            glasshouse::evaluation::record_routed_session(
                runtime,
                routed.chosen().id(),
                routed.chosen().id(),
                routed_cost_class(&user, project.as_ref(), routed.chosen()),
                routing_evidence_for(&health, routed.chosen(), observed_at),
                routed_tier(classified.as_ref()),
                observed_at,
            );
            // Line 1467: the session this work landed on is the sticky one.
            remember_classification(&sticky_cache, classified.as_ref(), routed.chosen().id());
            // Line 1716, on the path that migrates. Taken before
            // `resume_session` so the checkpoint describes the moment the
            // work left, and after the announcement above so the order a
            // person reads matches the order things happened.
            if checkpoint_first {
                checkpoint_before_moving(runtime, Some(routed.chosen().id()))?;
            }
            // Phase 17 line 760, on the branch that continues rather than
            // mints: the session this pane now hosts was recorded somewhere
            // else, so its record is moved here before it is resumed. Opened
            // and dropped before `resume_session` opens its own connection —
            // sequential, never two live handles (practice §65).
            if let Some(pane) = &hosted_pane {
                let sessions = ProjectSessions::open(runtime)?;
                sessions.store().set_presentation(
                    &SessionId::new(routed.chosen().id()),
                    SessionPresentation::External,
                    Some(pane.as_str()),
                )?;
            }
            return resume_session(
                runtime,
                routed.chosen().id(),
                harness_args,
                headless,
                RouteOnResume::AlreadyRouted,
            );
        }
        // A fresh session the *ranking* chose, with sessions it could have
        // continued and did not. Said out loud for the same reason the
        // continuation above is: the person is about to start over, and the
        // moment to learn that this project already had somewhere warm to go
        // is before the new session exists rather than after.
        //
        // Only when the ranking chose it. A `--fresh` the person typed is
        // already reported by the override line above, and repeating it as an
        // automation decision would attribute their own choice to Glasshouse.
        if routed.overrode().is_none() {
            let continuable = routed
                .considered()
                .iter()
                .filter(|(destination, _)| !destination.is_fresh())
                .count();
            if continuable > 0 {
                eprintln!(
                    "glasshouse: starting a new session; the ranking weighed {continuable} \
                     session(s) this project could have continued and preferred a new one. \
                     `glasshouse route` says why, and `--to <id>` overrules it."
                );
            }
            // Map line 372: no profile was pinned, so this fresh destination
            // is the ranking's own pick among every enabled profile rather
            // than the implied fallback — said out loud, reusing the same
            // `render()` `glasshouse route` prints rather than inventing a
            // second explanation for the same decision.
            if profile_selection {
                eprintln!(
                    "glasshouse: launching under profile `{}` — automatic routing's choice \
                     among the enabled profiles.\n{}",
                    routed.chosen().launch_profile(),
                    routed.render()
                );
            }
        }
    }

    // Line 1716 on every path that did **not** migrate into an existing
    // session — the fresh launches, and a project with nothing recorded. The
    // flag is a no-op here and says so rather than passing silently, because
    // a person who asked for a checkpoint and got none needs to know which of
    // the two happened.
    if checkpoint_first {
        checkpoint_before_moving(runtime, None)?;
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
        // …and, since line 1712, the ordinary answer whenever routing is off:
        // the profile this launch already resolved, which is `--profile`, the
        // profile named inside a `--to fresh:<harness>:<profile>`, or the
        // implied Native one. Reading `fresh_profile` rather than
        // `profile_name` is what makes a `--to` identifier mean the same
        // thing with the ranking off as it does with it on.
        .unwrap_or_else(|| fresh_profile.to_owned());

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
    // started — and the harness half of that entitlement's rule applied once
    // more here, through the same `EntitlementRules::refusal` the router
    // asked, for the one launch the router never saw: line 1712's routing-off
    // launch, where `routing_destinations` and `choose` do not run at all. A
    // rule about *this harness* needs no classification to apply; the tier
    // half does, and it is the router's (above). A contradiction in the
    // `[entitlements]` tables is refused here for the same reason a bad
    // profile is: it must cost nothing.
    //
    // 56A line 1969, the routing half: when the router chose a candidate
    // that carries an entitlement, THAT entry serves — resolved by name from
    // the same tables, never re-derived through the one-account lookup,
    // which a provider several accounts legitimately back would refuse as
    // ambiguous. The one-account lookup remains the answer for a launch the
    // router never saw (routing off) and for a chosen candidate no entry
    // describes; with routing off, a several-account provider is still
    // refused, because without the broker's ranking there is nothing honest
    // to pick an account by.
    let chosen_entitlement_name = routed
        .as_ref()
        .and_then(|routed| routed.chosen().entitlement())
        .map(|entitlement| entitlement.name().to_owned());
    // `mut`: the `GlasshouseGateway` arm below overwrites this once the
    // gateway has started and its serving provider is known — see the
    // consult after `start_if_required_with_degrade_sink`. For every other
    // backend this is the final value.
    let mut entitlement = match &chosen_entitlement_name {
        Some(name) => match effective.entitlements() {
            Ok(pool) => pool.into_iter().find(|entry| entry.name() == name),
            Err(err) => {
                eprintln!("glasshouse: {err}");
                return Ok(ExitCode::FAILURE);
            }
        },
        None => match effective.entitlement_for(launch_profile.harness, &launch_profile.backend) {
            Ok(entitlement) => entitlement,
            Err(err) => {
                eprintln!("glasshouse: {err}");
                return Ok(ExitCode::FAILURE);
            }
        },
    };
    // Every backend but the gateway asks and announces right here, before
    // anything else is resolved. A `GlasshouseGateway` profile cannot be
    // asked yet — `entitlement_for` returns `None` for it by construction,
    // because no provider is assigned until the gateway starts below — so
    // its consult, refusal and announcement happen once that provider is
    // known (see `start_if_required_with_degrade_sink`, further down).
    let is_gateway_backend = matches!(
        launch_profile.backend,
        glasshouse::profile::BackendResource::GlasshouseGateway
    );
    if !is_gateway_backend {
        if let Some(message) = entitlement_refusal_message(
            entitlement.as_ref(),
            launch_profile.harness,
            &launch_profile.name,
        ) {
            eprintln!("{message}");
            return Ok(ExitCode::FAILURE);
        }
        announce_entitlement(entitlement.as_ref(), &launch_profile, None);
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
        // Capability map line 1851: what the failure-domain term did to each
        // failover this gateway takes. A sink rather than a ledger handle —
        // `gateway::session::FailoverPreventionSink`'s own doc comment has
        // practice §65's reason — so the evaluation ledger is opened, written
        // and dropped inside the exchange thread that decided the failover,
        // and never held open across the provider hop.
        Some(failover_prevention_sink(runtime)),
    ) {
        Ok(gateway) => gateway,
        Err(err) => {
            eprintln!("glasshouse: {err}");
            return Ok(ExitCode::FAILURE);
        }
    };

    // Phase 56/1954, the gateway shape: now that the gateway has started,
    // its serving provider is known (`Gateway::serving_provider`), and this
    // asks the same question the direct/native path already asked above —
    // the same `EntitlementRules::refusal` check, the same refusal text
    // (`entitlement_refusal_message`), the same announcement — for the one
    // launch that could not be asked before the gateway existed.
    // `pool_entitlements_for` still returns nothing for `GlasshouseGateway`
    // (map line 1954's cause 3 stays true of the *router*), so
    // `chosen_entitlement_name` above is never `Some` for this backend; this
    // is the whole of the gateway's consult.
    if is_gateway_backend {
        let gateway_provider = gateway.as_ref().map(|gateway| gateway.serving_provider());
        entitlement = match gateway_provider {
            Some(provider) => match effective.entitlement_for_provider(provider) {
                Ok(entitlement) => entitlement,
                Err(err) => {
                    eprintln!("glasshouse: {err}");
                    return Ok(ExitCode::FAILURE);
                }
            },
            None => None,
        };
        if let Some(message) = entitlement_refusal_message(
            entitlement.as_ref(),
            launch_profile.harness,
            &launch_profile.name,
        ) {
            eprintln!("{message}");
            return Ok(ExitCode::FAILURE);
        }
        announce_entitlement(entitlement.as_ref(), &launch_profile, gateway_provider);
    }

    // 56A line 1969: the overlay may only resolve the serving account's own
    // credential — see `EntitlementScopedSecrets`. With zero or one
    // configured entitlement the foreign list is empty or names other
    // resources' accounts, and resolution answers exactly as before. The
    // gateway's own upstream resolution above deliberately keeps the
    // unwrapped store: which account serves a gateway-backed session is
    // assigned when the session starts (56A-4), not at this launch.
    let scoped_secrets = EntitlementScopedSecrets {
        inner: &secrets,
        foreign: effective
            .foreign_entitlement_credential_refs(entitlement.as_ref().map(|e| e.name())),
    };
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
    let pairing = session_pairing(&effective, &launch_profile);
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
    // Line 1467, the fresh half: the session just recorded is the one the
    // next low-risk turn will be in.
    remember_classification(&sticky_cache, classified.as_ref(), record.id.as_str());

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

    // Map lines 1835 and 1854: the route this launch chose, attributed to the
    // session it just produced.
    //
    // **Here, and not beside `record_routing_decision` above, because the id
    // does not exist up there.** A fresh launch mints its session id at
    // `store.create`, so a decision recorded before it can carry no session
    // and an outcome learned a turn later would have nothing to attach to.
    // Recording the decision itself later was the alternative and it is
    // rejected: lines 1829 and 1830 count decisions, and a launch refused
    // while resolving its profile made a decision and never reaches this
    // line. So the decision keeps its own moment and this row records what
    // that decision became — two rows, never an `UPDATE` of one.
    if let Some(routed) = &routed {
        let observed_at = glasshouse::evaluation::now_unix();
        glasshouse::evaluation::record_routed_session(
            runtime,
            record.id.as_str(),
            routed.chosen().id(),
            routed_cost_class(&user, project.as_ref(), routed.chosen()),
            routing_evidence_for(&health, routed.chosen(), observed_at),
            routed_tier(classified.as_ref()),
            observed_at,
        );
    }

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
    install_context_firewall_hook(
        runtime,
        &selection,
        effective,
        &session_dir,
        entitlement.as_ref(),
        &launch_profile.backend,
        &launch_profile.name,
    );
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
        run_headless(runtime, &store, &record.id, launch, deferred_briefing)
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

/// A briefing selected for a launch but not yet delivered — `GH-LAUNCH-BRIEFING`'s
/// rung two, handed from [`brief_launch_session`] to [`run_headless`] because
/// nothing can deliver it until a session runtime holds the PTY.
#[derive(Debug)]
struct DeferredBriefing {
    injection: glasshouse::memory::inject::Injection,
    binding: usize,
    failed_attempts: usize,
}

impl DeferredBriefing {
    /// The line printed once this briefing is actually delivered — shared
    /// between the rung-one and rung-two paths so the two report identically.
    fn announcement(&self) -> String {
        briefing_announcement(
            self.injection.memories().len(),
            self.binding,
            self.failed_attempts,
        )
    }
}

/// The `briefed with ...` line both delivery rungs print, once, on a
/// successful delivery — never composed twice so the wording cannot drift
/// between rungs.
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
enum LaunchBriefing {
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
fn brief_launch_session(
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
    let outcome =
        match inject::select_briefing(&project.store(), query, &std::collections::HashSet::new()) {
            Ok(outcome) => Some(outcome),
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
        Some(BriefingOutcome::NothingNew) | None => {
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
enum ExternalPresentation {
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
fn external_presentation(
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
fn pane_global_args(cli: &Cli, runtime: &Runtime) -> Vec<OsString> {
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
struct PaneLaunch<'a> {
    harness: Option<&'a str>,
    response_profile: Option<&'a str>,
    response_role: Option<&'a str>,
    profile: Option<&'a str>,
    from_checkpoint: Option<&'a str>,
    to: Option<&'a str>,
    fresh: bool,
    headless: bool,
    harness_args: &'a [String],
}

fn pane_launch_args(launch: PaneLaunch<'_>) -> Vec<OsString> {
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

/// `glasshouse sessions focus` — Phase 17 line 759. One `workspace select`
/// through the integration, for a session that has a pane; a session that
/// has none, or a cmux that is not available, is reported rather than
/// guessed around.
fn focus_session(runtime: &Runtime, session: &str) -> anyhow::Result<String> {
    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    let id = store.resolve_id(session)?;
    let record = store
        .get(&id)?
        .ok_or_else(|| anyhow::anyhow!("session `{id}` is not in this project"))?;
    let Some(reference) = record.presentation_ref.as_deref() else {
        anyhow::bail!(
            "session `{id}` has no external pane to focus; it is presented {}",
            record.presentation
        );
    };
    match cmux::detect() {
        cmux::Availability::Absent(reason) => anyhow::bail!(
            "session `{id}` is presented in cmux {reference}, but cmux is not available \
             from here ({reason})"
        ),
        cmux::Availability::Available(control) => {
            let pane = cmux::focus(reference, &control)?;
            Ok(format!(
                "glasshouse: focused cmux {pane} for session {id}\n"
            ))
        }
    }
}

/// The `PRESENTED` cell: the presentation word, followed by the pane when
/// one is recorded — `external workspace:349`. The word alone for every
/// other session, exactly as before the pane existed.
fn presented_cell(record: &SessionRecord) -> String {
    match record.presentation_ref.as_deref() {
        Some(reference) => format!("{} {reference}", record.presentation),
        None => record.presentation.to_string(),
    }
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
fn run_headless(
    runtime: &Runtime,
    store: &glasshouse::session::SessionStore<'_>,
    id: &SessionId,
    launch: HarnessLaunch<'_>,
    deferred_briefing: Option<DeferredBriefing>,
) -> anyhow::Result<ExitStatus> {
    /// How often the loop wakes to answer queries and check on the child.
    const POLL: std::time::Duration = std::time::Duration::from_millis(20);

    let live = Arc::new(Mutex::new(SessionRuntime::new()));
    lock(&live).start(id.clone(), SessionPresentation::Headless, &launch)?;

    // `GH-LAUNCH-BRIEFING`'s rung two: no adapter additive mechanism existed
    // to ride at `install_session_document` time, but this session runtime
    // now holds the PTY — the exact condition the design ruling names for
    // falling back to the door's own labelled-message delivery. Delivered
    // here, immediately after `start` registers the session as live and
    // before this loop's first poll, so it is the first thing the harness
    // reads after its own startup.
    if let Some(briefing) = deferred_briefing {
        let mut guard = lock(&live);
        let mut api = glasshouse::session::api::SessionApi::new(store, &mut guard);
        let delivered = api.send_text(
            id,
            briefing.injection.text(),
            glasshouse::events::MessageOrigin::Machine,
        );
        drop(guard);
        match delivered {
            Ok(()) => {
                glasshouse::evaluation::record_memory_retrieval(
                    runtime,
                    glasshouse::evaluation::RetrievalScope::Injection,
                    briefing
                        .injection
                        .memories()
                        .iter()
                        .map(glasshouse::memory::MemoryId::as_str),
                    Some(id.as_str()),
                    glasshouse::evaluation::now_unix(),
                );
                eprintln!("glasshouse: {}", briefing.announcement());
            }
            Err(err) => eprintln!(
                "glasshouse: warning: could not deliver this project's memory to session {id}; \
                 its task is being sent without it ({err:#})"
            ),
        }
    }

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
        // Line 1851, on the resume path too: a resumed session's gateway
        // fails over exactly as a launched one's does, and counting only the
        // launched ones would make the denominator a subset nobody stated.
        Some(failover_prevention_sink(runtime)),
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

/// Map lines 1991-1996: register the context firewall's `PostToolUse` hook
/// for a Claude Code session, when the effective configuration enables it.
///
/// **Never a second `--settings` flag.** Claude Code 2.1.247 silently
/// discards every `--settings` but the last (verified in
/// `session::HarnessSelection::install_session_document`'s own doc), so the
/// only safe way to add a hook is to merge it into the SAME document
/// [`install_session_document`] already wrote — this function reads that
/// file back, adds one `PostToolUse` key, and writes it in place. `args`
/// itself is never touched, which is what makes `mode = "off"` byte-identical
/// to a session built before this phase existed: this function returns
/// before touching anything when the harness is not Claude Code or the
/// effective mode is `off`.
///
/// Best effort, matching [`install_session_document`]'s own policy: any
/// failure here is a session that starts without the firewall bridge rather
/// than one that fails to start, and is logged rather than propagated.
///
/// Map lines 2023/2024: `entitlement` and `backend` are read only to
/// *classify* the reduction policy (subscription, metered or local) and to
/// resolve its thresholds through `effective`'s new accessors — never baked
/// into the registered command line themselves. The firewall core and the
/// hook subprocess this command line invokes stay entitlement-blind, exactly
/// as before this package: only numbers and a mode word ever reach them.
fn install_context_firewall_hook(
    runtime: &Runtime,
    selection: &session::HarnessSelection,
    effective: config::EffectiveConfig<'_>,
    session_dir: &std::path::Path,
    entitlement: Option<&glasshouse::config::ResolvedEntitlement>,
    backend: &glasshouse::profile::BackendResource,
    profile_name: &str,
) {
    use glasshouse::config::firewall::{FirewallMode, ReductionPolicyKind};
    use glasshouse::config::{EntitlementBacking, EntitlementKind};
    use glasshouse::harness::claude_code;
    use glasshouse::profile::BackendResource;
    use glasshouse::provider::registry::{Locality, ResourceKind};

    if selection.id() != glasshouse::integrations::IntegrationId::ClaudeCode {
        // A non-claude-code harness gets no registration and no warning
        // spam — one debug line, per the packet's own instruction.
        tracing::debug!(
            harness = selection.id().slug(),
            "context firewall: no verified PostToolUse bridge for this harness"
        );
        return;
    }

    // Map lines 2023/2024's classification: a subscription pays in rate
    // limits and context window, a key in tokens, local inference in
    // latency — so each gets its own default thresholds. Locality outranks
    // `EntitlementKind` here: an entitlement backed by a local provider is
    // classified `Local` regardless of what its own `kind` says, because
    // latency is what actually drives the policy for that resource. An
    // unresolved entitlement (`None` here) never guesses a kind.
    let provider_is_local =
        |provider: &str| ResourceKind::from_direct_provider(provider).locality() == Locality::Local;
    let kind = match entitlement {
        Some(entitlement) => match entitlement.backing() {
            EntitlementBacking::Provider(provider) if provider_is_local(provider) => {
                Some(ReductionPolicyKind::Local)
            }
            _ => match entitlement.kind() {
                Some(
                    EntitlementKind::Claude | EntitlementKind::ChatGpt | EntitlementKind::Gemini,
                ) => Some(ReductionPolicyKind::Subscription),
                Some(EntitlementKind::ApiKey) => Some(ReductionPolicyKind::Metered),
                None => None,
            },
        },
        // No entitlement describes this resource — still `Local` when the
        // session is served by a local provider directly (map lines
        // 2023/2024's "or a session served by such a provider with no
        // entitlement").
        None => match backend {
            BackendResource::DirectProvider { provider } if provider_is_local(provider) => {
                Some(ReductionPolicyKind::Local)
            }
            _ => None,
        },
    };

    let configured_mode =
        effective.context_firewall_policy_mode(kind, Some(profile_name), entitlement);
    if configured_mode == FirewallMode::Off {
        return;
    }

    let program = match std::env::current_exe() {
        Ok(program) => program,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "context firewall: could not find the Glasshouse executable; not registered"
            );
            return;
        }
    };

    // Map line 1994: verify at session start against the installed
    // harness. Cached for exactly this one registration — the hook
    // subprocess Claude Code spawns later for every tool call never
    // re-probes anything; the decision made here is baked into the
    // registered command line's own `--mode` flag.
    let probe = std::process::Command::new(selection.executable().path())
        .arg("--version")
        .output();
    let effective_mode = match probe {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).into_owned();
            match claude_code::parse_version(&text)
                .filter(|version| claude_code::supports_updated_tool_output(*version))
            {
                Some(_) => configured_mode,
                None => {
                    eprintln!(
                        "glasshouse: the installed Claude Code (`{}`) is below the verified \
                         floor for tool-output replacement ({}); the context firewall \
                         registers in shadow mode for this session",
                        text.trim(),
                        claude_code::MIN_UPDATED_OUTPUT_VERSION_STRING,
                    );
                    record_context_firewall_registration_fallback(runtime, "version-floor");
                    FirewallMode::Shadow
                }
            }
        }
        _ => {
            eprintln!(
                "glasshouse: could not verify the installed Claude Code's version; the \
                 context firewall registers in shadow mode for this session"
            );
            record_context_firewall_registration_fallback(runtime, "version-unprobed");
            FirewallMode::Shadow
        }
    };

    let passthrough_tokens = effective.context_firewall_policy_passthrough_tokens(
        effective_mode,
        kind,
        Some(profile_name),
        entitlement,
    );
    let min_semantic_tokens = effective.context_firewall_policy_min_semantic_tokens(
        kind,
        Some(profile_name),
        entitlement,
    );
    // Map line 1992: no mode, including aggressive, ever names a reducer —
    // there is no flag here that could carry one, which is the guard by
    // construction the box asks for.
    let emit_updated_output = effective_mode != FirewallMode::Shadow;
    let command_line = claude_code::context_firewall_command_line(
        &program,
        effective_mode,
        passthrough_tokens,
        emit_updated_output,
        min_semantic_tokens,
    );
    let hook_entry = claude_code::context_firewall_hook_entry(&command_line);

    let path = session_dir.join(claude_code::SETTINGS_FILE_NAME);
    let existing = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) => {
            tracing::warn!(
                error = %err,
                path = %path.display(),
                "context firewall: could not read the settings document to merge its hook \
                 into; not registered"
            );
            return;
        }
    };
    match claude_code::merge_context_firewall_hook(&existing, &hook_entry) {
        Ok(merged) => {
            if let Err(err) = std::fs::write(&path, merged) {
                tracing::warn!(
                    error = %err,
                    path = %path.display(),
                    "context firewall: could not write the merged settings document; not \
                     registered"
                );
            }
        }
        Err(err) => {
            tracing::warn!(
                error = %err,
                "context firewall: could not merge the PostToolUse hook; not registered"
            );
        }
    }
}

/// Map line 1994's fallback: one bypass-family telemetry row recorded at
/// registration time, distinct from [`record_context_firewall_telemetry`]'s
/// per-tool-call rows — this one is about the registration decision itself,
/// taken once per launch rather than once per hook invocation.
fn record_context_firewall_registration_fallback(runtime: &Runtime, reason: &str) {
    use glasshouse::routing::evidence::{
        CONTEXT_FIREWALL_BYPASS_PURPOSE, EvidenceLedger, NewObservation,
    };

    let ledger = match EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "routing evidence ledger unavailable; a context-firewall registration \
                 fallback is not recorded"
            );
            return;
        }
    };
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let observation = NewObservation::new("glasshouse", "context-firewall")
        .with_harness(Some(
            glasshouse::integrations::IntegrationId::ClaudeCode.slug(),
        ))
        .with_purpose(Some(CONTEXT_FIREWALL_BYPASS_PURPOSE))
        .with_route(Some(reason))
        .with_quota_context(Some("registration".to_string()))
        .with_timing(Some(now_unix), Some(now_unix));
    if let Err(err) = ledger.record(observation, now_unix) {
        tracing::warn!(error = %err, "could not record a context-firewall registration fallback");
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
/// That is deliberately stricter than "the user has configured a free
/// model". A free-model list is a statement about cost; it is not a request
/// that a hook running **inside a coding session** start making outbound
/// requests. Line 834 says *configurable*, and this is the configuration.
///
/// **What consent does not decide is *which* resource serves.** Once it is
/// given, [`disposable_extraction_model`] puts the named model into the
/// candidate set beside the user's own free ones and lets
/// `DisposableRouting::choose` rank them — line 530's *prefer free models
/// when quality is sufficient*, on the path that actually spends something.
/// It used to bypass the router entirely, which meant the policy chose only
/// when nothing would be called and the model that was called had never been
/// routed.
fn configured_extraction_choice(
    effective: &EffectiveConfig<'_>,
) -> Option<glasshouse::config::ExtractionModelRef> {
    effective.memory_extraction_model().value
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

/// The configured extraction model as one more resource
/// `DisposableRouting::choose` may rank — map line 530 applied to the model
/// the user named for this job.
///
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
/// # Why this is here and not in `routing`
///
/// `crate::routing::disposable` may not name a cache, a path or the
/// interactive policy class (`the_two_policy_classes_do_not_name_each_other`),
/// and `crate::routing::free` is a pure value. The bridge belongs to the
/// caller that has both, which is this file — the same place
/// [`observed_health_of`] reads the identical cache back.
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
///
/// A failure here is one debug line: this runs inside a hook process, and
/// Glasshouse's bookkeeping is never more important than the session it keeps
/// books about.
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

/// Phase 9I lines 530, 531 and 540's production caller, and
/// GH-ROUTED-EXTRACTION-CLIENT's: route this extraction through
/// `glasshouse::routing::disposable::DisposableRouting` over the resources
/// the user has actually configured, report the choice, and — when the user
/// has consented to a model being called at all — perform the extraction
/// through the resource that won.
///
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
///
/// Falls back to [`NoExtractionModel`] when the configuration cannot be
/// read at all — the same non-fatal-to-the-session posture
/// [`report_hook_with`]'s own doc comment describes for every other failure
/// on this path.
fn disposable_extraction_model(
    runtime: &Runtime,
    session: &glasshouse::session::SessionId,
) -> Box<dyn glasshouse::memory::ExtractionModel> {
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

    let mut candidates = disposable_candidates(
        &user,
        project.as_ref(),
        &effective,
        &secrets,
        &telemetry,
        now_unix,
    );
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
    .with_reserve_override(reserve_override)
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
    let mut routed =
        glasshouse::memory::RoutedModel::new(job, &candidates, &routing, health.pool());

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
                candidates.push(
                    DisposableCandidate::new(
                        name.clone(),
                        model.clone(),
                        credential_id.clone(),
                        provider_config.cost_of(model),
                    )
                    .with_capacity(capacity.clone())
                    .with_locality(locality)
                    .with_entitlement(entitlement.clone()),
                );
            }
        }
    }
    candidates
}

/// Phase 57B's production caller (map lines 1997, 2002): resolve
/// `[context_firewall].reducer` (and its optional `reducer_model` pin) into
/// a real [`glasshouse::firewall::reducer::Reducer`], routed through
/// [`glasshouse::routing::disposable::DisposableRouting`] over the same
/// candidates [`disposable_candidates`] builds for every other disposable
/// job — never a firewall-private provider client (map line 1997).
///
/// `None` whenever there is nothing to route: no `reducer` configured (map
/// line 1992's guarantee — an absent reducer disables the whole semantic
/// stage), no configured candidate matches it, or
/// [`glasshouse::routing::disposable::DisposableRouting::choose`] found no
/// resource at all — including because an entitlement's `deny_job_kinds`
/// refuses [`glasshouse::routing::disposable::JobKind::ContextReduction`]
/// for every matching candidate, which is this line's own per-entitlement
/// job-kind rule applying unchanged.
fn disposable_reducer(
    runtime: &Runtime,
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    session_id: &str,
) -> Option<Box<dyn glasshouse::firewall::reducer::Reducer>> {
    use glasshouse::provider::registry::Locality;
    use glasshouse::routing::disposable::{DisposableRouting, JobKind};
    use glasshouse::routing::free::{FreePool, FreePreferences};

    let effective = EffectiveConfig::new(user, project);
    let reducer_ref = effective.context_firewall_reducer().value?;

    // Phase 58, map lines 2028-2030: `local:<name>` selects an installed
    // out-of-process tool from `[context_firewall.local_reducers.<name>]`
    // instead of routing through `DisposableRouting` at all — a local tool
    // is local by construction, so `reducer_local_only` is satisfied without
    // being consulted here, and nothing below this branch (provider/model
    // candidates, free-resource routing, entitlement job-kind gating) applies
    // to it. design-decisions.md's *The local reducer seat*.
    if let Some(name) = reducer_ref.strip_prefix("local:") {
        return local_disposable_reducer(runtime, user, project, &effective, session_id, name);
    }

    let reducer_model_pin = effective.context_firewall_reducer_model().value;
    let local_only = effective.context_firewall_reducer_local_only().value;

    let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let telemetry = glasshouse::provider::resources::GatheredTelemetry::new().gather_gateway_quota(
        &glasshouse::provider::telemetry::GatewayQuotaCache::new(runtime.paths()),
    );
    let candidates =
        disposable_candidates(user, project, &effective, &secrets, &telemetry, now_unix);

    let mut filtered: Vec<_> = candidates
        .into_iter()
        .filter(|candidate| {
            candidate.provider() == reducer_ref
                || candidate
                    .entitlement()
                    .is_some_and(|entitlement| entitlement.name() == reducer_ref)
        })
        .filter(|candidate| {
            reducer_model_pin
                .as_deref()
                .is_none_or(|model| candidate.model() == model)
        })
        .collect();
    if local_only {
        filtered.retain(|candidate| candidate.locality() == Some(Locality::Local));
    }
    if filtered.is_empty() {
        return None;
    }

    let free_preferences = FreePreferences::new()
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
    let reserve_override = glasshouse::routing::disposable::ReserveOverride::for_sessions(
        effective.reserve_override_sessions().value,
    )
    .deciding_for(session_id.to_string());
    let routing = DisposableRouting::for_support_work(
        effective.prefer_free_routing().value,
        free_preferences,
    )
    .with_reserve_override(reserve_override)
    .with_reserve_policy(
        effective
            .reserve_policies()
            .for_scope(glasshouse::routing::pressure::ReserveScope::Background),
    );

    let pool = FreePool::new();
    let choice = routing
        .choose(
            JobKind::ContextReduction,
            &filtered,
            &pool,
            std::time::Instant::now(),
            None,
        )
        .ok()?;

    match context_firewall_reducer_model(user, project, choice.provider(), choice.model()) {
        Ok(reducer) => Some(Box::new(reducer)),
        Err(err) => {
            tracing::warn!(error = %err, "the configured context-firewall reducer cannot be used");
            None
        }
    }
}

/// `[context_firewall].reducer = "local:<name>"` — Phase 58, map lines
/// 2028-2030. Resolves `[context_firewall.local_reducers.<name>]` (project
/// before user, matching every other reducer field's own layering) into a
/// [`glasshouse::firewall::reducer::LocalToolReducer`], or logs why not and
/// leaves the whole semantic stage disabled for this hook invocation — the
/// same fail-open posture [`disposable_reducer`]'s own `Err` arm already
/// has. The child's cwd is a scratch directory under this session's own
/// state, never the project root; its environment is scrubbed of every
/// entitlement's credential variable via
/// [`EffectiveConfig::foreign_entitlement_credential_vars`], called with
/// `None` because a subprocess Glasshouse did not write is not "serving"
/// any entitlement and has no business carrying any of their keys.
fn local_disposable_reducer(
    runtime: &Runtime,
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    effective: &EffectiveConfig,
    session_id: &str,
    name: &str,
) -> Option<Box<dyn glasshouse::firewall::reducer::Reducer>> {
    use glasshouse::firewall::reducer::LocalToolReducer;

    let config = project
        .and_then(|p| p.context_firewall().local_reducer(name))
        .or_else(|| user.context_firewall().local_reducer(name));
    let Some(config) = config else {
        tracing::warn!(
            reducer = name,
            "the context-firewall reducer names a local tool this project has not configured"
        );
        return None;
    };

    let scratch_dir = runtime
        .session_dir(session_id)
        .join("context-firewall-reducer");
    let credential_vars = effective.foreign_entitlement_credential_vars(None);

    match LocalToolReducer::new(name, config, scratch_dir, credential_vars) {
        Ok(reducer) => Some(Box::new(reducer)),
        Err(err) => {
            tracing::warn!(error = %err, reducer = name, "the configured local reducer cannot be used");
            None
        }
    }
}

/// Build the [`glasshouse::firewall::reducer::ConfiguredReducer`]
/// `DisposableRouting` chose — [`classification_model`]'s exact shape,
/// restated for the reducer's own type, since both build a real client from
/// a provider name and a model name after routing has already decided them.
fn context_firewall_reducer_model(
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    provider_name: &str,
    model_name: &str,
) -> Result<glasshouse::firewall::reducer::ConfiguredReducer, String> {
    use glasshouse::firewall::reducer::{ConfiguredReducer, ConfiguredReducerError};
    use glasshouse::secret::{SecretRef, SecretStore as _};

    let Some(provider_config) = project
        .and_then(|p| p.providers().get(provider_name))
        .or_else(|| user.providers().get(provider_name))
    else {
        return Err(format!(
            "the context-firewall reducer names `{provider_name}`, which this project has not \
             configured"
        ));
    };
    if !provider_config.enabled() {
        return Err(format!(
            "the context-firewall reducer names `{provider_name}`, which is disabled"
        ));
    }
    let provider = provider_config.to_provider(provider_name).map_err(|err| {
        format!("the context-firewall reducer's provider does not resolve: {err}")
    })?;

    let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();
    let credential = provider
        .credential_env
        .iter()
        .find_map(|var| secrets.resolve(&SecretRef::Environment { var: var.clone() }));

    ConfiguredReducer::new(&provider, model_name, credential).map_err(|err| match err {
        ConfiguredReducerError::UnsupportedProtocol { protocol, .. } => format!(
            "the context-firewall reducer speaks OpenAI chat completions, and \
             `{provider_name}` serves `{protocol}`; configure a provider that serves \
             openai-chat"
        ),
        other => format!("the context-firewall reducer cannot be used: {other}"),
    })
}

/// What `routing_observations.purpose` records for a call `glasshouse
/// classify` made.
///
/// Spelled once — in `routing::evidence`, beside the reader that keys on it
/// (`EvidenceLedger::classification_record`), and only re-named here.
/// `purpose` is a `TEXT` column with no `CHECK` (`database.rs`'s migration
/// 11), so the only thing keeping the producer and the reader on one
/// spelling is that there is exactly one.
const CLASSIFICATION_PURPOSE: &str = glasshouse::routing::evidence::CLASSIFICATION_PURPOSE;

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
) -> Result<ClassifierRef, String> {
    // The tier this job's own demand implies, from the request itself. This
    // is `RoutedModel::new_for_request`'s fifth link, made by the one
    // `JobKind` its doc comment says the constructor was waiting for — a
    // request, not a transcript of a finished turn.
    let requirement = glasshouse::routing::classify::classify_heuristically(request_text);
    let choice =
        automatic_classification_choice(runtime, user, project, effective, Some(&requirement))
            .map_err(|reason| {
                format!("no resource is available to classify this request: {reason}")
            })?;

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
    use glasshouse::provider::telemetry::RoutingStickyCache;
    use glasshouse::routing::disposable::{AutomaticClassificationDecision, DisposableRouting};

    let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let telemetry = glasshouse::provider::resources::GatheredTelemetry::new().gather_gateway_quota(
        &glasshouse::provider::telemetry::GatewayQuotaCache::new(runtime.paths()),
    );
    let candidates =
        disposable_candidates(user, project, effective, &secrets, &telemetry, now_unix);
    let candidates = attach_classification_records(runtime, candidates, now_unix);
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
    // Map lines 1427 and 1435: the user's classification requirements,
    // layered like every other `[routing]` value. `max_router_latency_ms`
    // has a default (2000ms), so the ceiling is always stated; whether it
    // *applies* to a candidate is decided by whether that candidate has a
    // measured median — see `routing::disposable::classification_verdict`.
    let routing = DisposableRouting::for_support_work(
        effective.prefer_free_routing().value,
        free_preferences,
    )
    .with_classification_policy(
        glasshouse::routing::disposable::ClassificationPolicy::new()
            .with_max_latency_ms(Some(effective.max_router_latency().value.get()))
            .with_local_only(effective.classification_local_only().value),
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
///
/// # What reaches the model is the rendered request, and nothing else
///
/// Phase 34D: `request` is the structured [`RouterRequest`] — the task,
/// bounded, and the few session facts its caller held — and its rendering
/// is the whole of the request text `Prompt::for_request` scrubs and sends.
/// The heuristic tier the `Automatic` arm needs for the classification job
/// itself is read off the same request's task text.
///
/// [`RouterRequest`]: glasshouse::routing::request::RouterRequest
fn classify_with_routing_model(
    runtime: &Runtime,
    request: &glasshouse::routing::request::RouterRequest,
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

/// Ask `first` to classify, and when it cannot — unreachable, or answering
/// outside the schema — walk `routing.model_fallback` once (capability map
/// lines 1423 and 1795), never sending anything to a remote model while
/// `routing.classification_local_only` is set (line 1427).
///
/// # What one attempt leaves behind
///
/// Every attempt that reached a provider leaves one `routing_observations`
/// row through [`record_classification_observation`], carrying the parse
/// outcome and the clock at dispatch and completion — the producers
/// capability map lines 1422/1432 and 1421/1435 were missing. An attempt
/// that never reached a provider (a build failure, a transport error, or a
/// remote model declined under local-only) leaves no row, which is the
/// honest shape: there is no call whose cost could be recorded.
///
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
///
/// # Without a chain, this is exactly the behaviour it replaced
///
/// One attempt, one row, and the same failure sentence on standard error —
/// a single failure is reported bare, without the model's name in front of
/// it, so nothing a person or a test read before this function existed
/// changed shape.
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
/// `dispatched_at_unix` and `completed_at_unix` are the clock either side
/// of the call, in whole seconds because that is what the columns hold;
/// `RoutingObservation::duration_ms` is therefore honest to the second, and
/// [`glasshouse::routing::evidence::ClassificationRecord`] says so. This is
/// deliberately not `ModelCall::observation`'s job: that producer's own doc
/// records that it leaves timing unwritten, and this caller is the one
/// that actually held the clock.
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
                //
                // Gated on liveness for the reason `session::lifecycle::may_apply`
                // gates every lifecycle transition: a hook process outlives its
                // harness, and a `PreCompact` report arriving after the session
                // is recorded as finished must not move the record either —
                // `record_observed_compaction` itself has no such check (it is
                // an unconditional `UPDATE ... WHERE id = ?1`, by design, so a
                // session created before migration 16 still gets counted), so
                // the check belongs at this call site, the same way `may_apply`
                // belongs at the lifecycle-event call site below rather than
                // inside the write it guards.
                if record.lifecycle.is_live()
                    && let Err(err) = store.record_observed_compaction(&id)
                {
                    tracing::debug!(
                        error = %err,
                        session = %id,
                        "could not count an observed compaction"
                    );
                }
                if memory_extraction_enabled(runtime) {
                    // `hook_extraction`, not `run_extraction`: this is the
                    // trigger line 1174 is about, and a compaction that
                    // recorded nothing must say so where the person can read
                    // it rather than into a log that is off.
                    hook_extraction(
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
        // Map lines 1834, 1835, 1845 and 1854's outcome half — and the whole
        // of what Glasshouse is allowed to learn about how a route turned
        // out. `TurnEnded` is the only event that carries a harness's own
        // verdict, `session::lifecycle::event_for` is its single construction
        // site, and **both** outcomes are recorded: a turn that ended badly
        // is a fact about the route as much as one that succeeded, and
        // counting only completions would make every ratio here a fraction of
        // an unstated denominator.
        //
        // A `SessionEnd`, a process exit and output going quiet all arrive
        // somewhere else or nowhere, and none of them writes a row. The
        // decision they belong to simply stays *unknown*, which is what the
        // readers count it as.
        //
        // Ordered **before** the extraction and checkpoint triggers below,
        // for the reason the compaction counter above is ordered first: those
        // run on their own thread up to `EXTRACTION_BOUND`, and this process
        // can be torn down by the harness while one is still going. A verdict
        // the harness actually stated must not be lost to work Glasshouse
        // chose to do about it.
        //
        // Map lines 1821 and 1831's proxy denominator — a second row, on
        // every session this arm reaches rather than only routed ones.
        // `record_routing_outcome` refuses a session with no routed
        // destination, so a door-spawned session (never routed) would
        // otherwise record nothing about how its turn went; `record_turn_outcome`
        // asks no routing question at all. Called first, so a session with no
        // routing decision still gets its outcome counted before the routed
        // call below returns early for it. Refusal register, *"Phase 51's
        // memory proxy — 1821 and 1831"*, ruling (b).
        if let LifecycleEvent::TurnEnded { outcome } = translated {
            glasshouse::evaluation::record_turn_outcome(
                runtime,
                id.as_str(),
                outcome,
                glasshouse::evaluation::now_unix(),
            );
            glasshouse::evaluation::record_routing_outcome(
                runtime,
                id.as_str(),
                outcome,
                glasshouse::evaluation::now_unix(),
            );

            // Map lines 1149 and 1153 — *"after a successful Git commit"* and
            // *"record the relevant Git commit"*. Glasshouse installs no Git
            // hook: `.git/hooks` belongs to the user, `core.hooksPath` can
            // point anywhere, and nothing needs installing, because this
            // process already runs at every turn boundary and
            // `checkpoint::git` already reads HEAD out of `.git` without
            // spawning anything. A commit landing is therefore *HEAD is not
            // where this session last saw it*, and `note_head_commit` is that
            // comparison.
            //
            // **Outside the `memory_extraction` gate, deliberately**, for the
            // compaction counter's reason one arm up: that switch decides
            // whether Glasshouse *does* something about a boundary, and the
            // commit landed either way. A position recorded only while
            // extraction is enabled would make the switch's first turn back
            // on report a boundary spanning however long it was off.
            let landed = note_head_commit(runtime, &store, &id, record.last_seen_commit.as_deref());
            let completed = matches!(outcome, TurnOutcome::Completed);

            // One extraction per turn, and the more specific trigger wins.
            //
            // A completed turn that also landed a commit is **one** boundary
            // described two ways, not two boundaries: the same event window
            // is read either way, so running both would ask a model the same
            // question twice inside somebody's session and hand the second
            // answer to the duplicate check. `GitCommit` is the description
            // that carries more — it names the object, and line 1153 wants
            // that object on the memories — so it is the one recorded.
            //
            // A turn that ended badly still gets the Git trigger, and gets
            // nothing without one. `TurnOutcome` is the harness's verdict on
            // its *own* turn; a commit that landed is a fact about the
            // repository, and there is no reading of line 1149 in which a
            // commit becomes un-landed because the turn after it failed.
            if memory_extraction_enabled(runtime) {
                match (landed, completed) {
                    (Some(commit), _) => {
                        hook_extraction(
                            runtime,
                            &id,
                            model(&id),
                            glasshouse::memory::ExtractionTrigger::GitCommit { commit },
                        );
                    }
                    (None, true) => {
                        hook_extraction(
                            runtime,
                            &id,
                            model(&id),
                            glasshouse::memory::ExtractionTrigger::TaskCompleted,
                        );
                    }
                    (None, false) => {}
                }
            }
            if completed && automatic_checkpoint_enabled(runtime) {
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

/// Whether a commit landed since this session was last looked at, and record
/// where HEAD stands now — map line 1149.
///
/// Returns the **new** commit when it is a code-change boundary, and `None`
/// otherwise. Three different things produce `None` and they are not the
/// same, which is why they are separated here rather than at the call site:
///
/// - **the project is not a Git repository**, or HEAD cannot be read.
///   `GitPosition::detect` answers `None` for every such case by design, and
///   nothing is stored: a project with no repository has no code-change
///   boundaries to have.
/// - **nobody has looked before.** `previous` is `None` on a session whose
///   first turn this is, and on every session created before the column
///   existed. The position is recorded, and it is **not** a boundary: a
///   boundary is a *change*, and there is nothing here to have changed from.
///   Reporting the first turn of every session as a landed commit would make
///   the trigger fire hardest on sessions that have done nothing yet.
/// - **HEAD has not moved.** The ordinary case, and the one the comparison
///   exists for. Nothing is written, because nothing changed.
///
/// # A failed write is one debug line
///
/// Everything else on this path takes that stance and this is not more
/// important than the compaction counter beside it. The cost of the failure
/// is that the next turn re-reads the same position and calls it a boundary
/// once — a duplicate extraction the duplicate check already absorbs, which
/// is a far better failure than a hook that fell over inside somebody's
/// coding session.
fn note_head_commit(
    runtime: &Runtime,
    store: &glasshouse::session::SessionStore<'_>,
    id: &SessionId,
    previous: Option<&str>,
) -> Option<String> {
    let position = GitPosition::detect(runtime.project().root())?;
    if previous == Some(position.commit.as_str()) {
        return None;
    }

    if let Err(err) = store.record_seen_commit(id, &position.commit) {
        tracing::debug!(
            error = %err,
            session = %id,
            "could not record where HEAD stood for this session"
        );
    }

    // Recorded either way above; a boundary only when there was a position to
    // move *from*.
    previous.is_some().then_some(position.commit)
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

/// Run memory extraction over what this session has done — Phase 29's
/// **memory commit**, whatever started it.
///
/// # One operation, four triggers, and no second pipeline
///
/// Map line 1147 asks for *"a lightweight memory commit operation that
/// extracts durable project knowledge from recently completed work"* and
/// lines 1148-1151 ask for four ways to start one. This function is that
/// operation, and `trigger` is the whole of the difference between them:
/// `Manual` from `glasshouse memory commit`, `TaskCompleted` and `GitCommit`
/// from the `TurnEnded` arm of [`report_hook_with`], `BeforeCompaction` from
/// its `PreCompact` arm. A second extraction path for any of them would be a
/// second answer to what is worth remembering, a second credential screen and
/// a second duplicate check.
///
/// # The outcome is returned, and the hook path still ignores it
///
/// `Option<ExtractionOutcome>` rather than `()` so `glasshouse memory commit`
/// can print what its run actually did. It is not an error channel and does
/// not become one: `None` means the *preparation* failed or the bound expired
/// — both already logged here — and every failure of the extraction itself is
/// a field on the outcome, never a `Result`. The hook path discards it, which
/// is why nothing about its posture changes.
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
) -> Option<glasshouse::memory::ExtractionOutcome> {
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
            return None;
        }
    };

    // The commit is still deliberately not *read* here. `checkpoint::git`
    // knows how to find one and this process does not need to: a memory's
    // commit is "where the project was when this was learned", and a hook
    // process runs while the user's tree is mid-edit. `glasshouse memory
    // extract` takes the session's activity from a person who knows; this
    // path takes what the log holds and claims nothing more.
    //
    // Map line 1153 — *"record the relevant Git commit with memories produced
    // from a code-change boundary"* — is the one case where that objection
    // does not apply, and it does not apply because nothing is read. A
    // `GitCommit` trigger **is** a commit: the caller compared HEAD against
    // what this session had already seen, found it moved, and the object that
    // moved it is the trigger's own payload. So the commit recorded on these
    // memories is the boundary that caused the run, not a reading taken at an
    // arbitrary moment during it — which is exactly the distinction the
    // paragraph above refuses to blur. Every other trigger still carries
    // `None`.
    let chunk = chunk_for_session(id, &events, trigger.commit(), ChunkLimits::default());

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
    // Cloned rather than moved: `ExtractionTrigger` stopped being `Copy` when
    // `GitCommit` gained its commit, and the log lines below name the trigger
    // after the thread has taken its own.
    let thread_trigger = trigger.clone();
    std::thread::spawn(move || {
        let store = memory.store();
        let outcome = Extractor::new(&store, model.as_ref()).run(&chunk, thread_trigger);
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
            Some(outcome)
        }
        Err(_) => {
            tracing::warn!(
                session = %id,
                trigger = %trigger,
                bound_ms = EXTRACTION_BOUND.as_millis(),
                "memory extraction did not finish within its bound; the session is unaffected"
            );
            None
        }
    }
}

/// [`run_extraction`] on a hook's path, where a lost memory has to be said
/// out loud.
///
/// # Why this exists at all, when `run_extraction` already logs every failure
///
/// Because on this path nothing reads the log. `logging::LogConfig::resolve`
/// answers [`glasshouse::logging::LogSink::Disabled`] unless `GLASSHOUSE_LOG`
/// is set or a `--log-*` flag is given, and a harness spawning
/// `glasshouse hook` gives neither — so `run_extraction`'s
/// `"memory extraction produced nothing"` and its bound-expiry `warn!` are
/// both written to a subscriber that was never installed. Measured
/// 2026-08-31: a `PreCompact` hook whose model call failed exited **0**, with
/// **empty stderr**, having recorded nothing.
///
/// That is the precise thing capability map line 1174 is about. *"Record
/// enough pre-compaction durable memory that important project decisions do
/// not depend solely on a lossy native compact summary"* is not satisfied by
/// a trigger that fires, fails, and says nothing: the person then believes
/// their decisions were captured and goes on to compact, which is worse than
/// knowing they were not.
///
/// # Why stderr, and why one line
///
/// `main.rs`'s own [`run`] already draws this distinction for the overridden
/// safety refusal, three lines into the program and for exactly this reason:
/// *"logging is off by default, so a `tracing::warn!` there can go completely
/// unseen … it always gets a line on stderr, log or no log."* A memory the
/// compaction trigger was supposed to record and did not is user-facing in
/// the same sense.
///
/// Stderr and not stdout, and never a non-zero exit: Claude Code reads a
/// hook's exit code as a gate on the turn, and Phase 21's *"keep
/// memory-extraction failure non-fatal to the coding session"* is unchanged
/// by this. The hook still exits zero whatever extraction did.
///
/// Not used by `glasshouse memory commit`: that trigger is
/// [`glasshouse::memory::ExtractionTrigger::Manual`], it runs in front of a
/// person who is watching, and it prints its own report. This is the wrapper
/// for the triggers that run inside somebody's session with nobody watching.
fn hook_extraction(
    runtime: &Runtime,
    id: &SessionId,
    model: Box<dyn glasshouse::memory::ExtractionModel>,
    trigger: glasshouse::memory::ExtractionTrigger,
) {
    // Read before the call, because `run_extraction` takes the trigger.
    let named = trigger.as_str();
    let outcome = run_extraction(runtime, id, model, trigger);
    if let Some(notice) = lost_extraction_notice(named, outcome.as_ref()) {
        eprintln!("glasshouse: warning: {notice}");
        eprintln!(
            "glasshouse: the coding session is unaffected; this project's durable memory is not \
             updated for this boundary"
        );
    }
}

/// What to tell the person about an extraction that recorded nothing, or
/// [`None`] when nothing was lost.
///
/// Separated from [`hook_extraction`] so the decision can be tested without a
/// process: what this returns is the whole of the difference between a silent
/// loss and an observable one.
///
/// # The four cases, and why two of them are silent
///
/// - **no outcome at all.** [`run_extraction`] answers `None` for its two
///   preparation failures and for [`EXTRACTION_BOUND`] expiring. All three
///   are losses — a boundary went by and nothing was written — and the reason
///   is in a log that, on this path, does not exist.
/// - **a failure.** The model was unavailable, refused, timed out, panicked,
///   answered something the contract could not read, or the store could not
///   be read for duplicate detection. Each is a memory that should exist and
///   does not, and [`glasshouse::memory::extract::ExtractionFailure`]'s `Display` is a
///   fixed phrase by construction — no provider body reaches this line.
/// - **[`glasshouse::memory::extract::ExtractionFailure::NothingToExtract`] is
///   deliberately silent.** There was no session activity to extract from, so
///   there is no memory to have lost. A warning here would fire on every
///   compaction of a session that had not done anything yet, and a warning
///   that cries wolf is how the real one gets ignored.
/// - **rejections without a failure.** The model answered and some of what it
///   proposed did not survive the contract. Said out loud when *nothing*
///   survived, and silent when something did: a run that stored two memories
///   and rejected a third lost nothing a person needs to act on, and
///   duplicates and speculative drops are the mechanism working rather than
///   failing.
fn lost_extraction_notice(
    trigger: &str,
    outcome: Option<&glasshouse::memory::ExtractionOutcome>,
) -> Option<String> {
    use glasshouse::memory::extract::ExtractionFailure;

    let Some(outcome) = outcome else {
        return Some(format!(
            "memory extraction for `{trigger}` did not finish and recorded nothing (it was cut \
             off at its {}s bound, or this session's history could not be read)",
            EXTRACTION_BOUND.as_secs()
        ));
    };

    if let Some(failure) = &outcome.failure {
        // The one failure that is not a loss.
        if matches!(failure, ExtractionFailure::NothingToExtract) {
            return None;
        }
        return Some(format!(
            "memory extraction for `{trigger}` recorded nothing: {failure}"
        ));
    }

    if outcome.stored() == 0 && !outcome.rejected.is_empty() {
        return Some(format!(
            "memory extraction for `{trigger}` recorded nothing: the model answered, and all {} \
             of the memories it proposed were rejected",
            outcome.rejected.len()
        ));
    }

    None
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
    // Capability map line 1832. `ModelCall::observation` deliberately leaves
    // `purpose` unwritten — its own doc comment records that it fills no
    // column with a nearby value — so the stamp is applied here, by the
    // producer that knows what this call was *for*, the same way
    // `record_classification_observation` and `record_routing_latency` stamp
    // theirs.
    //
    // **Only rows written from here on.** Every extraction row already on
    // disk keeps its `NULL`, and the rendering counts those as *unstamped*
    // rather than re-labelling them: `NewObservation::with_purpose`'s own doc
    // comment is the rule, and back-filling would make "this build recorded
    // nothing here" indistinguishable from "this build recorded a purpose".
    let observation = observation.with_purpose(Some(EXTRACTION_PURPOSE));
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

/// Check point the session this work is leaving, before it moves —
/// capability map line 1716.
///
/// `moving_to` is where the work is going: a session identifier when this
/// launch or resume is continuing one, and `None` when it is starting a new
/// session. The session being **left** is whichever this project was most
/// recently active in, which is the same `active_session` rule
/// `glasshouse checkpoint save` and `Request::TakeCheckpoint` use for "the
/// current session".
///
/// # Three of the four cases are a no-op, and each says which
///
/// Nothing is being left when this project has no recorded session, when the
/// launch is starting a fresh one, or when the destination *is* the session
/// already in hand. Writing a checkpoint for any of those would produce a
/// handoff describing a migration that did not happen. The flag says so
/// instead of passing silently: a person who asked for a checkpoint and did
/// not get one needs to know which of the two occurred, and a silent no-op is
/// indistinguishable from a checkpoint that was taken (practice §68's shape).
///
/// # It invents nothing, and it fails loudly
///
/// The handoff records only what Glasshouse knows: which session was left,
/// where the work went, the Git position and this project's binding memories,
/// all through the same [`Checkpoint::capture`] the two existing checkpoint
/// paths use. It does not read the session's terminal for an objective —
/// `checkpoint_command`'s own doc says why that would be a confident fiction.
///
/// A failure here **stops the launch**. The person asked for a checkpoint
/// before the move; moving anyway would lose exactly what they asked to keep.
fn checkpoint_before_moving(runtime: &Runtime, moving_to: Option<&str>) -> anyhow::Result<()> {
    // Its own scope, closed before the caller opens the session store the
    // resume path needs — practice §65: two live connections to one SQLite
    // database in one process is invisible on Unix and a lock on Windows.
    let leaving = {
        let sessions = ProjectSessions::open(runtime)?;
        let store = sessions.store();
        let Some(record) = active_session(&sessions, None)? else {
            eprintln!(
                "glasshouse: --checkpoint-first had nothing to check point: this project has \
                 no recorded session to leave."
            );
            return Ok(());
        };
        let Some(destination) = moving_to else {
            eprintln!(
                "glasshouse: --checkpoint-first had nothing to check point: this starts a new \
                 session and leaves nothing behind."
            );
            return Ok(());
        };
        // Resolved through the store so a short identifier — the twelve
        // characters `glasshouse sessions` prints — compares equal to the
        // full one. Comparing the strings would make `--to <short id>` look
        // like a different session from the one it names.
        let destination = store.resolve_id(destination)?;
        if destination == record.id {
            eprintln!(
                "glasshouse: --checkpoint-first had nothing to check point: session {} is \
                 already where this work is.",
                short_id(&record.id)
            );
            return Ok(());
        }
        (record, destination)
    };
    let (record, destination) = leaving;

    let checkpoints = ProjectCheckpoints::open(runtime)?;
    let store = checkpoints.store();
    let stored = store.save(Checkpoint::capture(
        &record.id,
        &record.harness,
        // The person asked for it by passing the flag, which is exactly what
        // `Manual` means here. There is deliberately no new reason: the
        // stored vocabulary is fixed by a `CHECK` constraint in the schema,
        // and a third spelling would be a migration for a distinction the
        // handoff text below already carries.
        CheckpointReason::Manual,
        store.now(),
        runtime.project().root(),
        Handoff {
            objective: format!(
                "preserve session {} before this project's work moved to {}",
                short_id(&record.id),
                short_id(&destination)
            ),
            implementation_state: format!(
                "Glasshouse took this checkpoint because --checkpoint-first was passed to a \
                 command that moved work out of session {}. Nothing here was read from that \
                 session's terminal: what Glasshouse knows is which session was left, where \
                 the work went, this project's Git position, and its binding memories.",
                short_id(&record.id)
            ),
            decisions: Vec::new(),
            memory: binding_memory_lines(runtime),
            failed_approaches: Vec::new(),
            files: Vec::new(),
            test_state: None,
            next_actions: vec![format!(
                "continue in session {}, or reopen this one with `glasshouse resume {}`",
                short_id(&destination),
                short_id(&record.id)
            )],
        },
    ))?;

    eprintln!(
        "glasshouse: checkpoint {} saved for session {} before this work moved to {}.",
        stored.id.short(),
        short_id(&record.id),
        short_id(&destination)
    );
    Ok(())
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
        RouterInputs, RoutingMoment, RoutingOverride, TaskRequirements,
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

    let Ok(destinations) = routing_destinations(
        runtime,
        &effective,
        harness,
        DestinationScope::Everything,
        None,
    ) else {
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
    let health = observed_provider_health(runtime, &effective, &destinations);
    // Phase 34D does not reach this report: `glasshouse resume` carries no
    // task text, so there is nothing to classify and nothing is invented.
    // The moment a `resume` learns what the next task is, this is the site
    // that hands `classify_for_routing` a `TaskBoundary` moment.
    let inputs = RouterInputs {
        overrides: &overrides,
        health: health.pool(),
        now: std::time::Instant::now(),
        requirements: TaskRequirements::default(),
    };
    let Some(routed) = session_router(runtime, &effective, RoutingOverride::to(id.as_str()))
        .choose(
            RoutingMoment::TaskBoundary,
            current.as_ref(),
            &destinations,
            &inputs,
        )
    else {
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

    // Phase 56 line 1954 on the path that continues a session — reached by
    // `glasshouse resume` and by a launch the router steered into an existing
    // session. The profile is re-read the way `routing_destinations` reads it
    // for a recorded session, with the same fallback to the implied Native
    // profile, so the announcement names the entitlement the router weighed.
    // Announced and not gated: a session that already exists on this resource
    // is one the person asked to continue, and a resume is not the moment to
    // refuse — the comment above `overlay_resolution` says the same of an
    // overlay that no longer resolves.
    //
    // The gateway shape: `entitlement_for` still answers `None` for
    // `GlasshouseGateway` by construction, so a resumed gateway-backed
    // session reads the provider off the gateway `overlay_resolution` above
    // already started for this resume — no second gateway is started here —
    // and asks `entitlement_for_provider` instead, exactly as
    // `launch_session`'s gateway branch does. `None` when that overlay
    // itself did not resolve (reported above already): the fallback text is
    // still true then, because nothing here knows the provider either.
    let resume_entitlement = {
        let profile = record
            .launch_profile
            .as_deref()
            .and_then(|name| effective.launch_profile(name, selection.id()).ok())
            .map(|layered| layered.value)
            .unwrap_or_else(|| glasshouse::profile::LaunchProfile::native(selection.id()));
        let gateway_provider = matches!(
            profile.backend,
            glasshouse::profile::BackendResource::GlasshouseGateway
        )
        .then(|| {
            overlay_resolution
                .as_ref()
                .and_then(|(_, _, gateway)| gateway.as_ref())
        })
        .flatten()
        .map(|gateway| gateway.serving_provider());
        let lookup = match gateway_provider {
            Some(provider) => effective.entitlement_for_provider(provider),
            None => effective.entitlement_for(profile.harness, &profile.backend),
        };
        match lookup {
            Ok(entitlement) => {
                announce_entitlement(entitlement.as_ref(), &profile, gateway_provider);
                entitlement
            }
            Err(err) => {
                eprintln!("glasshouse: {err}");
                None
            }
        }
    };

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

    let mut launch = HarnessLaunch::new(selection.into_executable(), runtime.project()).args(args);
    // Map line 1973, on the path that continues a session — the same scrub
    // `launch_session` applies, for the same reason: the child inherits this
    // process's environment, and another account's credential variable has
    // no business in it.
    for var in
        effective.foreign_entitlement_credential_vars(resume_entitlement.as_ref().map(|e| e.name()))
    {
        launch = launch.env_remove(var);
    }
    let launch = launch;
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
        run_headless(runtime, &store, &resumable.id, launch, None)
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
    force_probe: bool,
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
    // Capability map lines 1316/1365: recent failures by class, from the
    // project's routing evidence ledger. Fail-soft: a project with no ledger
    // yet renders `unknown` on that line, as the caches do.
    if let Ok(ledger) = glasshouse::routing::evidence::EvidenceLedger::open(runtime) {
        telemetry = telemetry.gather_failure_classes(&ledger, now_unix);
    }
    if !no_harness {
        telemetry = telemetry.gather_harness_status(now_unix);
    }

    let mut probes = String::new();
    if !probe.is_empty() {
        use std::fmt::Write as _;
        let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();
        let _ = writeln!(probes, "PROBES\n");
        for name in probe {
            let authorization = glasshouse::provider::resources::authorize_probe(
                &effective, &telemetry, name, now_unix,
            );
            let reading = match authorization {
                glasshouse::provider::resources::ProbeAuthorization::Refused(budget)
                    if !force_probe =>
                {
                    glasshouse::provider::resources::ProbeReading::Refused {
                        remaining: budget.remaining,
                        cost: budget.cost,
                    }
                }
                glasshouse::provider::resources::ProbeAuthorization::Refused(budget) => {
                    glasshouse::provider::resources::render_forced_probe(
                        &mut probes,
                        name,
                        &budget,
                    );
                    glasshouse::provider::resources::probe_provider(
                        &effective, &secrets, name, now_unix,
                    )
                }
                glasshouse::provider::resources::ProbeAuthorization::Allowed => {
                    glasshouse::provider::resources::probe_provider(
                        &effective, &secrets, name, now_unix,
                    )
                }
            };
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
    render_routing_economics(&mut out, runtime, now_unix);
    Ok(out)
}

/// Capability map lines 1463, 1465 and 1466 — what routing itself costs, as
/// the block after `ROUTING MODEL` in `glasshouse resources`.
///
/// Three lines and a conditional fourth, every one carrying its
/// denominators: decisions *over* interactive hours, tokens *over* calls on
/// each side, and the overhead fraction beside the line it is judged
/// against. A figure nobody counted prints as *not counted*
/// ([`render_token_count`]'s rule), and a comparison that cannot be made
/// prints as *not comparable* with the reason — never `0%`, which would read
/// as "routing is free".
///
/// # Both ledgers are opened here and dropped here (practice §65)
///
/// Each store is opened inside the helper that reads it and closed before
/// the next one opens, so no handle is held across a read it is not part
/// of. A store that cannot be opened renders as *unavailable* with the
/// reason, and the command succeeds: [`resources_report`]'s own header says
/// no telemetry read may fail it, and this block is telemetry.
fn render_routing_economics(out: &mut String, runtime: &Runtime, now_unix: i64) {
    use glasshouse::routing::evidence::{
        CLASSIFICATION_EVIDENCE_WINDOW_SECONDS, ROUTING_OVERHEAD_WARNING_FRACTION,
    };
    use std::fmt::Write as _;

    let window_seconds = CLASSIFICATION_EVIDENCE_WINDOW_SECONDS;
    let from = now_unix.saturating_sub(window_seconds);
    let _ = writeln!(
        out,
        "\nROUTING ECONOMICS (last {} days)",
        window_seconds / (24 * 60 * 60)
    );

    match routing_decision_rate(runtime, from, now_unix) {
        Ok(rate) => {
            let per_hour = match rate.per_hour() {
                Some(per_hour) => format!("{per_hour:.1} per interactive hour"),
                None => "no interactive hour in the window, so no rate".to_owned(),
            };
            let _ = writeln!(
                out,
                "  {:<16}{} routing decisions over {} interactive hours — {per_hour}",
                "decisions", rate.decisions, rate.interactive_hours
            );
            let _ = writeln!(
                out,
                "  {:<16}an interactive hour is a wall-clock hour in which a session record \
                 shows activity (map line 1463)",
                ""
            );
        }
        Err(reason) => {
            let _ = writeln!(out, "  {:<16}unavailable — {reason:#}", "decisions");
        }
    }

    let tokens = |count: Option<i64>| match count {
        Some(count) => format!("{count} tokens"),
        None => "tokens not counted".to_owned(),
    };
    match routing_overhead(runtime, now_unix, window_seconds) {
        Ok(overhead) => {
            let _ = writeln!(
                out,
                "  {:<16}{} over {} classification calls",
                "routing spend",
                tokens(overhead.classification_tokens),
                overhead.classification_requests
            );
            let _ = writeln!(
                out,
                "  {:<16}{} over {} other calls — everything this project's ledger holds that \
                 is not classification (map line 1465)",
                "task spend",
                tokens(overhead.task_tokens),
                overhead.task_requests
            );
            // Capability map lines 1832 and 1833: what *"task spend"* above
            // is actually made of, each side with both its denominators —
            // tokens and calls. Every one of these is a subset of the line
            // above rather than a competing total, and they sum to it
            // exactly (`RoutingOverhead`'s own doc comment).
            let _ = writeln!(
                out,
                "  {:<16}{} over {} extraction calls — Glasshouse's own memory extraction, \
                 apart from the coding agent's work (map line 1832)",
                "extraction",
                tokens(overhead.extraction_tokens),
                overhead.extraction_requests
            );
            let _ = writeln!(
                out,
                "  {:<16}{} over {} decision rows — the routing model's own request \
                 consumption, which carries no tokens because a decision's latency is not a \
                 model call (map line 1833)",
                "routing model",
                tokens(overhead.routing_latency_tokens),
                overhead.routing_latency_requests
            );
            let _ = writeln!(
                out,
                "  {:<16}{} over {} decision rows — tier escalations and downgrades the session \
                 router acted on (map line 1566); no tokens, because a decision is not a model \
                 call",
                "tier movement",
                tokens(overhead.tier_movement_tokens),
                overhead.tier_movement_requests
            );
            let _ = writeln!(
                out,
                "  {:<16}{} over {} fallback rows — pool fallbacks the launch path acted on \
                 (map line 1970); no tokens, because a decision is not a model call",
                "pool fallback",
                tokens(overhead.entitlement_fallback_tokens),
                overhead.entitlement_fallback_requests
            );
            let _ = writeln!(
                out,
                "  {:<16}{} over {} relayed exchanges — interactive coding cost; the gateway \
                 relays a body it never parses, so a token count here is absent rather than \
                 zero",
                "coding agent",
                tokens(overhead.coding_agent_tokens),
                overhead.coding_agent_requests
            );
            let _ = writeln!(
                out,
                "  {:<16}{} over {} calls — rows written before this build stamped a purpose, \
                 counted as unstamped and never re-labelled",
                "unstamped",
                tokens(overhead.unstamped_tokens),
                overhead.unstamped_requests
            );
            match overhead.fraction() {
                Some(fraction) => {
                    let _ = writeln!(
                        out,
                        "  {:<16}{:.1}% of task spend — warns above {:.0}% (map line 1466)",
                        "overhead",
                        fraction * 100.0,
                        ROUTING_OVERHEAD_WARNING_FRACTION * 100.0
                    );
                    if overhead.exceeds(ROUTING_OVERHEAD_WARNING_FRACTION) {
                        let _ = writeln!(
                            out,
                            "  {:<16}routing is consuming {:.1}% of the task spend it exists to \
                             protect, above the {:.0}% line — the routing model is no longer \
                             cheap relative to what it saves",
                            "warning",
                            fraction * 100.0,
                            ROUTING_OVERHEAD_WARNING_FRACTION * 100.0
                        );
                    }
                }
                None => {
                    let why = if overhead.classification_tokens.is_none() {
                        "no classification call in the window carried a token count"
                    } else if overhead.task_tokens.is_none() {
                        "no other call in the window carried a token count"
                    } else {
                        "no task spend was counted in the window to compare against"
                    };
                    let _ = writeln!(out, "  {:<16}not comparable — {why}", "overhead");
                }
            }
        }
        Err(reason) => {
            let _ = writeln!(out, "  {:<16}unavailable — {reason:#}", "routing spend");
        }
    }
}

/// Capability map line 1463's two stores, each opened for exactly its own
/// read: the session store for activity spans, then the evaluation ledger
/// for the count.
fn routing_decision_rate(
    runtime: &Runtime,
    from: i64,
    to: i64,
) -> anyhow::Result<glasshouse::evaluation::RoutingDecisionRate> {
    let spans: Vec<(i64, i64)> = {
        let sessions = glasshouse::session::ProjectSessions::open(runtime)?;
        let records = sessions.store().list()?;
        records
            .into_iter()
            .map(|record| (record.created_at, record.last_activity_at))
            .collect()
    };
    let ledger = glasshouse::evaluation::EvaluationObservations::open(runtime)?;
    Ok(ledger.routing_decision_rate(spans, from, to)?)
}

/// Capability map line 1465's reading, from the evidence ledger alone.
fn routing_overhead(
    runtime: &Runtime,
    now_unix: i64,
    window_seconds: i64,
) -> anyhow::Result<glasshouse::routing::evidence::RoutingOverhead> {
    let ledger = glasshouse::routing::evidence::EvidenceLedger::open(runtime)?;
    let groups = ledger.consumption_by_purpose(now_unix, window_seconds)?;
    Ok(glasshouse::routing::evidence::RoutingOverhead::from_consumption(&groups))
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
///
/// `session` is the requesting session's id, when the caller has one in
/// scope — `GH-RETRIEVAL-ATTRIBUTION`'s gap 1. Both current callers pass
/// `None`: `memory_report`'s CLI command has no session to attribute a
/// person's own `memory search` to, and `query_memory`'s `Request::QueryMemory`
/// carries no session field to thread one from. Never guessed — see
/// [`glasshouse::evaluation::record_memory_retrieval`]'s own doc comment.
fn memory_search_grouped(
    runtime: &Runtime,
    query: &str,
    history: bool,
    limit: usize,
    session: Option<&str>,
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
    //
    // Map line 1865: a search that returned nothing in either group records
    // one miss row instead — never both, and never neither.
    if grouped.invariants_and_constraints.is_empty() && grouped.other.is_empty() {
        glasshouse::evaluation::record_memory_retrieval_miss(
            runtime,
            glasshouse::evaluation::RetrievalScope::from_history_flag(history),
            glasshouse::evaluation::now_unix(),
        );
    } else {
        glasshouse::evaluation::record_memory_retrieval(
            runtime,
            glasshouse::evaluation::RetrievalScope::from_history_flag(history),
            grouped
                .invariants_and_constraints
                .iter()
                .chain(grouped.other.iter())
                .map(|record| record.id.as_str()),
            session,
            glasshouse::evaluation::now_unix(),
        );
    }

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
    let grouped = memory_search_grouped(runtime, query, history, limit, None)?;
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

/// `glasshouse memory retrievals`: how retrieval has been doing over a
/// window — map lines 1822 and 1826's own numbers, plus map line 1865's
/// miss count, giving [`glasshouse::evaluation::EvaluationObservations::stale_retrievals`]
/// its first production caller (practice §90; `phase-51.md`'s 1822/1826
/// re-open).
fn memory_retrievals_report(runtime: &Runtime, hours: u32) -> anyhow::Result<String> {
    use glasshouse::evaluation::{EvaluationKind, EvaluationObservations};

    let ledger = EvaluationObservations::open(runtime)?;
    let to = glasshouse::evaluation::now_unix();
    let from = to - i64::from(hours) * 3600;
    let counts = ledger.stale_retrievals(from, to)?;
    let missed = ledger.count(EvaluationKind::MemoryRetrievalMiss, from, to)?;
    let usefulness = ledger.usefulness(from, to)?;
    let prevented_repetition = ledger.prevented_repetition(from, to)?;
    let caused_complexity = ledger.caused_complexity(from, to)?;
    let revalidation_accuracy = ledger.revalidation_accuracy(from, to)?;
    let challenge_accuracy = ledger.challenge_accuracy(from, to)?;
    Ok(render_memory_retrievals(
        runtime.project().id().as_str(),
        hours,
        &counts,
        missed,
        &usefulness,
        &prevented_repetition,
        &caused_complexity,
        &revalidation_accuracy,
        &challenge_accuracy,
    ))
}

/// Pure formatting half of [`memory_retrievals_report`].
///
/// **`stale` and `stale-under-history` are printed disjoint**, though
/// [`glasshouse::evaluation::StaleRetrievalCounts`] itself keeps
/// `stale_under_history` as a subset of `stale` by that struct's own
/// contract (`stale` counts every stale hit regardless of which scope asked
/// for it). This is the one place the distinction map line 1826 exists for
/// is rendered for a person: a superseded memory returned only because
/// `--history` explicitly asked for it is the tool doing what it was told,
/// not a defect, so it is printed once, under `stale-under-history`, and
/// subtracted out of `stale` rather than counted under both.
#[allow(clippy::too_many_arguments)]
fn render_memory_retrievals(
    project_id: &str,
    hours: u32,
    counts: &glasshouse::evaluation::StaleRetrievalCounts,
    missed: i64,
    usefulness: &glasshouse::evaluation::UsefulnessCounts,
    prevented_repetition: &glasshouse::evaluation::PreventedRepetitionCounts,
    caused_complexity: &glasshouse::evaluation::CausedComplexityCounts,
    revalidation_accuracy: &glasshouse::evaluation::RevalidationAccuracyCounts,
    challenge_accuracy: &glasshouse::evaluation::ChallengeAccuracyCounts,
) -> String {
    use std::fmt::Write as _;

    let stale_outside_history = counts.stale - counts.stale_under_history;
    let mut out = format!("Memory retrievals for project {project_id}, last {hours}h\n\n");
    let _ = writeln!(out, "  {:<20}{}", "returned", counts.retrievals);
    let _ = writeln!(out, "  {:<20}{}", "stale", stale_outside_history);
    let _ = writeln!(
        out,
        "  {:<20}{}",
        "stale-under-history", counts.stale_under_history
    );
    let _ = writeln!(out, "  {:<20}{}", "unresolved", counts.unresolved);
    let _ = writeln!(out, "  {:<20}{}", "missed", missed);

    let _ = write!(
        out,
        "\n{}",
        render_memory_quality(
            usefulness,
            prevented_repetition,
            caused_complexity,
            revalidation_accuracy,
            challenge_accuracy
        )
    );
    out
}

/// Memory quality for the same window `render_memory_retrievals` prints —
/// "Phase 51, the memory half of RC-B", user ruling 2026-09-02: an explicit
/// rating when given, a labelled `proxy` where the design decision defines
/// one, and `unknown`, always with its own denominator.
fn render_memory_quality(
    usefulness: &glasshouse::evaluation::UsefulnessCounts,
    prevented_repetition: &glasshouse::evaluation::PreventedRepetitionCounts,
    caused_complexity: &glasshouse::evaluation::CausedComplexityCounts,
    revalidation_accuracy: &glasshouse::evaluation::RevalidationAccuracyCounts,
    challenge_accuracy: &glasshouse::evaluation::ChallengeAccuracyCounts,
) -> String {
    use std::fmt::Write as _;

    let mut out = String::from("Memory quality\n\n");

    let rated = usefulness.explicit_useful + usefulness.explicit_not_useful;
    let _ = writeln!(out, "useful (1821):");
    let _ = writeln!(
        out,
        "  explicit useful {} / not-useful {} of {rated} rated",
        usefulness.explicit_useful, usefulness.explicit_not_useful
    );
    let _ = writeln!(
        out,
        "  proxy useful {} of {} retrieved-into-completed-turns",
        usefulness.proxy_useful, usefulness.proxy_denominator
    );
    let _ = writeln!(
        out,
        "  unknown {} of {} retrieved",
        usefulness.unknown, usefulness.retrieved
    );

    let _ = writeln!(out, "\nprevented-repetition (1831):");
    let _ = writeln!(
        out,
        "  explicit prevented-repetition {} of {} retrieved-failed-approach-memories",
        prevented_repetition.explicit, prevented_repetition.retrieved
    );
    let _ = writeln!(
        out,
        "  proxy prevented-repetition {} of {} retrieved-into-completed-turns",
        prevented_repetition.proxy, prevented_repetition.proxy_denominator
    );
    let _ = writeln!(
        out,
        "  unknown {} of {} retrieved-failed-approach-memories",
        prevented_repetition.unknown, prevented_repetition.retrieved
    );

    let _ = writeln!(out, "\ncaused-complexity (1823):");
    let _ = writeln!(
        out,
        "  explicit caused-complexity {} of {} retrieved-decision-memories",
        caused_complexity.explicit, caused_complexity.retrieved
    );
    let _ = writeln!(
        out,
        "  unknown {} of {} retrieved-decision-memories",
        caused_complexity.unknown, caused_complexity.retrieved
    );
    let _ = writeln!(out, "  no proxy: nothing observed bears on this");

    let _ = writeln!(out, "\nrevalidation-accuracy (1824):");
    let _ = writeln!(
        out,
        "  explicit revalidation-correct {} / revalidation-wrong {} of {} revalidations",
        revalidation_accuracy.correct,
        revalidation_accuracy.wrong,
        revalidation_accuracy.revalidations
    );
    let _ = writeln!(
        out,
        "  unknown {} of {} revalidations",
        revalidation_accuracy.unknown, revalidation_accuracy.revalidations
    );
    let _ = writeln!(out, "  no proxy: nothing observed bears on this");

    let _ = writeln!(out, "\nchallenge-accuracy (1825):");
    let _ = writeln!(
        out,
        "  explicit challenge-justified {} / challenge-unjustified {} of {} challenges",
        challenge_accuracy.justified, challenge_accuracy.unjustified, challenge_accuracy.challenges
    );
    let _ = writeln!(
        out,
        "  unknown {} of {} challenges",
        challenge_accuracy.unknown, challenge_accuracy.challenges
    );
    let _ = writeln!(out, "  no proxy: nothing observed bears on this");

    out
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
    // Phase 29: *every trigger names itself on the memory it produced.* On
    // its own line rather than appended above, because it answers a different
    // question from the three facts there — those say where this memory came
    // from, this says what made Glasshouse go and look.
    //
    // A memory with no recorded trigger prints **nothing** rather than
    // `unknown`, unlike its neighbours. Those three have been written for
    // every memory this build stores since Phase 20, so an `unknown` there
    // really does mean the producer did not know; a trigger is absent for
    // every memory recorded before the column existed, and a line reading
    // `trigger unknown` under all of them would be noise claiming to be a
    // finding.
    if let Some(trigger) = record.extraction_trigger.as_deref() {
        writeln!(out, "    trigger {trigger}")?;
    }
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

/// `glasshouse memory rate <id> <verdict> [--session <id>] [--note <text>]`
/// — "Phase 51, the memory half of RC-B", user ruling 2026-09-02: *"Both:
/// explicit rating when given, the labelled proxy otherwise."* This is the
/// explicit half — one new [`glasshouse::evaluation::EvaluationKind::MemoryRated`]
/// observation, never an edit of the retrieval it judges.
///
/// Project isolation the same way [`memory_challenge`] and
/// [`memory_resolve_conflict`] get it: [`glasshouse::memory::MemoryStore::resolve_id`]
/// refuses an id from another project by name before this ever opens the
/// evaluation ledger, so a rating can never be recorded against a memory
/// this project cannot see.
fn memory_rate(
    runtime: &Runtime,
    id: &str,
    verdict: glasshouse::evaluation::EvaluationOutcome,
    session: Option<&str>,
    note: Option<&str>,
) -> anyhow::Result<String> {
    use glasshouse::memory::ProjectMemory;

    let memory = ProjectMemory::open(runtime)?;
    let resolved = memory.store().resolve_id(id)?;

    glasshouse::evaluation::record_memory_rating(
        runtime,
        resolved.as_str(),
        verdict,
        session,
        note,
        glasshouse::evaluation::now_unix(),
    )?;

    Ok(format!(
        "{} rated {}\n",
        resolved.as_str(),
        verdict.as_str()
    ))
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

    // Map line 1824's own denominator, `GH-RETRIEVAL-ATTRIBUTION`: the store
    // mutation above is the real act and has already succeeded, so this row
    // records that a revalidation happened without being able to fail the
    // command that already did.
    glasshouse::evaluation::record_memory_revalidation(
        runtime,
        record.id.as_str(),
        outcome,
        glasshouse::evaluation::now_unix(),
    );

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
/// `glasshouse memory commit` — map line 1148, *"allow a memory commit to be
/// triggered manually."*
///
/// # It is the same operation the harness triggers, not a hand-written twin
///
/// This calls [`run_extraction`] with
/// [`glasshouse::memory::ExtractionTrigger::Manual`], which is the same
/// function the `TurnEnded` and `PreCompact` arms of [`report_hook_with`]
/// call. Everything a person could get wrong by hand — the event window, the
/// credential screen, the duplicate check, the bound, the working-tree
/// observation, the routing observation — is therefore identical by
/// construction rather than by two implementations agreeing.
///
/// It is deliberately *not* [`memory_extract`]. That command exists to
/// evaluate the contract without a provider, takes its reply from a file, and
/// says so on every run; this one asks the model the user configured, which
/// is what makes it a memory commit rather than a harness.
///
/// # Defaulting to the most recently active session
///
/// `SessionStore::list` is ordered `last_activity_at DESC`, which is the
/// project's own answer to *"what was I just working on"* and the same order
/// `glasshouse sessions` prints. A project with no sessions is an error
/// naming the flag rather than a silent success: there is no honest
/// "recently completed work" to commit, and reporting *stored 0* would be
/// indistinguishable from a model that looked and found nothing.
///
/// # One database handle at a time
///
/// The session lookup is scoped so `ProjectSessions` is closed before
/// [`run_extraction`] opens the event log and the memory store. That is
/// practice §65's rule taken seriously on a path that has the choice: a
/// handle held across work that does not need it is free on this developer's
/// machine and billed under Windows' mandatory locks.
fn memory_commit(runtime: &Runtime, session: Option<&str>) -> anyhow::Result<String> {
    use std::fmt::Write as _;

    use glasshouse::memory::ExtractionTrigger;

    let id = {
        let sessions = ProjectSessions::open(runtime)?;
        let store = sessions.store();
        match session {
            Some(session) => store.resolve_id(session)?,
            None => store
                .list()?
                .into_iter()
                .next()
                .map(|record| record.id)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "this project has no sessions to commit; name one with --session"
                    )
                })?,
        }
    };

    let model = disposable_extraction_model(runtime, &id);
    let Some(outcome) = run_extraction(runtime, &id, model, ExtractionTrigger::Manual) else {
        // `run_extraction` logged which of the two it was. Neither is a
        // failure of the command: nothing was stored, nothing was corrupted,
        // and the next commit will read the same activity.
        return Ok(format!(
            "memory commit for session {id} produced nothing;              see the log for why\n"
        ));
    };

    let mut out = String::new();
    writeln!(out, "trigger {}, model {}", outcome.trigger, outcome.model)?;
    writeln!(out, "session: {id}")?;
    if let Some(failure) = &outcome.failure {
        writeln!(out, "memory commit produced nothing: {failure}")?;
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
    for rejection in &outcome.rejected {
        writeln!(out, "    rejected  {rejection}")?;
    }
    Ok(out)
}

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

/// `glasshouse memory export --tracked` — Phase 50's tracked project
/// knowledge, and the only production caller of
/// [`glasshouse::memory::TrackedKnowledge::write`].
///
/// `tracked` gates writing outright: omitting `--tracked` prints an
/// explanation and writes nothing, so typing the subcommand alone is never
/// enough to put files in the tree. That is deliberately a second gate on top
/// of the subcommand existing at all — map lines 1810/1811 ask for an
/// explicit opt-in, not merely a discoverable one.
fn memory_export_tracked(
    runtime: &Runtime,
    tracked: bool,
    include_findings: bool,
    dry_run: bool,
) -> anyhow::Result<String> {
    use std::fmt::Write as _;

    use glasshouse::memory::{ProjectMemory, Selection, TrackedKnowledge};

    let mut out = String::new();
    if !tracked {
        writeln!(
            out,
            "tracked project knowledge is off by default; nothing was written. \
             Pass --tracked to opt in."
        )?;
        return Ok(out);
    }

    let memory = ProjectMemory::open(runtime)?;
    let selection = Selection { include_findings };
    let manifest = TrackedKnowledge::write(&memory, runtime.project().root(), selection, dry_run)?;

    if manifest.dry_run {
        writeln!(out, "dry run: nothing was written")?;
    }
    if manifest.written.is_empty() {
        writeln!(out, "no decisions or constraints to export yet")?;
    } else {
        for file in &manifest.written {
            writeln!(out, "{}  {}  {}", file.kind, file.id, file.path.display())?;
        }
    }
    writeln!(out, "{}", manifest.readme.display())?;

    if manifest.git_absent {
        writeln!(
            out,
            "note: {} has no .git directory; the files were still written",
            runtime.project().display_root().display()
        )?;
    }
    if manifest.gitignored {
        writeln!(
            out,
            "note: this project's .gitignore ignores .glasshouse/; the files were \
             still written, and Glasshouse does not edit .gitignore"
        )?;
    }

    Ok(out)
}

/// `glasshouse memory export-local` — map line 2040, Phase 58 item 6.
///
/// A sibling of [`memory_export_tracked`] above, not a variant of it:
/// [`MemoryCommand::Export`] projects tracked knowledge into
/// `.glasshouse/knowledge/`; this writes a gitignored harness file instead,
/// and is opt-in the same way — nothing here runs unless this subcommand is
/// typed.
///
/// `harness` defaults to
/// [`glasshouse::memory::export_local::LocalHarness::DEFAULT_SLUG`]
/// (`claude-code`), the only harness this build knows a native local
/// instruction file for.
fn memory_export_local(
    runtime: &Runtime,
    harness: Option<&str>,
    limit: usize,
    exclude: bool,
) -> anyhow::Result<String> {
    use glasshouse::memory::export_local::{self, LocalHarness};
    use glasshouse::memory::{MemoryKind, ProjectMemory};

    let harness_slug = harness.unwrap_or(LocalHarness::DEFAULT_SLUG);

    let memory = ProjectMemory::open(runtime)?;
    let store = memory.store();
    let mut records = store.binding(limit)?;
    records.extend(store.current_of_kind(MemoryKind::FailedAttempt, limit)?);

    let outcome = export_local::export(
        runtime.project().root(),
        harness_slug,
        &records,
        glasshouse::evaluation::now_unix(),
        exclude,
    )?;

    let exclude_note = match outcome.exclude {
        export_local::ExcludeAction::Added => "added to .git/info/exclude",
        export_local::ExcludeAction::AlreadyExcluded => "already gitignored",
        export_local::ExcludeAction::Skipped => "--no-exclude: left untouched",
        export_local::ExcludeAction::NotGitRepo => "no .git directory: nothing to exclude",
    };

    Ok(format!(
        "{harness_slug}: {} {} written to {} ({exclude_note})\n",
        outcome.exported,
        if outcome.block_present {
            "memories"
        } else {
            "memories (block removed)"
        },
        outcome.path.display(),
    ))
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

    // The one column whose width depends on the data: `external
    // workspace:<n>` is wider than any presentation word, and a listing with
    // no pane in it is laid out exactly as it was before panes existed.
    let presented_width = records
        .iter()
        .map(|record| presented_cell(record).len())
        .chain(std::iter::once(PRESENTED_WIDTH))
        .max()
        .unwrap_or(PRESENTED_WIDTH);
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
            "LAST ACTIVITY",
            presented_width,
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
                &presented_cell(record),
                &format_age(record.last_activity_at),
                presented_width,
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

    // Map line 1963: every configured entitlement is its own resource, named
    // here one entry per account — never merged by vendor, kind or backing,
    // because two accounts of one vendor being two resources is what makes
    // the pool a pool. A user with no `[entitlements]` entries sees no line.
    //
    // Map line 1965: each entry then carries its four telemetry facets —
    // capacity band, time until reset, recent throttling, the models it can
    // serve — from the telemetry the provider actually exposes, `unknown`
    // spelled out where nothing exists, and every shared reading marked with
    // its scope. The sources are read here, once, and handed to the one
    // resolver, so two entitlements of one provider cannot be handed
    // different provider-wide readings.
    let user = UserConfig::load(runtime.paths())?;
    let project_config = config::load_project_config(runtime.project())?;
    let effective = EffectiveConfig::new(&user, project_config.as_ref());
    // One resolver, one set of sources — `entitlement_pool_with_telemetry`,
    // which `glasshouse entitlements` reads through as well so the two
    // commands cannot describe one account differently.
    match entitlement_pool_with_telemetry(runtime, &effective) {
        Ok(entitlements) if entitlements.is_empty() => {}
        Ok(entitlements) => {
            let names: Vec<String> = entitlements
                .iter()
                .map(|entry| format!("`{}`", entry.name()))
                .collect();
            let _ = writeln!(
                out,
                "Entitlements {} configured — {}",
                entitlements.len(),
                names.join(", ")
            );
            let thresholds = effective.capacity_band_thresholds().value;
            for entry in &entitlements {
                let _ = writeln!(
                    out,
                    "  `{}`  {}",
                    entry.name(),
                    entitlement_facets(entry, &thresholds)
                );
            }
        }
        Err(err) => {
            let _ = writeln!(out, "Entitlements not resolvable — {err}");
        }
    }

    // Map line 2006: mode and per-session aggregate savings, shown only
    // when the firewall is configured on — "with the firewall off, nothing
    // changes" (the guarantee every one of map lines 1980-2003 already
    // keeps) extends to this section too, so an `off` session's status
    // output stays exactly what it was before this package existed.
    let firewall_mode = effective.context_firewall_mode().value;
    if firewall_mode != glasshouse::config::firewall::FirewallMode::Off {
        let _ = writeln!(out);
        let _ = writeln!(out, "Context firewall  mode: {firewall_mode}");
        match context_firewall_savings_summary(runtime) {
            Some(summary) => {
                let _ = writeln!(out, "  {summary}");
            }
            None => {
                let _ = writeln!(out, "  no context-firewall activity recorded yet");
            }
        }
    }

    Ok(out)
}

/// Map line 2006's savings figure: an honest aggregate over every entry the
/// raw store currently holds, walked with [`glasshouse::firewall::RawStore::all_entries`]
/// rather than any evidence-ledger reader — the packet's own constraint
/// stands (map line 1987's ruling): the ledger's token columns are a
/// provider's own reported count, and this build's raw/forwarded figures
/// are `chars/4` estimates, so they are never written there. Chosen over a
/// bare request-count ("N of M reduced") because [`RawEntry::original_token_estimate`]
/// and [`RawEntry::forwarded_token_estimate`] are already persisted per
/// entry (map line 2005) and a token figure is closer to what "savings"
/// means than a request count alone — see this package's report for the
/// full reasoning. `None` when the store holds nothing yet, a different
/// fact from "0 saved".
fn context_firewall_savings_summary(runtime: &Runtime) -> Option<String> {
    let store = glasshouse::firewall::RawStore::open(runtime.state_dir().join("context-firewall"));
    let entries = store.all_entries().ok()?;
    if entries.is_empty() {
        return None;
    }

    let sessions: std::collections::HashSet<&str> = entries
        .iter()
        .map(|entry| entry.session_id.as_str())
        .collect();
    let mut original_of_estimated = 0u64;
    let mut forwarded_total = 0u64;
    let mut unestimated = 0usize;
    for entry in &entries {
        match entry.forwarded_token_estimate {
            Some(forwarded) => {
                original_of_estimated += entry.original_token_estimate;
                forwarded_total += forwarded;
            }
            // An entry recorded before map line 2005 carries no comparison
            // — counted toward "results", never folded into a savings
            // figure it never measured.
            None => unestimated += 1,
        }
    }
    let kept_local = original_of_estimated.saturating_sub(forwarded_total);
    let unestimated_note = if unestimated > 0 {
        format!(" ({unestimated} without a recorded estimate)")
    } else {
        String::new()
    };

    Some(format!(
        "{} session{}, {} result{} reduced, ~{kept_local} of ~{original_of_estimated} estimated \
         tokens kept local{unestimated_note}",
        sessions.len(),
        if sessions.len() == 1 { "" } else { "s" },
        entries.len(),
        if entries.len() == 1 { "" } else { "s" },
    ))
}

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
fn entitlement_pool_with_telemetry(
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

/// `glasshouse entitlements` — map line 1972's inspectable view of the pool.
///
/// A pure function returning a `String`, like [`status_report`] and
/// `resources_report`: what it prints is testable without a terminal, which
/// is the only reason a view of this kind can be asserted at all.
///
/// # Every configured entitlement, including the ones nothing measured
///
/// The rows come from the **configuration**, not from the telemetry and not
/// from the sessions table, so an account no reading describes still gets a
/// row and reads `unknown` on the facets it has no reading for. An
/// entitlement missing from the view because nothing had measured it is the
/// exact failure 56A step 2's Cluster E discipline exists to prevent: unknown
/// is a rendered word, never full, never empty, never a number.
///
/// # Why `served` is *not* one of those unknowns
///
/// The four telemetry facets are `unknown` when nobody looked. `served` is
/// different in kind: this function **does** look, at every session row this
/// project recorded, and an account with no rows has a *measured* zero. That
/// is `SessionRecord::observed_compactions`' distinction, and rendering
/// "nothing recorded" where the sessions table is empty rather than `unknown`
/// is what keeps the two apart.
///
/// # Names, never credentials
///
/// An entitlement is named by its `[entitlements.<name>]` key and described
/// by its kind and vendor. Its `credential` is a `config::SecretRef` and this
/// function never touches it — nothing here opens a secret store, and there
/// is no branch on which this view could print a value.
fn entitlements_report(runtime: &Runtime) -> anyhow::Result<String> {
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
        for entry in &entitlements {
            let _ = writeln!(out, "`{}`  ({})", entry.name(), entry.describe());
            // The same renderer `glasshouse status` uses, deliberately: the
            // two commands describing one account differently would be a
            // defect nobody could act on.
            let _ = writeln!(out, "  {}", entitlement_facets(entry, &thresholds));
            let _ = writeln!(out, "  served: {}", served_phrase(served.get(entry.name())));
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
            short_id(&latest.id),
            format_age(latest.last_activity_at)
        ),
    }
}

fn entitlement_facets(
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
            rendered
        }
        (None, None) => "headroom estimate: unknown".to_owned(),
    };

    format!("{capacity} · {reset} · {throttling} · {models} · {headroom_estimate}")
}

/// One line of the session listing, header included.
///
/// The header and the rows go through the same function so their columns
/// cannot drift apart — the usual way a hand-aligned table stops lining up is
/// someone widening a column in one of the two format strings.
/// The `PRESENTED` column's width when no row needs more: the header's own
/// length, which every presentation word fits inside.
const PRESENTED_WIDTH: usize = "PRESENTED".len();

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
    presented_width: usize,
) -> String {
    // Widths fit the longest value each column can hold: `resumable`,
    // `orchestrator`. `presented` is the one column sized by the listing —
    // see `session_report` — because `external workspace:<n>` is wider than
    // any presentation word and a listing without one should not pay for
    // it. `name` and `purpose` are the two the user controls, and they are
    // truncated by the format rather than bounded here — the store already
    // refuses anything longer than 64 and 32.
    format!(
        "{session:<12}  {name:<16}  {purpose:<10}  {harness:<14}  {profile:<12}  {state:<9}  \
         {role:<12}  {presented:<presented_width$}  {activity}"
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
        "presentation ref",
        record.presentation_ref.as_deref().unwrap_or("-"),
    );
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

    // Phase 21K lines 1048 and 1049: the session's open premises and its
    // last gate, on a handle opened after the session store's is gone
    // (practice §65), and bounded so the normal view is not flooded.
    drop(store);
    drop(sessions);
    out.push_str(&assumption_section(runtime, &id));
    Ok(out)
}

/// How many open premises `glasshouse sessions show` lists before it says
/// how many more there are — line 1048's *"without flooding"*.
const SHOWN_OPEN_PREMISES: usize = 3;

/// The `sessions show` lines for a session's assumptions: a count line, at
/// most [`SHOWN_OPEN_PREMISES`] open premises, the last gate and the
/// override in force. A ledger that cannot be read collapses to `-`, like
/// every other field above it.
fn assumption_section(runtime: &Runtime, id: &glasshouse::session::SessionId) -> String {
    use glasshouse::guardrails::{AssumptionState, TransitionKind, quote};
    use std::fmt::Write as _;

    let mut out = String::new();
    let mut line = |label: &str, value: &str| {
        let _ = writeln!(out, "{label:<19}{value}");
    };

    let Ok(ledger) = AssumptionStore::open(runtime) else {
        line("assumptions", "-");
        return out;
    };
    let session = id.as_str();
    let (Ok(counts), Ok(open)) = (
        ledger.counts(Some(session)),
        ledger.open_for_session(session),
    ) else {
        line("assumptions", "-");
        return out;
    };
    let count_of = |state: AssumptionState| {
        counts
            .iter()
            .find(|(s, _)| *s == state)
            .map_or(0, |(_, n)| *n)
    };
    let total: i64 = counts.iter().map(|(_, n)| n).sum();
    if total == 0 {
        line("assumptions", "none stated");
    } else {
        line(
            "assumptions",
            &format!(
                "{} open · {} supported · {} refuted · {} waived",
                open.len(),
                count_of(AssumptionState::Supported),
                count_of(AssumptionState::Refuted),
                count_of(AssumptionState::WaivedByUser)
            ),
        );
    }
    for view in open.iter().take(SHOWN_OPEN_PREMISES) {
        line(
            "  open premise",
            &format!(
                "[{}] {} ({})",
                view.state,
                quote(&view.record.claim, 96),
                view.record.id.short()
            ),
        );
    }
    if open.len() > SHOWN_OPEN_PREMISES {
        line(
            "",
            &format!(
                "… and {} more; `glasshouse assumptions --session {}`",
                open.len() - SHOWN_OPEN_PREMISES,
                short_id(id)
            ),
        );
    }
    if let Ok(gates) = ledger.session_events(session, Some(TransitionKind::Gate), 1)
        && let Some(gate) = gates.first()
    {
        line(
            "  last gate",
            &format!(
                "{} — {}",
                gate.subject.as_deref().unwrap_or("?"),
                format_age(gate.at)
            ),
        );
    }
    if let Ok(Some((kind, row))) = ledger.latest_override(session) {
        line(
            "  guardrail",
            &format!(
                "{kind} (recorded by {}, {})",
                row.origin,
                format_age(row.at)
            ),
        );
    }
    out
}

/// `--guardrail`'s value, or a refusal naming the three spellings.
fn parse_guardrail_override(value: &str) -> anyhow::Result<GuardrailOverride> {
    GuardrailOverride::from_stored(value.trim()).ok_or_else(|| {
        anyhow::anyhow!(
            "`{value}` is not a guardrail override; use one of {}",
            GuardrailOverride::spellings()
        )
    })
}

/// `glasshouse assumptions [--session <id>] [--limit N]` — Phase 21K lines
/// 1048, 1049, 1051.
///
/// Every line is read from the ledger and rendered through
/// `guardrails::quote`, so what an agent stated reaches the terminal with
/// nothing in it that could act on the terminal. The session, when named,
/// is resolved through the session store's own prefix rule and that handle
/// is dropped before the ledger's is opened.
fn assumptions_report(
    runtime: &Runtime,
    session: Option<&str>,
    limit: usize,
) -> anyhow::Result<String> {
    use glasshouse::guardrails::{AssumptionState, TransitionKind, quote};
    use std::fmt::Write as _;

    let session = match session {
        Some(named) => {
            let sessions = ProjectSessions::open(runtime)?;
            let id = sessions.store().resolve_id(named)?;
            Some(id.as_str().to_owned())
        }
        None => None,
    };
    let ledger = AssumptionStore::open(runtime)?;
    let counts = ledger.counts(session.as_deref())?;
    let views = ledger.list(session.as_deref(), limit)?;

    let mut out = String::new();
    match &session {
        Some(id) => writeln!(out, "assumptions stated for session {id}")?,
        None => writeln!(out, "assumptions stated in this project")?,
    }
    let summary = counts
        .iter()
        .map(|(state, n)| format!("{state} {n}"))
        .collect::<Vec<_>>()
        .join(" · ");
    writeln!(out, "{summary}")?;
    if views.is_empty() {
        writeln!(
            out,
            "\nnone recorded — an agent states one through the control API's \
             record_assumption or the glasshouse_record_assumption tool; nothing is inferred"
        )?;
    }
    for view in &views {
        let record = &view.record;
        writeln!(out)?;
        writeln!(
            out,
            "{}  {:<14} {}/{}  {}{}",
            record.id.short(),
            view.state,
            record.uncertainty,
            record.evidence_source,
            format_age(record.created_at),
            record
                .session_id
                .as_deref()
                .filter(|_| session.is_none())
                .map(|s| format!("  session {}", &s[..s.len().min(8)]))
                .unwrap_or_default()
        )?;
        writeln!(out, "    claim         {}", quote(&record.claim, 280))?;
        writeln!(out, "    evidence      {}", quote(&record.evidence, 200))?;
        writeln!(out, "    affects       {}", quote(&record.affected, 200))?;
        writeln!(
            out,
            "    verify        {}",
            quote(&record.verification, 200)
        )?;
        let latest = &view.latest;
        let mut trail = format!(
            "{} by {} {}",
            latest.state.map_or("-", AssumptionState::as_str),
            latest.origin,
            format_age(latest.at)
        );
        if let Some(response) = latest.response {
            let _ = write!(trail, ", response {response}");
        }
        if let Some(note) = &latest.note {
            let _ = write!(trail, " — {}", quote(note, 200));
        }
        if view.transitions > 1 {
            let _ = write!(trail, " ({} transitions)", view.transitions);
        }
        writeln!(out, "    latest        {trail}")?;
    }

    if let Some(id) = &session {
        let events = ledger.session_events(id, None, 20)?;
        if !events.is_empty() {
            writeln!(out)?;
            writeln!(out, "gates, overrides and budgets for this session")?;
            for event in &events {
                let what = match event.kind {
                    TransitionKind::Gate => "gate",
                    TransitionKind::Override => "override",
                    TransitionKind::BudgetExceeded => "budget exceeded",
                    TransitionKind::Transition => "transition",
                };
                writeln!(
                    out,
                    "  {:<16} {:<32} {}{}",
                    what,
                    event.subject.as_deref().unwrap_or("-"),
                    format_age(event.at),
                    event
                        .note
                        .as_deref()
                        .map(|note| format!("  — {}", quote(note, 120)))
                        .unwrap_or_default()
                )?;
            }
        }
    }
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

/// The compiled-in adapter for a session's own recorded harness slug, or
/// `None` when the record names an integration this build has no adapter for
/// — a session recorded by a differently-built binary.
fn harness_adapter_for(
    harness_slug: &str,
) -> Option<&'static dyn glasshouse::harness::HarnessAdapter> {
    glasshouse::harness::all().find(|adapter| adapter.id().slug() == harness_slug)
}

/// This session's warmth for the restyle warning gate — line 619.
///
/// Deliberately simpler than `warm_session`, the router's own reader of the
/// same fact: that one asks whether a *candidate* session is reachable from a
/// routing decision being made about a different destination, which is why it
/// takes a `DestinationScope`. Here there is no candidate set — the session
/// named on the command line is the only session this question is ever about
/// — so an `Active` session is always the relevant one.
fn restyle_warmth(
    record: &SessionRecord,
    now_unix: i64,
) -> Option<glasshouse::config::pairing::WarmSession> {
    use glasshouse::config::pairing::{WarmSession, WarmSessionState};

    let state = match record.disposition() {
        SessionDisposition::Active => WarmSessionState::Live,
        SessionDisposition::Resumable => WarmSessionState::Resumable,
        SessionDisposition::Closed | SessionDisposition::Failed => return None,
    };
    Some(WarmSession {
        state,
        idle_seconds: (now_unix - record.last_activity_at).max(0),
    })
}

/// Refuse instruction text that could smuggle more than the one line it
/// promises, rather than trying to escape it.
///
/// The same conservatism `integrations::cmux`'s `PayloadHasBackslash` uses,
/// for the same reason: [`SessionApi::send_text`](glasshouse::session) appends
/// exactly one `\r` and writes the rest of the string as data, so a `\r` (or
/// any other control byte) already inside the text would submit as more than
/// one line once it reaches the pty. There is no correct way to transform
/// that away, so it is refused instead.
fn refuse_control_bytes(text: &str) -> anyhow::Result<()> {
    if text.chars().any(char::is_control) {
        anyhow::bail!(
            "this instruction contains a control byte (a line break or similar); refusing to \
             deliver it rather than trying to escape it, so it cannot submit as more than the \
             one line this override promises"
        );
    }
    Ok(())
}

/// Deliver one lightweight communication instruction into a running session,
/// for this turn only — capability map line 620.
///
/// Refuses by name for a harness whose communication-style declaration is
/// [`Declared::Unverified`](glasshouse::harness::Declared): typing an
/// unframed instruction at a harness nobody has read a mechanism for is a
/// guess, not an override, and 618's correction is explicit that inventing a
/// declaration here would invert the policy rather than merely degrade it.
/// Delivery itself goes through `crate::api::send_message` — the same input
/// path `glasshouse api send` and a person's own typing use — so it is never
/// a second copy of the write path, and it inherits that path's project scope
/// and liveness checks rather than repeating them.
fn tell_session(runtime: &Runtime, session: &str, instruction: &str) -> anyhow::Result<()> {
    refuse_control_bytes(instruction)?;

    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    let id = store.resolve_id(session)?;
    let record = store
        .get(&id)?
        .ok_or_else(|| anyhow::anyhow!("session `{id}` is not in this project"))?;

    let adapter = harness_adapter_for(&record.harness).ok_or_else(|| {
        anyhow::anyhow!(
            "no adapter registered for harness `{}` recorded on session {}",
            record.harness,
            short_id(&id)
        )
    })?;

    if adapter.describe().communication_style.value().is_none() {
        anyhow::bail!(
            "{} declares no communication-style mechanism Glasshouse has read, so there is no \
             verified way to frame a one-turn instruction for it; refusing rather than typing \
             unframed text into session {}",
            adapter.id().display_name(),
            short_id(&id)
        );
    }

    let framed = glasshouse::harness::response::one_turn_override(instruction);
    api::send_message(runtime, id.as_str(), &framed)
}

/// Warn before, then carry out, a profile change on a running session —
/// capability map line 619.
///
/// The warning fires only when the adapter's own
/// [`StyleChange`](glasshouse::harness::StyleChange) declaration says the
/// harness needs a new native session for this change **and** the session is
/// genuinely warm ([`restyle_warmth`]); refusing it (no `--accept-loss`)
/// returns before anything is read from the harness's own declarations beyond
/// what decided the warning, so the session, its settings and its stored
/// response profile are left exactly as they were. A cold session, or one
/// whose harness can change style in place, proceeds straight to delivery.
///
/// Delivery reuses [`tell_session`]'s own mechanism — the resolved preset's
/// instruction text, framed the same way, sent through the same input path —
/// rather than writing a second copy of it: 619 asks for a warning in front
/// of a change, not a second way of making one.
fn restyle_session(
    runtime: &Runtime,
    session: &str,
    profile: &str,
    accept_loss: bool,
) -> anyhow::Result<()> {
    let preset = glasshouse::profile::response::presets()
        .iter()
        .find(|preset| preset.name == profile)
        .ok_or_else(|| {
            let names: Vec<&str> = glasshouse::profile::response::presets()
                .iter()
                .map(|preset| preset.name)
                .collect();
            anyhow::anyhow!(
                "`{profile}` is not a response preset Glasshouse knows; the presets are: {}",
                names.join(", ")
            )
        })?;

    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    let id = store.resolve_id(session)?;
    let record = store
        .get(&id)?
        .ok_or_else(|| anyhow::anyhow!("session `{id}` is not in this project"))?;

    let adapter = harness_adapter_for(&record.harness).ok_or_else(|| {
        anyhow::anyhow!(
            "no adapter registered for harness `{}` recorded on session {}",
            record.harness,
            short_id(&id)
        )
    })?;

    let described = adapter.describe();
    let Some(style) = described.communication_style.value() else {
        anyhow::bail!(
            "{} declares no communication-style mechanism Glasshouse has read, so there is no \
             verified way to restyle session {} without guessing; refusing rather than typing an \
             unframed instruction into it",
            adapter.id().display_name(),
            short_id(&id)
        );
    };
    let change = style.change;

    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    let warmth = restyle_warmth(&record, now_unix);

    if change == glasshouse::harness::StyleChange::NewSession
        && let Some(warm) = warmth
        && !accept_loss
    {
        anyhow::bail!(
            "restyling session {} to `{profile}` needs a new {} session — its communication-\
             style mechanism cannot change in place, and this session is warm ({}, idle {}s). \
             Refusing leaves the session, its settings and its stored response profile \
             untouched; re-run with --accept-loss to give it up and restyle anyway.",
            short_id(&id),
            adapter.id().display_name(),
            warm.state,
            warm.idle_seconds
        );
    }

    let framed = glasshouse::harness::response::one_turn_override(&preset.profile.instruction());
    api::send_message(runtime, id.as_str(), &framed)
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

    /// Capability map line 1851's structural half: every gateway this binary
    /// starts is also told where to report what the failure-domain term did
    /// to a failover it takes.
    ///
    /// The same standing and the same limits as the two scans above — it
    /// proves *presence*, not behaviour, and line 1851 does not close on it.
    /// `evaluation_producers::a_failover_the_domain_term_prevented_is_\
    /// counted_and_one_it_did_not_is_not` is what proves the behaviour, and it
    /// enters at `start_if_required_with_degrade_sink`, the very door both
    /// sites below call. What it cannot see is these two arguments: a launch
    /// that fails over needs a gateway-backed profile, a provider that really
    /// answers badly, and a harness process that really talks to it, and
    /// nothing in this crate builds all three. So this is the §35 guard for
    /// the one link that test cannot reach — an edit dropping either site
    /// back to `None` would otherwise leave every suite green with line
    /// 1851's producer unreachable from the shipped binary.
    #[test]
    fn every_gateway_the_binary_starts_is_told_where_to_report_a_prevented_failover() {
        let code = production_code(include_str!("main.rs"));

        let starts = code.matches("start_if_required_with_degrade_sink(").count();
        let sinks = code
            .matches("Some(failover_prevention_sink(runtime)),")
            .count();
        assert_eq!(
            starts, 2,
            "this binary should start a gateway at exactly two sites (launch and \
             resume); if that changed, this test needs to change with it"
        );
        assert_eq!(
            sinks, starts,
            "a gateway is started somewhere without a failover-prevention sink, so what \
             failure-domain evidence did to its failovers would be counted nowhere — which \
             is the state map line 1851 was left in"
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
    /// Capability map line 1852's write, read back the way `glasshouse
    /// route` reads it: by purpose, and by nothing that would make it an
    /// exchange.
    #[test]
    fn a_correlation_steered_failover_is_recorded_by_purpose_and_never_as_an_exchange() {
        use glasshouse::routing::evidence::{
            CORRELATION_PURPOSE, EvidenceLedger, ObservationQuery, RouteIdentity,
        };

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            tmp.path().join("data").to_str().unwrap(),
            "--config-dir",
            tmp.path().join("config").to_str().unwrap(),
        ])
        .unwrap();
        let runtime = glasshouse::bootstrap(&cli, &root).unwrap();

        record_correlation_steer(
            &runtime,
            &RouteIdentity::new("looked-independent", "the-model"),
            1_800_000_000,
        );

        let ledger = EvidenceLedger::open(&runtime).unwrap();
        let rows = ledger
            .recent(
                ObservationQuery {
                    provider: "looked-independent",
                    model: "the-model",
                    route: None,
                    harness: None,
                },
                10,
            )
            .unwrap();
        assert_eq!(rows.len(), 1, "one steered failover, one row: {rows:?}");
        assert_eq!(rows[0].purpose.as_deref(), Some(CORRELATION_PURPOSE));
        assert_eq!(
            (rows[0].outcome, rows[0].failure_class),
            (None, None),
            "the row is about a decision, not an exchange, and must never count as one"
        );
        let steered: usize = ledger
            .consumption_by_purpose(1_800_000_000, 60)
            .unwrap()
            .iter()
            .filter(|group| group.purpose.as_deref() == Some(CORRELATION_PURPOSE))
            .map(|group| group.sample_count)
            .sum();
        assert_eq!(steered, 1, "`glasshouse route` counts it back by purpose");
        assert!(
            ledger
                .failure_classes_by_provider(1_800_000_000, 60)
                .unwrap()
                .get("looked-independent")
                .is_none_or(|counts| counts.is_empty()),
            "a steered failover is not an exchange on the route it steered off"
        );
    }

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

    /// `GH-LAUNCH-BRIEFING`'s test (e): the delivery ladder's third rung —
    /// no adapter additive mechanism and no session runtime to fall back to
    /// (this launch is not headless). Every adapter this build ships except
    /// Claude Code declares no additive mechanism (`response.rs`'s own
    /// `an_adapter_that_declares_nothing_says_so_rather_than_inventing_a_mechanism`),
    /// so Codex stands in for "the harness whose adapter declares none".
    ///
    /// A unit test on `brief_launch_session` itself rather than a
    /// shipped-binary test, per the packet's own escape hatch: reaching rung
    /// three through the real binary needs an *embedded* (non-headless)
    /// launch, and `session::attach` refuses to run at all without a real
    /// terminal on both ends — which a `cargo test` process never has. That
    /// makes the harness never spawn, so there is no argv to read back and
    /// nothing to assert `"not briefed"` against other than a vacuous
    /// absence (§17). This test asserts the ladder's own decision directly
    /// instead.
    #[test]
    fn rung_three_fires_with_no_additive_mechanism_and_no_session_runtime() {
        use glasshouse::integrations::IntegrationId;
        use glasshouse::memory::{MemoryAuthority, MemoryKind, NewMemory, ProjectMemory};

        let codex = glasshouse::harness::adapter_for(IntegrationId::Codex).unwrap();
        assert!(
            codex.additive_response_injection().is_none(),
            "this test is vacuous unless Codex declares no additive mechanism"
        );

        let tmp = tempfile::tempdir().unwrap();
        let runtime = fixture_with_enabled_claude_code(tmp.path());
        let project = ProjectMemory::open(&runtime).unwrap();
        project
            .store()
            .record(
                NewMemory::new(MemoryKind::Constraint, "Some current binding memory.")
                    .with_authority(Some(MemoryAuthority::Constraint)),
            )
            .unwrap();
        drop(project);

        let mut response_application = glasshouse::harness::response::Application::none(
            "no response profile is under test here",
        );
        let session = SessionId::new("rung-three-test-session");
        let briefing = brief_launch_session(
            &runtime,
            &session,
            codex,
            false, // headless: false, so there is no session runtime to fall back to
            false, // no_memory
            true,  // inject_at_launch
            None,
            &mut response_application,
        );

        match briefing {
            LaunchBriefing::NotBriefed(reason) => {
                assert!(
                    reason.contains("no mechanism"),
                    "the reason must name why: {reason}"
                );
            }
            other => panic!(
                "expected rung three (`NotBriefed`) with no additive mechanism and headless \
                 false; got {other:?}"
            ),
        }
        assert!(
            response_application.args().is_empty(),
            "rung three must never touch the response application's arguments"
        );
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
            false,
            ExternalPresentation::Embedded,
            &[],
            None,
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
            false,
            ExternalPresentation::Embedded,
            &[],
            None,
        )
        .unwrap();
        assert_eq!(status, ExitCode::FAILURE);

        let sessions = glasshouse::session::ProjectSessions::open(&runtime).unwrap();
        assert!(sessions.store().list().unwrap().is_empty());
    }

    // --- map line 372 clause 2: automatic routing selects the profile too -

    /// The most recent routing decision `launch_session` recorded, as the
    /// `fresh:<harness>:<profile>` id [`record_routing_decision`] wrote —
    /// read back through the same evaluation ledger `glasshouse route`'s own
    /// counters read, rather than by capturing stderr, which this test
    /// module has no idiom for.
    fn last_routed_destination(runtime: &Runtime) -> String {
        use glasshouse::evaluation::{EvaluationKind, EvaluationObservations};

        let ledger = EvaluationObservations::open(runtime).unwrap();
        let rows = ledger
            .recent_of_kind(EvaluationKind::RoutingContinuationDecided, 1)
            .unwrap();
        assert_eq!(
            rows.len(),
            1,
            "launch_session must record exactly one routing decision when it routes at all"
        );
        rows[0]
            .detail
            .clone()
            .expect("a routing-continuation row always carries the destination id")
    }

    /// Line 372 clause 1's own mechanism (`enabled = false`), reused here so
    /// the profile that alphabetically leads every name in this fixture is
    /// never a legal candidate — which is what makes a winner other than it
    /// prove the ranking ran rather than a first-configured-name fallback.
    fn disabled_profile(
        harness: glasshouse::integrations::IntegrationId,
    ) -> glasshouse::config::ProfileConfig {
        let mut profile = glasshouse::config::ProfileConfig::new(harness);
        profile.set_enabled(false);
        profile
    }

    /// An enabled profile whose approval is `Bypass` and unacknowledged —
    /// `an_unacknowledged_bypass_also_starts_no_process_and_records_no_session`'s
    /// own construction, reused because it fails *after* routing decides and
    /// *before* any process starts, on a `BackendResource::Native` backend
    /// (`ProfileConfig::default`'s own backend) so its router score is the
    /// same shape as the implied Native profile's — nothing about the
    /// failure mode should tilt the ranking toward or away from it.
    fn unacknowledged_bypass_profile(
        harness: glasshouse::integrations::IntegrationId,
    ) -> glasshouse::config::ProfileConfig {
        let mut profile = glasshouse::config::ProfileConfig::new(harness);
        profile.set_approval(glasshouse::config::ProfileApproval::Bypass);
        profile
    }

    /// Required behaviour 1: automatic on, no pinned profile — the ranked
    /// winner among the *enabled* profiles is what launches, and it is not
    /// simply the first configured name.
    ///
    /// `aaa-disabled` sorts first among every configured name (including
    /// `native`) and is disabled, so it can never be offered at all —
    /// `bbb-yolo` is the only other non-native candidate, and it and
    /// `native` are the same backend class, so nothing but their id order
    /// separates them for the router in this bare fixture. A mutation that
    /// swaps the ranked winner for `effective.profile_names()`'s first
    /// element would answer `aaa-disabled`, which is not even an enabled
    /// candidate — so this fails loudly rather than by coincidence.
    #[test]
    fn automatic_routing_selects_among_enabled_profiles_when_none_is_pinned() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = fixture_with_enabled_claude_code(tmp.path());
        let harness = glasshouse::integrations::IntegrationId::ClaudeCode;

        let mut user = UserConfig::load(runtime.paths()).unwrap();
        user.profiles_mut()
            .set("aaa-disabled", disabled_profile(harness));
        user.profiles_mut()
            .set("bbb-yolo", unacknowledged_bypass_profile(harness));
        user.save(runtime.paths()).unwrap();

        let status = launch_session(
            &runtime,
            Some("claude-code"),
            LaunchDestination::default(),
            &ResponseRequest::default(),
            false,
            false,
            ExternalPresentation::Embedded,
            &[],
            None,
        )
        .unwrap();
        // `bbb-yolo` wins the ranking (see above) and then fails the same
        // unacknowledged-bypass check the pinned case does — after routing
        // decided, before any process starts.
        assert_eq!(status, ExitCode::FAILURE);
        let sessions = glasshouse::session::ProjectSessions::open(&runtime).unwrap();
        assert!(sessions.store().list().unwrap().is_empty());

        assert_eq!(
            last_routed_destination(&runtime),
            "fresh:claude-code:bbb-yolo",
            "the ranking's winner must be an enabled profile the router actually ranked, not \
             `aaa-disabled` — the first configured name, and disabled"
        );
    }

    /// Required behaviour 2: automatic on, a profile pinned with `--profile`
    /// — the pin wins exactly as it always has, even when the ranking would
    /// have preferred a different enabled profile.
    #[test]
    fn a_pinned_profile_still_beats_the_automatic_ranking() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = fixture_with_enabled_claude_code(tmp.path());
        let harness = glasshouse::integrations::IntegrationId::ClaudeCode;

        let mut user = UserConfig::load(runtime.paths()).unwrap();
        // Unpinned, the ranking would prefer `bbb-yolo` (it sorts first and
        // ties with `ccc-yolo` on every axis in this bare fixture) — pinning
        // `ccc-yolo` instead must still be what launches.
        user.profiles_mut()
            .set("bbb-yolo", unacknowledged_bypass_profile(harness));
        user.profiles_mut()
            .set("ccc-yolo", unacknowledged_bypass_profile(harness));
        user.save(runtime.paths()).unwrap();

        let status = launch_session(
            &runtime,
            Some("claude-code"),
            LaunchDestination {
                profile: Some("ccc-yolo"),
                ..LaunchDestination::default()
            },
            &ResponseRequest::default(),
            false,
            false,
            ExternalPresentation::Embedded,
            &[],
            None,
        )
        .unwrap();
        assert_eq!(status, ExitCode::FAILURE);

        assert_eq!(
            last_routed_destination(&runtime),
            "fresh:claude-code:ccc-yolo",
            "an explicit `--profile` pin must win over the ranking's own preference"
        );
    }

    /// Required behaviour 3: automatic routing off leaves the launch path
    /// byte-identical to before this box existed — no routing decision is
    /// even taken, on a launch that would otherwise reach one.
    #[test]
    fn automatic_routing_off_never_reaches_the_profile_ranking() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = fixture_with_enabled_claude_code(tmp.path());
        let harness = glasshouse::integrations::IntegrationId::ClaudeCode;

        let mut user = UserConfig::load(runtime.paths()).unwrap();
        user.routing_mut().set_automatic(Some(false));
        user.profiles_mut()
            .set("bbb-yolo", unacknowledged_bypass_profile(harness));
        user.save(runtime.paths()).unwrap();

        let status = launch_session(
            &runtime,
            Some("claude-code"),
            LaunchDestination {
                profile: Some("bbb-yolo"),
                ..LaunchDestination::default()
            },
            &ResponseRequest::default(),
            false,
            false,
            ExternalPresentation::Embedded,
            &[],
            None,
        )
        .unwrap();
        // The same refusal as the pinned-bypass regression above — routing
        // being off changes nothing about how a pinned profile resolves.
        assert_eq!(status, ExitCode::FAILURE);

        let ledger = glasshouse::evaluation::EvaluationObservations::open(&runtime).unwrap();
        let rows = ledger
            .recent_of_kind(
                glasshouse::evaluation::EvaluationKind::RoutingContinuationDecided,
                10,
            )
            .unwrap();
        assert!(
            rows.is_empty(),
            "with automatic routing off, `routing_destinations` and `choose` must not run at \
             all, so no routing decision is ever recorded: {rows:?}"
        );
    }

    /// Required behaviour 4, and clause 1's own filter asserted through this
    /// new path rather than rebuilt: a disabled profile is never offered to
    /// automatic profile selection, at the producer `launch_session` now
    /// calls under [`DestinationScope::LaunchableAcrossProfiles`].
    #[test]
    fn automatic_profile_selection_never_offers_a_disabled_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = fixture_with_enabled_claude_code(tmp.path());
        let harness = glasshouse::integrations::IntegrationId::ClaudeCode;

        let mut user = UserConfig::load(runtime.paths()).unwrap();
        user.profiles_mut()
            .set("aaa-disabled", disabled_profile(harness));
        user.profiles_mut()
            .set("bbb-enabled", unacknowledged_bypass_profile(harness));
        user.save(runtime.paths()).unwrap();

        let project = config::load_project_config(runtime.project()).unwrap();
        let effective = EffectiveConfig::new(&user, project.as_ref());
        let destinations = routing_destinations(
            &runtime,
            &effective,
            harness,
            DestinationScope::LaunchableAcrossProfiles,
            None,
        )
        .unwrap();

        assert!(
            destinations
                .iter()
                .all(|destination| destination.launch_profile() != "aaa-disabled"),
            "a disabled profile must never reach automatic profile selection's candidate set: \
             {:?}",
            destinations
                .iter()
                .map(|d| d.launch_profile())
                .collect::<Vec<_>>()
        );
        assert!(
            destinations
                .iter()
                .any(|destination| destination.launch_profile() == "bbb-enabled"),
            "the enabled profile must still be offered"
        );
        assert!(
            destinations
                .iter()
                .any(|destination| destination.launch_profile()
                    == glasshouse::profile::NATIVE_PROFILE_NAME),
            "the implied Native profile is always offered"
        );
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

    /// An outcome that recorded nothing, which every case below varies one
    /// field of.
    ///
    /// A struct literal rather than a constructor because
    /// `ExtractionOutcome::empty` is private to its own module and needs a
    /// `SessionChunk` this decision has nothing to do with. One literal, so a
    /// field added to the outcome breaks one place.
    fn recorded_nothing() -> glasshouse::memory::ExtractionOutcome {
        glasshouse::memory::ExtractionOutcome {
            trigger: glasshouse::memory::ExtractionTrigger::BeforeCompaction,
            model: "a test model".to_owned(),
            session_id: "s".to_owned(),
            commit: None,
            recorded: Vec::new(),
            lowered: Vec::new(),
            speculative: 0,
            duplicates: 0,
            rejected: Vec::new(),
            activity_dropped: 0,
            activity_truncated: 0,
            redactions: 0,
            failure: None,
            call: None,
        }
    }

    /// A model that could not be asked is a memory that should exist and does
    /// not, and [`lost_extraction_notice`] says so naming both the trigger and
    /// the reason.
    ///
    /// The reason is [`glasshouse::memory::ModelError`]'s own `Display`, which
    /// is a fixed phrase by construction — this line reaches a person's
    /// terminal, and a provider error body can echo the prompt that was sent.
    #[test]
    fn a_failed_extraction_is_reported_with_its_trigger_and_its_reason() {
        let mut outcome = recorded_nothing();
        outcome.failure = Some(glasshouse::memory::extract::ExtractionFailure::Model(
            glasshouse::memory::ModelError::Refused,
        ));

        let notice = lost_extraction_notice("before_compaction", Some(&outcome))
            .expect("a failed extraction is a lost memory");
        assert!(
            notice.contains("before_compaction"),
            "the notice must name which boundary lost its memory: {notice}"
        );
        assert!(
            notice.contains("the extraction model declined the request"),
            "the notice must say why: {notice}"
        );
    }

    /// A session with no activity has no memory to have lost, and a warning
    /// here would fire on every compaction of a session that had not done
    /// anything yet.
    ///
    /// This is the assertion that keeps the notice worth reading: a warning
    /// that cries wolf is indistinguishable from one that matters, and the way
    /// this one stops being read is by appearing when nothing is wrong.
    #[test]
    fn a_compaction_with_no_session_activity_is_not_reported_as_a_loss() {
        let mut outcome = recorded_nothing();
        outcome.failure = Some(glasshouse::memory::extract::ExtractionFailure::NothingToExtract);

        assert_eq!(
            lost_extraction_notice("before_compaction", Some(&outcome)),
            None,
            "nothing was extracted because there was nothing to extract; that is not a loss"
        );
    }

    /// [`run_extraction`] answers [`None`] for its preparation failures and
    /// for [`EXTRACTION_BOUND`] expiring, and all of those are losses — a
    /// boundary went by and nothing was written.
    #[test]
    fn an_extraction_that_never_produced_an_outcome_is_reported_as_a_loss() {
        let notice = lost_extraction_notice("before_compaction", None)
            .expect("no outcome at all is a lost memory");
        assert!(
            notice.contains("before_compaction") && notice.contains("recorded nothing"),
            "{notice}"
        );
        assert!(
            notice.contains(&EXTRACTION_BOUND.as_secs().to_string()),
            "the notice must name the bound it may have been cut off at: {notice}"
        );
    }

    /// A run that stored something and rejected something else lost nothing a
    /// person needs to act on, and neither did one that found only
    /// duplicates.
    ///
    /// The discriminating half of the case below it: rejections are reported
    /// only when *nothing* survived them.
    #[test]
    fn a_run_that_stored_a_memory_is_silent_even_when_it_also_rejected_one() {
        let mut outcome = recorded_nothing();
        outcome
            .recorded
            .push(glasshouse::memory::MemoryId::new("m1"));
        outcome
            .rejected
            .push(glasshouse::memory::extract::Rejection::Store(
                "a rejected one".to_owned(),
            ));
        assert_eq!(
            lost_extraction_notice("task_completed", Some(&outcome)),
            None
        );

        let mut duplicates_only = recorded_nothing();
        duplicates_only.duplicates = 2;
        assert_eq!(
            lost_extraction_notice("task_completed", Some(&duplicates_only)),
            None,
            "a duplicate is the duplicate check working, not a memory lost"
        );
    }

    /// And the case that is a loss: the model answered, and nothing it
    /// proposed survived the contract.
    #[test]
    fn a_run_whose_every_memory_was_rejected_is_reported_as_a_loss() {
        let mut outcome = recorded_nothing();
        outcome
            .rejected
            .push(glasshouse::memory::extract::Rejection::Store(
                "the store refused it".to_owned(),
            ));

        let notice = lost_extraction_notice("before_compaction", Some(&outcome))
            .expect("a run that stored none of what it proposed lost every one of them");
        assert!(notice.contains("rejected"), "{notice}");
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
            PRESENTED_WIDTH,
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
            PRESENTED_WIDTH,
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
    // directly, through `heuristic_answer(..).requirements()` — the producer
    // `classify_for_routing` uses on every path that asks no model, which is
    // what `route_report` reaches at its `RouterInputs` construction site
    // with no routing model configured. A mutation that hardcodes
    // `needs_tool_calls: false` into `RouterAnswer::requirements` fails this
    // test, because the `KnownAbsent` destination would stop being rejected
    // for a task that plainly asks for shell execution.
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
        let tool_use_requirements = heuristic_answer(
            "run cargo test and fix whatever fails",
            glasshouse::routing::request::HeuristicReason::NoRoutingModel,
        )
        .requirements();
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
            glasshouse::routing::HardConstraint::ToolSemantics { evidence: None }
        );

        // The absent-`--task` behaviour: `classify_for_routing` answers
        // `None` and the caller falls back to the default, so
        // `needs_tool_calls` stays `false` and the same `known-absent`
        // destination is no longer rejected.
        let no_task_requirements = glasshouse::routing::session::TaskRequirements::default();
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

    // -----------------------------------------------------------------------
    // Lines 1357/1358: a `[routing.score_weights]` override must reach the
    // router `glasshouse route`/`launch` actually build — `session_router()`
    // — not merely `provider_health`/`quota_pressure` called by hand. This
    // is the one place that can prove it, because `session_router` is
    // private to this binary. Written by GH-AUDIT-WAVE79 as a tripwire: it
    // was RED against the tree that had ticked both lines, because the
    // constructor never called `with_score_weights`. The mutation that keeps
    // it honest is deleting that one line from `session_router`.
    // -----------------------------------------------------------------------
    #[test]
    fn a_configured_score_weight_reaches_the_real_session_router() {
        use glasshouse::config::{EffectiveConfig, UserConfig};
        use glasshouse::integrations::IntegrationId;
        use glasshouse::routing::free::{FreePool, FreeResource};
        use glasshouse::routing::session::{
            Destination, RouterInputs, RoutingMoment, RoutingOverride, ScoreWeights,
            TaskRequirements,
        };
        use glasshouse::routing::{AssignedModel, Backend, Cost, CredentialId, ToolSemantics};
        use glasshouse::secret::SecretRef;
        use std::time::Instant;

        let fixture = CliFixture::new();

        let mut user = UserConfig::default();
        let overridden = ScoreWeights {
            health_failure_penalty: -50.0,
            ..ScoreWeights::default()
        };
        user.routing_mut()
            .set_score_weights(Some(overridden.into()));
        let effective_overridden = EffectiveConfig::new(&user, None);
        assert_eq!(
            effective_overridden.score_weights().value,
            overridden,
            "premise: the config layer resolves the override correctly"
        );
        let default_user = UserConfig::default();
        let effective_default = EffectiveConfig::new(&default_user, None);

        fn backend() -> Backend {
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
                ToolSemantics::Verified,
            )
        }
        let dest = Destination::fresh(
            "dest-1",
            IntegrationId::ClaudeCode,
            "default",
            backend(),
            None,
        );
        let resource = FreeResource::new(
            dest.backend().credential().clone(),
            dest.backend().model().label(),
        );
        let mut health = FreePool::new();
        // One observed failure: `provider_health`'s additive term is
        // non-zero here, so a weight change is visible in the total.
        health.adopt_observed(&resource, 1, None, None, false);

        let overrides = glasshouse::harness::pairing::PairingOverrides::from_parts(
            "no configuration",
            std::collections::BTreeMap::new(),
            std::collections::BTreeMap::new(),
        );
        let inputs = RouterInputs {
            overrides: &overrides,
            health: &health,
            now: Instant::now(),
            requirements: TaskRequirements::default(),
        };

        let router_default = session_router(
            &fixture.runtime,
            &effective_default,
            RoutingOverride::none(),
        );
        let routed_default = router_default
            .choose(
                RoutingMoment::SessionStart,
                None,
                std::slice::from_ref(&dest),
                &inputs,
            )
            .expect("one destination offered");

        let router_overridden = session_router(
            &fixture.runtime,
            &effective_overridden,
            RoutingOverride::none(),
        );
        let routed_overridden = router_overridden
            .choose(RoutingMoment::SessionStart, None, &[dest], &inputs)
            .expect("one destination offered");

        assert_ne!(
            routed_default.explanation().total(),
            routed_overridden.explanation().total(),
            "lines 1357/1358: `session_router()` — the constructor `glasshouse route`/`launch` \
             actually call — produced the identical total ({}) whether or not \
             `[routing.score_weights]` was configured, so the configured weights are not \
             reaching the router that makes real decisions.",
            routed_default.explanation().total()
        );
    }

    // Line 1546's glue in THIS binary: `observed_health_of` must hand the
    // persisted `cooldown_cause` to `FreePool::adopt_observed` unchanged.
    // Both halves it connects were already mutation-proven in their own
    // modules; this exact passthrough was not, and GH-AUDIT-WAVE79's
    // mutation of it (`reading.cooldown_cause` -> `None`) SURVIVED the
    // binary's own suite. Now it does not.
    #[test]
    fn observed_health_of_hands_the_persisted_cooldown_cause_to_the_pool() {
        use glasshouse::provider::telemetry::{GatewayHealthCache, GatewayHealthReading};
        use glasshouse::routing::free::{CooldownCause, FreeResource};
        use glasshouse::routing::{AssignedModel, CredentialId};
        use glasshouse::secret::SecretRef;

        let fixture = CliFixture::new();
        let credential = CredentialId::new(
            "anthropic",
            SecretRef::Environment {
                var: "ANTHROPIC_API_KEY".to_owned(),
            },
        );
        let resource = FreeResource::new(
            credential.clone(),
            AssignedModel::named("claude-opus-4").label(),
        );
        let now_unix = glasshouse::provider::cache::now_unix_seconds();
        let reading = GatewayHealthReading {
            credential_label: credential.label().to_owned(),
            model: resource.model().to_owned(),
            consecutive_failures: 0,
            cooling_down_until_unix: Some(now_unix + 600),
            cooldown_cause: Some(CooldownCause::Declared),
            credential_rejected: false,
        };
        GatewayHealthCache::new(fixture.runtime.paths()).store("anthropic", &[reading], now_unix);

        let observed = observed_health_of(&fixture.runtime, [resource.clone()]);
        assert_eq!(
            observed.pool.health(&resource).cooldown_cause(),
            Some(CooldownCause::Declared),
            "line 1546: the provider-declared cause persisted by the gateway must reach the \
             router's pool through this binary's own adoption loop, not be dropped on the way"
        );
    }

    // -----------------------------------------------------------------------
    // GH-INPUT-SIZE-PRODUCER — map lines 1298, 1299 and 1304: the producer
    // itself. `estimated_project_memory_tokens`, `session_checkpoint_tokens`,
    // `latest_checkpoint_tokens` and `routing_destinations` all live only in
    // this binary, so a `tests/routing_pricing.rs` or
    // `tests/routing_evidence.rs` integration test cannot reach them at
    // all — this is the only place their wiring against a real
    // `ProjectMemory`/`ProjectCheckpoints` store can be proven.
    // -----------------------------------------------------------------------

    #[test]
    fn estimated_project_memory_tokens_measures_the_real_briefing_and_changes_with_it() {
        use glasshouse::memory::{MemoryKind, NewMemory, ProjectMemory};

        let fixture = CliFixture::new();
        assert_eq!(
            estimated_project_memory_tokens(&fixture.runtime, "kestrel deploy"),
            None,
            "a project with no memories has nothing to inject, which is absent, not zero"
        );

        ProjectMemory::open(&fixture.runtime)
            .unwrap()
            .store()
            .record(NewMemory::new(
                MemoryKind::Finding,
                "The kestrel deploy runs on one instance.",
            ))
            .unwrap();
        let short = estimated_project_memory_tokens(&fixture.runtime, "kestrel deploy")
            .expect("a matching memory must be measured, not left absent");
        assert!(short > 0);

        ProjectMemory::open(&fixture.runtime)
            .unwrap()
            .store()
            .record(NewMemory::new(
                MemoryKind::Finding,
                "The kestrel deploy runs on one instance, in one region, behind one load \
                 balancer, with one on-call rotation watching it around the clock every day.",
            ))
            .unwrap();
        let longer = estimated_project_memory_tokens(&fixture.runtime, "kestrel deploy")
            .expect("a matching memory must still be measured");
        assert!(
            longer > short,
            "the estimate must count the briefing's real rendered size, not a constant: \
             {short} then {longer}"
        );

        assert_eq!(
            estimated_project_memory_tokens(&fixture.runtime, "an unrelated wombat migration"),
            None,
            "a task nothing matches has nothing to inject, which the estimate reads as absent"
        );
    }

    #[test]
    fn session_checkpoint_tokens_measures_the_real_document_and_stays_within_its_own_session() {
        let fixture = CliFixture::new();
        let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
        let checkpointed = sessions
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();
        let untouched = sessions
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();

        let command = CheckpointCommand::Save {
            objective: "prove the cold-resume estimate measures the real checkpoint".to_owned(),
            state: "wiring session_checkpoint_tokens to checkpoint::store::latest_for".to_owned(),
            session: Some(checkpointed.id.as_str().to_owned()),
            decisions: Vec::new(),
            failed_approaches: Vec::new(),
            files: Vec::new(),
            tests: None,
            next_actions: Vec::new(),
        };
        assert_eq!(
            checkpoint_command(&fixture.runtime, &command).unwrap(),
            ExitCode::SUCCESS
        );

        let checkpoints = ProjectCheckpoints::open(&fixture.runtime).unwrap();
        let checkpointed_tokens = session_checkpoint_tokens(Some(&checkpoints), &checkpointed.id)
            .expect("the checkpointed session's own document must be measured");
        assert!(checkpointed_tokens > 0);
        assert_eq!(
            session_checkpoint_tokens(Some(&checkpoints), &untouched.id),
            None,
            "a session with no checkpoint of its own is unknown, not zero — even though this \
             project has a checkpoint, it belongs to a different session"
        );
    }

    #[test]
    fn latest_checkpoint_tokens_is_absent_until_the_project_has_one_then_measures_it() {
        let fixture = CliFixture::new();
        assert_eq!(latest_checkpoint_tokens(&fixture.runtime), None);

        let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
        let session = sessions
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();
        let command = CheckpointCommand::Save {
            objective: "prove the fresh-session estimate measures the project's own latest \
                        checkpoint"
                .to_owned(),
            state: "wiring latest_checkpoint_tokens to checkpoint::store::latest".to_owned(),
            session: Some(session.id.as_str().to_owned()),
            decisions: Vec::new(),
            failed_approaches: Vec::new(),
            files: Vec::new(),
            tests: None,
            next_actions: Vec::new(),
        };
        assert_eq!(
            checkpoint_command(&fixture.runtime, &command).unwrap(),
            ExitCode::SUCCESS
        );

        let tokens = latest_checkpoint_tokens(&fixture.runtime)
            .expect("a project with a checkpoint must have it measured");
        assert!(tokens > 0);
    }

    #[test]
    fn routing_destinations_attaches_a_fresh_estimate_naming_project_memory_and_checkpoint() {
        use glasshouse::memory::{MemoryKind, NewMemory, ProjectMemory};

        let fixture = CliFixture::new();
        ProjectMemory::open(&fixture.runtime)
            .unwrap()
            .store()
            .record(NewMemory::new(
                MemoryKind::Finding,
                "The kestrel deploy runs on one instance.",
            ))
            .unwrap();
        let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
        let session = sessions
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();
        let command = CheckpointCommand::Save {
            objective: "prove routing_destinations attaches a fresh-session estimate".to_owned(),
            state: "wiring EstimatedInputSize into routing_destinations".to_owned(),
            session: Some(session.id.as_str().to_owned()),
            decisions: Vec::new(),
            failed_approaches: Vec::new(),
            files: Vec::new(),
            tests: None,
            next_actions: Vec::new(),
        };
        assert_eq!(
            checkpoint_command(&fixture.runtime, &command).unwrap(),
            ExitCode::SUCCESS
        );

        let user = UserConfig::load(fixture.runtime.paths()).unwrap();
        let project = config::load_project_config(fixture.runtime.project()).unwrap();
        let effective = EffectiveConfig::new(&user, project.as_ref());
        let destinations = routing_destinations(
            &fixture.runtime,
            &effective,
            glasshouse::integrations::IntegrationId::ClaudeCode,
            DestinationScope::Everything,
            Some("kestrel deploy"),
        )
        .unwrap();

        let fresh = destinations
            .iter()
            .find(|d| d.is_fresh())
            .expect("at least the implied Native profile offers a fresh destination");
        let size = fresh.estimated_input_size();
        assert!(
            size.project_memory_tokens().is_some(),
            "a matching memory must reach the fresh destination's own estimate"
        );
        assert!(
            size.checkpoint_tokens().is_some(),
            "the project's latest checkpoint must reach the fresh destination's own estimate"
        );
    }

    /// Required behavior: a project with no memories, no checkpoint and no
    /// `pricing.toml` must reproduce `04060da` exactly. This is the estimate
    /// half of that: with neither component readable, the fresh destination
    /// this empty project offers must carry no estimate at all.
    #[test]
    fn routing_destinations_reproduces_the_empty_project_regression() {
        let fixture = CliFixture::new();
        let user = UserConfig::load(fixture.runtime.paths()).unwrap();
        let project = config::load_project_config(fixture.runtime.project()).unwrap();
        let effective = EffectiveConfig::new(&user, project.as_ref());

        let destinations = routing_destinations(
            &fixture.runtime,
            &effective,
            glasshouse::integrations::IntegrationId::ClaudeCode,
            DestinationScope::Everything,
            Some("an unrelated task naming nothing this empty project has"),
        )
        .unwrap();

        let fresh = destinations
            .iter()
            .find(|d| d.is_fresh())
            .expect("at least the implied Native profile offers a fresh destination");
        assert_eq!(
            fresh.estimated_input_size().total_tokens(),
            None,
            "a project with no memories and no checkpoint must estimate nothing at all — the \
             regression `04060da` must reproduce exactly"
        );
    }

    /// Map line 1511: `routing_destinations` builds its vector in two
    /// passes — existing sessions first (`main.rs:1093-1195`), fresh
    /// destinations second (`:1198-1308`) — and `SessionRouter::choose`
    /// treats vector order as its own tiebreaker
    /// (`routing/session.rs:4314-4315`). This drives the real generator and
    /// asserts the existing session's position precedes every fresh one;
    /// the census's mutation (reverse the two passes) puts the fresh
    /// destination first instead.
    #[test]
    fn routing_destinations_generates_existing_sessions_before_fresh_ones_1511() {
        let fixture = CliFixture::new();
        let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
        sessions
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();

        let user = UserConfig::load(fixture.runtime.paths()).unwrap();
        let project = config::load_project_config(fixture.runtime.project()).unwrap();
        let effective = EffectiveConfig::new(&user, project.as_ref());
        let destinations = routing_destinations(
            &fixture.runtime,
            &effective,
            glasshouse::integrations::IntegrationId::ClaudeCode,
            DestinationScope::Everything,
            None,
        )
        .unwrap();

        let existing_index = destinations
            .iter()
            .position(|d| !d.is_fresh())
            .expect("the session created above must offer an existing destination");
        let fresh_index = destinations
            .iter()
            .position(|d| d.is_fresh())
            .expect("at least the implied Native profile offers a fresh destination");
        assert!(
            existing_index < fresh_index,
            "existing sessions must be generated before fresh ones: existing at \
             {existing_index}, fresh at {fresh_index}"
        );
    }

    /// Map line 1512: the fresh-destination loop (`main.rs:1237-1308`) builds
    /// one `Destination` per *enabled* profile, and the implied Native
    /// profile is always in that enabled set
    /// (`EffectiveConfig::profile_enabled`, `config/mod.rs:5045-5048`). A
    /// project with no other profiles configured must still offer a fresh
    /// Native destination — the census's mutation (skip Native profiles in
    /// the generation loop) leaves no fresh destination at all here.
    #[test]
    fn routing_destinations_offers_a_fresh_native_destination_from_the_enabled_profile_1512() {
        let fixture = CliFixture::new();
        let user = UserConfig::load(fixture.runtime.paths()).unwrap();
        let project = config::load_project_config(fixture.runtime.project()).unwrap();
        let effective = EffectiveConfig::new(&user, project.as_ref());
        let harness = glasshouse::integrations::IntegrationId::ClaudeCode;

        let destinations = routing_destinations(
            &fixture.runtime,
            &effective,
            harness,
            DestinationScope::Everything,
            None,
        )
        .unwrap();

        let native = destinations
            .iter()
            .find(|d| d.is_fresh() && d.backend().provider() == harness.slug())
            .expect(
                "the enabled implied Native profile must offer a fresh destination for this \
                 harness",
            );
        assert_eq!(
            native.launch_profile(),
            glasshouse::profile::NATIVE_PROFILE_NAME,
            "the fresh Native destination must carry the implied Native profile's own name"
        );
    }

    /// Map line 1515: `disposable_candidates` builds one `DisposableCandidate`
    /// per configured provider's free and metered models
    /// (`main.rs:6901-6960`). The census's mutation (make the function
    /// return empty when free/metered models are configured) leaves a
    /// configured provider with no candidate at all.
    #[test]
    fn disposable_candidates_builds_one_per_configured_free_and_metered_model_1515() {
        const VAR: &str = "GLASSHOUSE_TEST_ONLY_1515_CANDIDATE_KEY";
        // SAFETY: `VAR` is unique to this test and removed again below.
        unsafe {
            std::env::set_var(VAR, "sk-fabricated-test-value-not-a-real-credential");
        }

        let fixture = CliFixture::new();
        let mut user = UserConfig::load(fixture.runtime.paths()).unwrap();
        let mut provider = glasshouse::config::ProviderConfig::new("openai-compatible");
        provider.set_credential_env(vec![VAR.to_owned()]);
        provider.set_free_models(vec!["free-model-1515".to_owned()]);
        provider.set_metered_models(vec!["metered-model-1515".to_owned()]);
        user.providers_mut().set("test-provider-1515", provider);
        user.save(fixture.runtime.paths()).unwrap();

        let user = UserConfig::load(fixture.runtime.paths()).unwrap();
        let effective = EffectiveConfig::new(&user, None);
        let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();
        let telemetry = glasshouse::provider::resources::GatheredTelemetry::new();
        let now_unix = glasshouse::provider::cache::now_unix_seconds();

        let candidates =
            disposable_candidates(&user, None, &effective, &secrets, &telemetry, now_unix);

        unsafe {
            std::env::remove_var(VAR);
        }

        let models: Vec<&str> = candidates
            .iter()
            .filter(|candidate| candidate.provider() == "test-provider-1515")
            .map(|candidate| candidate.model())
            .collect();
        assert!(
            models.contains(&"free-model-1515"),
            "a configured free model must produce a candidate: {models:?}"
        );
        assert!(
            models.contains(&"metered-model-1515"),
            "a configured metered model must produce a candidate: {models:?}"
        );
    }

    /// Map line 1520's generation-time half: a disabled profile never reaches
    /// the offered set at all (`main.rs:1233`,
    /// `.filter(|name| effective.profile_enabled(name).value)`), so no
    /// `Destination` is ever built for it. The census's mutation (bypass
    /// this filter) would let a disabled profile reach generation.
    #[test]
    fn routing_destinations_excludes_a_disabled_profile_before_generation_1520() {
        let fixture = CliFixture::new();
        let harness = glasshouse::integrations::IntegrationId::ClaudeCode;
        let mut user = UserConfig::load(fixture.runtime.paths()).unwrap();
        let mut profile = glasshouse::config::ProfileConfig::new(harness);
        profile.set_enabled(false);
        user.profiles_mut().set("disabled-profile-1520", profile);
        user.save(fixture.runtime.paths()).unwrap();

        let user = UserConfig::load(fixture.runtime.paths()).unwrap();
        let effective = EffectiveConfig::new(&user, None);
        let destinations = routing_destinations(
            &fixture.runtime,
            &effective,
            harness,
            DestinationScope::Everything,
            None,
        )
        .unwrap();

        assert!(
            !destinations
                .iter()
                .any(|d| d.launch_profile() == "disabled-profile-1520"),
            "a profile disabled by user policy must never reach generation: {:?}",
            destinations
                .iter()
                .map(|d| d.launch_profile())
                .collect::<Vec<_>>()
        );
    }

    /// Map line 1945's destination-carrying half: two enabled launch profiles
    /// of one harness, differing in backend and model, each yield their own
    /// fresh destination — `routing_destinations` never collapses them onto
    /// one backend or one model.
    ///
    /// Mutation target: make `EffectiveConfig::launch_profile` return the
    /// native profile for every name (collapse the lookup) → this test fails
    /// because both fresh destinations would carry the native backend.
    #[test]
    fn routing_destinations_1945_carries_each_profiles_own_backend_and_model() {
        let fixture = CliFixture::new();
        let harness = glasshouse::integrations::IntegrationId::ClaudeCode;

        let mut user = UserConfig::default();

        let mut native_profile = glasshouse::config::ProfileConfig::new(harness);
        native_profile.set_model(Some("claude-native-model".to_owned()));
        user.profiles_mut().set("alpha-native", native_profile);

        let mut direct_profile = glasshouse::config::ProfileConfig::new(harness);
        direct_profile.set_backend(glasshouse::config::ProfileBackend::DirectProvider {
            provider: "openrouter".to_owned(),
        });
        direct_profile.set_model(Some("some/other-model".to_owned()));
        user.profiles_mut().set("beta-direct", direct_profile);

        let effective = EffectiveConfig::new(&user, None);

        let destinations = routing_destinations(
            &fixture.runtime,
            &effective,
            harness,
            DestinationScope::Everything,
            None,
        )
        .unwrap();

        let alpha = destinations
            .iter()
            .find(|d| d.is_fresh() && d.launch_profile() == "alpha-native")
            .expect("the native-backed profile must offer its own fresh destination");
        assert_eq!(alpha.backend().provider(), harness.slug());
        assert_eq!(
            alpha.backend().model(),
            &glasshouse::routing::AssignedModel::Named("claude-native-model".to_owned())
        );

        let beta = destinations
            .iter()
            .find(|d| d.is_fresh() && d.launch_profile() == "beta-direct")
            .expect("the direct-provider profile must offer its own fresh destination");
        assert_eq!(beta.backend().provider(), "openrouter");
        assert_eq!(
            beta.backend().model(),
            &glasshouse::routing::AssignedModel::Named("some/other-model".to_owned())
        );

        assert_ne!(
            alpha.backend(),
            beta.backend(),
            "two profiles of the same harness must resolve to two independent destinations, \
             never collapsed onto one backend"
        );
    }

    // -------------------------------------------------------------------------
    // GH-RETRIEVAL-CRITERIA — map line 1865: the briefing door's own miss
    // -------------------------------------------------------------------------

    /// `estimated_project_memory_tokens` is one of `briefing`'s two
    /// production callers (the other, `api::unix::select_memory`, is
    /// unreachable from this binary's own test module — see
    /// `tests/context_injection.rs` for that door's coverage of `briefing`
    /// itself). A task that matches nothing records map line 1865's miss
    /// row, at the `injection` scope that distinguishes this door from the
    /// CLI/API door's `current`/`historical`.
    ///
    /// Deleting the miss-recording arm from `estimated_project_memory_tokens`
    /// kills this test.
    #[test]
    fn a_briefing_that_matches_nothing_records_one_miss_row_under_injection_scope() {
        let fixture = CliFixture::new();

        assert_eq!(
            estimated_project_memory_tokens(&fixture.runtime, "an unrelated wombat migration"),
            None
        );

        let ledger =
            glasshouse::evaluation::EvaluationObservations::open(&fixture.runtime).unwrap();
        let rows = ledger.recent(10).unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(
            rows[0].kind,
            glasshouse::evaluation::EvaluationKind::MemoryRetrievalMiss
        );
        assert_eq!(rows[0].subject.as_deref(), Some("injection"));
        assert_eq!(rows[0].memory_id, None);
    }

    /// The other half of mutation `conflate-outcomes`: a task whose search
    /// matched something real, but every match was excluded — here, an idea
    /// nobody has reaffirmed (`inject::is_unreaffirmed_idea`, line 934) — is
    /// not a miss. The search worked; it correctly withheld what it found.
    #[test]
    fn a_briefing_whose_matches_are_all_excluded_records_no_miss_row() {
        use glasshouse::memory::{MemoryAuthority, MemoryKind, NewMemory, ProjectMemory};

        let fixture = CliFixture::new();
        ProjectMemory::open(&fixture.runtime)
            .unwrap()
            .store()
            .record(
                NewMemory::new(
                    MemoryKind::Finding,
                    "The kestrel deploy is still experimental.",
                )
                .with_authority(Some(MemoryAuthority::Idea)),
            )
            .unwrap();

        assert_eq!(
            estimated_project_memory_tokens(&fixture.runtime, "kestrel deploy"),
            None,
            "an unreaffirmed idea is excluded, so there is nothing to measure"
        );

        let ledger =
            glasshouse::evaluation::EvaluationObservations::open(&fixture.runtime).unwrap();
        let rows = ledger.recent(10).unwrap();
        assert!(
            rows.is_empty(),
            "the search matched something real; excluding it is not a retrieval miss: {rows:?}"
        );
    }

    /// Map line 1923's missing link: `routing_destinations` must itself count
    /// a fresh destination's own ledger rows and hand them to
    /// `Destination::with_pairing_prior_evidence`, rather than leaving every
    /// destination at the default `0` `pairing_prior_evidence` carries until
    /// wired (`docs/product/evidence/phase-55.md`'s 1923 HELD entry).
    ///
    /// Mutation target: hand `0` instead of the count → this test fails,
    /// because `alpha`'s six matching rows would no longer be reported.
    #[test]
    fn routing_destinations_1923_reports_pairing_prior_evidence_from_the_ledger() {
        use glasshouse::routing::evidence::{EvidenceLedger, NewObservation};

        let fixture = CliFixture::new();
        let harness = glasshouse::integrations::IntegrationId::ClaudeCode;

        let mut user = UserConfig::default();
        let mut native_profile = glasshouse::config::ProfileConfig::new(harness);
        native_profile.set_model(Some("claude-native-model".to_owned()));
        user.profiles_mut().set("alpha-native", native_profile);

        let mut direct_profile = glasshouse::config::ProfileConfig::new(harness);
        direct_profile.set_backend(glasshouse::config::ProfileBackend::DirectProvider {
            provider: "openrouter".to_owned(),
        });
        direct_profile.set_model(Some("some/other-model".to_owned()));
        user.profiles_mut().set("beta-direct", direct_profile);

        let effective = EffectiveConfig::new(&user, None);

        let now_unix = glasshouse::provider::cache::now_unix_seconds();
        let ledger = EvidenceLedger::open(&fixture.runtime).unwrap();
        for _ in 0..6 {
            ledger
                .record(
                    NewObservation::new(harness.slug(), "claude-native-model"),
                    now_unix,
                )
                .unwrap();
        }

        let destinations = routing_destinations(
            &fixture.runtime,
            &effective,
            harness,
            DestinationScope::Everything,
            None,
        )
        .unwrap();

        let alpha = destinations
            .iter()
            .find(|d| d.is_fresh() && d.launch_profile() == "alpha-native")
            .expect("the native-backed profile must offer its own fresh destination");
        assert_eq!(
            alpha.pairing_prior_evidence(),
            6,
            "six ledger rows matching this destination's provider and model must all count"
        );

        let beta = destinations
            .iter()
            .find(|d| d.is_fresh() && d.launch_profile() == "beta-direct")
            .expect("the direct-provider profile must offer its own fresh destination");
        assert_eq!(
            beta.pairing_prior_evidence(),
            0,
            "a destination with no matching ledger rows must report zero, byte-identical to \
             today"
        );
    }

    // ---------------------------------------------------------------------
    // GH-POOL-ALLOWANCE — 1302, 531: `observed_provider_health` gives the
    // router's pool the allowance its setters were written for.
    // ---------------------------------------------------------------------

    /// A profile whose backend is a direct provider, so `destination_capacity`
    /// treats it as a `ResourceKind::DirectProvider` and this package's join
    /// asks the same telemetry that shape reads. No `[providers.*]` entry is
    /// configured — matching `routing_destinations_1923`'s `beta-direct` — so
    /// nothing here depends on credential resolution succeeding.
    fn pool_allowance_test_profile(
        harness: glasshouse::integrations::IntegrationId,
        provider: &str,
        model: &str,
    ) -> UserConfig {
        let mut user = UserConfig::default();
        let mut profile = glasshouse::config::ProfileConfig::new(harness);
        profile.set_backend(glasshouse::config::ProfileBackend::DirectProvider {
            provider: provider.to_owned(),
        });
        profile.set_model(Some(model.to_owned()));
        user.profiles_mut().set("pool-allowance-profile", profile);
        user
    }

    /// Required behaviour 1: a stored gateway quota reading becomes
    /// `Allowance::RequestPool { remaining: Some(n), .. }`, and — for a
    /// destination that also has a burn forecast — the `request-pool cost`
    /// term prices it rather than staying inert.
    ///
    /// The eight ledger rows are all stamped at the same `now_unix`, which
    /// `routing::burn::bucket_counts` folds into one bucket of eight — a
    /// median rate of `8 * 3600 / 300 = 96` requests/hour. At 50 remaining
    /// that is `50 / 96 * 3600 ≈ 1875s` to exhaustion against the reading's
    /// own 3600s reset: past half the reset (1800s), so
    /// `exhausts_well_before_reset` is false and the term must contribute —
    /// the case beside 1302's own guard, not the one it exists to skip.
    ///
    /// Mutation target: drop the `record_pool` call this package adds → the
    /// allowance assertion below fails, reading `unknown_pool()` instead.
    #[test]
    fn pool_allowance_1302_531_a_measured_remaining_requests_becomes_a_request_pool_and_prices_the_term()
     {
        use glasshouse::provider::telemetry::{GatewayQuotaCache, RateLimitHeaders};
        use glasshouse::routing::evidence::{EvidenceLedger, NewObservation};
        use glasshouse::routing::free::Allowance;
        use glasshouse::routing::session::{RouterInputs, RoutingMoment, RoutingOverride};

        const PROVIDER: &str = "wire-pool-allowance-request-pool-provider";
        const MODEL: &str = "pool-allowance-model";

        let fixture = CliFixture::new();
        let harness = glasshouse::integrations::IntegrationId::ClaudeCode;
        let user = pool_allowance_test_profile(harness, PROVIDER, MODEL);
        let effective = EffectiveConfig::new(&user, None);

        let now_unix = glasshouse::provider::cache::now_unix_seconds();
        GatewayQuotaCache::new(fixture.runtime.paths()).store(
            PROVIDER,
            &RateLimitHeaders::read(vec![
                ("x-ratelimit-limit-requests", "100"),
                ("x-ratelimit-remaining-requests", "50"),
                ("x-ratelimit-reset-requests", "3600s"),
            ]),
            now_unix,
        );

        let ledger = EvidenceLedger::open(&fixture.runtime).unwrap();
        for _ in 0..8 {
            ledger
                .record(NewObservation::new(PROVIDER, MODEL), now_unix)
                .unwrap();
        }

        let destinations = routing_destinations(
            &fixture.runtime,
            &effective,
            harness,
            DestinationScope::Everything,
            None,
        )
        .unwrap();
        let destination = destinations
            .iter()
            .find(|d| d.is_fresh() && d.launch_profile() == "pool-allowance-profile")
            .expect("the direct-provider profile must offer its own fresh destination");
        assert!(
            destination
                .burn_forecast()
                .is_some_and(|forecast| !forecast.exhausts_well_before_reset()),
            "the fixture's own numbers must land outside the well-before-reset guard, or \
             nothing below is attributable to this package: {:?}",
            destination.burn_forecast()
        );

        let health = observed_provider_health(&fixture.runtime, &effective, &destinations);
        let credential = destination.backend().credential().clone();
        match health.pool().allowance(&credential) {
            Allowance::RequestPool {
                limit, remaining, ..
            } => {
                assert_eq!(
                    limit,
                    Some(100),
                    "the provider's own limit, nothing derived"
                );
                assert_eq!(
                    remaining,
                    Some(50),
                    "the provider's own remaining count, nothing derived"
                );
            }
            other => panic!("expected a request pool with a measured remaining count: {other:?}"),
        }

        let overrides = effective.pairing_overrides();
        let inputs = RouterInputs {
            overrides: &overrides,
            health: health.pool(),
            now: std::time::Instant::now(),
            requirements: glasshouse::routing::session::TaskRequirements::default(),
        };
        let routed = session_router(&fixture.runtime, &effective, RoutingOverride::none())
            .choose(RoutingMoment::SessionStart, None, &destinations, &inputs)
            .expect("one destination with no hard constraint must be chosen");
        // `choose` also ranks the always-present implied Native profile
        // (`routing_destinations`' own doc, "the implied Native profile ...
        // by construction rather than by configuration"), which is not a
        // request pool and may outscore ours — so this reads *our*
        // destination's own explanation out of `considered`, not whichever
        // one won.
        let (_, explanation) = routed
            .considered()
            .iter()
            .find(|(destination, _)| destination.launch_profile() == "pool-allowance-profile")
            .expect("our destination must be among the ranked candidates");
        let pool_term = explanation
            .contributions()
            .iter()
            .find(|c| c.name() == "request-pool cost")
            .expect("a request-pool destination must always carry this term");
        assert!(
            pool_term.magnitude() < 0.0,
            "a measured remaining count with a live burn forecast must price the term rather \
             than leave it inert: {}",
            pool_term.evidence()
        );
    }

    /// Required behaviour 2: a `pricing.toml` entry for the pair, with no
    /// quota reading at all, becomes `Allowance::TokenPriced`, and the
    /// `request-pool cost` term reads *priced per token*.
    ///
    /// Mutation target: drop the `declare_token_priced` call this package
    /// adds → the allowance assertion below fails, reading `unknown_pool()`
    /// instead.
    #[test]
    fn pool_allowance_1302_531_a_pricing_toml_entry_with_no_quota_reading_becomes_token_priced() {
        use glasshouse::routing::free::Allowance;
        use glasshouse::routing::session::{RouterInputs, RoutingMoment, RoutingOverride};

        const PROVIDER: &str = "wire-pool-allowance-token-priced-provider";
        const MODEL: &str = "pool-allowance-priced-model";

        let fixture = CliFixture::new();
        let harness = glasshouse::integrations::IntegrationId::ClaudeCode;
        let user = pool_allowance_test_profile(harness, PROVIDER, MODEL);
        let effective = EffectiveConfig::new(&user, None);

        std::fs::write(
            fixture
                .runtime
                .paths()
                .config_dir()
                .join(glasshouse::provider::pricing::PRICING_FILE_NAME),
            format!(
                "[[prices]]\nprovider = \"{PROVIDER}\"\nmodel = \"{MODEL}\"\n\
                 input_per_million_usd = 3.0\noutput_per_million_usd = 15.0\n"
            ),
        )
        .unwrap();

        let destinations = routing_destinations(
            &fixture.runtime,
            &effective,
            harness,
            DestinationScope::Everything,
            None,
        )
        .unwrap();
        let destination = destinations
            .iter()
            .find(|d| d.is_fresh() && d.launch_profile() == "pool-allowance-profile")
            .expect("the direct-provider profile must offer its own fresh destination");
        assert!(
            destination.burn_forecast().is_none(),
            "no quota reading and no ledger rows were seeded; a forecast here would be invented"
        );

        let health = observed_provider_health(&fixture.runtime, &effective, &destinations);
        let credential = destination.backend().credential().clone();
        assert_eq!(
            health.pool().allowance(&credential),
            Allowance::TokenPriced,
            "a priced pair with no quota reading must declare token-priced, never a pool"
        );

        let overrides = effective.pairing_overrides();
        let inputs = RouterInputs {
            overrides: &overrides,
            health: health.pool(),
            now: std::time::Instant::now(),
            requirements: glasshouse::routing::session::TaskRequirements::default(),
        };
        let routed = session_router(&fixture.runtime, &effective, RoutingOverride::none())
            .choose(RoutingMoment::SessionStart, None, &destinations, &inputs)
            .expect("one destination with no hard constraint must be chosen");
        // See the request-pool test's own note: the implied Native profile is
        // also ranked, so this reads our destination's explanation out of
        // `considered` rather than assuming it won.
        let (_, explanation) = routed
            .considered()
            .iter()
            .find(|(destination, _)| destination.launch_profile() == "pool-allowance-profile")
            .expect("our destination must be among the ranked candidates");
        let pool_term = explanation
            .contributions()
            .iter()
            .find(|c| c.name() == "request-pool cost")
            .expect("the term is always present, inert or not");
        assert_eq!(
            pool_term.magnitude(),
            0.0,
            "priced-per-token is never priced by this term"
        );
        assert!(
            pool_term.evidence().contains("priced per token"),
            "the explanation must say why the term is inert: {}",
            pool_term.evidence()
        );
    }

    /// Required behaviour 3: neither signal — no quota reading, no
    /// `pricing.toml` entry — leaves `unknown_pool()`, byte-identical to
    /// this project's behaviour before this package. Pinned so a later
    /// change cannot invent a count for a credential nothing has read.
    #[test]
    fn pool_allowance_1302_531_neither_signal_leaves_the_pool_unknown() {
        use glasshouse::routing::free::Allowance;

        const PROVIDER: &str = "wire-pool-allowance-unknown-provider";
        const MODEL: &str = "pool-allowance-unpriced-model";

        let fixture = CliFixture::new();
        let harness = glasshouse::integrations::IntegrationId::ClaudeCode;
        let user = pool_allowance_test_profile(harness, PROVIDER, MODEL);
        let effective = EffectiveConfig::new(&user, None);

        let destinations = routing_destinations(
            &fixture.runtime,
            &effective,
            harness,
            DestinationScope::Everything,
            None,
        )
        .unwrap();
        let destination = destinations
            .iter()
            .find(|d| d.is_fresh() && d.launch_profile() == "pool-allowance-profile")
            .expect("the direct-provider profile must offer its own fresh destination");

        let health = observed_provider_health(&fixture.runtime, &effective, &destinations);
        let credential = destination.backend().credential().clone();
        assert_eq!(
            health.pool().allowance(&credential),
            Allowance::unknown_pool(),
            "with neither a quota reading nor a price, the allowance must stay exactly what it \
             was before this package"
        );
    }

    // ---------------------------------------------------------------------
    // Line 1469 — the text-keyed classification cache.
    // ---------------------------------------------------------------------

    #[test]
    fn classification_cache_resolution_tag_names_the_pin_and_nothing_else() {
        use glasshouse::config::{RoutingFallback, RoutingModelResolution};

        assert_eq!(
            classification_cache_resolution_tag(&RoutingModelResolution::Pinned {
                provider: "route-probe".to_owned(),
                model: "router-model".to_owned(),
            }),
            Some("pinned:route-probe/router-model".to_owned())
        );
        assert_eq!(
            classification_cache_resolution_tag(&RoutingModelResolution::Automatic),
            None,
            "automatic selection can differ call to call for the same text, so this cache \
             never claims to know the identity in advance"
        );
        assert_eq!(
            classification_cache_resolution_tag(&RoutingModelResolution::Heuristics(
                RoutingFallback::NotConfigured
            )),
            None
        );
        // A different pin is a different tag: two pins never share a cache
        // entry, even for the same normalised text.
        assert_ne!(
            classification_cache_resolution_tag(&RoutingModelResolution::Pinned {
                provider: "route-probe".to_owned(),
                model: "router-model".to_owned(),
            }),
            classification_cache_resolution_tag(&RoutingModelResolution::Pinned {
                provider: "route-probe".to_owned(),
                model: "a-different-model".to_owned(),
            })
        );
    }

    /// The store round-trips a record by key, and — Phase 34E's "bounded" —
    /// keeps at most [`CLASSIFICATION_TEXT_CACHE_CAPACITY`] entries, dropping
    /// the oldest by `recorded_at_unix` rather than growing without limit.
    #[test]
    fn a_classification_cache_round_trips_a_text_keyed_entry_and_bounds_its_size() {
        use glasshouse::routing::request::{CachedClassification, RoutingFingerprint};

        let fixture = CliFixture::new();
        let cache = ClassificationTextCache::new(
            fixture.runtime.paths(),
            fixture.runtime.project().id().as_str(),
        );

        assert!(
            cache.load().is_empty(),
            "no file on disk yet must read as an empty cache, not an error"
        );

        let classification = glasshouse::routing::classify::classify_heuristically(
            "what is a mutex? (bin-test fixture)",
        );
        let fingerprint = RoutingFingerprint::new(None, &[], std::iter::empty::<String>());
        for i in 0..(CLASSIFICATION_TEXT_CACHE_CAPACITY + 5) {
            cache.store(CachedClassification::new(
                format!("key-{i}"),
                fingerprint.clone(),
                "pinned:route-probe/router-model",
                &classification,
                1_000 + i as i64,
            ));
        }

        let entries = cache.load();
        assert_eq!(
            entries.len(),
            CLASSIFICATION_TEXT_CACHE_CAPACITY,
            "the cache must never grow past its named capacity"
        );
        assert!(
            !entries.contains_key("key-0"),
            "the oldest entry must be the one dropped"
        );
        assert!(
            entries.contains_key(&format!("key-{}", CLASSIFICATION_TEXT_CACHE_CAPACITY + 4)),
            "the newest entry must survive"
        );

        let round_tripped = cache
            .lookup(&format!("key-{}", CLASSIFICATION_TEXT_CACHE_CAPACITY + 4))
            .expect("the newest entry round-trips");
        assert_eq!(round_tripped.classification(), Some(classification));
    }

    /// A file this build cannot parse — corrupt, or written by a build with
    /// a different vocabulary — reads as an empty cache, the same rule
    /// [`ClassificationStickyCache::load`] follows for its own file.
    #[test]
    fn an_unreadable_classification_cache_file_reads_as_empty() {
        let fixture = CliFixture::new();
        let cache = ClassificationTextCache::new(
            fixture.runtime.paths(),
            fixture.runtime.project().id().as_str(),
        );
        std::fs::create_dir_all(
            fixture
                .runtime
                .paths()
                .project_state_dir(fixture.runtime.project().id().as_str()),
        )
        .unwrap();
        std::fs::write(
            fixture
                .runtime
                .paths()
                .project_state_dir(fixture.runtime.project().id().as_str())
                .join("routing-classification-cache.json"),
            b"not json",
        )
        .unwrap();
        assert!(cache.load().is_empty());
        assert!(cache.lookup("anything").is_none());
    }

    /// A [`tracing::Subscriber`] that records only whether *any* event fired
    /// while it was the active dispatcher — enough to catch `store()`'s own
    /// `tracing::debug!("could not persist ...")`, which is the only tracing
    /// call either store makes and which fires exactly when its write
    /// attempt returned `Err` (a collided fixed temporary's rename failing
    /// against the other writer's, per the primitive's own
    /// `write_json_atomically_cannot_succeed_by_writing_the_target_directly`
    /// class of failure). `store()` deliberately swallows that error rather
    /// than propagating it — the write is best-effort — so a test that only
    /// inspects the final file on disk cannot see a write that silently lost
    /// the race; this dispatcher is what makes that silent loss observable.
    struct EventFired(std::sync::Arc<std::sync::atomic::AtomicBool>);

    impl tracing::Subscriber for EventFired {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
        fn event(&self, _event: &tracing::Event<'_>) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        fn enter(&self, _span: &tracing::span::Id) {}
        fn exit(&self, _span: &tracing::span::Id) {}
    }

    /// `GH-ATOMIC-WRITE-MAIN-COPIES`: both stores now write through
    /// [`glasshouse::provider::cache::write_json_atomically`] instead of
    /// reimplementing it with a single fixed `.json.writing` name. Two
    /// threads storing to the same sticky-classification path concurrently
    /// must never leave a mixed or truncated file, and — since a fixed
    /// shared temporary makes one writer's rename collide with the other's
    /// and fail — neither write may be silently lost either. The same
    /// property the helper's own
    /// `concurrent_writers_to_one_path_never_produce_a_mixed_file` proves for
    /// the primitive, exercised here through the store itself and repeated
    /// over many rounds because the collision is a race.
    #[test]
    fn concurrent_sticky_classification_writes_never_produce_a_mixed_file() {
        use glasshouse::routing::request::{RoutingFingerprint, StickyClassification};

        let fixture = CliFixture::new();
        let paths = fixture.runtime.paths().clone();
        let project_id = fixture.runtime.project().id().as_str().to_owned();
        let classification = glasshouse::routing::classify::classify_heuristically(
            "what is a mutex? (bin-test fixture)",
        );
        let fingerprint = RoutingFingerprint::new(None, &[], std::iter::empty::<String>());
        let saw_write_error = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        for round in 0..30 {
            let session_a = "a".repeat(200_000);
            let session_b = "b".repeat(200_000);
            let record_a = StickyClassification::new(
                session_a.clone(),
                fingerprint.clone(),
                &classification,
                round,
            );
            let record_b = StickyClassification::new(
                session_b.clone(),
                fingerprint.clone(),
                &classification,
                round + 1,
            );

            let (paths_a, paths_b) = (paths.clone(), paths.clone());
            let (project_a, project_b) = (project_id.clone(), project_id.clone());
            let (dispatch_a, dispatch_b) = (
                tracing::Dispatch::new(EventFired(saw_write_error.clone())),
                tracing::Dispatch::new(EventFired(saw_write_error.clone())),
            );
            let handle_a = std::thread::spawn(move || {
                tracing::dispatcher::with_default(&dispatch_a, || {
                    ClassificationStickyCache::new(&paths_a, &project_a).store(&record_a)
                })
            });
            let handle_b = std::thread::spawn(move || {
                tracing::dispatcher::with_default(&dispatch_b, || {
                    ClassificationStickyCache::new(&paths_b, &project_b).store(&record_b)
                })
            });
            handle_a.join().expect("thread a");
            handle_b.join().expect("thread b");

            let cache = ClassificationStickyCache::new(&paths, &project_id);
            let loaded = cache.load().unwrap_or_else(|| {
                panic!(
                    "round {round}: the file must parse as one writer's whole record, never a mix"
                )
            });
            assert!(
                loaded.session() == session_a || loaded.session() == session_b,
                "round {round}: the final record must be exactly one writer's session, never a mix"
            );
        }

        assert!(
            !saw_write_error.load(std::sync::atomic::Ordering::SeqCst),
            "neither writer's attempt may fail: a fixed shared temporary makes one \
             writer's rename collide with and lose to the other's"
        );
    }

    /// The same property as
    /// [`concurrent_sticky_classification_writes_never_produce_a_mixed_file`],
    /// for [`ClassificationTextCache`].
    #[test]
    fn concurrent_classification_text_cache_writes_never_produce_a_mixed_file() {
        use glasshouse::routing::request::{CachedClassification, RoutingFingerprint};

        let fixture = CliFixture::new();
        let paths = fixture.runtime.paths().clone();
        let project_id = fixture.runtime.project().id().as_str().to_owned();
        let classification = glasshouse::routing::classify::classify_heuristically(
            "what is a mutex? (bin-test fixture)",
        );
        let fingerprint = RoutingFingerprint::new(None, &[], std::iter::empty::<String>());
        let saw_write_error = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        for round in 0..30 {
            let resolution_a = "a".repeat(200_000);
            let resolution_b = "b".repeat(200_000);
            let record_a = CachedClassification::new(
                "shared-key",
                fingerprint.clone(),
                resolution_a,
                &classification,
                round,
            );
            let record_b = CachedClassification::new(
                "shared-key",
                fingerprint.clone(),
                resolution_b,
                &classification,
                round + 1,
            );

            let (paths_a, paths_b) = (paths.clone(), paths.clone());
            let (project_a, project_b) = (project_id.clone(), project_id.clone());
            let (dispatch_a, dispatch_b) = (
                tracing::Dispatch::new(EventFired(saw_write_error.clone())),
                tracing::Dispatch::new(EventFired(saw_write_error.clone())),
            );
            let handle_a = std::thread::spawn(move || {
                tracing::dispatcher::with_default(&dispatch_a, || {
                    ClassificationTextCache::new(&paths_a, &project_a).store(record_a)
                })
            });
            let handle_b = std::thread::spawn(move || {
                tracing::dispatcher::with_default(&dispatch_b, || {
                    ClassificationTextCache::new(&paths_b, &project_b).store(record_b)
                })
            });
            handle_a.join().expect("thread a");
            handle_b.join().expect("thread b");

            let cache = ClassificationTextCache::new(&paths, &project_id);
            let entries = cache.load();
            assert!(
                entries.len() == 1,
                "round {round}: the file must parse as one writer's whole map, never a mix: got {} entries",
                entries.len()
            );
        }

        assert!(
            !saw_write_error.load(std::sync::atomic::Ordering::SeqCst),
            "neither writer's attempt may fail: a fixed shared temporary makes one \
             writer's rename collide with and lose to the other's"
        );
    }
}
