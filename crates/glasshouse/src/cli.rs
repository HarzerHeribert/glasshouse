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
    /// List the sessions Glasshouse has recorded for this project.
    ///
    /// Glasshouse keeps its own record of every session it starts, separate
    /// from whatever session files the harness writes for itself, so the list
    /// is the same whether or not a harness kept its own history.
    Sessions,
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

        /// Which launch profile to resolve the session through.
        ///
        /// Names a profile configured in `.glasshouse/config.toml` or the
        /// user-level configuration file — see `glasshouse setup`. Absent
        /// means the selected harness's implied Native profile, which uses
        /// the harness's own first-party authentication and configuration
        /// unchanged.
        #[arg(long, value_name = "NAME")]
        profile: Option<String>,

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

        /// Which launch profile to resolve the session through.
        ///
        /// Names a profile configured in `.glasshouse/config.toml` or the
        /// user-level configuration file — see `glasshouse setup`. Absent
        /// means the selected harness's implied Native profile, which uses
        /// the harness's own first-party authentication and configuration
        /// unchanged.
        #[arg(long, value_name = "NAME")]
        profile: Option<String>,

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
            profile,
            harness_args,
        }) = cli.command
        else {
            panic!("expected a launch command");
        };
        assert_eq!(harness.as_deref(), Some("claude-code"));
        assert_eq!(profile, None);
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
        }) = run.command
        else {
            panic!("expected a run command");
        };
        let Some(Command::Launch {
            harness: launch_harness,
            profile: launch_profile,
            harness_args: launch_args,
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
