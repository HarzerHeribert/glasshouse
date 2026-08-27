//! Command-line surface.
//!
//! Bare `glasshouse` operates on the current project. Every option here is
//! global because Glasshouse is project scoped: the project must be resolved
//! before any subcommand can do anything.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

const AFTER_HELP: &str = "\
ENVIRONMENT:
  GLASSHOUSE_DATA_DIR    Override the per-user application-data directory.
  GLASSHOUSE_CONFIG_DIR  Override the per-user configuration directory.
  GLASSHOUSE_LOG         Enable logging with a tracing filter, e.g. `debug`.

PROJECT SCOPE:
  Glasshouse operates on exactly one project root. The root is the containing
  Git repository when there is one, otherwise the current directory. All state,
  sessions, and memory are isolated per project root.
";

#[derive(Debug, Parser)]
#[command(
    name = "glasshouse",
    version,
    about = "A lean, project-scoped control plane for native coding-agent harnesses.",
    after_help = AFTER_HELP,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Select the project root explicitly instead of discovering it from Git.
    #[arg(long, value_name = "PATH", global = true)]
    pub scope: Option<PathBuf>,

    /// Permit a project root that Glasshouse would normally refuse.
    #[arg(long, global = true)]
    pub allow_unsafe_scope: bool,

    /// Override the per-user application-data directory.
    #[arg(long, value_name = "PATH", global = true)]
    pub data_dir: Option<PathBuf>,

    /// Override the per-user configuration directory.
    #[arg(long, value_name = "PATH", global = true)]
    pub config_dir: Option<PathBuf>,

    /// Enable logging at a tracing filter level, e.g. `info` or `glasshouse=debug`.
    #[arg(long, value_name = "FILTER", global = true)]
    pub log_level: Option<String>,

    /// Write logs to this file instead of the project log file.
    #[arg(long, value_name = "PATH", global = true)]
    pub log_file: Option<PathBuf>,

    /// Write logs to stderr. Not usable while the interactive TUI is running.
    #[arg(long, global = true, conflicts_with = "log_file")]
    pub log_stderr: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Non-interactive commands.
