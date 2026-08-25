//! User-level and optional project-level Glasshouse configuration.
//!
//! Two files, same small shape:
//!
//! - `<config_dir>/config.toml` — user-level. Onboarding decisions and
//!   per-integration enable/executable overrides. Loaded by every run;
//!   never created automatically for you to lose data to — a missing file
//!   just means the defaults apply (see [`load`]).
//! - `<project root>/.glasshouse/config.toml` — project-level, optional,
//!   and layered *over* the user file (see [`EffectiveConfig`]). It is
//!   never written except in response to an explicit user decision — see
//!   [`write_project_config_with_consent`].
//!
//! The schema is deliberately tiny. The capability map is explicit that
//! configuration should stay small until real usage demonstrates a need for
//! more (Phase 49): no provider, launch-profile, routing, or budget fields
//! belong here yet — those are later phases and would be speculative today.
//!
//! ## No secrets here — structurally, not just by convention
//!
//! [`IntegrationConfig`], the only per-item shape either file stores, has
//! exactly two fields: whether the user turned the integration on, and an
//! optional path to its executable. Nothing in [`UserConfig`] or
//! [`ProjectConfig`] can hold an API key, token, or any other credential —
//! there is no field capable of it. That is Phase 9E's rule applied here:
//! "Never write API keys into tracked `.glasshouse` project files" and
//! "Store only secret references in provider configuration whenever
//! possible." Provider credentials belong to the separate `SecretStore`
//! abstraction (not built by this module), never to this one. See
//! [`tests::serialized_form_has_no_secret_capable_field`] for a structural
//! guard, not just a string search.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::integrations::IntegrationId;
use crate::paths::RuntimePaths;
use crate::project::{Project, ScopeError};

/// Configuration schema version this build of Glasshouse writes and fully
/// understands. Bump this only when the schema changes in a way that
/// matters for [`UserConfig::save`]'s forward-compatibility check below.
const CURRENT_SCHEMA_VERSION: u32 = 1;

fn current_schema_version() -> u32 {
    CURRENT_SCHEMA_VERSION
}

/// Relative path of the optional project-level configuration file, inside
/// the project root.
const PROJECT_CONFIG_RELATIVE_PATH: &str = ".glasshouse/config.toml";

/// Errors from loading or saving Glasshouse configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read configuration file `{path}`: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// The file exists but is not valid TOML, or its shape does not match
    /// what this build expects. Deliberately never followed by a write:
    /// overwriting a file we could not parse would destroy whatever the
    /// user actually has on disk.
    #[error("configuration file `{path}` is not valid TOML: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },

    #[error("could not create configuration directory `{path}`: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("could not write configuration file `{path}`: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("could not serialize configuration for `{path}`: {source}")]
    Serialize {
        path: PathBuf,
        #[source]
        source: Box<toml::ser::Error>,
    },

    /// The file's `version` is newer than this build understands. Reading it
    /// (see [`load`] / [`load_project_config`]) still succeeds — refusing to
    /// even parse a file some other Glasshouse install wrote would be an
    /// unnecessary hostility. Only *writing* is refused, because this build
    /// cannot know what the newer fields mean and would otherwise silently
    /// drop them.
    #[error(
        "configuration file `{path}` was written by a newer version of Glasshouse (schema version {found}, this build understands up to {supported}); refusing to overwrite it. The file can still be read; upgrade Glasshouse to write it again."
    )]
    UnsupportedVersion {
        path: PathBuf,
        found: u32,
        supported: u32,
    },

    /// The project-level configuration path did not resolve inside the
    /// project root. See [`load_project_config`] and
    /// [`write_project_config_with_consent`] for why this can never
    /// actually point outside the project.
    #[error("project configuration path could not be resolved inside the project root: {0}")]
    Scope(#[from] ScopeError),
}

/// Per-integration configuration: whether the user turned it on, and an
/// optional explicit executable path.
///
/// `enabled` is genuinely tri-state per field: `None` means the user has
/// never recorded a decision (the key is absent), while `Some(_)` records
/// an explicit enable or disable. This distinction matters for layering —
/// see [`IntegrationTable::is_enabled`] and [`EffectiveConfig::enabled`].
///
/// Deliberately has no other fields — see the module-level "No secrets
/// here" section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    executable: Option<PathBuf>,
    /// Consent to write this harness's lifecycle hooks *inside the user's
    /// own project*, for a harness whose only hook mechanism reads from
    /// there (Codex's `.codex/hooks.json`; see
    /// [`crate::harness::HookDestination::ProjectLocal`]). `None` means the
    /// user has never been asked, which must be treated as consent withheld,
    /// never as consent granted.
    ///
    /// `Option<bool>` for the same reason `enabled` is: a plain `bool` here
    /// would repeat the exact defect `enabled` already caused once — a
    /// project file that overrides only one of these two fields would parse
    /// the other as its type's default rather than "not recorded", and
    /// `false` silently winning is precisely the wrong default for consent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_hooks: Option<bool>,
}

impl IntegrationConfig {
    pub fn enabled(&self) -> Option<bool> {
        self.enabled
    }

