use std::process::ExitCode;

use std::io::IsTerminal;

use glasshouse::cli::{ApiCommand, ContextFirewallCommand, GatewayCommand, McpCommand};
use glasshouse::config::{self, EffectiveConfig, UserConfig};
use glasshouse::integrations::cmux;
use glasshouse::profile::response::Dimension;
use glasshouse::{Cli, Command, MemoryCommand, SessionCommand, logging, shutdown};

use clap::Parser;

mod api;
mod commands;

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
            print!("{}", crate::commands::status::status_report(&runtime)?);
        }
        Some(Command::Entitlements) => {
            print!(
                "{}",
                crate::commands::entitlements::entitlements_report(&runtime)?
            );
        }
        Some(Command::Doctor) => {
            print!("{}", glasshouse::integrations::doctor_report(&runtime));
        }
        Some(Command::Gateway { command }) => match command {
            GatewayCommand::Pairs => {
                print!("{}", crate::commands::gateway::gateway_pairs_report());
            }
        },
        Some(Command::Setup) => {
            if !crate::commands::setup::setup(
                &runtime,
                crate::commands::setup::SetupTrigger::Requested,
            )? {
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
            let request = match crate::commands::response::response_request(
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
                crate::commands::resources::resources_report(
                    &runtime,
                    *verbose,
                    probe,
                    *no_harness,
                    *force
                )?
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
            let text_cache = crate::commands::routing_classification::ClassificationTextCache::new(
                runtime.paths(),
                runtime.project().id().as_str(),
            );
            let text_key = glasshouse::routing::request::normalised_task_key(&request);
            let resolution_tag = match (
                UserConfig::load(runtime.paths()),
                config::load_project_config(runtime.project()),
            ) {
                (Ok(user), Ok(project)) => {
                    let effective = EffectiveConfig::new(&user, project.as_ref());
                    crate::commands::routing_classification::classification_cache_resolution_tag(
                        &effective.routing_model_resolution().value,
                    )
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
                None => match crate::commands::routing_classification::classify_with_routing_model(
                    &runtime,
                    &glasshouse::routing::request::RouterRequest::for_text(&request),
                    // Line 1419: `glasshouse classify` has chosen no launch
                    // profile — there is nothing here to protect.
                    None,
                ) {
                    crate::commands::routing_classification::ClassificationAttempt::NotConfigured => None,
                    crate::commands::routing_classification::ClassificationAttempt::Answered(classification) => {
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
                    crate::commands::routing_classification::ClassificationAttempt::Failed(why) => {
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
        }) => match crate::commands::route::route_report(
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
        Some(Command::RoutingCost { hours }) => {
            match crate::commands::routing_cost::routing_cost_report(&runtime, *hours) {
                Ok(report) => print!("{report}"),
                Err(err) => {
                    eprintln!("glasshouse: {err:#}");
                    return Ok(ExitCode::FAILURE);
                }
            }
        }
        Some(Command::ContextFirewall { command }) => match command {
            ContextFirewallCommand::Hook {
                session,
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
                crate::commands::context_firewall::context_firewall_hook(
                    &runtime,
                    *passthrough_tokens,
                    *min_semantic_tokens,
                    task,
                    tools,
                    *emit_updated_output,
                    mode,
                    session.as_deref(),
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
                    match crate::commands::context_firewall::context_firewall_show_stats(
                        &runtime, id,
                    )? {
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
                        crate::commands::context_firewall::ExpansionRequest::Candidate(
                            *candidate_id,
                        )
                    } else if let Some(file) = file {
                        crate::commands::context_firewall::ExpansionRequest::File(file.clone())
                    } else if let Some(range) = range {
                        match crate::commands::context_firewall::parse_line_range(range) {
                            Ok(bounds) => {
                                crate::commands::context_firewall::ExpansionRequest::Range(bounds)
                            }
                            Err(reason) => {
                                eprintln!("glasshouse: {reason}");
                                return Ok(ExitCode::FAILURE);
                            }
                        }
                    } else {
                        crate::commands::context_firewall::ExpansionRequest::Whole
                    };
                    match crate::commands::context_firewall::context_firewall_show(
                        &runtime, id, request,
                    )? {
                        crate::commands::context_firewall::ExpansionOutcome::Content(content) => {
                            print!("{content}")
                        }
                        crate::commands::context_firewall::ExpansionOutcome::NotFound => {
                            eprintln!(
                                "glasshouse: no context-firewall raw result stored under `{id}`"
                            );
                            return Ok(ExitCode::FAILURE);
                        }
                        crate::commands::context_firewall::ExpansionOutcome::Refused(reason) => {
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
            None => print!("{}", crate::commands::sessions::session_report(&runtime)?),
            Some(SessionCommand::Show { session, debug }) => {
                print!(
                    "{}",
                    crate::commands::sessions::session_detail(&runtime, session, *debug)?
                );
            }
            Some(SessionCommand::Rename {
                session,
                name,
                clear,
            }) => match crate::commands::sessions::rename_session(
                &runtime,
                session,
                name.as_deref(),
                *clear,
            ) {
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
            }) => match crate::commands::sessions::tag_session(
                &runtime,
                session,
                purpose.as_deref(),
                *clear,
            ) {
                Ok(report) => print!("{report}"),
                Err(err) => {
                    eprintln!("glasshouse: {err}");
                    return Ok(ExitCode::FAILURE);
                }
            },
            Some(SessionCommand::Focus { session }) => {
                match crate::commands::sessions::focus_session(&runtime, session) {
                    Ok(report) => print!("{report}"),
                    Err(err) => {
                        eprintln!("glasshouse: {err}");
                        return Ok(ExitCode::FAILURE);
                    }
                }
            }
            Some(SessionCommand::Close { session }) => {
                match crate::commands::sessions::close_session(&runtime, session) {
                    Ok(report) => print!("{report}"),
                    Err(err) => {
                        eprintln!("glasshouse: {err}");
                        return Ok(ExitCode::FAILURE);
                    }
                }
            }
            Some(SessionCommand::Reserve { session, clear }) => {
                match crate::commands::sessions::reserve_override_session(&runtime, session, *clear)
                {
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
                if let Err(err) = crate::commands::sessions::restyle_session(
                    &runtime,
                    session,
                    profile,
                    *accept_loss,
                ) {
                    eprintln!("glasshouse: {err:#}");
                    return Ok(ExitCode::FAILURE);
                }
            }
            Some(SessionCommand::Tell {
                session,
                instruction,
            }) => {
                if let Err(err) =
                    crate::commands::sessions::tell_session(&runtime, session, instruction)
                {
                    eprintln!("glasshouse: {err:#}");
                    return Ok(ExitCode::FAILURE);
                }
            }
        },
        Some(Command::Claim {
            path,
            session,
            release,
            list,
        }) => match crate::commands::sessions::claim_command(
            &runtime,
            path.as_deref(),
            session.as_deref(),
            *release,
            *list,
        ) {
            Ok(report) => print!("{report}"),
            Err(err) => {
                eprintln!("glasshouse: {err:#}");
                return Ok(ExitCode::FAILURE);
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
            let response = match crate::commands::response::response_request(
                response_role.as_deref(),
                response_profile.clone(),
                [],
            ) {
                Ok(request) => request,
                Err(err) => {
                    eprintln!("glasshouse: {err}");
                    return Ok(ExitCode::FAILURE);
                }
            };
            // Phase 21K line 1008: refused here, before anything is
            // resolved or recorded, so a misspelt override costs nothing.
            let guardrail = match guardrail
                .as_deref()
                .map(crate::commands::resume::parse_guardrail_override)
            {
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
            let external = match crate::commands::launch::external_presentation(
                presentation.as_deref(),
                presentation_ref.as_deref(),
                || {
                    let executable = std::env::current_exe()?;
                    let launch = crate::commands::launch::pane_launch_args(
                        crate::commands::launch::PaneLaunch {
                            harness: harness.as_deref(),
                            response_profile: response_profile.as_deref(),
                            response_role: response_role.as_deref(),
                            profile: profile.as_deref(),
                            from_checkpoint: from_checkpoint.as_deref(),
                            to: to.as_deref(),
                            fresh: *fresh,
                            headless: *headless,
                            harness_args,
                        },
                    );
                    Ok(cmux::pane_command(
                        &executable,
                        &crate::commands::launch::pane_global_args(cli, &runtime),
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
            return crate::commands::launch::launch_session(
                &runtime,
                harness.as_deref(),
                crate::commands::launch::LaunchDestination {
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
                crate::commands::resume::checkpoint_before_moving(&runtime, Some(session))?;
            }
            return crate::commands::resume::resume_session(
                &runtime,
                session,
                harness_args,
                false,
                crate::commands::resume::RouteOnResume::AtTaskBoundary,
            );
        }
        Some(Command::Memory { command }) => match command {
            MemoryCommand::Search {
                query,
                path,
                for_edit,
                history,
                limit,
                explain,
            } => {
                // `--for-edit` changes only how a `--path` answer is ordered,
                // so without one there is nothing for it to mean. An error
                // rather than a silent no-op: a caller that passed it
                // believes it asked for something.
                if *for_edit && path.is_none() {
                    eprintln!(
                        "glasshouse: --for-edit orders a file's memories for an intended edit \
                         of that file, so it needs --path"
                    );
                    return Ok(ExitCode::FAILURE);
                }
                match path.as_deref() {
                    Some(path) => print!(
                        "{}",
                        crate::commands::memory::memory_path_report(
                            &runtime, path, *for_edit, *history, *limit
                        )?
                    ),
                    None if *explain => {
                        print!(
                            "{}",
                            crate::commands::memory::memory_search_explain(
                                &runtime,
                                &query.join(" ")
                            )?
                        )
                    }
                    None => print!(
                        "{}",
                        crate::commands::memory::memory_report(
                            &runtime,
                            &query.join(" "),
                            *history,
                            *limit
                        )?
                    ),
                }
            }
            MemoryCommand::Promote { id, authority } => {
                print!(
                    "{}",
                    crate::commands::memory::memory_promote(&runtime, id, authority)?
                );
            }
            MemoryCommand::Challenge { id, reason } => {
                print!(
                    "{}",
                    crate::commands::memory::memory_challenge(&runtime, id, reason)?
                );
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
                    print!(
                        "{}",
                        crate::commands::memory::memory_revalidate_list(&runtime, *limit)?
                    );
                } else {
                    let id = id.as_deref().ok_or_else(|| {
                        anyhow::anyhow!("an id is required unless --list is given")
                    })?;
                    let outcome = outcome.as_deref().ok_or_else(|| {
                        anyhow::anyhow!("an outcome is required unless --list is given")
                    })?;
                    print!(
                        "{}",
                        crate::commands::memory::memory_revalidate(
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
                print!(
                    "{}",
                    crate::commands::memory::memory_conflicts_list(&runtime, *limit)?
                );
            }
            MemoryCommand::Resolve { id, outcome } => {
                print!(
                    "{}",
                    crate::commands::memory::memory_resolve_conflict(&runtime, id, outcome)?
                );
            }
            MemoryCommand::Commit { session } => {
                print!(
                    "{}",
                    crate::commands::memory::memory_commit(&runtime, session.as_deref())?
                );
            }
            MemoryCommand::Extract {
                session,
                activity,
                from_events,
                reply_from,
            } => {
                print!(
                    "{}",
                    crate::commands::memory::memory_extract(
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
                    crate::commands::memory::memory_export_tracked(
                        &runtime,
                        *tracked,
                        *include_findings,
                        *dry_run
                    )?
                );
            }
            MemoryCommand::ExportLocal {
                harness,
                limit,
                no_exclude,
            } => {
                print!(
                    "{}",
                    crate::commands::memory::memory_export_local(
                        &runtime,
                        harness.as_deref(),
                        *limit,
                        !*no_exclude
                    )?
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
                    crate::commands::memory::memory_rate(
                        &runtime,
                        id,
                        *verdict,
                        session.as_deref(),
                        note.as_deref()
                    )?
                );
            }
            MemoryCommand::Retrievals {
                hours,
                session,
                limit,
            } => {
                print!(
                    "{}",
                    crate::commands::memory::memory_retrievals_report(
                        &runtime,
                        *hours,
                        session.as_deref(),
                        *limit
                    )?
                );
            }
        },
        Some(Command::Checkpoint { command }) => {
            return crate::commands::checkpoint::checkpoint_command(&runtime, command);
        }
        Some(Command::Hook { session, event }) => {
            crate::commands::hook::install_quiet_panic_hook();
            crate::commands::hook::report_hook(&runtime, session, event);
        }
        Some(Command::Shim {
            harness,
            profile,
            dir,
            name,
            force,
        }) => {
            return crate::commands::shim::run_shim(harness, profile, dir, name.as_deref(), *force);
        }
        // Phase 21K line 1048: what agents have stated, and what became of
        // it — read from the ledger, never inferred from a transcript.
        Some(Command::Assumptions { session, limit }) => {
            match crate::commands::assumptions::assumptions_report(
                &runtime,
                session.as_deref(),
                *limit,
            ) {
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
            crate::commands::setup::setup(
                &runtime,
                crate::commands::setup::SetupTrigger::FirstRun,
            )?;

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

#[cfg(test)]
mod tests;
