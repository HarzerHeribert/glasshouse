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
            Some(p) => reject_literal_tilde(p, "--data-dir")?,
            None => match std::env::var_os(ENV_DATA_DIR) {
                Some(v) if !v.is_empty() => reject_literal_tilde(&PathBuf::from(v), ENV_DATA_DIR)?,
                _ => dirs.as_ref().map(|d| d.data_dir().to_path_buf()).context(
                    "could not determine a per-user application-data directory; \
                         set GLASSHOUSE_DATA_DIR or pass --data-dir",
                )?,
            },
        };

        let config_dir = match config_override {
            Some(p) => reject_literal_tilde(p, "--config-dir")?,
            None => match std::env::var_os(ENV_CONFIG_DIR) {
                Some(v) if !v.is_empty() => {
                    reject_literal_tilde(&PathBuf::from(v), ENV_CONFIG_DIR)?
                }
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

/// Refuse a path whose first component is a literal `~`, rather than
/// silently creating a directory named `~`.
///
/// `~` only ever expands to the home directory inside a shell. None of the
/// four callers of this function run one — `--data-dir`/`--config-dir` land
/// here as already-parsed [`Path`] arguments, and `GLASSHOUSE_DATA_DIR`/
/// `GLASSHOUSE_CONFIG_DIR` as raw environment strings — so a literal `~` is
/// unambiguous evidence of a shell-expansion step that never ran (a
/// non-interactive launcher such as a systemd unit, a CI job, or a cmux pane
/// setting the env var directly). Refusing costs no home-directory lookup
/// and matches this codebase's stated preference for refusing untrusted
/// input over guessing what it meant (see `shim::check_name`'s doc).
///
/// `source` names the flag or environment variable, so the error tells the
/// caller which of the two disagreed with what it received. `path` is
/// formatted with `{path:?}` (escaped, quoted) rather than `{path}` (raw):
/// on Unix, both a CLI argument and an environment variable may contain any
/// byte but NUL, so an unescaped echo could inject a newline into whatever
/// this error is logged into.
fn reject_literal_tilde(path: &Path, source: &str) -> Result<PathBuf> {
    if path.starts_with("~") {
        anyhow::bail!(
            "{source} is {path:?}, which starts with a literal `~`, not your home directory — \
             this argument reaches Glasshouse before any shell would expand it. Give an \
             absolute path, or a path relative to the current directory."
        );
    }
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_literal_tilde_flag_override_is_refused_for_data_and_config_dir() {
        let tilde = Path::new("~/nonexistent_gh_test");
        let elsewhere = Path::new("/tmp/glasshouse-paths-test-elsewhere");

        let err = RuntimePaths::resolve(Some(tilde), Some(elsewhere)).unwrap_err();
        assert!(
            err.to_string().contains("--data-dir"),
            "error should name --data-dir: {err}"
        );

        let err = RuntimePaths::resolve(Some(elsewhere), Some(tilde)).unwrap_err();
        assert!(
            err.to_string().contains("--config-dir"),
            "error should name --config-dir: {err}"
        );
    }

    #[test]
    fn a_bare_tilde_is_also_refused() {
        let elsewhere = Path::new("/tmp/glasshouse-paths-test-elsewhere");
        let err = RuntimePaths::resolve(Some(Path::new("~")), Some(elsewhere)).unwrap_err();
        assert!(
            err.to_string().contains("--data-dir"),
            "a bare `~` should be refused: {err}"
        );
    }

    #[test]
    fn a_literal_tilde_env_var_is_refused_for_data_and_config_dir() {
        // Environment variables are process-global state; both mutations and
        // their cleanup stay inside one test so no other #[test] in this
        // crate (which never sets these two vars itself — see the crate's
        // only production caller in `lib.rs`) can observe them mid-flight.
        let elsewhere = Path::new("/tmp/glasshouse-paths-test-elsewhere");

        // SAFETY: `ENV_DATA_DIR`/`ENV_CONFIG_DIR` are set and removed within
        // this single test, which never runs concurrently with itself.
        unsafe {
            std::env::set_var(ENV_DATA_DIR, "~/nonexistent_gh_test");
        }
        let result = RuntimePaths::resolve(None, Some(elsewhere));
        unsafe {
            std::env::remove_var(ENV_DATA_DIR);
        }
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains(ENV_DATA_DIR),
            "error should name {ENV_DATA_DIR}: {err}"
        );

        // SAFETY: see above.
        unsafe {
            std::env::set_var(ENV_CONFIG_DIR, "~/nonexistent_gh_test");
        }
        let result = RuntimePaths::resolve(Some(elsewhere), None);
        unsafe {
            std::env::remove_var(ENV_CONFIG_DIR);
        }
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains(ENV_CONFIG_DIR),
            "error should name {ENV_CONFIG_DIR}: {err}"
        );
    }

    /// The control case: an ordinary override is unaffected by the new
    /// check and resolves exactly as it did before this fix.
    #[test]
    fn an_ordinary_override_is_unaffected() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let config = tmp.path().join("config");

        let paths = RuntimePaths::resolve(Some(&data), Some(&config)).unwrap();

        assert_eq!(paths.data_dir(), data);
        assert_eq!(paths.config_dir(), config);
    }
}