    /// The recorded decision, or `default` when none was ever recorded.
    pub fn enabled_or(&self, default: bool) -> bool {
        self.enabled.unwrap_or(default)
    }

    pub fn executable(&self) -> Option<&Path> {
        self.executable.as_deref()
    }

    /// Whether the user has consented to project-local lifecycle hooks for
    /// this harness. `None` means never asked — see the field's own doc.
    pub fn project_hooks(&self) -> Option<bool> {
        self.project_hooks
    }

    /// The recorded consent decision, or `default` when none was ever
    /// recorded. Callers resolving this for real use must pass `false`: an
    /// unrecorded decision is withheld consent, not granted consent.
    pub fn project_hooks_or(&self, default: bool) -> bool {
        self.project_hooks.unwrap_or(default)
    }

    pub fn set_enabled(&mut self, enabled: bool) -> &mut Self {
        self.enabled = Some(enabled);
        self
    }

    pub fn set_executable(&mut self, executable: Option<PathBuf>) -> &mut Self {
        self.executable = executable;
        self
    }

    pub fn set_project_hooks(&mut self, consent: bool) -> &mut Self {
        self.project_hooks = Some(consent);
        self
    }
}

/// A map of per-integration configuration, keyed by [`IntegrationId::slug`].
///
/// A `BTreeMap<String, _>` rather than an `IntegrationId`-keyed map so that
/// a slug this build does not recognize — written by a newer Glasshouse —
/// round-trips through load/save instead of failing to parse, and so the
/// serialized order is deterministic (stable diffs, easy manual review).
/// `#[serde(transparent)]` makes this behave exactly like the bare map for
/// (de)serialization, so the TOML shape stays the plain
/// `[integrations.claude-code]` form shown in the module docs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IntegrationTable(BTreeMap<String, IntegrationConfig>);

impl IntegrationTable {
    /// The recorded configuration for `id`, if the user has ever recorded
    /// one. `None` here is a real state, not "disabled" — see
    /// [`IntegrationTable::is_enabled`].
    pub fn get(&self, id: IntegrationId) -> Option<&IntegrationConfig> {
        self.0.get(id.slug())
    }

    /// Mutable access, creating a default (no recorded decision, no explicit
    /// executable) entry if `id` has no recorded configuration yet.
    pub fn entry(&mut self, id: IntegrationId) -> &mut IntegrationConfig {
        self.0.entry(id.slug().to_owned()).or_default()
    }

    pub fn set(&mut self, id: IntegrationId, config: IntegrationConfig) {
        self.0.insert(id.slug().to_owned(), config);
    }

    pub fn remove(&mut self, id: IntegrationId) -> Option<IntegrationConfig> {
        self.0.remove(id.slug())
    }

    /// Tri-state: `Some(true)`/`Some(false)` is an explicit user decision,
    /// `None` means the user has never been asked about `id` (including the
    /// case where an entry exists but records only, say, an executable
    /// override). Onboarding needs exactly this distinction to know whether
    /// to prompt.
    pub fn is_enabled(&self, id: IntegrationId) -> Option<bool> {
        self.get(id).and_then(IntegrationConfig::enabled)
    }

    /// Like [`IntegrationTable::is_enabled`], collapsing the never-asked
    /// case to a caller-supplied default instead of an `Option`.
    pub fn is_enabled_or_default(&self, id: IntegrationId, default: bool) -> bool {
        self.is_enabled(id).unwrap_or(default)
    }

    /// Every recorded entry, keyed by its raw slug (including slugs this
    /// build does not recognize).
    pub fn iter(&self) -> impl Iterator<Item = (&str, &IntegrationConfig)> {
        self.0.iter().map(|(slug, cfg)| (slug.as_str(), cfg))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Onboarding progress, persisted so the first-run wizard runs at most once
/// per user (Phase 2C: "Persist onboarding choices in user-level Glasshouse
/// configuration").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingState {
    #[serde(default)]
    completed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completed_at_version: Option<String>,
}

impl OnboardingState {
    pub fn completed(&self) -> bool {
        self.completed
    }

    /// The Glasshouse version that was running when onboarding was last
    /// completed, if known. Informational only (e.g. for deciding whether a
    /// changelog-driven "what's new" prompt applies) — nothing in this
    /// module gates behavior on it.
    pub fn completed_at_version(&self) -> Option<&str> {
        self.completed_at_version.as_deref()
    }

    pub fn mark_completed(&mut self, version: impl Into<String>) {
        self.completed = true;
        self.completed_at_version = Some(version.into());
    }

    /// Reset onboarding so the wizard runs again. Phase 2C: "Allow the
    /// onboarding wizard to be reopened later from settings."
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// User-level Glasshouse configuration: `<config_dir>/config.toml`.
///
/// Unknown top-level keys and unknown fields inside known tables are
/// ignored on load rather than rejected, so a file written by a newer
/// Glasshouse still loads here (see [`ConfigError::UnsupportedVersion`] for
/// what still gets refused: writing it back).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserConfig {
    #[serde(default = "current_schema_version")]
    version: u32,
    #[serde(default)]
    onboarding: OnboardingState,
    #[serde(default)]
    integrations: IntegrationTable,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            version: CURRENT_SCHEMA_VERSION,
            onboarding: OnboardingState::default(),
            integrations: IntegrationTable::default(),
        }
    }
}

