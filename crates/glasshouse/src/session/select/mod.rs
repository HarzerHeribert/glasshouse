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

    /// Write the one document Glasshouse owns for this session, and return the
    /// arguments that make the harness read it.
    ///
    /// At most one document per file name, and at most one flag pointing at
    /// it: when the adapter's hook installation and its response-profile
    /// settings name the same file, the profile's keys are merged into the
    /// hook document and the hook installation's own arguments are used once
    /// — a second, unmerged `--settings` silently discards the first on at
    /// least one real harness (Claude Code; see design-decisions.md).
    ///
    /// Where the document goes is the adapter's declared [`HookDestination`]:
    /// [`GlasshouseOwned`](HookDestination::GlasshouseOwned) is always
    /// written, inside the project's own state, never the harness's
    /// configuration. [`ProjectLocal`](HookDestination::ProjectLocal) is
    /// written only with `project_hooks_consent`, since it lands inside the
    /// user's own repository.
    ///
    /// An empty result is the ordinary case of a harness with no verified
    /// hook mechanism, not a failure.
    // History: design-decisions.md, "Trims: session module docs, second packet", session/select/mod.rs `install_session_document`.
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
mod tests;
