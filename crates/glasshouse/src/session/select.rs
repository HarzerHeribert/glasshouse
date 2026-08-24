//! Selecting exactly one harness and one executable for a new session.
//!
//! Opening a session answers two questions, in this order: *which* harness
//! (an explicit user request, else the single enabled harness — never a
//! guess between several), and *which* executable runs it (the configured
//! override if any layer recorded one, else PATH discovery over the
//! integration's candidate names).
//!
//! Both questions refuse ambiguity rather than resolving it silently. Every
//! failure carries an actionable message naming the valid choices, so the
//! user is never left guessing what Glasshouse wanted from them.
//!
//! Nothing here touches the environment beyond what the injected resolvers
//! already do, and nothing logs or formats anything but paths and
//! integration names.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::config::{EffectiveConfig, Layer};
use crate::integrations::{IntegrationId, IntegrationKind};
use crate::platform::exec::{self, ResolveError, ResolvedExecutable};

/// Where a selected executable came from, reported back so callers (and the
/// settings view) can show whether a project-level choice or a user-level
/// one decided the launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutableSource {
    /// An explicit path recorded in configuration. The deciding [`Layer`] is
    /// part of the value: `config.executable(id)` already applies
    /// project-over-user precedence, and this records which layer won.
    Configured { layer: Layer, path: PathBuf },
    /// Discovered on `PATH` under this candidate name.
    Path { name: String },
}

impl fmt::Display for ExecutableSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configured {
                layer: Layer::Project,
                ..
            } => f.write_str("project configuration"),
            Self::Configured {
                layer: Layer::User, ..
            } => f.write_str("user configuration"),
            // Unreachable through `select` (a configured executable always
            // comes from a real config layer), but `Layer` has the variant,
            // so it gets honest text rather than a dead match arm.
            Self::Configured {
                layer: Layer::Default,
                ..
            } => f.write_str("built-in default configuration"),
            Self::Path { name } => write!(f, "PATH (`{name}`)"),
        }
    }
}

/// One harness chosen for a session, together with the executable that will
/// run and where that executable came from.
///
/// Construction goes through [`select`] only: every field combination this
/// type could hold is supposed to have passed the selection rules, so there
/// is deliberately no other public constructor.
#[derive(Debug, Clone)]
pub struct HarnessSelection {
    id: IntegrationId,
    executable: ResolvedExecutable,
    source: ExecutableSource,
}

impl HarnessSelection {
    pub fn id(&self) -> IntegrationId {
        self.id
    }

    pub fn executable(&self) -> &ResolvedExecutable {
        &self.executable
    }

    pub fn source(&self) -> &ExecutableSource {
        &self.source
    }

    pub fn into_executable(self) -> ResolvedExecutable {
        self.executable
    }
}

/// Why a session could not select a harness or executable.
///
/// Every variant's message is written to be shown to the user verbatim: it
/// names what went wrong and states the concrete remedy.
#[derive(Debug, thiserror::Error)]
pub enum SelectionError {
    #[error(
        "`{name}` is not a known harness; valid harness names are: {}",
        harness_slugs()
    )]
    UnknownHarness { name: String },

    #[error(
        "`{name}` is {}, not a coding-agent harness Glasshouse can open a session in; \
         name a harness instead ({})",
        kind_noun(.id.kind()),
        harness_slugs()
    )]
    NotAHarness { name: String, id: IntegrationId },

    #[error(
        "{} is disabled in your Glasshouse configuration; re-enable it with \
         `glasshouse setup`, or name a different harness",
        .id.display_name()
    )]
    Disabled { id: IntegrationId },

    #[error(
        "no harness is enabled yet; run `glasshouse setup` to enable one, or name a harness \
         explicitly (valid names: {})",
        harness_slugs()
    )]
    NoneEnabled,

    #[error(
        "several harnesses are enabled ({}); Glasshouse will not guess between them — name \
         exactly one explicitly, e.g. `glasshouse launch {}`",
        .enabled.iter().map(|id| id.slug()).collect::<Vec<_>>().join(", "),
        .enabled.first().map(|id| id.slug()).unwrap_or("<none>"),
    )]
    Ambiguous { enabled: Vec<IntegrationId> },

    #[error(
        "the executable `{path}` configured for {} could not be resolved; fix or remove the \
         configured path instead of relying on PATH discovery",
        .id.display_name()
    )]
    ConfiguredExecutable {
        id: IntegrationId,
        path: PathBuf,
        #[source]
        source: ResolveError,
    },

    #[error(
        "{} is not installed: none of its known executable names were found on PATH \
         (tried: {}). Install it, or configure an explicit executable path with \
         `glasshouse setup`.",
        .id.display_name(),
        .id.executable_candidates().join(", ")
    )]
    NotInstalled { id: IntegrationId },
}

