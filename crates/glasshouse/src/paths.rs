//! Runtime path resolution.
//!
//! Every runtime location Glasshouse uses is resolvable from an explicit
//! override, so the binary can be run from a user-owned tools directory with no
//! package-manager installation and no fixed system paths.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Environment variable overriding the per-user application-data location.
pub const ENV_DATA_DIR: &str = "GLASSHOUSE_DATA_DIR";
/// Environment variable overriding the per-user configuration location.
pub const ENV_CONFIG_DIR: &str = "GLASSHOUSE_CONFIG_DIR";

/// Resolved user-level Glasshouse locations.
///
/// These are *user* scoped. Everything project scoped hangs off
/// [`RuntimePaths::project_state_dir`] and is keyed by a project identifier, so
/// two projects can never share a state directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    data_dir: PathBuf,
    config_dir: PathBuf,
}

impl RuntimePaths {
    /// Resolve runtime paths from explicit overrides, then environment
    /// variables, then the operating system's conventional per-user locations.
    pub fn resolve(data_override: Option<&Path>, config_override: Option<&Path>) -> Result<Self> {
        let dirs = directories::ProjectDirs::from("", "", "glasshouse");

        let data_dir = match data_override {
            Some(p) => p.to_path_buf(),
            None => match std::env::var_os(ENV_DATA_DIR) {
                Some(v) if !v.is_empty() => PathBuf::from(v),
                _ => dirs.as_ref().map(|d| d.data_dir().to_path_buf()).context(
                    "could not determine a per-user application-data directory; \
                         set GLASSHOUSE_DATA_DIR or pass --data-dir",
                )?,
            },
        };

        let config_dir = match config_override {
            Some(p) => p.to_path_buf(),
            None => match std::env::var_os(ENV_CONFIG_DIR) {
                Some(v) if !v.is_empty() => PathBuf::from(v),
                _ => dirs
                    .as_ref()
                    .map(|d| d.config_dir().to_path_buf())
                    .context(
                        "could not determine a per-user configuration directory; \
                         set GLASSHOUSE_CONFIG_DIR or pass --config-dir",
                    )?,
            },
        };

        Ok(Self {
            data_dir,
            config_dir,
        })
    }

    /// Build runtime paths directly from two directories. Intended for tests
    /// and portable installations.
    pub fn new(data_dir: impl Into<PathBuf>, config_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            config_dir: config_dir.into(),
        }
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    /// Root directory holding all per-project state directories.
    pub fn projects_dir(&self) -> PathBuf {
        self.data_dir.join("projects")
    }

    /// State directory for one project identifier.
    ///
    /// Each project gets a physically separate directory; nothing about a
    /// project's state lives in a shared file.
    pub fn project_state_dir(&self, project_id: &str) -> PathBuf {
        self.projects_dir().join(project_id)
    }

    /// User-level configuration file.
    pub fn user_config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    /// Where discovered provider metadata is cached between runs.
    ///
    /// **Under the data directory, deliberately not the configuration one.**
    /// A discovered model catalogue is not configuration: the user did not
    /// type it, it carries a provenance and an age, and Glasshouse rewrites
    /// it on its own when asked to refresh. Putting it beside
    /// [`RuntimePaths::user_config_file`] would mean four hundred model
    /// identifiers and a machine-written timestamp in a file whose whole
    /// purpose is to record decisions a person made. See
    /// [`mod@crate::provider::cache`], which is the only thing that reads or
    /// writes in here.
    ///
    /// User scoped rather than project scoped, like everything else on this
    /// type: a provider's model list is a property of the service, not of the
    /// repository someone happens to be standing in, and two projects using
    /// the same provider would otherwise each pay for their own fetch.
    pub fn provider_cache_dir(&self) -> PathBuf {
        self.data_dir.join("providers")
    }
}