///
/// Every one of these is project scoped: it operates on the project resolved
/// from the working directory or `--scope`, never across projects.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Report detected harnesses, optional integrations, and setup problems.
    Doctor,
    /// Reopen the first-run setup wizard.
    ///
    /// Setup runs by itself the first time Glasshouse is used in an
    /// interactive terminal; this is how to revisit those choices later.
    Setup,
    /// Report what Glasshouse believes about each harness-and-model pairing.
    ///
    /// Who publishes a harness, who developed a model, and who serves that
    /// model are three different questions, and a router that ran them
    /// together would treat a reseller as an author. This prints them apart,
    /// with the class each pairing falls into and the evidence behind it.
    ///
    /// Anything Glasshouse has not read stays `unknown` — a model named
    /// after a company is not evidence it was made there. Correct one with a
    /// `[pairing.models."<model id>"]` table in the configuration file; the
    /// next run reflects it, and no router code changes.
    Pairing {
        /// Ask about one model by its exact id, whether or not any launch
        /// profile names it.
        #[arg(long, value_name = "ID")]
        model: Option<String>,

        /// Narrow `--model` to one harness, by its identifier — for example
        /// `claude-code`.
        #[arg(long, value_name = "ID")]
        harness: Option<String>,
    },
    /// Report the response profile in effect and what each harness would do
    /// with it.
    ///
    /// A response profile governs how an answer reads — its verbosity,
    /// audience, progress narration, evidence presentation and final-answer
    /// format — and nothing else. It cannot change reasoning depth,
    /// diligence, validation, permissions, safety or tool use, and no profile
    /// can use concision to reduce what is reported.
    ///
    /// This prints the resolved profile with the precedence layer each of the
    /// five axes came from, the reports no profile may drop, and the native
    /// mechanism, additive instruction or refusal each harness would answer
    /// with. Record one with a `[response]` table in the configuration file;
    /// the next run reflects it.
    ///
    /// Deliberately not called `profile`: `--profile` already names a
    /// *launch* profile, and the two are separate things that a shared name
    /// would collapse.
    Response {
        /// Resolve for one role — `orchestrator`, `worker`, `reviewer`,
        /// `explainer`, or `interactive`. Absent means `interactive`.
        #[arg(long, value_name = "ROLE")]
        role: Option<String>,

        /// A preset asked for at the session layer, as `glasshouse launch
        /// --response-profile` would.
        #[arg(long, value_name = "NAME")]
        session: Option<String>,

        /// Override one axis for this task, above every other layer.
        #[arg(long, value_name = "VALUE")]
        verbosity: Option<String>,

        /// Override the intended audience for this task.
        #[arg(long, value_name = "VALUE")]
        audience: Option<String>,

        /// Override progress narration for this task.
        #[arg(long, value_name = "VALUE")]
        narration: Option<String>,

        /// Override evidence presentation for this task.
        #[arg(long, value_name = "VALUE")]
        evidence: Option<String>,

        /// Override the final-answer format for this task.
        #[arg(long, value_name = "VALUE")]
        format: Option<String>,
    },
    /// List the sessions Glasshouse has recorded for this project, or act on
    /// one of them.
    ///
    /// Glasshouse keeps its own record of every session it starts, separate
    /// from whatever session files the harness writes for itself, so the list
    /// is the same whether or not a harness kept its own history.
    ///
    /// With no subcommand this prints the list, which is what it has always
    /// done. `show` prints everything one session recorded; `rename`, `tag`
    /// and `close` are the three things a person can change about a record,
    /// and none of them touches the harness's own history.
    Sessions {
        #[command(subcommand)]
        command: Option<SessionCommand>,
    },
    /// Search this project's durable memory.
    ///
    /// Memory is project-scoped: this reads the database belonging to the
    /// project you are standing in, and there is no way to ask it for
    /// another project's knowledge.
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    /// Take, list, or read a portable session checkpoint.
    ///
    /// A checkpoint is what one session hands the next when work has to
    /// move: the objective, where it got to, what was already ruled out, and
    /// what to do next. It is deliberately small, it is project-scoped, and
    /// it is kept apart from this project's durable memory.
    ///
    /// Glasshouse fills in the session, the harness, the timestamp and the
    /// Git position by itself. It does not invent the objective or the state
    /// — those are yours, and a checkpoint whose objective Glasshouse had
    /// guessed would be worse than no checkpoint at all.
    Checkpoint {
        #[command(subcommand)]
        command: CheckpointCommand,
    },
    /// Report a harness lifecycle event. Run by harnesses, not by people.
    ///
    /// Glasshouse installs hooks that invoke this command, so a session's
    /// state comes from the harness saying what happened rather than from
    /// Glasshouse reading its terminal and guessing.
    #[command(hide = true)]
    Hook {
        /// The Glasshouse session the event belongs to.
        #[arg(long)]
        session: String,

        /// The harness's own name for the event.
        #[arg(long)]
        event: String,
    },
    /// Resume a recorded session in its own harness, inside this project.
    ///
    /// Only a session this project recorded, and only one that has something
    /// to resume to: a harness that never produced an identifier, or one that
    /// is still running, is refused rather than reopened as something blank.
    Resume {
        /// Which session, by the identifier `glasshouse sessions` prints.
        ///
        /// The listing shows the first twelve characters, and that short form
        /// is enough — any leading part of an identifier works, as long as it
        /// picks out exactly one session.
        session: String,

        /// Arguments passed straight through to the harness, after `--`.
        #[arg(last = true, allow_hyphen_values = true)]
        harness_args: Vec<String>,
    },
    /// Open a session in an installed harness, inside this project.
    ///
    /// The harness runs in a pseudo-terminal whose working directory is this
    /// project's root, attached directly to the current terminal: its own
    /// interface, its own key bindings, its own session. Glasshouse starts it
    /// and stays out of the way.
    Launch {
        /// Which harness to open, by its identifier — for example
        /// `claude-code`, `codex`, or `opencode`.
        ///
        /// Optional when exactly one harness is enabled. With several
        /// enabled, Glasshouse asks rather than guessing.
        harness: Option<String>,

        /// Open the session with this response profile, by preset name.
        ///
        /// This is the session layer of the response-profile precedence
        /// chain: it wins over the role's default, this project's
        /// configuration and your user default, and loses to nothing but a
        /// task override. `glasshouse response --session <name>` prints
        /// exactly what a session started this way would resolve to.
        #[arg(long, value_name = "NAME")]
        response_profile: Option<String>,

        /// Open the session in this role, for response-profile purposes —
        /// `orchestrator`, `worker`, `reviewer`, `explainer`, or
        /// `interactive`.
        ///
        /// A spawned worker never inherits a communication style from
        /// whatever started it: its profile is resolved here, explicitly, and
        /// recorded in the launch log.
        #[arg(long, value_name = "ROLE")]
        response_role: Option<String>,

        /// Which launch profile to resolve the session through.
        ///
        /// Names a profile configured in `.glasshouse/config.toml` or the
        /// user-level configuration file — see `glasshouse setup`. Absent
        /// means the selected harness's implied Native profile, which uses
        /// the harness's own first-party authentication and configuration
        /// unchanged.
        #[arg(long, value_name = "NAME")]
        profile: Option<String>,

        /// Start the harness with a stored checkpoint's handoff as its
        /// opening prompt.
        ///
        /// Takes the identifier `glasshouse checkpoint list` prints, or
        /// `latest` for the most recent one in this project. The prompt is
        /// plain text that names no harness, so a checkpoint written while
        /// one harness was running can start the work in another.
        ///
        /// It is appended as the harness's own trailing argument, which is
        /// exactly what typing the prompt after `--` would do — so a harness
        /// that does not take an opening prompt reports that itself, in its
        /// own words, rather than having Glasshouse guess on its behalf.
        /// Your own `--` arguments still come after it and still win.
        #[arg(long, value_name = "ID")]
        from_checkpoint: Option<String>,

        /// Run the session with no terminal of its own.
        ///
        /// The harness still runs in a real pseudo-terminal, inside this
        /// project's root, with its output captured — it simply never takes
        /// over the terminal you started it from, and Glasshouse records it
        /// as a headless session. Useful for a harness given its whole task
        /// on the command line, and for starting one from a terminal you
        /// want to keep.
        ///
        /// Glasshouse stays in the foreground until the harness exits: there
        /// is no daemon behind this, and a session whose parent went away
        /// would lose the pseudo-terminal it is reading from.
        #[arg(long)]
        headless: bool,

        /// Arguments passed straight through to the harness, after `--`.
        ///
        /// Glasshouse does not interpret these; `glasshouse launch
        /// claude-code -- --resume` starts the harness with `--resume`.
        #[arg(last = true, allow_hyphen_values = true)]
        harness_args: Vec<String>,
    },
    /// Open a session exactly like `launch`, under the name a generated shim
    /// expects.
    ///
    /// This is not a second launch path: it dispatches through the same
    /// code as `launch`, so the two can never come to behave differently. It
    /// exists only because a shim (`glasshouse shim`) needs a stable
    /// subcommand to `exec` into.
    Run {
        /// Which harness to open, by its identifier — for example
        /// `claude-code`, `codex`, or `opencode`.
        ///
        /// Optional when exactly one harness is enabled. With several
        /// enabled, Glasshouse asks rather than guessing.
        harness: Option<String>,

        /// Open the session with this response profile, by preset name.
        ///
        /// This is the session layer of the response-profile precedence
        /// chain: it wins over the role's default, this project's
        /// configuration and your user default, and loses to nothing but a
        /// task override. `glasshouse response --session <name>` prints
        /// exactly what a session started this way would resolve to.
        #[arg(long, value_name = "NAME")]
        response_profile: Option<String>,

        /// Open the session in this role, for response-profile purposes —
        /// `orchestrator`, `worker`, `reviewer`, `explainer`, or
        /// `interactive`.
        ///
        /// A spawned worker never inherits a communication style from
        /// whatever started it: its profile is resolved here, explicitly, and
        /// recorded in the launch log.
        #[arg(long, value_name = "ROLE")]
        response_role: Option<String>,

        /// Which launch profile to resolve the session through.
        ///
        /// Names a profile configured in `.glasshouse/config.toml` or the
        /// user-level configuration file — see `glasshouse setup`. Absent
        /// means the selected harness's implied Native profile, which uses
        /// the harness's own first-party authentication and configuration
        /// unchanged.
        #[arg(long, value_name = "NAME")]
        profile: Option<String>,

        /// Start the harness with a stored checkpoint's handoff as its
        /// opening prompt.
        ///
        /// Takes the identifier `glasshouse checkpoint list` prints, or
        /// `latest` for the most recent one in this project. The prompt is
        /// plain text that names no harness, so a checkpoint written while
        /// one harness was running can start the work in another.
        ///
        /// It is appended as the harness's own trailing argument, which is
        /// exactly what typing the prompt after `--` would do — so a harness
        /// that does not take an opening prompt reports that itself, in its
        /// own words, rather than having Glasshouse guess on its behalf.
        /// Your own `--` arguments still come after it and still win.
        #[arg(long, value_name = "ID")]
        from_checkpoint: Option<String>,

        /// Run the session with no terminal of its own.
        ///
        /// The harness still runs in a real pseudo-terminal, inside this
        /// project's root, with its output captured — it simply never takes
        /// over the terminal you started it from, and Glasshouse records it
        /// as a headless session. Useful for a harness given its whole task
        /// on the command line, and for starting one from a terminal you
        /// want to keep.
        ///
        /// Glasshouse stays in the foreground until the harness exits: there
        /// is no daemon behind this, and a session whose parent went away
        /// would lose the pseudo-terminal it is reading from.
        #[arg(long)]
        headless: bool,

        /// Arguments passed straight through to the harness, after `--`.
        ///
        /// Glasshouse does not interpret these; `glasshouse run claude-code
        /// -- --resume` starts the harness with `--resume`.
        #[arg(last = true, allow_hyphen_values = true)]
        harness_args: Vec<String>,
    },
    /// Generate a small executable that opens a harness through a launch
    /// profile.
    ///
    /// Writes exactly one file to `--dir`, which is required: there is no
    /// default system-wide location and no `PATH` guessing. The file's
    /// entire job is to `exec` `glasshouse run <harness> --profile <name>`,
    /// forwarding its own arguments — it names no secret, no base URL, and
    /// copies no profile, only the harness name, the profile name, and this
    /// executable's own path.
    ///
    /// Deleting the generated file is all it takes to remove it. Glasshouse
    /// never writes to a shell startup file to make it reachable on `PATH`;
    /// if the chosen directory is not already there, that is left for the
    /// user to decide.
    Shim {
        /// Which harness the shim opens, by its identifier.
        harness: String,

        /// Which launch profile the shim resolves the session through.
        #[arg(long, value_name = "NAME")]
        profile: String,

        /// Directory to write the shim into. Required: there is no default.
        #[arg(long, value_name = "PATH")]
        dir: PathBuf,

        /// File name for the shim. Defaults to the harness name (`.cmd` on
        /// Windows).
        #[arg(long, value_name = "FILE")]
        name: Option<String>,

        /// Overwrite a file already at the destination.
        #[arg(long)]
        force: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn version_is_the_crate_version() {
        assert_eq!(
            Cli::command().get_version(),
            Some(env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn parses_scope_and_logging_options() {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--scope",
            "/tmp/p",
            "--log-level",
            "debug",
            "--log-stderr",
        ])
        .unwrap();
        assert_eq!(cli.scope, Some(PathBuf::from("/tmp/p")));
        assert_eq!(cli.log_level.as_deref(), Some("debug"));
        assert!(cli.log_stderr);
    }

    #[test]
    fn parses_launch_with_a_named_harness_and_passthrough_arguments() {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "launch",
            "claude-code",
            "--",
            "--resume",
            "--model=x",
        ])
        .unwrap();
        let Some(Command::Launch {
            harness,
            response_profile,
            response_role,
            profile,
            from_checkpoint,
            headless,
            harness_args,
        }) = cli.command
        else {
            panic!("expected a launch command");
        };
        assert_eq!(harness.as_deref(), Some("claude-code"));
        assert_eq!(profile, None);
        // Opt-in like every other launch flag: a launch that names no
        // response profile and no role leaves the harness's own
        // communication behaviour untouched.
        assert_eq!(response_profile, None);
        assert_eq!(response_role, None);
        // Opt-in, like `--headless`: a launch that does not name a checkpoint
        // is the plain launch it has always been.
        assert_eq!(from_checkpoint, None);
        // A session takes the terminal unless it is explicitly told not to:
        // the flag is opt-in, so a launch that does not name it is the
        // attached one it has always been.
        assert!(!headless);
        // Hyphenated arguments after `--` reach the harness untouched rather
        // than being parsed as Glasshouse options.
        assert_eq!(harness_args, vec!["--resume", "--model=x"]);
    }

    #[test]
    fn launch_without_a_harness_name_is_allowed() {
        let cli = Cli::try_parse_from(["glasshouse", "launch"]).unwrap();
        let Some(Command::Launch {
            harness,
            profile,
            harness_args,
            ..
        }) = cli.command
        else {
            panic!("expected a launch command");
        };
        assert_eq!(harness, None);
        assert_eq!(profile, None);
        assert!(harness_args.is_empty());
    }

    #[test]
    fn parses_launch_with_an_explicit_profile() {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "launch",
            "claude-code",
            "--profile",
            "fast",
            "--",
            "--resume",
        ])
        .unwrap();
        let Some(Command::Launch {
            harness, profile, ..
        }) = cli.command
        else {
            panic!("expected a launch command");
        };
        assert_eq!(harness.as_deref(), Some("claude-code"));
        assert_eq!(profile.as_deref(), Some("fast"));
    }

    #[test]
    fn log_file_and_log_stderr_conflict() {
        assert!(
            Cli::try_parse_from(["glasshouse", "--log-file", "a.log", "--log-stderr"]).is_err()
        );
    }

    // --- `run` parses the same shape as `launch` --------------------------

    #[test]
    fn parses_run_with_a_named_harness_and_passthrough_arguments() {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "run",
            "claude-code",
            "--",
            "--resume",
            "--model=x",
        ])
        .unwrap();
        let Some(Command::Run {
            harness,
            profile,
            harness_args,
            ..
        }) = cli.command
        else {
            panic!("expected a run command");
        };
        assert_eq!(harness.as_deref(), Some("claude-code"));
        assert_eq!(profile, None);
        assert_eq!(harness_args, vec!["--resume", "--model=x"]);
    }

    #[test]
    fn run_without_a_harness_name_is_allowed() {
        let cli = Cli::try_parse_from(["glasshouse", "run"]).unwrap();
        let Some(Command::Run {
            harness,
            profile,
            harness_args,
            ..
        }) = cli.command
        else {
            panic!("expected a run command");
        };
        assert_eq!(harness, None);
        assert_eq!(profile, None);
        assert!(harness_args.is_empty());
    }

    /// `glasshouse run` exists so a generated shim has a stable name to
    /// `exec` into, and Phase 9B's guarantee is that it behaves exactly like
    /// `launch` — proved here at the point both commands are parsed into
    /// their fields, and in `main.rs` at the point those fields are
    /// dispatched (see `glasshouse_run_and_glasshouse_launch_take_the_same_path`).
    #[test]
    fn a_profile_behaves_identically_from_run_and_from_launch() {
        let run = Cli::try_parse_from([
            "glasshouse",
            "run",
            "claude-code",
            "--profile",
            "fast",
            "--",
            "--resume",
            "--model=x",
        ])
        .unwrap();
        let launch = Cli::try_parse_from([
            "glasshouse",
            "launch",
            "claude-code",
            "--profile",
            "fast",
            "--",
            "--resume",
            "--model=x",
        ])
        .unwrap();

        let Some(Command::Run {
            harness: run_harness,
            profile: run_profile,
            harness_args: run_args,
            ..
        }) = run.command
        else {
            panic!("expected a run command");
        };
        let Some(Command::Launch {
            harness: launch_harness,
            profile: launch_profile,
            harness_args: launch_args,
            ..
        }) = launch.command
        else {
            panic!("expected a launch command");
        };

        assert_eq!(run_harness, launch_harness);
        assert_eq!(run_profile, launch_profile);
        assert_eq!(run_args, launch_args);
        // The user's trailing arguments stay last, identically, from both
        // entry points.
        assert_eq!(run_args, vec!["--resume", "--model=x"]);
    }

    // --- `shim` --------------------------------------------------------

    #[test]
    fn parses_shim_with_required_and_optional_flags() {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "shim",
            "claude-code",
            "--profile",
            "fast",
            "--dir",
            "/tmp/tools",
        ])
        .unwrap();
        let Some(Command::Shim {
            harness,
            profile,
            dir,
            name,
            force,
        }) = cli.command
        else {
            panic!("expected a shim command");
        };
        assert_eq!(harness, "claude-code");
        assert_eq!(profile, "fast");
        assert_eq!(dir, PathBuf::from("/tmp/tools"));
        assert_eq!(name, None);
        assert!(!force);
    }

    #[test]
    fn shim_accepts_a_custom_name_and_force() {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "shim",
            "claude-code",
            "--profile",
            "fast",
            "--dir",
            "/tmp/tools",
            "--name",
            "claude",
            "--force",
        ])
        .unwrap();
        let Some(Command::Shim { name, force, .. }) = cli.command else {
            panic!("expected a shim command");
        };
        assert_eq!(name.as_deref(), Some("claude"));
        assert!(force);
    }

    #[test]
    fn shim_requires_a_profile_and_a_dir() {
        assert!(Cli::try_parse_from(["glasshouse", "shim", "claude-code"]).is_err());
        assert!(
            Cli::try_parse_from(["glasshouse", "shim", "claude-code", "--profile", "fast"])
                .is_err()
        );
    }
}