/// What an integration *is*, for a diagnostic that has to explain why a
/// session cannot be opened in it. Naming the category is more useful than
/// repeating the name the user just typed.
fn kind_noun(kind: IntegrationKind) -> &'static str {
    match kind {
        IntegrationKind::Harness => "a coding-agent harness",
        IntegrationKind::Multiplexer => "a terminal multiplexer",
        IntegrationKind::LocalInference => "a local inference server",
    }
}

/// Comma-separated list of the slugs a session can actually be opened in.
fn harness_slugs() -> String {
    IntegrationId::ALL
        .iter()
        .copied()
        .filter(|id| id.kind() == IntegrationKind::Harness)
        .map(IntegrationId::slug)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolve the harness and executable for a session about to be opened.
///
/// `requested` is an explicit harness slug (`Some`) or automatic selection
/// (`None`). Uses the real machine resolvers; see [`select_with`] for the
/// injectable form used by tests.
pub fn select(
    requested: Option<&str>,
    config: EffectiveConfig<'_>,
) -> Result<HarnessSelection, SelectionError> {
    select_with(requested, config, exec::resolve_explicit, exec::resolve)
}

/// Core of [`select`], with both executable resolvers injected so tests
/// never depend on the real filesystem layout or `PATH`.
///
/// - `resolve_configured` validates an explicitly configured path (real
///   implementation: [`exec::resolve_explicit`]).
/// - `resolve_on_path` searches `PATH` for a candidate name (real
///   implementation: [`exec::resolve`]).
pub(crate) fn select_with(
    requested: Option<&str>,
    config: EffectiveConfig<'_>,
    resolve_configured: impl Fn(&Path) -> Result<ResolvedExecutable, ResolveError>,
    resolve_on_path: impl Fn(&str) -> Result<ResolvedExecutable, ResolveError>,
) -> Result<HarnessSelection, SelectionError> {
    let id = match requested {
        Some(slug) => {
            let id = find_by_slug(slug).ok_or_else(|| SelectionError::UnknownHarness {
                name: slug.to_owned(),
            })?;
            if id.kind() != IntegrationKind::Harness {
                return Err(SelectionError::NotAHarness {
                    name: slug.to_owned(),
                    id,
                });
            }
            // Asymmetry, deliberate: an explicit request is treated as
            // intent, so a harness the user was never asked about defaults
            // to *allowed* (`default_enabled = true`). Only an explicit
            // `enabled = false` in some config layer overrides the user
            // standing right here asking for it. Automatic selection (the
            // arm below) is the opposite: silence means *not* enabled,
            // because nothing was ever requested.
            if !config.enabled(id, true).value {
                return Err(SelectionError::Disabled { id });
            }
            id
        }
        None => {
            let enabled: Vec<IntegrationId> = IntegrationId::ALL
                .iter()
                .copied()
                .filter(|id| id.kind() == IntegrationKind::Harness)
                .filter(|id| config.enabled(*id, false).value)
                .collect();
            match enabled.as_slice() {
                [] => return Err(SelectionError::NoneEnabled),
                [only] => *only,
                _ => return Err(SelectionError::Ambiguous { enabled }),
            }
        }
    };

    resolve_executable(id, config, resolve_configured, resolve_on_path).map(
        |(executable, source)| HarnessSelection {
            id,
            executable,
            source,
        },
    )
}

fn find_by_slug(slug: &str) -> Option<IntegrationId> {
    IntegrationId::ALL
        .iter()
        .copied()
        .find(|id| id.slug() == slug)
}

/// Resolve the executable for an already-chosen harness.
///
/// Order matters and is strict: a configured path — from whichever layer
/// recorded it, project over user — is tried first, and a *failed* attempt
/// to use it is an error, never a fallback to PATH discovery. Silently
/// falling back would launch a binary the user did not configure while
/// hiding the fact that their explicit choice is broken (stale path,
/// uninstalled, renamed): the resulting session would look configured but
/// run something else entirely, which is precisely the surprise this module
/// exists to prevent. Only when no layer has recorded any path at all does
/// candidate-name PATH discovery apply, in declared priority order.
fn resolve_executable(
    id: IntegrationId,
    config: EffectiveConfig<'_>,
    resolve_configured: impl Fn(&Path) -> Result<ResolvedExecutable, ResolveError>,
    resolve_on_path: impl Fn(&str) -> Result<ResolvedExecutable, ResolveError>,
) -> Result<(ResolvedExecutable, ExecutableSource), SelectionError> {
    if let Some(configured) = config.executable(id) {
        let executable = resolve_configured(&configured.value).map_err(|source| {
            SelectionError::ConfiguredExecutable {
                id,
                path: configured.value.clone(),
                source,
            }
        })?;
        let source = ExecutableSource::Configured {
            layer: configured.layer,
            path: configured.value,
        };
        return Ok((executable, source));
    }

    for &name in id.executable_candidates() {
        if let Ok(executable) = resolve_on_path(name) {
            return Ok((
                executable,
                ExecutableSource::Path {
                    name: name.to_owned(),
                },
            ));
        }
    }

    Err(SelectionError::NotInstalled { id })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ProjectConfig, UserConfig};
    use std::io::Write as _;

    /// Create a real decoy executable file (the only way to build a
    /// `ResolvedExecutable` for tests: via `exec::resolve_explicit`).
    fn write_decoy(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(b"#!/bin/sh\nexit 0\n").unwrap();
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    fn resolved(path: &Path) -> ResolvedExecutable {
        exec::resolve_explicit(path).expect("decoy file must resolve")
    }

    fn no_path_lookup(
        what: &'static str,
    ) -> impl Fn(&str) -> Result<ResolvedExecutable, ResolveError> {
        move |_| panic!("{what} must not fall back to PATH discovery")
    }

    fn no_configured_lookup(
        what: &'static str,
    ) -> impl Fn(&Path) -> Result<ResolvedExecutable, ResolveError> {
        move |_| panic!("{what} has no configured executable to resolve")
    }

    #[test]
    fn project_configured_executable_wins_over_user_level() {
        let tmp = tempfile::tempdir().unwrap();
        let project_exe = write_decoy(tmp.path(), "project-claude");
        let user_exe = write_decoy(tmp.path(), "user-claude");
        assert_ne!(project_exe, user_exe, "decoys must differ");

        let mut user = UserConfig::default();
        user.integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(true)
            .set_executable(Some(user_exe));
        // A present integration entry always carries its own `enabled`
        // bool (see `IntegrationConfig`), so the project layer here
        // explicitly enables Claude Code and overrides the executable;
        // selection must prefer this layer's path over the user-level one.
        let mut project = ProjectConfig::default();
        project
            .integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(true)
            .set_executable(Some(project_exe.clone()));

        let selection = select_with(
            Some("claude-code"),
            EffectiveConfig::new(&user, Some(&project)),
            exec::resolve_explicit,
            no_path_lookup("a resolved configured path"),
        )
        .unwrap();

        assert_eq!(
            selection.executable().path(),
            std::fs::canonicalize(&project_exe).unwrap()
        );
        assert_eq!(
            selection.source(),
            &ExecutableSource::Configured {
                layer: Layer::Project,
                path: project_exe,
            }
        );
        assert_eq!(selection.source().to_string(), "project configuration");
    }

    #[test]
    fn user_configured_executable_is_used_when_project_has_none() {
        let tmp = tempfile::tempdir().unwrap();
        let user_exe = write_decoy(tmp.path(), "user-codex");

        let mut user = UserConfig::default();
        user.integrations_mut()
            .entry(IntegrationId::Codex)
            .set_enabled(true)
            .set_executable(Some(user_exe.clone()));

        let selection = select_with(
            Some("codex"),
            EffectiveConfig::new(&user, None),
            exec::resolve_explicit,
            no_path_lookup("a resolved configured path"),
        )
        .unwrap();

        assert_eq!(selection.id(), IntegrationId::Codex);
        assert_eq!(
            selection.source(),
            &ExecutableSource::Configured {
                layer: Layer::User,
                path: user_exe,
            }
        );
    }

    #[test]
    fn without_configuration_the_first_resolving_candidate_name_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let decoy = resolved(&write_decoy(tmp.path(), "claude-decoy"));

        // No layer has an executable for Claude Code, so PATH discovery
        // applies; the resolver only answers for the harness's declared
        // candidate name.
        let found = decoy.clone();
        let selection = select_with(
            Some("claude-code"),
            EffectiveConfig::new(&UserConfig::default(), None),
            no_configured_lookup("claude-code"),
            move |name| {
                assert_eq!(name, "claude");
                Ok(found.clone())
            },
        )
        .unwrap();

        assert_eq!(selection.id(), IntegrationId::ClaudeCode);
        assert_eq!(
            selection.source(),
            &ExecutableSource::Path {
                name: "claude".to_owned()
            }
        );
        assert_eq!(selection.executable().path(), decoy.path());
        assert_eq!(selection.source().to_string(), "PATH (`claude`)");
    }

    #[test]
    fn a_failing_configured_executable_never_falls_back_to_path() {
        let tmp = tempfile::tempdir().unwrap();
        let stale = tmp.path().join("removed-from-disk");

        let mut user = UserConfig::default();
        user.integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(true)
            .set_executable(Some(stale));

        // Real `resolve_explicit` on a nonexistent path: deterministic
        // failure with no machine dependence. The PATH resolver panics, so
        // reaching it would fail the test loudly.
        let err = select_with(
            Some("claude-code"),
            EffectiveConfig::new(&user, None),
            exec::resolve_explicit,
            no_path_lookup("a broken configured path"),
        )
        .unwrap_err();

        match err {
            SelectionError::ConfiguredExecutable { id, ref source, .. } => {
                assert_eq!(id, IntegrationId::ClaudeCode);
                assert!(
                    matches!(source, ResolveError::NotFound { .. }),
                    "{source:?}"
                );
            }
            other => panic!("expected ConfiguredExecutable, got {other:?}"),
        }
        assert!(err.to_string().contains("could not be resolved"), "{err}");
    }

    #[test]
    fn automatic_selection_picks_a_sole_enabled_harness() {
        let tmp = tempfile::tempdir().unwrap();
        let decoy = resolved(&write_decoy(tmp.path(), "codex"));

        let mut user = UserConfig::default();
        user.integrations_mut()
            .entry(IntegrationId::Codex)
            .set_enabled(true);

        let found = decoy.clone();
        let selection = select_with(
            None,
            EffectiveConfig::new(&user, None),
            no_configured_lookup("Codex"),
            move |name| {
                assert_eq!(name, "codex");
                Ok(found.clone())
            },
        )
        .unwrap();

        assert_eq!(selection.id(), IntegrationId::Codex);
        assert_eq!(
            selection.source(),
            &ExecutableSource::Path {
                name: "codex".to_owned()
            }
        );
    }

    #[test]
    fn automatic_selection_with_two_enabled_harnesses_is_ambiguous() {
        let mut user = UserConfig::default();
        user.integrations_mut()
            .entry(IntegrationId::ClaudeCode)
            .set_enabled(true);
        user.integrations_mut()
            .entry(IntegrationId::Codex)
            .set_enabled(true);

        let err = select_with(
            None,
            EffectiveConfig::new(&user, None),
            no_configured_lookup("automatic selection"),
            no_path_lookup("ambiguous selection"),
        )
        .unwrap_err();

        let msg = match &err {
            SelectionError::Ambiguous { enabled } => {
                assert_eq!(enabled, &[IntegrationId::ClaudeCode, IntegrationId::Codex]);
                err.to_string()
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        };
        assert!(
            msg.contains("claude-code") && msg.contains("codex"),
            "{msg}"
        );
        // The remedy must be a command the user can actually run: the
        // harness is a positional argument, not a `--harness` flag.
        assert!(msg.contains("glasshouse launch claude-code"), "{msg}");
    }

    #[test]
    fn automatic_selection_with_none_enabled_reports_none_enabled() {
        let err = select_with(
            None,
            EffectiveConfig::new(&UserConfig::default(), None),
            no_configured_lookup("automatic selection"),
            no_path_lookup("automatic selection"),
        )
        .unwrap_err();

        assert!(matches!(err, SelectionError::NoneEnabled));
        let msg = err.to_string();
        assert!(msg.contains("glasshouse setup"), "{msg}");
    }

    #[test]
    fn an_explicit_request_overrides_a_never_recorded_decision_but_not_an_explicit_no() {
        // Never asked about Claude Code: the explicit request IS the intent,
        // so selection proceeds past the enabled check and resolves on PATH.
        let tmp = tempfile::tempdir().unwrap();
        let decoy = resolved(&write_decoy(tmp.path(), "never-asked-claude"));
        let found = decoy.clone();
        let selection = select_with(
            Some("claude-code"),
            EffectiveConfig::new(&UserConfig::default(), None),
            no_configured_lookup("Claude Code"),
            move |_| Ok(found.clone()),
        )
        .unwrap();
        assert_eq!(selection.id(), IntegrationId::ClaudeCode);
        assert_eq!(selection.executable().path(), decoy.path());

        // Explicitly declined: refused outright.
        let mut user = UserConfig::default();
        user.integrations_mut()
            .entry(IntegrationId::OpenCode)
            .set_enabled(false);

        let err = select_with(
            Some("opencode"),
            EffectiveConfig::new(&user, None),
            no_configured_lookup("OpenCode"),
            no_path_lookup("a disabled harness"),
        )
        .unwrap_err();
        assert!(
            matches!(err, SelectionError::Disabled { id } if id == IntegrationId::OpenCode),
            "{err:?}"
        );
    }

    #[test]
    fn cmux_is_not_a_harness() {
        let err = select_with(
            Some("cmux"),
            EffectiveConfig::new(&UserConfig::default(), None),
            no_configured_lookup("cmux"),
            no_path_lookup("cmux"),
        )
        .unwrap_err();

        assert!(
            matches!(
                err,
                SelectionError::NotAHarness {
                    id: IntegrationId::Cmux,
                    ..
                }
            ),
            "{err:?}"
        );
        assert!(
            err.to_string().contains("not a coding-agent harness"),
            "{err}"
        );
    }

    #[test]
    fn a_nonsense_slug_is_unknown_and_names_the_valid_ones() {
        let err = select_with(
            Some("definitely-not-real"),
            EffectiveConfig::new(&UserConfig::default(), None),
            no_configured_lookup("unknown harness"),
            no_path_lookup("unknown harness"),
        )
        .unwrap_err();

        assert!(
            matches!(err, SelectionError::UnknownHarness { ref name } if name == "definitely-not-real"),
            "{err:?}"
        );
        let msg = err.to_string();
        for slug in ["claude-code", "codex", "antigravity", "opencode"] {
            assert!(msg.contains(slug), "{msg}");
        }
        assert!(
            !msg.contains("cmux"),
            "only harness slugs belong here: {msg}"
        );
    }
}