impl UserConfig {
    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn onboarding(&self) -> &OnboardingState {
        &self.onboarding
    }

    pub fn onboarding_mut(&mut self) -> &mut OnboardingState {
        &mut self.onboarding
    }

    pub fn integrations(&self) -> &IntegrationTable {
        &self.integrations
    }

    pub fn integrations_mut(&mut self) -> &mut IntegrationTable {
        &mut self.integrations
    }

    /// Load the user-level configuration file named by `paths`.
    ///
    /// A missing file is not an error: it returns [`UserConfig::default`]
    /// (onboarding not completed, no integration decisions recorded). This
    /// is what makes "no initialization command required" true for a normal
    /// first run. A malformed file *is* an error — see [`ConfigError::Parse`].
    pub fn load(paths: &RuntimePaths) -> Result<Self, ConfigError> {
        load_toml_or_default(&paths.user_config_file())
    }

    /// Atomically write this configuration to the user-level configuration
    /// file named by `paths`, creating the configuration directory
    /// (owner-only on Unix) if it does not exist yet.
    ///
    /// Refuses to write, without touching the file, if `version` is newer
    /// than this build understands — see [`ConfigError::UnsupportedVersion`].
    /// That situation only arises by loading a newer file and saving it back
    /// unmodified or by constructing a [`UserConfig`] with an inflated
    /// version by hand; a config this build created itself always carries
    /// [`CURRENT_SCHEMA_VERSION`] and never hits it.
    pub fn save(&self, paths: &RuntimePaths) -> Result<(), ConfigError> {
        let path = paths.user_config_file();
        if self.version > CURRENT_SCHEMA_VERSION {
            return Err(ConfigError::UnsupportedVersion {
                path,
                found: self.version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }
        write_atomic_toml(paths.config_dir(), &path, self)
    }
}

/// Optional project-level Glasshouse configuration:
/// `<project root>/.glasshouse/config.toml`.
///
/// Same overridable shape as [`UserConfig`]'s integrations — see
/// [`EffectiveConfig`] for how the two are layered together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectConfig {
    #[serde(default = "current_schema_version")]
    version: u32,
    #[serde(default)]
    integrations: IntegrationTable,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            version: CURRENT_SCHEMA_VERSION,
            integrations: IntegrationTable::default(),
        }
    }
}

impl ProjectConfig {
    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn integrations(&self) -> &IntegrationTable {
        &self.integrations
    }

    pub fn integrations_mut(&mut self) -> &mut IntegrationTable {
        &mut self.integrations
    }
}

/// Resolve the project-level configuration file path inside `project`'s
/// scope.
///
/// Going through [`crate::project::ProjectScope::resolve`] rather than a
/// plain `project.root().join(...)` is the point: even though the relative
/// path here is a fixed constant we control, resolving it through the scope
/// guard means the write path can never end up outside the project root
/// through a symlink planted at `.glasshouse` (or anywhere along it), and it
/// keeps this module honest with every other component that touches a
/// project-relative path.
fn project_config_path(project: &Project) -> Result<PathBuf, ConfigError> {
    project
        .scope()
        .resolve(PROJECT_CONFIG_RELATIVE_PATH)
        .map_err(ConfigError::Scope)
}

/// Load the optional project-level configuration for `project`, if the user
/// has ever created one.
///
/// Returns `Ok(None)` when no such file exists. This function never creates
/// one — see [`write_project_config_with_consent`] for the only way this
/// file comes into existence.
pub fn load_project_config(project: &Project) -> Result<Option<ProjectConfig>, ConfigError> {
    let path = project_config_path(project)?;
    match std::fs::read_to_string(&path) {
        Ok(contents) => parse_toml(&path, &contents).map(Some),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ConfigError::Read { path, source }),
    }
}

/// Write `config` into `project`'s `.glasshouse/config.toml`, creating the
/// `.glasshouse` directory (owner-only on Unix) if needed.
///
/// # This requires the user's explicit consent
///
/// This writes inside the user's project tree — Phase 2D requires "explicit
/// confirmation before writing project-level configuration into the
/// repository," and this function performs none of that confirmation
/// itself; it is the caller's (the settings UI's) job to have obtained it
/// first. The `_with_consent` suffix exists so this is never reached for
/// unconditionally, by-default, or "just in case" writes — a project that
/// has not opted in must never grow a `.glasshouse/config.toml` on its own.
pub fn write_project_config_with_consent(
    project: &Project,
    config: &ProjectConfig,
) -> Result<(), ConfigError> {
    let path = project_config_path(project)?;
    if config.version > CURRENT_SCHEMA_VERSION {
        return Err(ConfigError::UnsupportedVersion {
            path,
            found: config.version,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }
    // `path` is `<root>/.glasshouse/config.toml`, so it always has a parent;
    // the fallback only guards a `PROJECT_CONFIG_RELATIVE_PATH` that no
    // longer names a nested file, which would be a bug in this module, not
    // something a caller can trigger.
    let dir = path.parent().unwrap_or(&path).to_path_buf();
    write_atomic_toml(&dir, &path, config)
}

/// Which configuration layer supplied a resolved value. Surfaced so the
/// Phase 2D settings view can visibly distinguish a user-level default from
/// a project-level override, as required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// Read from the project-level file.
    Project,
    /// Read from the user-level file.
    User,
    /// Neither layer had a recorded value; this is a hardcoded fallback.
    Default,
}

/// A resolved value together with which layer produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layered<T> {
    pub value: T,
    pub layer: Layer,
}

