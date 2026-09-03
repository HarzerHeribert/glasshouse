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
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use glasshouse::Runtime;
    use glasshouse::checkpoint::ProjectCheckpoints;
    use glasshouse::checkpoint::git::GitPosition;
    use glasshouse::cli::CheckpointCommand;
    use glasshouse::config::response::ResponseRequest;
    use glasshouse::events::EventLog;
    use glasshouse::session::{NewSession, ProjectSessions, SessionId, SessionRuntime};

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

        let pairing = crate::commands::launch::resolved_gateway_pairing(&effective);

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

        let pairing = crate::commands::launch::resolved_gateway_pairing(&effective);

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

        let reached = crate::commands::resume::close_before_forced_exit(
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
        let reached = crate::commands::resume::close_before_forced_exit(
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

        let one_shot = crate::commands::resume::close_before_forced_exit(
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
        let code = all_production_code();

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
        let code = all_production_code();

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
        let code = all_production_code();

        let starts = code.matches("start_if_required_with_degrade_sink(").count();
        let sinks = code
            .matches(
                "Some(crate::commands::routing_destinations::failover_prevention_sink(runtime)),",
            )
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

    /// Every file `main.rs` was decomposed into (Phase 59, objective 2b) —
    /// the source scans below used to read `main.rs` alone because every
    /// subcommand's implementation lived there; the move relocated the code
    /// these scans look for without changing it, so the scans now have to
    /// cover `commands/*.rs` too or they would silently find nothing. Add a
    /// new file here whenever one is added under `commands/`.
    const PRODUCTION_SOURCE_FILES: &[&str] = &[
        include_str!("main.rs"),
        include_str!("commands/status.rs"),
        include_str!("commands/entitlements.rs"),
        include_str!("commands/gateway.rs"),
        include_str!("commands/setup.rs"),
        include_str!("commands/response.rs"),
        include_str!("commands/resources.rs"),
        include_str!("commands/route.rs"),
        include_str!("commands/routing_cost.rs"),
        include_str!("commands/context_firewall.rs"),
        include_str!("commands/sessions.rs"),
        include_str!("commands/memory.rs"),
        include_str!("commands/memory_extraction.rs"),
        include_str!("commands/checkpoint.rs"),
        include_str!("commands/hook.rs"),
        include_str!("commands/shim.rs"),
        include_str!("commands/assumptions.rs"),
        include_str!("commands/launch.rs"),
        include_str!("commands/resume.rs"),
        include_str!("commands/routing_destinations.rs"),
        include_str!("commands/routing_classification.rs"),
        include_str!("commands/shared.rs"),
    ];

    /// [`production_code`], applied to every file in [`PRODUCTION_SOURCE_FILES`]
    /// and joined — the whole binary crate's production source as one string,
    /// each file's own `#[cfg(test)]` tail stripped first so a later file's
    /// tests cannot leak into an earlier file's stripped output.
    fn all_production_code() -> String {
        PRODUCTION_SOURCE_FILES
            .iter()
            .map(|source| production_code(source))
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
        let code = all_production_code();
        // `return ...launch_session(` matches only an actual call, never the
        // `fn launch_session(` definition line itself. Phase 59 moved the
        // dispatch arm's call site into `main.rs` behind a qualified path,
        // so the call now reads `return crate::commands::launch::launch_session(`.
        let call_sites = code
            .matches("return crate::commands::launch::launch_session(")
            .count();
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

        crate::commands::routing_destinations::record_correlation_steer(
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
        let briefing = crate::commands::launch::brief_launch_session(
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
            crate::commands::launch::LaunchBriefing::NotBriefed(reason) => {
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

        let status = crate::commands::launch::launch_session(
            &runtime,
            Some("claude-code"),
            crate::commands::launch::LaunchDestination {
                profile: Some("gateway"),
                ..crate::commands::launch::LaunchDestination::default()
            },
            &ResponseRequest::default(),
            false,
            false,
            crate::commands::launch::ExternalPresentation::Embedded,
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

        let status = crate::commands::launch::launch_session(
            &runtime,
            Some("claude-code"),
            crate::commands::launch::LaunchDestination {
                profile: Some("yolo"),
                ..crate::commands::launch::LaunchDestination::default()
            },
            &ResponseRequest::default(),
            false,
            false,
            crate::commands::launch::ExternalPresentation::Embedded,
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

        let status = crate::commands::launch::launch_session(
            &runtime,
            Some("claude-code"),
            crate::commands::launch::LaunchDestination::default(),
            &ResponseRequest::default(),
            false,
            false,
            crate::commands::launch::ExternalPresentation::Embedded,
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

        let status = crate::commands::launch::launch_session(
            &runtime,
            Some("claude-code"),
            crate::commands::launch::LaunchDestination {
                profile: Some("ccc-yolo"),
                ..crate::commands::launch::LaunchDestination::default()
            },
            &ResponseRequest::default(),
            false,
            false,
            crate::commands::launch::ExternalPresentation::Embedded,
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

        let status = crate::commands::launch::launch_session(
            &runtime,
            Some("claude-code"),
            crate::commands::launch::LaunchDestination {
                profile: Some("bbb-yolo"),
                ..crate::commands::launch::LaunchDestination::default()
            },
            &ResponseRequest::default(),
            false,
            false,
            crate::commands::launch::ExternalPresentation::Embedded,
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
        let destinations = crate::commands::routing_destinations::routing_destinations(
            &runtime,
            &effective,
            harness,
            crate::commands::routing_destinations::DestinationScope::LaunchableAcrossProfiles,
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
        assert!(crate::commands::resume::mechanism_summary(&overlay).contains("automatic review"));
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
    fn hook_handler_source() -> String {
        let full = all_production_code();
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
        body.to_owned()
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
        let source = strip_comments(&hook_handler_source());

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
            paths_dropped: 0,
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

        let notice = crate::commands::memory_extraction::lost_extraction_notice(
            "before_compaction",
            Some(&outcome),
        )
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
            crate::commands::memory_extraction::lost_extraction_notice(
                "before_compaction",
                Some(&outcome)
            ),
            None,
            "nothing was extracted because there was nothing to extract; that is not a loss"
        );
    }

    /// [`run_extraction`] answers [`None`] for its preparation failures and
    /// for [`EXTRACTION_BOUND`] expiring, and all of those are losses — a
    /// boundary went by and nothing was written.
    #[test]
    fn an_extraction_that_never_produced_an_outcome_is_reported_as_a_loss() {
        let notice =
            crate::commands::memory_extraction::lost_extraction_notice("before_compaction", None)
                .expect("no outcome at all is a lost memory");
        assert!(
            notice.contains("before_compaction") && notice.contains("recorded nothing"),
            "{notice}"
        );
        assert!(
            notice.contains(
                &crate::commands::memory_extraction::EXTRACTION_BOUND
                    .as_secs()
                    .to_string()
            ),
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
            crate::commands::memory_extraction::lost_extraction_notice(
                "task_completed",
                Some(&outcome)
            ),
            None
        );

        let mut duplicates_only = recorded_nothing();
        duplicates_only.duplicates = 2;
        assert_eq!(
            crate::commands::memory_extraction::lost_extraction_notice(
                "task_completed",
                Some(&duplicates_only)
            ),
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

        let notice = crate::commands::memory_extraction::lost_extraction_notice(
            "before_compaction",
            Some(&outcome),
        )
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
        let finished = crate::commands::hook::abandon_after(bound, || {
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
            crate::commands::hook::abandon_after(std::time::Duration::from_secs(30), move || {
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

        assert_eq!(crate::commands::shared::format_age(now), "just now");
        assert_eq!(crate::commands::shared::format_age(now - 30), "just now");
        assert_eq!(crate::commands::shared::format_age(now - 120), "2m ago");
        assert_eq!(crate::commands::shared::format_age(now - 7_200), "2h ago");
        assert_eq!(
            crate::commands::shared::format_age(now - 3 * 86_400),
            "3d ago"
        );

        // A future timestamp must not print a negative age.
        let ahead = crate::commands::shared::format_age(now + 10_000);
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
            let text = crate::commands::shared::format_age(extreme);
            assert!(!text.is_empty() && !text.contains('-'), "bad age: {text}");
        }
        assert_eq!(
            crate::commands::shared::format_age(i64::MAX),
            "just now",
            "the far future reads as now"
        );
    }

    /// The header and every row go through `session_row`, so their columns
    /// cannot drift apart. Checked here rather than trusted.
    #[test]
    fn listing_columns_line_up_between_the_header_and_a_row() {
        let header = crate::commands::sessions::session_row(
            "SESSION",
            "NAME",
            "PURPOSE",
            "HARNESS",
            "PROFILE",
            "STATE",
            "ROLE",
            "PRESENTED",
            "LAST",
            crate::commands::sessions::PRESENTED_WIDTH,
        );
        let row = crate::commands::sessions::session_row(
            "abc123",
            "the auth probe",
            "auth",
            "claude-code",
            "native",
            "resumable",
            "orchestrator",
            "embedded",
            "2h ago",
            crate::commands::sessions::PRESENTED_WIDTH,
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

    /// Map line 1139's safety property, at the one branch a black-box test
    /// cannot reach.
    ///
    /// `tests/file_aware_memory.rs` proves the hook's response is identical
    /// across every recording outcome a real invocation can reach — recorded,
    /// no session, not a writing tool, outside the root. It cannot reach a
    /// *failed append*, because this binary's own bootstrap opens the project
    /// database before any subcommand runs and refuses to start if it cannot,
    /// so a database made unwritable never gets as far as the hook.
    ///
    /// In process the branch is reachable, and this is what it must do:
    /// return, having logged, propagating nothing. The type system carries
    /// the rest — `record_file_touches` returns `()`, so the response written
    /// afterwards cannot read anything from it.
    ///
    /// Unix only, and the reason is the injection rather than the behaviour:
    /// making a write fail means taking write permission away, and Windows'
    /// ACLs do not honour a mode bit. The code under test has no `#[cfg]`.
    #[cfg(unix)]
    #[test]
    fn record_file_touches_never_propagates_a_failure() {
        use std::os::unix::fs::PermissionsExt as _;

        let fixture = CliFixture::new();
        let root = fixture.runtime.project().root().to_path_buf();
        let event = glasshouse::firewall::adapter::PostToolUseEvent {
            tool_name: "Edit".to_owned(),
            tool_input: serde_json::json!({ "file_path": root.join("a.rs") }),
            tool_response: serde_json::json!({ "type": "text", "text": "done" }),
            tool_use_id: "tu".to_owned(),
            session_id: "cc".to_owned(),
        };

        // First, the healthy case, so a later `is_empty` cannot pass because
        // the event was never recordable in the first place.
        crate::commands::context_firewall::record_file_touches(
            &fixture.runtime,
            Some("s-1"),
            &event,
        );
        let log = glasshouse::events::EventLog::open(&fixture.runtime).unwrap();
        assert_eq!(
            log.all().unwrap().len(),
            1,
            "the healthy case must record, or the failure below proves nothing"
        );
        drop(log);

        let state = fixture.runtime.state_dir().to_path_buf();
        let original = std::fs::metadata(&state).unwrap().permissions();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o555)).unwrap();
        let database = fixture.runtime.database_path();
        let mut readonly = std::fs::metadata(&database).unwrap().permissions();
        readonly.set_mode(0o444);
        std::fs::set_permissions(&database, readonly).unwrap();

        // The whole assertion: this returns. It does not panic, and there is
        // no error for a caller to have to handle.
        crate::commands::context_firewall::record_file_touches(
            &fixture.runtime,
            Some("s-1"),
            &event,
        );

        std::fs::set_permissions(&state, original).unwrap();
        let mut writable = std::fs::metadata(&database).unwrap().permissions();
        writable.set_mode(0o644);
        std::fs::set_permissions(&database, writable).unwrap();
        let log = glasshouse::events::EventLog::open(&fixture.runtime).unwrap();
        assert_eq!(
            log.all().unwrap().len(),
            1,
            "the second call really must have failed to write, or this proves nothing"
        );
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

        let report =
            crate::commands::memory::memory_report(&fixture.runtime, "kestrel", false, 20).unwrap();

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

        let promoted =
            crate::commands::memory::memory_promote(&fixture.runtime, id.as_str(), "invariant")
                .unwrap();
        assert!(promoted.contains("invariant"), "{promoted}");
        assert_eq!(
            project.store().get(&id).unwrap().unwrap().authority,
            Some(glasshouse::memory::MemoryAuthority::Invariant)
        );

        // Demotion is never refused: 21A's concern is memories becoming
        // binding without anyone deciding they should.
        let demoted =
            crate::commands::memory::memory_promote(&fixture.runtime, id.as_str(), "preference")
                .unwrap();
        assert!(demoted.contains("preference"), "{demoted}");

        let cleared =
            crate::commands::memory::memory_promote(&fixture.runtime, id.as_str(), "unclassified")
                .unwrap();
        assert!(cleared.contains("unclassified"), "{cleared}");
        assert_eq!(project.store().get(&id).unwrap().unwrap().authority, None);

        // A class that does not exist is refused by name rather than
        // silently storing nothing.
        let refused = crate::commands::memory::memory_promote(
            &fixture.runtime,
            id.as_str(),
            "extremely-important",
        );
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

        let before =
            crate::commands::memory::memory_report(&fixture.runtime, "egret", false, 10).unwrap();
        assert!(before.contains(BODY), "{before}");

        let challenged = crate::commands::memory::memory_challenge(
            &fixture.runtime,
            id.as_str(),
            "production_incident",
        )
        .unwrap();
        assert!(challenged.contains("needs_review"), "{challenged}");
        assert!(challenged.contains("production_incident"), "{challenged}");

        // No longer returned as current, settled knowledge.
        let after =
            crate::commands::memory::memory_report(&fixture.runtime, "egret", false, 10).unwrap();
        assert!(
            !after.contains(BODY),
            "a challenged memory must not appear in a default search:\n{after}"
        );

        // Still reachable as history, with the reason recorded and readable.
        let history =
            crate::commands::memory::memory_report(&fixture.runtime, "egret", true, 10).unwrap();
        assert!(history.contains(BODY), "{history}");
        assert!(history.contains("needs_review"), "{history}");
        assert!(
            history.contains("production_incident"),
            "the challenge reason must be readable in the history report:\n{history}"
        );

        // A reason that is not one of the six is refused, and nothing is
        // written.
        let refused =
            crate::commands::memory::memory_challenge(&fixture.runtime, id.as_str(), "vibes");
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

        let before =
            crate::commands::memory::memory_report(&fixture.runtime, "heron", false, 10).unwrap();
        assert!(before.contains(BODY), "{before}");

        crate::commands::memory::memory_challenge(&fixture.runtime, id.as_str(), "project_state")
            .unwrap();
        let after_challenge =
            crate::commands::memory::memory_report(&fixture.runtime, "heron", false, 10).unwrap();
        assert!(
            !after_challenge.contains(BODY),
            "a challenged memory must drop out of a default search:\n{after_challenge}"
        );

        let revalidated = crate::commands::memory::memory_revalidate(
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
        let after_revalidate =
            crate::commands::memory::memory_report(&fixture.runtime, "heron", false, 10).unwrap();
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
        let refused = crate::commands::memory::memory_revalidate(
            &fixture.runtime,
            id.as_str(),
            "vibes",
            None,
            None,
            false,
        );
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

        let listing = crate::commands::memory::memory_revalidate_list(&fixture.runtime, 2).unwrap();
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

        let report =
            crate::commands::memory::memory_report(&fixture.runtime, "kite", false, 10).unwrap();
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
            crate::commands::hook::report_hook_with(
                &fixture.runtime,
                id.as_str(),
                "Stop",
                move |_| {
                    Box::new(Canned {
                        reply: ONE_FINDING.to_owned(),
                        asked: std::sync::Arc::clone(&asked),
                    })
                },
            );
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
                crate::commands::hook::report_hook_with(
                    &fixture.runtime,
                    id.as_str(),
                    event,
                    move |_| {
                        Box::new(Canned {
                            reply: ONE_FINDING.to_owned(),
                            asked: std::sync::Arc::clone(&asked),
                        })
                    },
                );
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

            crate::commands::hook::report_hook_with(
                &fixture.runtime,
                id.as_str(),
                "Stop",
                move |_| {
                    Box::new(Hostile(match kind {
                        HostileKind::Refuses => HostileKind::Refuses,
                        HostileKind::Panics => HostileKind::Panics,
                        HostileKind::Hangs => HostileKind::Hangs,
                    }))
                },
            );

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
        crate::commands::hook::report_hook_with(&fixture.runtime, id.as_str(), "Stop", |_| {
            Box::new(Hostile(HostileKind::Hangs))
        });
        let waited = started.elapsed();

        assert!(
            waited < crate::commands::memory_extraction::EXTRACTION_BOUND * 3,
            "the hook waited {waited:?} on a model that sleeps for a minute;              the bound is {:?}",
            crate::commands::memory_extraction::EXTRACTION_BOUND
        );
        assert!(
            waited >= crate::commands::memory_extraction::EXTRACTION_BOUND,
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
        crate::commands::hook::report_hook(&fixture.runtime, id.as_str(), "Stop");

        assert!(stored_memories(&fixture.runtime).is_empty());
        let described = crate::commands::memory_extraction::NoExtractionModel.describe();
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
        let before = crate::commands::routing_classification::disposable_extraction_model(
            &fixture.runtime,
            &granted,
        )
        .describe();

        let report = crate::commands::sessions::reserve_override_session(
            &fixture.runtime,
            granted.as_str(),
            false,
        )
        .unwrap();
        let after_granted = crate::commands::routing_classification::disposable_extraction_model(
            &fixture.runtime,
            &granted,
        )
        .describe();
        let after_other = crate::commands::routing_classification::disposable_extraction_model(
            &fixture.runtime,
            &other,
        )
        .describe();

        crate::commands::sessions::reserve_override_session(
            &fixture.runtime,
            granted.as_str(),
            true,
        )
        .unwrap();
        let after_clear = crate::commands::routing_classification::disposable_extraction_model(
            &fixture.runtime,
            &granted,
        )
        .describe();

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

        let model = crate::commands::routing_classification::disposable_extraction_model(
            &fixture.runtime,
            &a_session_not_overridden(),
        );
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

        let model = crate::commands::routing_classification::disposable_extraction_model(
            &fixture.runtime,
            &a_session_not_overridden(),
        );
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

    /// Capability map line 1367, at the production entry point: a free
    /// resource whose entire measured remainder is already claimed by
    /// another dispatch is not chosen, and the explanation says why rather
    /// than only that.
    ///
    /// The reservation is planted rather than claimed, because a claim taken
    /// by *this* process would be this process's own — the fact under test
    /// is what one dispatcher does about another's, and there is only one
    /// process in a unit test.
    #[test]
    fn a_free_resource_whose_remaining_requests_are_all_reserved_is_not_chosen() {
        const VAR: &str = "GLASSHOUSE_TEST_ONLY_WIRE_DISPOSABLE_RESERVED_KEY";
        const PROVIDER: &str = "wire-disposable-reserved-test-provider";
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
                ("x-ratelimit-limit-requests", "10"),
                ("x-ratelimit-remaining-requests", "1"),
            ]),
            now_unix,
        );
        let label = format!("{PROVIDER}/{VAR}");
        glasshouse::provider::telemetry::DispatchReservationCache::new(fixture.runtime.paths())
            .plant(
                0,
                &glasshouse::provider::telemetry::DispatchReservation {
                    credential_label: label.clone(),
                    model: "a-free-model".to_owned(),
                    requests: 1,
                    process_id: 999_999,
                    reserved_at_unix: now_unix,
                    expires_at_unix: now_unix + 60,
                },
            )
            .unwrap();

        let model = crate::commands::routing_classification::disposable_extraction_model(
            &fixture.runtime,
            &a_session_not_overridden(),
        );
        let described = model.describe();

        unsafe {
            std::env::remove_var(VAR);
        }

        assert!(
            described.contains("reserved by another dispatch"),
            "the explanation must say why the free resource was passed over: {described}"
        );
        assert!(
            described.contains(&label),
            "and which allowance it was: {described}"
        );
        assert!(
            described.contains("no model was called"),
            "nothing else is configured, so the refusal is today's: {described}"
        );
    }

    /// The lease in `provider::telemetry` and the bound in this file are two
    /// constants that only mean anything in relation to each other: the
    /// reservation exists to cover the extraction, so it has to outlive it.
    /// Nothing else would fail if one of them were edited alone — the
    /// reservation would simply start expiring under live calls — which is
    /// exactly the kind of drift a test is for.
    #[test]
    fn the_reservation_lease_outlives_the_extraction_it_covers() {
        assert_eq!(
            glasshouse::provider::telemetry::DISPATCH_RESERVATION_LEASE,
            crate::commands::memory_extraction::EXTRACTION_BOUND * 2,
            "the lease is twice the bound on the work it covers; see its own doc for why"
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

        let model = crate::commands::routing_classification::disposable_extraction_model(
            &fixture.runtime,
            &a_session_not_overridden(),
        );
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

        let model = crate::commands::routing_classification::disposable_extraction_model(
            &fixture.runtime,
            &a_session_not_overridden(),
        );
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

        let model = crate::commands::routing_classification::disposable_extraction_model(
            &fixture.runtime,
            &a_session_not_overridden(),
        );
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
        let full = all_production_code();
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

        let upstream = crate::commands::resume::gateway_upstream(
            &user,
            project.as_ref(),
            &effective,
            &secrets,
        )
        .unwrap();
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

        let report =
            crate::commands::memory::memory_report(&fixture.runtime, "kestrel", false, 10).unwrap();

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
        crate::commands::hook::report_hook(&fixture.runtime, id.as_str(), "UserPromptSubmit");
        crate::commands::hook::report_hook(&fixture.runtime, id.as_str(), "Stop");

        let dir = tempfile::tempdir().unwrap();
        let reply = dir.path().join("reply.json");
        std::fs::write(&reply, ONE_FINDING).unwrap();

        let report = crate::commands::memory::memory_extract(
            &fixture.runtime,
            id.as_str(),
            None,
            true,
            &reply,
        )
        .unwrap();

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

        let report = crate::commands::memory::memory_extract(
            &fixture.runtime,
            "s-1",
            Some(&activity),
            false,
            &reply,
        )
        .unwrap();

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

        let report = crate::commands::memory::memory_extract(
            &fixture.runtime,
            "s-1",
            Some(&activity),
            false,
            &reply,
        )
        .unwrap();
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

        let report = crate::commands::memory::memory_extract(
            &fixture.runtime,
            "s-1",
            Some(&activity),
            false,
            &reply,
        );
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
        let status =
            crate::commands::checkpoint::checkpoint_command(&fixture.runtime, &command).unwrap();
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

        let lines = crate::commands::resume::binding_memory_lines(&fixture.runtime);

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
        let tool_use_requirements = crate::commands::routing_classification::heuristic_answer(
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

        let router_default = crate::commands::routing_destinations::session_router(
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

        let router_overridden = crate::commands::routing_destinations::session_router(
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

        let observed = crate::commands::routing_destinations::observed_health_of(
            &fixture.runtime,
            [resource.clone()],
        );
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
            crate::commands::routing_destinations::estimated_project_memory_tokens(
                &fixture.runtime,
                "kestrel deploy"
            ),
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
        let short = crate::commands::routing_destinations::estimated_project_memory_tokens(
            &fixture.runtime,
            "kestrel deploy",
        )
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
        let longer = crate::commands::routing_destinations::estimated_project_memory_tokens(
            &fixture.runtime,
            "kestrel deploy",
        )
        .expect("a matching memory must still be measured");
        assert!(
            longer > short,
            "the estimate must count the briefing's real rendered size, not a constant: \
             {short} then {longer}"
        );

        assert_eq!(
            crate::commands::routing_destinations::estimated_project_memory_tokens(
                &fixture.runtime,
                "an unrelated wombat migration"
            ),
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
            crate::commands::checkpoint::checkpoint_command(&fixture.runtime, &command).unwrap(),
            ExitCode::SUCCESS
        );

        let checkpoints = ProjectCheckpoints::open(&fixture.runtime).unwrap();
        let checkpointed_tokens = crate::commands::routing_destinations::session_checkpoint_tokens(
            Some(&checkpoints),
            &checkpointed.id,
        )
        .expect("the checkpointed session's own document must be measured");
        assert!(checkpointed_tokens > 0);
        assert_eq!(
            crate::commands::routing_destinations::session_checkpoint_tokens(
                Some(&checkpoints),
                &untouched.id
            ),
            None,
            "a session with no checkpoint of its own is unknown, not zero — even though this \
             project has a checkpoint, it belongs to a different session"
        );
    }

    #[test]
    fn latest_checkpoint_tokens_is_absent_until_the_project_has_one_then_measures_it() {
        let fixture = CliFixture::new();
        assert_eq!(
            crate::commands::routing_destinations::latest_checkpoint_tokens(&fixture.runtime),
            None
        );

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
            crate::commands::checkpoint::checkpoint_command(&fixture.runtime, &command).unwrap(),
            ExitCode::SUCCESS
        );

        let tokens =
            crate::commands::routing_destinations::latest_checkpoint_tokens(&fixture.runtime)
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
            crate::commands::checkpoint::checkpoint_command(&fixture.runtime, &command).unwrap(),
            ExitCode::SUCCESS
        );

        let user = UserConfig::load(fixture.runtime.paths()).unwrap();
        let project = config::load_project_config(fixture.runtime.project()).unwrap();
        let effective = EffectiveConfig::new(&user, project.as_ref());
        let destinations = crate::commands::routing_destinations::routing_destinations(
            &fixture.runtime,
            &effective,
            glasshouse::integrations::IntegrationId::ClaudeCode,
            crate::commands::routing_destinations::DestinationScope::Everything,
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

        let destinations = crate::commands::routing_destinations::routing_destinations(
            &fixture.runtime,
            &effective,
            glasshouse::integrations::IntegrationId::ClaudeCode,
            crate::commands::routing_destinations::DestinationScope::Everything,
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
        let destinations = crate::commands::routing_destinations::routing_destinations(
            &fixture.runtime,
            &effective,
            glasshouse::integrations::IntegrationId::ClaudeCode,
            crate::commands::routing_destinations::DestinationScope::Everything,
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

        let destinations = crate::commands::routing_destinations::routing_destinations(
            &fixture.runtime,
            &effective,
            harness,
            crate::commands::routing_destinations::DestinationScope::Everything,
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

        let candidates = crate::commands::routing_classification::disposable_candidates(
            &user, None, &effective, &secrets, &telemetry, now_unix,
        );

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
        let destinations = crate::commands::routing_destinations::routing_destinations(
            &fixture.runtime,
            &effective,
            harness,
            crate::commands::routing_destinations::DestinationScope::Everything,
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

        let destinations = crate::commands::routing_destinations::routing_destinations(
            &fixture.runtime,
            &effective,
            harness,
            crate::commands::routing_destinations::DestinationScope::Everything,
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
            crate::commands::routing_destinations::estimated_project_memory_tokens(
                &fixture.runtime,
                "an unrelated wombat migration"
            ),
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
            crate::commands::routing_destinations::estimated_project_memory_tokens(
                &fixture.runtime,
                "kestrel deploy"
            ),
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

        let destinations = crate::commands::routing_destinations::routing_destinations(
            &fixture.runtime,
            &effective,
            harness,
            crate::commands::routing_destinations::DestinationScope::Everything,
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

        let destinations = crate::commands::routing_destinations::routing_destinations(
            &fixture.runtime,
            &effective,
            harness,
            crate::commands::routing_destinations::DestinationScope::Everything,
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

        let health = crate::commands::routing_destinations::observed_provider_health(
            &fixture.runtime,
            &effective,
            &destinations,
        );
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
        let routed = crate::commands::routing_destinations::session_router(
            &fixture.runtime,
            &effective,
            RoutingOverride::none(),
        )
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

        let destinations = crate::commands::routing_destinations::routing_destinations(
            &fixture.runtime,
            &effective,
            harness,
            crate::commands::routing_destinations::DestinationScope::Everything,
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

        let health = crate::commands::routing_destinations::observed_provider_health(
            &fixture.runtime,
            &effective,
            &destinations,
        );
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
        let routed = crate::commands::routing_destinations::session_router(
            &fixture.runtime,
            &effective,
            RoutingOverride::none(),
        )
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

        let destinations = crate::commands::routing_destinations::routing_destinations(
            &fixture.runtime,
            &effective,
            harness,
            crate::commands::routing_destinations::DestinationScope::Everything,
            None,
        )
        .unwrap();
        let destination = destinations
            .iter()
            .find(|d| d.is_fresh() && d.launch_profile() == "pool-allowance-profile")
            .expect("the direct-provider profile must offer its own fresh destination");

        let health = crate::commands::routing_destinations::observed_provider_health(
            &fixture.runtime,
            &effective,
            &destinations,
        );
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
            crate::commands::routing_classification::classification_cache_resolution_tag(
                &RoutingModelResolution::Pinned {
                    provider: "route-probe".to_owned(),
                    model: "router-model".to_owned(),
                }
            ),
            Some("pinned:route-probe/router-model".to_owned())
        );
        assert_eq!(
            crate::commands::routing_classification::classification_cache_resolution_tag(
                &RoutingModelResolution::Automatic
            ),
            None,
            "automatic selection can differ call to call for the same text, so this cache \
             never claims to know the identity in advance"
        );
        assert_eq!(
            crate::commands::routing_classification::classification_cache_resolution_tag(
                &RoutingModelResolution::Heuristics(RoutingFallback::NotConfigured)
            ),
            None
        );
        // A different pin is a different tag: two pins never share a cache
        // entry, even for the same normalised text.
        assert_ne!(
            crate::commands::routing_classification::classification_cache_resolution_tag(
                &RoutingModelResolution::Pinned {
                    provider: "route-probe".to_owned(),
                    model: "router-model".to_owned(),
                }
            ),
            crate::commands::routing_classification::classification_cache_resolution_tag(
                &RoutingModelResolution::Pinned {
                    provider: "route-probe".to_owned(),
                    model: "a-different-model".to_owned(),
                }
            )
        );
    }

    /// The store round-trips a record by key, and — Phase 34E's "bounded" —
    /// keeps at most [`CLASSIFICATION_TEXT_CACHE_CAPACITY`] entries, dropping
    /// the oldest by `recorded_at_unix` rather than growing without limit.
    #[test]
    fn a_classification_cache_round_trips_a_text_keyed_entry_and_bounds_its_size() {
        use glasshouse::routing::request::{CachedClassification, RoutingFingerprint};

        let fixture = CliFixture::new();
        let cache = crate::commands::routing_classification::ClassificationTextCache::new(
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
        for i in
            0..(crate::commands::routing_classification::CLASSIFICATION_TEXT_CACHE_CAPACITY + 5)
        {
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
            crate::commands::routing_classification::CLASSIFICATION_TEXT_CACHE_CAPACITY,
            "the cache must never grow past its named capacity"
        );
        assert!(
            !entries.contains_key("key-0"),
            "the oldest entry must be the one dropped"
        );
        assert!(
            entries.contains_key(&format!(
                "key-{}",
                crate::commands::routing_classification::CLASSIFICATION_TEXT_CACHE_CAPACITY + 4
            )),
            "the newest entry must survive"
        );

        let round_tripped = cache
            .lookup(&format!(
                "key-{}",
                crate::commands::routing_classification::CLASSIFICATION_TEXT_CACHE_CAPACITY + 4
            ))
            .expect("the newest entry round-trips");
        assert_eq!(round_tripped.classification(), Some(classification));
    }

    /// A file this build cannot parse — corrupt, or written by a build with
    /// a different vocabulary — reads as an empty cache, the same rule
    /// [`ClassificationStickyCache::load`] follows for its own file.
    #[test]
    fn an_unreadable_classification_cache_file_reads_as_empty() {
        let fixture = CliFixture::new();
        let cache = crate::commands::routing_classification::ClassificationTextCache::new(
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
                    crate::commands::routing_classification::ClassificationStickyCache::new(
                        &paths_a, &project_a,
                    )
                    .store(&record_a)
                })
            });
            let handle_b = std::thread::spawn(move || {
                tracing::dispatcher::with_default(&dispatch_b, || {
                    crate::commands::routing_classification::ClassificationStickyCache::new(
                        &paths_b, &project_b,
                    )
                    .store(&record_b)
                })
            });
            handle_a.join().expect("thread a");
            handle_b.join().expect("thread b");

            let cache = crate::commands::routing_classification::ClassificationStickyCache::new(
                &paths,
                &project_id,
            );
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
                    crate::commands::routing_classification::ClassificationTextCache::new(
                        &paths_a, &project_a,
                    )
                    .store(record_a)
                })
            });
            let handle_b = std::thread::spawn(move || {
                tracing::dispatcher::with_default(&dispatch_b, || {
                    crate::commands::routing_classification::ClassificationTextCache::new(
                        &paths_b, &project_b,
                    )
                    .store(record_b)
                })
            });
            handle_a.join().expect("thread a");
            handle_b.join().expect("thread b");

            let cache = crate::commands::routing_classification::ClassificationTextCache::new(
                &paths,
                &project_id,
            );
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
