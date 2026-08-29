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

use anyhow::Context as _;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::config::{EffectiveConfig, Layer};
use crate::harness::response::Application;
use crate::harness::{HarnessAdapter, HookCommand, HookDestination};
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

    /// The adapter for the selected harness.
    ///
    /// Never `None`: selection only ever yields an [`IntegrationKind::Harness`]
    /// (the two arms above both enforce it), and every harness has an adapter.
    /// The `expect` records that invariant at the one place it could be
    /// violated rather than pushing an `Option` onto every caller for a case
    /// that cannot happen.
    pub fn adapter(&self) -> &'static dyn HarnessAdapter {
        crate::harness::adapter_for(self.id)
            .expect("selection only yields harnesses, and every harness has an adapter")
    }

    /// The arguments a new session starts with: what the harness's adapter
    /// declares, then whatever the user asked for.
    ///
    /// Order is the contract. The adapter's arguments are how the harness is
    /// opened at all, and the user's follow so that an explicit request always
    /// has the last word — most command-line parsers let a later occurrence
    /// win, and a user who typed something deserves for it to be the thing
    /// that survives.
    ///
    /// No harness needs a start argument today, so for now this is exactly the
    /// user's own list. It is nonetheless the seam both production start paths
    /// go through, because the alternative is two call sites that each have to
    /// remember the rule the day a harness does need one.
    pub fn start_args<I, S>(&self, native_session: Option<&str>, user_args: I) -> Vec<OsString>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let adapter = self.adapter();
        let mut args = adapter.start().args().to_vec();
        // The identifier, when Glasshouse is assigning one. Between the
        // adapter's own arguments and the user's, so that a user who passes
        // their harness a session flag by hand still has the last word.
        if let Some(native) = native_session
            && let Some(assignment) = adapter.assign_session_id(native)
        {
            args.extend(assignment.args().iter().cloned());
        }
        args.extend(user_args.into_iter().map(Into::into));
        args
    }

    /// The arguments that resume `native_session`, or `None` when this
    /// harness has no verified resume mechanism.
    ///
    /// The adapter's own start arguments come first, exactly as for a new
    /// session, because resuming is still starting the harness — it is only
    /// the conversation that is continued.
    pub fn resume_args<I, S>(&self, native_session: &str, user_args: I) -> Option<Vec<OsString>>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let adapter = self.adapter();
        let resume = adapter.resume(native_session)?;
        let mut args = adapter.start().args().to_vec();
        args.extend(resume.args().iter().cloned());
        args.extend(user_args.into_iter().map(Into::into));
        Some(args)
    }

    /// Write the one document Glasshouse owns for this session, and return
    /// the arguments that make the harness read it.
    ///
    /// # Why lifecycle hooks and a response profile share one function
    ///
    /// Because for Claude Code they share one *file*, and finding that out the
    /// hard way would have cost a feature silently. Probed on Claude Code
    /// 2.1.247 on 2026-08-27: `claude --settings A --settings B doctor`
    /// validates only `B`. A second `--settings` does not merge and does not
    /// error — **it discards the first**. So a response profile that appended
    /// its own `--settings` after `install_hooks` had appended one would have
    /// turned off every lifecycle hook in the session, and nothing would have
    /// said so.
    ///
    /// The rule this encodes: **at most one document per file name, and at
    /// most one flag pointing at it.** When the adapter's hook installation
    /// and its response-profile settings name the same file, the profile's
    /// keys are merged into the hook document and the hook installation's own
    /// arguments are used once.
    ///
    /// # Where the document goes
    ///
    /// Unchanged from before, and it is the adapter's declared
    /// [`HookDestination`] that decides:
    ///
    /// - [`GlasshouseOwned`](HookDestination::GlasshouseOwned) — a directory
    ///   Glasshouse owns, inside the project's own state, never the harness's
    ///   own configuration. Always written; this is what keeps a Glasshouse
    ///   session leaving the user's `claude` exactly as it found it.
    /// - [`ProjectLocal`](HookDestination::ProjectLocal) — a fixed path
    ///   inside the user's own project, because that harness reads hooks from
    ///   nowhere else. Written only when `project_hooks_consent` is `true`;
    ///   otherwise this creates no file and no directory for the hooks, and a
    ///   response profile that needs a settings document gets its own,
    ///   Glasshouse-owned one. A working session with less telemetry is still
    ///   a working session; a surprise file in the user's repository is not.
    ///
    /// An empty result is the ordinary case of a harness with no verified hook
    /// mechanism and nothing to apply, which is not a failure.
    pub fn install_session_document(
        &self,
        report: &HookCommand,
        project_hooks_consent: bool,
        response: &Application,
    ) -> anyhow::Result<SessionDocument> {
        let mut args: Vec<OsString> = Vec::new();
        let mut hooks_installed = false;
        let installation = self.adapter().hook_installation(report);

        // The hook document, where the adapter declares one and the
        // destination allows writing it.
        let hooks_file = match &installation {
            Some(installation) => {
                let path = match installation.destination {
                    HookDestination::GlasshouseOwned => {
                        std::fs::create_dir_all(report.directory()).with_context(|| {
                            format!(
                                "could not create the session directory `{}`",
                                report.directory().display()
                            )
                        })?;
                        Some(report.file(installation.file_name))
                    }
                    HookDestination::ProjectLocal { relative_path } => {
                        if project_hooks_consent {
                            let path = report.scope().join(relative_path);
                            if let Some(parent) = path.parent() {
                                std::fs::create_dir_all(parent).with_context(|| {
                                    format!("could not create `{}`", parent.display())
                                })?;
                            }
                            // The first write into someone's repository is not
                            // a silent event: this is the one place Glasshouse
                            // ever writes inside the user's own project, and it
                            // happens only after consent.
                            tracing::info!(
                                path = %path.display(),
                                "writing project-local lifecycle hooks"
                            );
                            Some(path)
                        } else {
                            None
                        }
                    }
                };
                path.map(|path| (path, installation.file_name, installation.contents.clone()))
            }
            None => None,
        };

        let profile_file = response.settings_file();
        let mut profile_keys_written = false;

        if let Some((path, file_name, contents)) = hooks_file {
            hooks_installed = true;
            let contents = if profile_file == Some(file_name) && !response.settings().is_empty() {
                profile_keys_written = true;
                merge_settings(&contents, response.settings())
                    .with_context(|| format!("could not compose `{}`", path.display()))?
            } else {
                contents
            };
            std::fs::write(&path, contents.as_bytes())
                .with_context(|| format!("could not write `{}`", path.display()))?;
            args.extend(
                installation
                    .as_ref()
                    .expect("a hook document exists only where an installation did")
                    .args
                    .args()
                    .iter()
                    .cloned(),
            );
        }

        // A response profile whose keys did not ride along with a hook
        // document gets its own, in the directory Glasshouse owns.
        if !profile_keys_written
            && !response.settings().is_empty()
            && let (Some(file_name), Some(flag)) = (profile_file, response.settings_flag())
        {
            std::fs::create_dir_all(report.directory()).with_context(|| {
                format!(
                    "could not create the session directory `{}`",
                    report.directory().display()
                )
            })?;
            let path = report.file(file_name);
            std::fs::write(&path, merge_settings("{}", response.settings())?.as_bytes())
                .with_context(|| format!("could not write `{}`", path.display()))?;
            args.push(OsString::from(flag));
            args.push(path.into_os_string());
        }

        // Whatever the profile puts on the command line — an additive
        // instruction, or a native style a harness selects with a flag.
        args.extend(response.args().iter().cloned());

        Ok(SessionDocument {
            args,
            hooks_installed,
        })
    }

    /// [`HarnessSelection::install_session_document`] for a caller with no
    /// response profile to apply.
    ///
    /// # This is a real gap, kept visible rather than hidden
    ///
    /// The shell's quick-open (`n`) is the one caller. It resolves no launch
    /// profile either — it starts a `Native` session with no overlay — so a
    /// session opened that way gets the harness untouched in every respect,
    /// not only this one. Closing that means giving the shell a launch profile
    /// and a response request, which is a change to
    /// [`mod@crate::shell`] rather than to this file.
    pub fn install_hooks(
        &self,
        report: &HookCommand,
        project_hooks_consent: bool,
    ) -> anyhow::Result<Option<Vec<OsString>>> {
        let document = self.install_session_document(
            report,
            project_hooks_consent,
            &Application::none(
                "this launch path resolves no response profile, so the harness's own \
                 communication behaviour is untouched",
            ),
        )?;
        // `Some(vec![])` and `None` are *different answers* here, and Codex is
        // why: it finds `.codex/hooks.json` itself, so a successful
        // installation contributes no arguments at all. Deriving the `Option`
        // from whether the argument list is empty would report that as "no
        // hooks installed".
        Ok(document.hooks_installed.then_some(document.args))
    }

    /// Whether this harness lets Glasshouse choose its native session
    /// identifier, in which case one should be minted before the session
    /// record is created.
    pub fn assigns_native_session_id(&self) -> bool {
        self.adapter().assign_session_id("probe").is_some()
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

/// What [`HarnessSelection::install_session_document`] wrote and composed.
///
/// Two fields rather than one, because `args` alone cannot answer "were hooks
/// installed": a Codex installation succeeds and contributes no arguments,
/// since Codex finds its own `.codex/hooks.json`. A caller that inferred
/// installation from a non-empty argument list would report that as a failure.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionDocument {
    /// Arguments to put in front of the harness's own.
    pub args: Vec<OsString>,
    /// Whether a lifecycle-hook document was actually written.
    pub hooks_installed: bool,
}