/// What to do with this project's session checkpoints.
#[derive(Debug, Subcommand)]
pub enum CheckpointCommand {
    /// Record a checkpoint for a session.
    ///
    /// With no `--session`, the project's most recently active session — the
    /// one `glasshouse sessions` prints first, which is what "the active
    /// session" means outside the interactive interface.
    Save {
        /// What this work is trying to achieve.
        #[arg(long, value_name = "TEXT")]
        objective: String,

        /// Where it has got to.
        #[arg(long, value_name = "TEXT")]
        state: String,

        /// Which session, by the identifier `glasshouse sessions` prints.
        #[arg(long, value_name = "ID")]
        session: Option<String>,

        /// A decision discovered during this task. Repeatable.
        #[arg(long = "decision", value_name = "TEXT")]
        decisions: Vec<String>,

        /// An approach already tried and abandoned. Repeatable.
        #[arg(long = "failed", value_name = "TEXT")]
        failed_approaches: Vec<String>,

        /// A file or symbol that matters to this work. Repeatable.
        #[arg(long = "file", value_name = "TEXT")]
        files: Vec<String>,

        /// What the tests currently say.
        #[arg(long, value_name = "TEXT")]
        tests: Option<String>,

        /// What to do next. Repeatable, in order.
        #[arg(long = "next", value_name = "TEXT")]
        next_actions: Vec<String>,
    },
    /// List this project's checkpoints, most recent first.
    List,
    /// Print a checkpoint.
    ///
    /// By default the plain-text handoff a fresh session in any harness can
    /// be given; `--document` prints the portable JSON instead.
    Show {
        /// Which checkpoint, by the identifier `list` prints. Omit for the
        /// most recent one in this project.
        checkpoint: Option<String>,

        /// Print the portable document rather than the handoff prompt.
        #[arg(long)]
        document: bool,
    },
}