impl<T> Layered<T> {
    pub fn new(value: T, layer: Layer) -> Self {
        Self { value, layer }
    }
}

/// User configuration layered with an optional project-level override.
///
/// Kept intentionally small — a couple of lookup methods, not a generic
/// layering framework — because today only per-integration `enabled` and
/// `executable` need layering. Project always wins when it has recorded a
/// value; otherwise the user-level value applies; otherwise a caller
/// supplied default applies. Each lookup reports which of those three
/// happened via [`Layer`].
#[derive(Debug, Clone, Copy)]
pub struct EffectiveConfig<'a> {
    user: &'a UserConfig,
    project: Option<&'a ProjectConfig>,
}

impl<'a> EffectiveConfig<'a> {
    pub fn new(user: &'a UserConfig, project: Option<&'a ProjectConfig>) -> Self {
        Self { user, project }
    }

    /// Resolve whether `id` is enabled, reporting which layer decided it.
    /// Falls back to `default_enabled` (reported as [`Layer::Default`]) when
    /// neither layer has ever recorded a decision.
    pub fn enabled(&self, id: IntegrationId, default_enabled: bool) -> Layered<bool> {
        if let Some(enabled) = self.project.and_then(|p| p.integrations().is_enabled(id)) {
            return Layered::new(enabled, Layer::Project);
        }
        if let Some(enabled) = self.user.integrations().is_enabled(id) {
            return Layered::new(enabled, Layer::User);
        }
        Layered::new(default_enabled, Layer::Default)
    }

    /// Resolve whether `id` has the user's consent to write its lifecycle
    /// hooks inside the project itself, reporting which layer decided it.
    /// Falls back to `false` (reported as [`Layer::Default`]) when neither
    /// layer has ever recorded a decision — unlike
    /// [`EffectiveConfig::enabled`], callers never get to choose that
    /// default, because a session with no consent on record must run without
    /// project-local hooks rather than assume the answer either way.
    pub fn project_hooks(&self, id: IntegrationId) -> Layered<bool> {
        if let Some(consent) = self
            .project
            .and_then(|p| p.integrations().get(id))
            .and_then(IntegrationConfig::project_hooks)
        {
            return Layered::new(consent, Layer::Project);
        }
        if let Some(consent) = self
            .user
            .integrations()
            .get(id)
            .and_then(IntegrationConfig::project_hooks)
        {
            return Layered::new(consent, Layer::User);
        }
        Layered::new(false, Layer::Default)
    }

    /// Resolve the explicit executable override for `id`, if any layer has
    /// recorded one. `None` means neither layer has an override, i.e. normal
    /// `PATH` discovery applies — there is no "default" executable path to
    /// report here, so unlike [`EffectiveConfig::enabled`] this has no
    /// [`Layer::Default`] case.
    pub fn executable(&self, id: IntegrationId) -> Option<Layered<PathBuf>> {
        if let Some(exe) = self
            .project
            .and_then(|p| p.integrations().get(id))
            .and_then(IntegrationConfig::executable)
        {
            return Some(Layered::new(exe.to_path_buf(), Layer::Project));
        }
        if let Some(exe) = self
            .user
            .integrations()
            .get(id)
            .and_then(IntegrationConfig::executable)
        {
            return Some(Layered::new(exe.to_path_buf(), Layer::User));
        }
        None
    }
}

/// Load a TOML-serialized `T` from `path`, or `T::default()` if the file
/// does not exist.
fn load_toml_or_default<T: Default + serde::de::DeserializeOwned>(
    path: &Path,
) -> Result<T, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => parse_toml(path, &contents),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(source) => Err(ConfigError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn parse_toml<T: serde::de::DeserializeOwned>(
    path: &Path,
    contents: &str,
) -> Result<T, ConfigError> {
    toml::from_str(contents).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source: Box::new(source),
    })
}

