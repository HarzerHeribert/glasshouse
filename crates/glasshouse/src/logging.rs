//! Structured application logging.
//!
//! Logging is off unless explicitly enabled, and when enabled it defaults to a
//! file so diagnostic output can never be interleaved into the interactive TUI.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

/// Environment variable enabling logging, e.g. `GLASSHOUSE_LOG=debug`.
pub const ENV_LOG: &str = "GLASSHOUSE_LOG";

/// Where log records are written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogSink {
    /// Logging is disabled.
    Disabled,
    /// Append to a file. Safe while the TUI owns the terminal.
    File(PathBuf),
    /// Write to stderr. Only valid for non-interactive commands.
    Stderr,
}

/// Resolved logging configuration.
#[derive(Debug, Clone)]
pub struct LogConfig {
    pub filter: String,
    pub sink: LogSink,
}

impl LogConfig {
    /// Resolve logging from CLI flags and the environment.
    ///
    /// `default_dir` is where a log file is placed when logging is enabled but
    /// no explicit path was given.
    pub fn resolve(
        level: Option<&str>,
        file: Option<&Path>,
        to_stderr: bool,
        default_dir: &Path,
    ) -> Self {
        let filter = level
            .map(str::to_owned)
            .or_else(|| std::env::var(ENV_LOG).ok().filter(|v| !v.is_empty()));

        let Some(filter) = filter else {
            // An explicit destination still implies the user wants logs.
            if file.is_some() || to_stderr {
                return Self {
                    filter: "info".to_owned(),
                    sink: sink_for(file, to_stderr, default_dir),
                };
            }
            return Self {
                filter: "off".to_owned(),
                sink: LogSink::Disabled,
            };
        };

        Self {
            filter,
            sink: sink_for(file, to_stderr, default_dir),
        }
    }
}

fn sink_for(file: Option<&Path>, to_stderr: bool, default_dir: &Path) -> LogSink {
    if let Some(path) = file {
        LogSink::File(path.to_path_buf())
    } else if to_stderr {
        LogSink::Stderr
    } else {
        LogSink::File(default_dir.join("glasshouse.log"))
    }
}

/// Install the global tracing subscriber.
///
/// Returns the log file path when logging to a file, so the CLI can tell the
/// user where diagnostics went.
pub fn init(config: &LogConfig) -> Result<Option<PathBuf>> {
    if config.sink == LogSink::Disabled {
        return Ok(None);
    }

    let env_filter = EnvFilter::try_new(&config.filter)
        .with_context(|| format!("invalid log filter `{}`", config.filter))?;

    let builder = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(true)
        .with_ansi(false);

    match &config.sink {
        LogSink::Disabled => unreachable!("handled above"),
        LogSink::Stderr => {
            builder
                .with_writer(std::io::stderr)
                .try_init()
                .map_err(|e| anyhow::anyhow!("could not install log subscriber: {e}"))?;
            Ok(None)
        }
        LogSink::File(path) => {
            let file = open_log_file(path)?;
            builder
                .with_writer(std::sync::Mutex::new(file))
                .try_init()
                .map_err(|e| anyhow::anyhow!("could not install log subscriber: {e}"))?;
            Ok(Some(path.clone()))
        }
    }
}

fn open_log_file(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create log directory `{}`", parent.display()))?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("could not open log file `{}`", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logging_is_off_by_default() {
        let config = LogConfig::resolve(None, None, false, Path::new("/tmp"));
        assert_eq!(config.sink, LogSink::Disabled);
    }

    #[test]
    fn a_level_enables_file_logging_by_default() {
        let config = LogConfig::resolve(Some("debug"), None, false, Path::new("/tmp/logs"));
        assert_eq!(config.filter, "debug");
        assert_eq!(
            config.sink,
            LogSink::File(PathBuf::from("/tmp/logs/glasshouse.log"))
        );
    }

    #[test]
    fn an_explicit_destination_implies_enabled_logging() {
        let config = LogConfig::resolve(None, Some(Path::new("/tmp/x.log")), false, Path::new("/"));
        assert_eq!(config.filter, "info");
        assert_eq!(config.sink, LogSink::File(PathBuf::from("/tmp/x.log")));

        let config = LogConfig::resolve(None, None, true, Path::new("/"));
        assert_eq!(config.sink, LogSink::Stderr);
    }
}