/// What to do with this project's durable memory.
///
/// A subcommand rather than a bare `glasshouse memory <query>` because Phase
/// 48 names the command `glasshouse memory search <query>`, and because
/// memory will grow operations that are not searches.
/// Things a person can do to one recorded session.
///
/// Three of the four change something, which is why they are subcommands
/// rather than flags on the listing: renaming, tagging and closing are user
/// actions on a record, and a report is not the place to hide one.
#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    /// Print everything Glasshouse recorded about one session.
    ///
    /// The harness, the launch profile, the backend resource, the model, the
    /// pairing class, the wire protocol and the response profile are printed
    /// as seven separate answers, because they are seven separate facts —
    /// collapsing them into one "agent" line is what the session model exists
    /// to prevent.
    Show {
        /// The session, or the leading part of its identifier.
        session: String,
    },

    /// Give a session a name of your own.
    ///
    /// The name is Glasshouse's own label. It never changes the harness's
    /// native session identifier, which is what a resume continues from, and
    /// it never changes the Glasshouse session identifier either.
    Rename {
        /// The session, or the leading part of its identifier.
        session: String,

        /// The new name. Omit it with `--clear` to remove one.
        #[arg(value_name = "NAME", required_unless_present = "clear")]
        name: Option<String>,

        /// Remove the session's name instead of setting one.
        #[arg(long, conflicts_with = "name")]
        clear: bool,
    },

    /// Tag a session with a lightweight purpose, such as `auth`, `tests`, or
    /// `research`.
    ///
    /// Free text: the three above are examples, not a list to choose from.
    Tag {
        /// The session, or the leading part of its identifier.
        session: String,

        /// The purpose. Omit it with `--clear` to remove one.
        #[arg(value_name = "PURPOSE", required_unless_present = "clear")]
        purpose: Option<String>,

        /// Remove the session's purpose instead of setting one.
        #[arg(long, conflicts_with = "purpose")]
        clear: bool,
    },

    /// Retire Glasshouse's record of a session.
    ///
    /// This closes Glasshouse's own record and nothing else. The harness's
    /// session files are not read, not moved and not deleted — Glasshouse
    /// does not own them — and the native session identifier stays recorded,
    /// so the history remains findable afterwards. A session that is still
    /// running is refused rather than closed underneath itself.
    Close {
        /// The session, or the leading part of its identifier.
        session: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum MemoryCommand {
    /// Find memories matching free-form text.
    ///
    /// The text is not a query language: it is matched against every
    /// memory's subject and body, best match first.
    Search {
        /// Free-form text to look for.
        query: Vec<String>,

        /// Include superseded, rejected, resolved, invalidated,
        /// needs-review and conflicted memories.
        ///
        /// Off by default: current project knowledge is what a search
        /// normally means, and history is an explicit ask.
        #[arg(long)]
        history: bool,

        /// Most results to print.
        #[arg(long, value_name = "N",
              default_value_t = crate::memory::search::DEFAULT_SEARCH_LIMIT)]
        limit: usize,
    },

    /// Promote or demote a memory's authority class.
    ///
    /// The only way to create an `invariant`: automatic extraction is capped
    /// at `constraint`, because the only certainty it has access to is a
    /// model's report of its own confidence, and Phase 21K requires that be
    /// treated as presentation rather than evidence.
    Promote {
        /// The memory, or the leading part of its identifier.
        id: String,

        /// The class to set, or `unclassified` to clear it.
        #[arg(value_name = "AUTHORITY")]
        authority: String,
    },

    /// Run memory extraction over a session's recorded activity.
    ///
    /// `--reply-from` supplies a model's reply from a file, which is what
    /// makes this usable before Phase 39 exists: everything except the model
    /// call runs for real. It is an evaluation harness, not a model call, and
    /// the output says so on every run.
    Extract {
        /// The session the activity belongs to, or the leading part of its
        /// identifier when reading from the event log.
        #[arg(long)]
        session: String,

        /// A file holding the activity, one entry per line.
        ///
        /// Mutually exclusive with `--from-events`, and one of the two is
        /// required: extraction is never run over activity nobody chose.
        #[arg(
            long,
            value_name = "PATH",
            conflicts_with = "from_events",
            required_unless_present = "from_events"
        )]
        activity: Option<std::path::PathBuf>,

        /// Read the activity from this session's own recorded lifecycle
        /// events instead of from a file.
        ///
        /// This is the same material automatic extraction reads after a
        /// completed turn, and the memories it produces carry the range of
        /// the event log they came from. It carries no conversation: the
        /// project event log has no column one could reach.
        #[arg(long)]
        from_events: bool,

        /// A file holding a model's reply, instead of calling a model.
        #[arg(long, value_name = "PATH")]
        reply_from: std::path::PathBuf,
    },
}
