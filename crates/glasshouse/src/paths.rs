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
/// The **gateway's** own overrides, spelled here because [`RuntimePaths`]
/// must answer with the same locations the gateway binary would — see
/// `inference_gateway::config::default_data_dir`.
pub const ENV_GATEWAY_DATA_DIR: &str = "INFERENCE_GATEWAY_DATA_DIR";
pub const ENV_GATEWAY_CONFIG: &str = "INFERENCE_GATEWAY_CONFIG";
/// The gateway's configuration file name inside its configuration directory.
const GATEWAY_CONFIG_FILE: &str = "gateway.toml";

/// Resolved user-level Glasshouse locations.
///
/// These are *user* scoped. Everything project scoped hangs off
/// [`RuntimePaths::project_state_dir`] and is keyed by a project identifier, so
/// two projects can never share a state directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    data_dir: PathBuf,
    config_dir: PathBuf,
    /// The **gateway's** private state root — the one owner of every
    /// subscription broker, managed tool, model catalogue and quota/health
    /// cache since the 2026-09-11 ruling. Glasshouse resolves it so that it
    /// can *read* that state and hand a path to a broker it supervises; it
    /// never owns what lives there.
    gateway_data_dir: PathBuf,
    /// The gateway's own `gateway.toml` — where accounts and providers are
    /// configured. Glasshouse reads it as an input and never writes it
    /// outside `glasshouse migrate-gateway-state`.
    gateway_config_path: PathBuf,
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

        // **A relocated Glasshouse does not reach into the machine's default
        // gateway store.** `INFERENCE_GATEWAY_CONFIG`/`_DATA_DIR` win, because
        // they are the gateway's own overrides and a caller that set one meant
        // it. Failing those, a Glasshouse location that was itself overridden —
        // by a flag or by `GLASSHOUSE_*` — carries the gateway with it, exactly
        // as [`RuntimePaths::new`] does: a portable install in a tools
        // directory, and every test that spawns this binary with `--data-dir`,
        // must be self-contained or it reads and writes the developer's own
        // accounts. Only an install running from its platform locations
        // resolves the gateway's platform locations.
        let relocated = data_override.is_some()
            || config_override.is_some()
            || std::env::var_os(ENV_DATA_DIR).is_some_and(|v| !v.is_empty())
            || std::env::var_os(ENV_CONFIG_DIR).is_some_and(|v| !v.is_empty());
        let gateway_data_dir = match std::env::var_os(ENV_GATEWAY_DATA_DIR) {
            Some(v) if !v.is_empty() => PathBuf::from(v),
            _ if relocated => data_dir.join("gateway"),
            _ => inference_gateway::config::default_data_dir().context(
                "could not determine the inference gateway's data directory; \
                 set INFERENCE_GATEWAY_DATA_DIR",
            )?,
        };
        let gateway_config_path = match std::env::var_os(ENV_GATEWAY_CONFIG) {
            Some(v) if !v.is_empty() => PathBuf::from(v),
            _ if relocated => config_dir.join(GATEWAY_CONFIG_FILE),
            _ => inference_gateway::config::default_config_path().context(
                "could not determine the inference gateway's configuration file; \
                 set INFERENCE_GATEWAY_CONFIG",
            )?,
        };

        Ok(Self {
            data_dir,
            config_dir,
            gateway_data_dir,
            gateway_config_path,
        })
    }

    /// Build runtime paths directly from two directories. Intended for tests
    /// and portable installations.
    ///
    /// **The gateway's own locations are derived from these two, not from the
    /// platform.** A test that resolved the real
    /// `inference_gateway::config::default_data_dir()` here would read and
    /// write the developer's own accounts; deriving keeps a portable install
    /// and a test tree self-contained. `resolve` is the constructor that
    /// finds the gateway where the gateway itself puts it, and
    /// [`RuntimePaths::with_gateway`] is how a test names one explicitly —
    /// never by setting `INFERENCE_GATEWAY_*` inside an in-process test,
    /// because these tests run on parallel threads of one process.
    pub fn new(data_dir: impl Into<PathBuf>, config_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        let config_dir = config_dir.into();
        let gateway_data_dir = data_dir.join("gateway");
        let gateway_config_path = config_dir.join(GATEWAY_CONFIG_FILE);
        Self {
            data_dir,
            config_dir,
            gateway_data_dir,
            gateway_config_path,
        }
    }

    /// The same paths with the gateway's data directory and configuration
    /// file named explicitly — the override an in-process test uses instead
    /// of `INFERENCE_GATEWAY_DATA_DIR`/`INFERENCE_GATEWAY_CONFIG`.
    #[must_use]
    pub fn with_gateway(
        mut self,
        gateway_data_dir: impl Into<PathBuf>,
        gateway_config_path: impl Into<PathBuf>,
    ) -> Self {
        self.gateway_data_dir = gateway_data_dir.into();
        self.gateway_config_path = gateway_config_path.into();
        self
    }

    /// The gateway's private state root. Everything under it — brokers,
    /// managed tools, model catalogues, the quota and health caches — is the
    /// gateway's, and Glasshouse only reads it.
    pub fn gateway_data_dir(&self) -> &Path {
        &self.gateway_data_dir
    }

    /// The gateway's `gateway.toml`.
    pub fn gateway_config_path(&self) -> &Path {
        &self.gateway_config_path
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
    /// **The gateway's own cache, since the 2026-09-11 ruling**:
    /// `inference_gateway::config::model_cache_dir` under
    /// [`RuntimePaths::gateway_data_dir`], so one catalogue serves a
    /// standalone gateway and a Glasshouse-supervised one alike. Glasshouse
    /// reads it; it does not own it.
    ///
    /// **Under a data directory, deliberately not the configuration one.**
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
        inference_gateway::config::model_cache_dir(&self.gateway_data_dir)
    }

    /// The gateway's managed third-party executables.
    ///
    /// These are data rather than user-authored configuration, and since the
    /// 2026-09-11 ruling they are the **gateway's** data: the broker binary a
    /// subscription account runs is the gateway's to install and replace.
    pub fn managed_tools_dir(&self) -> PathBuf {
        self.gateway_data_dir.join("tools")
    }

    /// The managed CLIProxyAPI executable used by the subscription broker.
    pub fn cliproxyapi_executable(&self) -> PathBuf {
        let name = if cfg!(windows) {
            "CLIProxyAPI.exe"
        } else {
            "CLIProxyAPI"
        };
        let root = self.managed_tools_dir().join("cliproxyapi");
        let marker = root.join("current");
        if let Ok(version) = std::fs::read_to_string(&marker) {
            if valid_cliproxyapi_version(&version) {
                return root.join(version).join(if cfg!(windows) {
                    "cliproxyapi.exe"
                } else {
                    "cliproxyapi"
                });
            }
            return root.join(".invalid-current-marker");
        } else if marker.exists() {
            return root.join(".unreadable-current-marker");
        }
        self.managed_tools_dir().join(name)
    }

    /// Private state roots for the one-sidecar-per-entitlement broker, under
    /// the **gateway's** data directory — the layout
    /// `inference_gateway::config::broker_paths` reads, so a standalone
    /// gateway and a Glasshouse-supervised session find the same login.
    pub fn subscription_brokers_dir(&self) -> PathBuf {
        self.gateway_data_dir.join("subscription-brokers")
    }

    /// Stable, traversal-safe state root for one entitlement.
    ///
    /// Hex encoding preserves identity without putting user-controlled path
    /// separators into a filesystem path.
    pub fn subscription_broker_entitlement_dir(&self, entitlement: &str) -> PathBuf {
        self.subscription_brokers_dir().join(format!(
            "entitlement-{}",
            hex::encode(entitlement.as_bytes())
        ))
    }

    /// Stable OAuth state shared by subscription login and broker serving.
    pub fn subscription_broker_auth_dir(&self, entitlement: &str) -> PathBuf {
        self.subscription_broker_entitlement_dir(entitlement)
            .join("auth")
    }

    /// This layout expressed in the four paths one entitlement's broker
    /// needs.
    ///
    /// The translation lives here, not in `gateway`: the broker owns a
    /// sidecar's lifecycle and must stay usable by a host with no
    /// [`RuntimePaths`], so Glasshouse resolves the **gateway's** layout and
    /// hands the result over. The dependency points one way only.
    pub fn subscription_broker_paths(
        &self,
        entitlement: &str,
    ) -> crate::gateway::subscription_broker::BrokerPaths {
        crate::gateway::subscription_broker::BrokerPaths {
            brokers_dir: self.subscription_brokers_dir(),
            entitlement_dir: self.subscription_broker_entitlement_dir(entitlement),
            auth_dir: self.subscription_broker_auth_dir(entitlement),
            executable: self.cliproxyapi_executable(),
        }
    }
}

