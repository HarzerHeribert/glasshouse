//! Command-line surface.
//!
//! Bare `glasshouse` operates on the current project. Every option here is
//! global because Glasshouse is project scoped: the project must be resolved
//! before any subcommand can do anything.

use std::path::PathBuf;

use clap::Parser;

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
    fn log_file_and_log_stderr_conflict() {
        assert!(
            Cli::try_parse_from(["glasshouse", "--log-file", "a.log", "--log-stderr"]).is_err()
        );
    }
}
