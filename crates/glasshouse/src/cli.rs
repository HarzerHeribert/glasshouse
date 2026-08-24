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

        /// Arguments passed straight through to the harness, after `--`.
        ///
        /// Glasshouse does not interpret these; `glasshouse launch
        /// claude-code -- --resume` starts the harness with `--resume`.
        #[arg(last = true, allow_hyphen_values = true)]
        harness_args: Vec<String>,
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
            harness_args,
        }) = cli.command
        else {
            panic!("expected a launch command");
        };
        assert_eq!(harness.as_deref(), Some("claude-code"));
        // Hyphenated arguments after `--` reach the harness untouched rather
        // than being parsed as Glasshouse options.
        assert_eq!(harness_args, vec!["--resume", "--model=x"]);
    }

    #[test]
    fn launch_without_a_harness_name_is_allowed() {
        let cli = Cli::try_parse_from(["glasshouse", "launch"]).unwrap();
        let Some(Command::Launch {
            harness,
            harness_args,
        }) = cli.command
        else {
            panic!("expected a launch command");
        };
        assert_eq!(harness, None);
        assert!(harness_args.is_empty());
    }

    #[test]
    fn log_file_and_log_stderr_conflict() {
        assert!(
            Cli::try_parse_from(["glasshouse", "--log-file", "a.log", "--log-stderr"]).is_err()
        );
    }
}