fn valid_cliproxyapi_version(version: &str) -> bool {
    version.strip_prefix("sha256-").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

/// Refuse a path whose first component is a literal `~`, rather than
/// silently creating a directory named `~`.
///
/// `~` only ever expands to the home directory inside a shell, and none of
/// this function's five callers run one, so a literal `~` is unambiguous
/// evidence of a shell-expansion step that never ran (a non-interactive
/// launcher such as a systemd unit or a CI job). Refusing matches this
/// codebase's stated preference for refusing untrusted input over guessing
/// what it meant. `path` is formatted with `{path:?}` rather than `{path}`:
/// on Unix a CLI argument or environment variable may contain any byte but
/// NUL, so an unescaped echo could inject a newline into whatever this error
/// is logged into.
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `reject_literal_tilde`.
pub(crate) fn reject_literal_tilde(path: &Path, source: &str) -> Result<PathBuf> {
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
    fn broker_paths_are_managed_and_entitlements_cannot_escape_their_root() {
        let paths = RuntimePaths::new("/private/data", "/private/config");
        assert_eq!(
            paths.gateway_data_dir(),
            Path::new("/private/data/gateway"),
            "a portable/test install keeps the gateway's state under its own data dir"
        );
        assert_eq!(
            paths.managed_tools_dir(),
            Path::new("/private/data/gateway/tools")
        );
        assert_eq!(
            paths.cliproxyapi_executable(),
            Path::new("/private/data/gateway/tools").join(if cfg!(windows) {
                "CLIProxyAPI.exe"
            } else {
                "CLIProxyAPI"
            })
        );
        assert_eq!(
            paths.provider_cache_dir(),
            Path::new("/private/data/gateway/model-catalogues"),
            "the model catalogue cache is the gateway's"
        );

        // The explicit override a test uses instead of the two
        // `INFERENCE_GATEWAY_*` environment variables.
        let elsewhere = paths
            .clone()
            .with_gateway("/other/gw-data", "/other/gw.toml");
        assert_eq!(elsewhere.gateway_data_dir(), Path::new("/other/gw-data"));
        assert_eq!(elsewhere.gateway_config_path(), Path::new("/other/gw.toml"));
        assert_eq!(
            elsewhere.subscription_brokers_dir(),
            Path::new("/other/gw-data/subscription-brokers")
        );

        let account = paths.subscription_broker_entitlement_dir("../../account-a");
        assert!(account.starts_with(paths.subscription_brokers_dir()));
        assert_eq!(
            account.parent(),
            Some(paths.subscription_brokers_dir().as_path())
        );
        assert!(!account.to_string_lossy().contains("../"));
    }

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