/// `document` with `keys` set on its top-level object.
///
/// A real JSON parse rather than string splicing, because the hook document
/// this merges into is nested and a textual insertion would be one escaped
/// quote away from writing a document the harness silently ignores.
///
/// An existing key is replaced. Nothing else in the document is touched — in
/// particular the `hooks` map an adapter composed is carried through
/// unchanged, which is the whole reason the two share a file.
fn merge_settings(document: &str, keys: &[(&'static str, String)]) -> anyhow::Result<String> {
    let mut value: serde_json::Value =
        serde_json::from_str(document).context("the harness settings document is not JSON")?;
    let object = value
        .as_object_mut()
        .context("the harness settings document is not a JSON object")?;
    for (key, setting) in keys {
        object.insert(
            (*key).to_owned(),
            serde_json::Value::String(setting.clone()),
        );
    }
    Ok(format!("{}\n", serde_json::to_string_pretty(&value)?))
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
/// (`None`). Uses the real machine resolvers; see `select_with` for the
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

    // --- the adapter seam -------------------------------------------------

    /// An adapter that needs an argument to start, which none of the real
    /// seven do today. The composition rule has to hold for the day one does,
    /// and that day must not be the first time it is exercised.
    #[derive(Debug)]
    struct NeedsAnArgument;

    impl crate::harness::HarnessAdapter for NeedsAnArgument {
        fn id(&self) -> IntegrationId {
            IntegrationId::ClaudeCode
        }

        fn executable_candidates(&self) -> &'static [&'static str] {
            &["pretend"]
        }

        fn start(&self) -> crate::harness::Invocation {
            crate::harness::Invocation::of(["--interactive", "--no-colour"])
        }

        fn resume(&self, _native_session: &str) -> Option<crate::harness::Invocation> {
            None
        }

        fn describe(&self) -> crate::harness::HarnessDescription {
            crate::harness::HarnessDescription {
                vendor: crate::harness::Declared::Unverified,
                hooks: crate::harness::Declared::Unverified,
                session_ids: crate::harness::Declared::Unverified,
                capabilities: crate::harness::Capabilities::UNVERIFIED,
                backends: crate::harness::Backends::UNVERIFIED,
                approvals: crate::harness::ApprovalModes::UNVERIFIED,
                communication_style: crate::harness::Declared::Unverified,
            }
        }
    }

    /// Mirrors `HarnessSelection::start_args` against an adapter that actually
    /// declares arguments. The production method reads its adapter from the
    /// registry, so this composes the same two pieces in the same order
    /// against a double — the rule under test is the ordering, not the
    /// declaration.
    fn compose(adapter: &dyn crate::harness::HarnessAdapter, user: &[&str]) -> Vec<String> {
        let mut args: Vec<String> = adapter
            .start()
            .args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        args.extend(user.iter().map(|s| (*s).to_string()));
        args
    }

    #[test]
    fn a_sessions_arguments_are_the_adapters_first_then_the_users() {
        assert_eq!(
            compose(&NeedsAnArgument, &["--resume", "abc"]),
            vec!["--interactive", "--no-colour", "--resume", "abc"],
        );
    }

    #[test]
    fn start_args_passes_the_users_arguments_through_unchanged() {
        // Every shipped adapter starts bare, so today this is exactly the
        // user's list — and `glasshouse launch claude-code -- --resume x` must
        // reach the harness as `--resume x` and nothing else.
        let tmp = tempfile::tempdir().unwrap();
        let selection = HarnessSelection {
            id: IntegrationId::ClaudeCode,
            executable: resolved(&write_decoy(tmp.path(), "claude")),
            source: ExecutableSource::Path {
                name: "claude".to_string(),
            },
        };
        let args: Vec<String> = selection
            .start_args(None, ["--resume", "x"])
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["--resume", "x"]);
    }

    #[test]
    fn every_selectable_harness_resolves_to_its_own_adapter() {
        let tmp = tempfile::tempdir().unwrap();
        for &id in IntegrationId::ALL {
            if id.kind() != IntegrationKind::Harness {
                continue;
            }
            let selection = HarnessSelection {
                id,
                executable: resolved(&write_decoy(tmp.path(), id.slug())),
                source: ExecutableSource::Path {
                    name: "x".to_string(),
                },
            };
            assert_eq!(selection.adapter().id(), id);
        }
    }

    // --- Codex's project-local hooks require consent ---------------------

    fn codex_selection(tmp: &Path) -> HarnessSelection {
        HarnessSelection {
            id: IntegrationId::Codex,
            executable: resolved(&write_decoy(tmp, "codex")),
            source: ExecutableSource::Path {
                name: "codex".to_string(),
            },
        }
    }

    /// A [`HookCommand`] rooted under `tmp`, with a real `project` directory
    /// already created so a test can assert on exactly what did or did not
    /// get written under it.
    fn hook_command(tmp: &Path) -> (HookCommand, PathBuf) {
        let project_root = tmp.join("project");
        std::fs::create_dir_all(&project_root).unwrap();
        let report = HookCommand::new(
            tmp.join("glasshouse"),
            "abc123",
            tmp.join("state/sessions/abc123"),
            project_root.clone(),
            tmp.join("data"),
            tmp.join("config"),
        );
        (report, project_root)
    }

    #[test]
    fn codex_hooks_are_written_into_the_project_only_with_consent() {
        let tmp = tempfile::tempdir().unwrap();
        let selection = codex_selection(tmp.path());
        let (report, project_root) = hook_command(tmp.path());

        let result = selection.install_hooks(&report, false).unwrap();

        assert_eq!(result, None, "no consent means no hooks installed");
        assert!(
            !project_root.join(".codex").exists(),
            "no `.codex` directory may appear without consent"
        );
    }

    #[test]
    fn codex_hooks_are_written_where_codex_reads_them() {
        let tmp = tempfile::tempdir().unwrap();
        let selection = codex_selection(tmp.path());
        let (report, project_root) = hook_command(tmp.path());

        let result = selection.install_hooks(&report, true).unwrap();

        // Codex finds `.codex/hooks.json` itself; nothing points it there.
        assert_eq!(result, Some(Vec::new()));

        let written = project_root.join(".codex/hooks.json");
        let contents = std::fs::read_to_string(&written).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&contents)
            .unwrap_or_else(|err| panic!("not valid JSON: {err}\n{contents}"));
        let mut events: Vec<&str> = parsed["hooks"]
            .as_object()
            .expect("a hooks object")
            .keys()
            .map(String::as_str)
            .collect();
        events.sort_unstable();
        let mut expected = vec![
            "PermissionRequest",
            // Compaction is *observed* (logged), not recorded as a
            // `SessionLifecycle` state — see `harness::codex::REPORTED_EVENTS`
            // and `docs/product/evidence/phase-8.md`. This assertion is on the
            // file Codex actually reads, so it is the one that proves the two
            // events reach disk rather than only the adapter's constant.
            "PostCompact",
            "PreCompact",
            "SessionEnd",
            "SessionStart",
            "Stop",
            "UserPromptSubmit",
        ];
        expected.sort_unstable();
        assert_eq!(events, expected);
    }
}