/// Monotonic counter mixed into temporary file names so that two saves
/// racing inside the same process (as can happen across test threads, which
/// share a process id) never pick the same temporary path.
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Serialize `value` as pretty TOML and atomically replace `path` with it.
///
/// Writes to a fresh temporary file inside `dir` (which must be the same
/// directory as `path`, so the following rename stays on one filesystem)
/// with owner-only permissions, then `rename`s it over `path`. `rename` is
/// atomic on both POSIX and Windows when source and destination share a
/// filesystem, so a crash or power loss during the write can only ever
/// leave the previous file intact or the new file complete — never a
/// half-written config on disk. `dir` is created first (owner-only on Unix,
/// mirroring `create_state_dir` in `lib.rs`) if it does not exist yet.
fn write_atomic_toml<T: Serialize>(dir: &Path, path: &Path, value: &T) -> Result<(), ConfigError> {
    create_secure_dir(dir).map_err(|source| ConfigError::CreateDir {
        path: dir.to_path_buf(),
        source,
    })?;

    let contents = toml::to_string_pretty(value).map_err(|source| ConfigError::Serialize {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_owned());
    let unique = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_path = dir.join(format!(".{file_name}.{}.{unique}.tmp", std::process::id()));

    write_secure_file(&tmp_path, contents.as_bytes()).map_err(|source| ConfigError::Write {
        path: tmp_path.clone(),
        source,
    })?;

    if let Err(source) = std::fs::rename(&tmp_path, path) {
        // Best-effort cleanup: leaving the temp file behind on a failed
        // rename is better than leaving nothing, but never mask the real
        // error with a cleanup failure.
        let _ = std::fs::remove_file(&tmp_path);
        return Err(ConfigError::Write {
            path: path.to_path_buf(),
            source,
        });
    }

    Ok(())
}

/// Create `dir` (and parents) restricted to its owner on Unix. Mirrors
/// `create_state_dir` in `lib.rs`: a config file can carry integration
/// executable paths and, later, other user-specific detail, so default
/// (typically world-readable) directory permissions are not appropriate.
/// When `dir` already exists as something else, this keeps whatever
/// permissions it already had — it neither widens nor narrows a directory
/// it did not create.
#[cfg(unix)]
fn create_secure_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_secure_dir(dir: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new().recursive(true).create(dir)
}

/// Write `contents` to a new file at `path`, restricted to its owner on
/// Unix. `path` is always a fresh temporary file name (see
/// [`write_atomic_toml`]), so `create_new` semantics are not required here;
/// `create + truncate` is enough.
#[cfg(unix)]
fn write_secure_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

#[cfg(not(unix))]
fn write_secure_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;

    /// Build a `Project` rooted at `root` for tests. `root` must already
    /// exist; a plain (non-Git) temp directory falls back to
    /// `RootSource::WorkingDirectory`, which is exactly what these tests
    /// want — no `.git` scaffolding needed.
    fn test_project(root: &Path) -> Project {
        Project::discover(root, None, false).expect("test project root must be usable")
    }

    fn fully_populated_user_config() -> UserConfig {
        let mut config = UserConfig::default();
        config.onboarding_mut().mark_completed("0.1.0".to_owned());
        config
            .integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(true)
            .set_executable(Some(PathBuf::from("/opt/claude-code/bin/claude")));
        config
            .integrations_mut()
            .entry(IntegrationId::Codex)
            .set_enabled(false);
        config
    }

    #[test]
    fn missing_file_loads_as_default() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

        let config = UserConfig::load(&paths).unwrap();
        assert_eq!(config, UserConfig::default());
        assert!(!config.onboarding().completed());
        assert!(config.integrations().is_empty());
        // Loading must not have created anything.
        assert!(!paths.user_config_file().exists());
    }

    #[test]
    fn round_trip_save_load_preserves_every_field() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

        let original = fully_populated_user_config();
        original.save(&paths).unwrap();

        let loaded = UserConfig::load(&paths).unwrap();
        assert_eq!(loaded, original);
        assert_eq!(loaded.onboarding().completed_at_version(), Some("0.1.0"));
        assert_eq!(
            loaded
                .integrations()
                .get(IntegrationId::ClaudeCode)
                .unwrap()
                .executable(),
            Some(Path::new("/opt/claude-code/bin/claude"))
        );
        assert_eq!(
            loaded.integrations().is_enabled(IntegrationId::Codex),
            Some(false)
        );
    }

    #[test]
    fn a_file_written_by_a_newer_version_loads_but_refuses_to_save() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(
            paths.user_config_file(),
            "version = 999\n\n[onboarding]\ncompleted = true\n",
        )
        .unwrap();

        let loaded = UserConfig::load(&paths).unwrap();
        assert_eq!(loaded.version(), 999);
        assert!(loaded.onboarding().completed());

        let err = loaded.save(&paths).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::UnsupportedVersion {
                found: 999,
                supported: CURRENT_SCHEMA_VERSION,
                ..
            }
        ));
        let msg = err.to_string();
        assert!(msg.contains("newer version"), "{msg}");

        // The file on disk must be untouched by the failed save.
        let raw = std::fs::read_to_string(paths.user_config_file()).unwrap();
        assert!(raw.contains("999"));
    }

    #[test]
    fn unknown_keys_and_fields_do_not_break_parsing() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(
            paths.user_config_file(),
            r#"
                version = 1
                some_future_top_level_key = "ignored"

                [onboarding]
                completed = true
                completed_at_version = "9.9.9"
                some_future_onboarding_field = 42

                [integrations.claude-code]
                enabled = true
                some_future_integration_field = true

                [integrations.a-future-harness-this-build-does-not-know]
                enabled = true
            "#,
        )
        .unwrap();

        let config = UserConfig::load(&paths).unwrap();
        assert!(config.onboarding().completed());
        assert_eq!(
            config.integrations().is_enabled(IntegrationId::ClaudeCode),
            Some(true)
        );
        // The unrecognized slug round-trips through the map even though no
        // `IntegrationId` variant names it.
        assert_eq!(
            config
                .integrations()
                .iter()
                .find(|(slug, _)| *slug == "a-future-harness-this-build-does-not-know")
                .map(|(_, cfg)| cfg.enabled()),
            Some(Some(true))
        );
    }

    #[test]
    fn missing_version_field_defaults_to_current_schema_version() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(paths.user_config_file(), "[onboarding]\ncompleted = true\n").unwrap();

        let config = UserConfig::load(&paths).unwrap();
        assert_eq!(config.version(), CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn malformed_toml_is_an_error_naming_the_path_and_does_not_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        let broken = "version = 1\n[onboarding\ncompleted = true\n";
        std::fs::write(paths.user_config_file(), broken).unwrap();

        let err = UserConfig::load(&paths).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
        let msg = err.to_string();
        assert!(
            msg.contains(&paths.user_config_file().display().to_string()),
            "{msg}"
        );

        // Nothing must have touched the file: same content, no temp files.
        let raw = std::fs::read_to_string(paths.user_config_file()).unwrap();
        assert_eq!(raw, broken);
        let entries: Vec<_> = std::fs::read_dir(paths.config_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("config.toml")]);
    }

    #[test]
    fn atomic_save_leaves_no_temporary_file_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));

        fully_populated_user_config().save(&paths).unwrap();

        let entries: Vec<_> = std::fs::read_dir(paths.config_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["config.toml".to_owned()], "{entries:?}");
    }

    #[cfg(unix)]
    #[test]
    fn unix_permissions_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let paths = RuntimePaths::new(tmp.path().join("data"), tmp.path().join("config"));
        UserConfig::default().save(&paths).unwrap();

        let dir_mode = std::fs::metadata(paths.config_dir())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700, "config dir mode was {dir_mode:o}");

        let file_mode = std::fs::metadata(paths.user_config_file())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600, "config file mode was {file_mode:o}");
    }

    #[test]
    fn tri_state_enabled_distinguishes_never_asked_from_a_decision() {
        let mut config = UserConfig::default();
        assert_eq!(
            config.integrations().is_enabled(IntegrationId::ClaudeCode),
            None,
            "never asked"
        );
        assert!(
            config
                .integrations()
                .is_enabled_or_default(IntegrationId::ClaudeCode, true)
        );
        assert!(
            !config
                .integrations()
                .is_enabled_or_default(IntegrationId::ClaudeCode, false)
        );

        config
            .integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(false);
        assert_eq!(
            config.integrations().is_enabled(IntegrationId::ClaudeCode),
            Some(false),
            "explicitly declined"
        );
        assert!(
            !config
                .integrations()
                .is_enabled_or_default(IntegrationId::ClaudeCode, true)
        );

        config
            .integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(true);
        assert_eq!(
            config.integrations().is_enabled(IntegrationId::ClaudeCode),
            Some(true),
            "explicitly accepted"
        );
        assert!(
            config
                .integrations()
                .is_enabled_or_default(IntegrationId::ClaudeCode, false)
        );
    }

    #[test]
    fn tri_state_project_hooks_consent_distinguishes_never_asked_from_a_decision() {
        let mut config = UserConfig::default();
        assert_eq!(
            config.integrations().get(IntegrationId::Codex),
            None,
            "never asked"
        );

        config
            .integrations_mut()
            .entry(IntegrationId::Codex)
            .set_project_hooks(false);
        assert_eq!(
            config
                .integrations()
                .get(IntegrationId::Codex)
                .unwrap()
                .project_hooks(),
            Some(false),
            "explicitly declined"
        );

        config
            .integrations_mut()
            .entry(IntegrationId::Codex)
            .set_project_hooks(true);
        assert_eq!(
            config
                .integrations()
                .get(IntegrationId::Codex)
                .unwrap()
                .project_hooks(),
            Some(true),
            "explicitly consented"
        );

        // Recording a decision about `enabled` must not silently record one
        // about `project_hooks` too — the whole reason this is a second
        // `Option<bool>` field rather than folded into `enabled`.
        let mut only_enabled = UserConfig::default();
        only_enabled
            .integrations_mut()
            .entry(IntegrationId::Codex)
            .set_enabled(true);
        assert_eq!(
            only_enabled
                .integrations()
                .get(IntegrationId::Codex)
                .unwrap()
                .project_hooks(),
            None
        );
    }

    #[test]
    fn effective_config_defaults_project_hooks_consent_to_withheld() {
        // Absent consent must resolve to `false`, never `true` — a session
        // with no recorded decision must run without project-local hooks.
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let consent = effective.project_hooks(IntegrationId::Codex);
        assert!(!consent.value);
        assert_eq!(consent.layer, Layer::Default);
    }

    #[test]
    fn effective_config_project_hooks_consent_layers_like_enabled() {
        let mut user = UserConfig::default();
        user.integrations_mut()
            .entry(IntegrationId::Codex)
            .set_project_hooks(true);

        let mut project = ProjectConfig::default();
        project
            .integrations_mut()
            .entry(IntegrationId::Codex)
            .set_project_hooks(false);

        let effective = EffectiveConfig::new(&user, Some(&project));
        let consent = effective.project_hooks(IntegrationId::Codex);
        assert!(!consent.value, "the project layer withdraws consent");
        assert_eq!(consent.layer, Layer::Project);

        let effective_without_project = EffectiveConfig::new(&user, None);
        let consent = effective_without_project.project_hooks(IntegrationId::Codex);
        assert!(consent.value, "the user layer's consent still applies");
        assert_eq!(consent.layer, Layer::User);
    }

    #[test]
    fn project_config_layering_reports_the_correct_source_layer() {
        let mut user = UserConfig::default();
        user.integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(true);
        user.integrations_mut()
            .entry(IntegrationId::Codex)
            .set_enabled(true)
            .set_executable(Some(PathBuf::from("/usr/local/bin/codex")));

        let mut project = ProjectConfig::default();
        // Project explicitly disables what the user enabled: project wins.
        project
            .integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(false);

        let effective = EffectiveConfig::new(&user, Some(&project));

        // Case 1: project overrides user.
        let claude = effective.enabled(IntegrationId::ClaudeCode, true);
        assert!(!claude.value);
        assert_eq!(claude.layer, Layer::Project);

        // Case 2: only user has a decision.
        let codex = effective.enabled(IntegrationId::Codex, false);
        assert!(codex.value);
        assert_eq!(codex.layer, Layer::User);
        let codex_exe = effective.executable(IntegrationId::Codex).unwrap();
        assert_eq!(codex_exe.value, PathBuf::from("/usr/local/bin/codex"));
        assert_eq!(codex_exe.layer, Layer::User);

        // Case 3: neither layer has a decision, so the caller default wins.
        let ollama = effective.enabled(IntegrationId::Ollama, true);
        assert!(ollama.value);
        assert_eq!(ollama.layer, Layer::Default);
        assert!(effective.executable(IntegrationId::Ollama).is_none());
    }

    #[test]
    fn effective_config_without_a_project_file_falls_back_to_user_then_default() {
        let mut user = UserConfig::default();
        user.integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(true);

        let effective = EffectiveConfig::new(&user, None);
        let claude = effective.enabled(IntegrationId::ClaudeCode, false);
        assert!(claude.value);
        assert_eq!(claude.layer, Layer::User);

        let codex = effective.enabled(IntegrationId::Codex, false);
        assert!(!codex.value);
        assert_eq!(codex.layer, Layer::Default);
    }

    #[test]
    fn project_config_is_never_created_automatically() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        let project = test_project(&root);

        let loaded = load_project_config(&project).unwrap();
        assert!(loaded.is_none());
        assert!(!root.join(".glasshouse").exists());
    }

    #[test]
    fn project_config_round_trips_and_requires_the_consent_named_call() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        let project = test_project(&root);

        let mut config = ProjectConfig::default();
        config
            .integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(true)
            .set_executable(Some(PathBuf::from("./vendored/claude")));

        write_project_config_with_consent(&project, &config).unwrap();

        assert!(root.join(".glasshouse/config.toml").is_file());
        let loaded = load_project_config(&project).unwrap().unwrap();
        assert_eq!(loaded, config);
    }

    // The relative path this module resolves (`.glasshouse/config.toml`) is a
    // fixed constant, not caller-controlled input, so there is no untrusted
    // string that could ever literally spell its way outside the project
    // root. The one honest way to make `ProjectScope::resolve` actually
    // reject it is the scenario its own doc comment names: a symlink planted
    // at (or under) `.glasshouse` that resolves outside the root. A raw
    // `root.join(".glasshouse/config.toml")` would happily write through
    // such a symlink; going through the scope guard must not.
    //
    // Symlinks are POSIX-only in this test; `std::os::windows::fs::symlink_dir`
    // requires a privilege this sandbox does not reliably have, and the
    // `resolve` codepath under test is exercised identically on every
    // platform (see `crate::project::scope`'s own cross-platform tests), so
    // one platform is enough to prove this module wires it up correctly.
    #[cfg(unix)]
    #[test]
    fn project_config_path_is_resolved_through_the_project_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        // `.glasshouse` itself is a symlink escaping the project root.
        std::os::unix::fs::symlink(&outside, root.join(".glasshouse")).unwrap();
        let project = test_project(&root);

        let err = load_project_config(&project).unwrap_err();
        assert!(matches!(err, ConfigError::Scope(_)), "{err:?}");

        let err =
            write_project_config_with_consent(&project, &ProjectConfig::default()).unwrap_err();
        assert!(matches!(err, ConfigError::Scope(_)), "{err:?}");
        // And critically: the write must not have gone through to the
        // symlink target either.
        assert!(!outside.join("config.toml").exists());
    }

    #[test]
    fn project_executable_only_override_falls_through_to_user_enabled_decision() {
        let mut user = UserConfig::default();
        user.integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(true);

        let mut project = ProjectConfig::default();
        project
            .integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_executable(Some(PathBuf::from("/opt/bin/claude")));

        let effective = EffectiveConfig::new(&user, Some(&project));

        let enabled = effective.enabled(IntegrationId::ClaudeCode, true);
        assert!(enabled.value);
        assert_eq!(enabled.layer, Layer::User);

        let executable = effective.executable(IntegrationId::ClaudeCode).unwrap();
        assert_eq!(executable.value, PathBuf::from("/opt/bin/claude"));
        assert_eq!(executable.layer, Layer::Project);
    }

    #[test]
    fn explicit_project_disable_still_wins_over_user_enable() {
        let mut user = UserConfig::default();
        user.integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(true);

        let mut project = ProjectConfig::default();
        project
            .integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(false);

        let effective = EffectiveConfig::new(&user, Some(&project));
        let enabled = effective.enabled(IntegrationId::ClaudeCode, true);
        assert!(!enabled.value);
        assert_eq!(enabled.layer, Layer::Project);
    }

    #[test]
    fn enabled_key_parses_to_some_and_its_absence_parses_to_none() {
        let enabled_true: IntegrationConfig =
            toml::from_str("enabled = true\nexecutable = \"/x/y\"").unwrap();
        assert_eq!(enabled_true.enabled(), Some(true));

        let explicit_false: ProjectConfig = toml::from_str(
            r#"
                [integrations.claude-code]
                enabled = false
            "#,
        )
        .unwrap();
        assert_eq!(
            explicit_false
                .integrations()
                .is_enabled(IntegrationId::ClaudeCode),
            Some(false)
        );

        let omitted: ProjectConfig = toml::from_str(
            r#"
                [integrations.claude-code]
                executable = "/opt/bin/claude"
            "#,
        )
        .unwrap();
        assert_eq!(
            omitted
                .integrations()
                .get(IntegrationId::ClaudeCode)
                .unwrap()
                .enabled(),
            None
        );
        assert_eq!(
            omitted.integrations().is_enabled(IntegrationId::ClaudeCode),
            None,
            "an entry without a recorded decision is None, not Some(false)"
        );
    }

    #[test]
    fn serializing_no_decision_omits_the_enabled_key() {
        let no_decision = IntegrationConfig {
            enabled: None,
            executable: Some(PathBuf::from("/opt/bin/claude")),
            project_hooks: None,
        };
        let toml_text = toml::to_string_pretty(&no_decision).unwrap();
        assert!(
            !toml_text.contains("enabled"),
            "no-decision entry must not serialize an `enabled` key:\n{toml_text}"
        );
        assert!(
            !toml_text.contains("project_hooks"),
            "no-decision entry must not serialize a `project_hooks` key:\n{toml_text}"
        );

        let explicit_false = IntegrationConfig {
            enabled: Some(false),
            executable: None,
            project_hooks: None,
        };
        let toml_text = toml::to_string_pretty(&explicit_false).unwrap();
        assert!(
            toml_text.contains("enabled = false"),
            "explicit disable must serialize `enabled = false`:\n{toml_text}"
        );
    }

    #[test]
    fn enabled_or_returns_recorded_decision_or_supplied_default() {
        let decided = IntegrationConfig {
            enabled: Some(true),
            executable: None,
            project_hooks: None,
        };
        assert!(decided.enabled_or(false));

        let declined = IntegrationConfig {
            enabled: Some(false),
            executable: None,
            project_hooks: None,
        };
        assert!(!declined.enabled_or(true));

        let undecided = IntegrationConfig::default();
        assert!(undecided.enabled_or(true));
        assert!(!undecided.enabled_or(false));
    }

    /// Structural guard, not a string search: enumerate every field this
    /// module's config types can hold and assert none of them is
    /// credential-shaped. If a future edit adds a field, this test forces a
    /// conscious look rather than an accidental secret leaking into a
    /// tracked `.glasshouse` file or the user config.
    #[test]
    fn serialized_form_has_no_secret_capable_field() {
        // `IntegrationConfig` — the only per-item shape stored anywhere in
        // this module — has exactly these three fields.
        let cfg = IntegrationConfig {
            enabled: Some(true),
            executable: Some(PathBuf::from("/usr/bin/example")),
            project_hooks: Some(true),
        };
        let value = toml::Value::try_from(&cfg).unwrap();
        let table = value.as_table().unwrap();
        let mut keys: Vec<&str> = table.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["enabled", "executable", "project_hooks"],
            "IntegrationConfig grew a field — confirm it cannot hold a credential \
             before widening this list"
        );

        // `UserConfig`'s top level, likewise.
        let user = fully_populated_user_config();
        let user_value = toml::Value::try_from(&user).unwrap();
        let mut user_keys: Vec<&str> = user_value
            .as_table()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        user_keys.sort_unstable();
        assert_eq!(user_keys, vec!["integrations", "onboarding", "version"]);

        // And the serialized TOML text itself contains none of the names a
        // secret field would plausibly carry, as a cheap extra check on top
        // of the structural one above.
        let serialized = toml::to_string_pretty(&user).unwrap();
        for forbidden in ["key", "token", "secret", "password", "credential"] {
            assert!(
                !serialized.to_lowercase().contains(forbidden),
                "serialized UserConfig unexpectedly contains `{forbidden}`:\n{serialized}"
            );
        }
    }
}
