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

/// Size threshold, checked at open time, past which the previous log file is
/// rotated out of the way before a new one is opened.
const MAX_LOG_BYTES: u64 = 16 * 1024 * 1024;

fn open_log_file(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        create_dir_secure(parent)
            .with_context(|| format!("could not create log directory `{}`", parent.display()))?;
    }

    rotate_if_large(path);

    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // The log can carry diagnostic detail about the project and its
        // sessions; default file permissions would leave it world-readable.
        options.mode(0o600);
    }
    options
        .open(path)
        .with_context(|| format!("could not open log file `{}`", path.display()))
}

/// Create a directory restricted to its owner on Unix. Mirrors
/// `glasshouse::create_state_dir`: the log directory hangs off the project
/// state directory and should get the same treatment, not fall back to
/// default (typically world-readable) permissions.
#[cfg(unix)]
fn create_dir_secure(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_dir_secure(dir: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new().recursive(true).create(dir)
}

/// Rotate `path` to `path` + `.1` if it has grown past [`MAX_LOG_BYTES`].
///
/// One generation is enough for a diagnostic log nobody is expected to keep
/// long-term — a background thread or a time-based scheme would be more
/// machinery than the problem earns. Rotation only happens at open time, so
/// a single run that grows past the threshold keeps appending to one file;
/// the next start is what rotates it. A rename failure (permissions, a
/// concurrent process holding the `.1` path, ...) is not fatal: it is not
/// worth failing startup over, so logging just keeps appending to the
/// existing file instead.
fn rotate_if_large(path: &Path) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    if metadata.len() <= MAX_LOG_BYTES {
        return;
    }
    let mut rotated = path.as_os_str().to_owned();
    rotated.push(".1");
    let _ = std::fs::rename(path, PathBuf::from(rotated));
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

    #[test]
    fn a_small_log_file_is_not_rotated() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("glasshouse.log");
        std::fs::write(&path, b"small").unwrap();

        rotate_if_large(&path);

        assert!(path.exists());
        assert!(!tmp.path().join("glasshouse.log.1").exists());
    }

    #[test]
    fn a_log_file_past_the_threshold_is_rotated_on_open() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("glasshouse.log");
        std::fs::write(&path, vec![0u8; (MAX_LOG_BYTES + 1) as usize]).unwrap();
        let rotated = tmp.path().join("glasshouse.log.1");
        std::fs::write(&rotated, b"stale generation").unwrap();

        let file = open_log_file(&path).unwrap();
        drop(file);

        assert!(path.exists(), "a fresh log file must exist after rotation");
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            0,
            "the newly opened log file must start empty"
        );
        assert_eq!(
            std::fs::read(&rotated).unwrap().len(),
            (MAX_LOG_BYTES + 1) as usize,
            "rotation must replace any existing `.1` with the file just rotated"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_log_file_and_directory_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("logs");
        let path = dir.join("glasshouse.log");

        let file = open_log_file(&path).unwrap();
        let file_mode = file.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600, "log file mode was {file_mode:o}");

        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "log directory mode was {dir_mode:o}");
    }
}
