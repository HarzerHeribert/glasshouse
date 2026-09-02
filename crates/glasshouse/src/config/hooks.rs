//! Per-integration configuration: enable/disable, hooks consent, and the executable override.
//!

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::integrations::IntegrationId;

/// Per-integration configuration: whether the user turned it on, and an
/// optional explicit executable path.
///
/// `enabled` is genuinely tri-state per field: `None` means the user has
/// never recorded a decision (the key is absent), while `Some(_)` records
/// an explicit enable or disable. This distinction matters for layering —
/// see [`IntegrationTable::is_enabled`] and [`crate::config::effective::EffectiveConfig::enabled`].
///
/// Deliberately has no other fields — see the module-level "No secrets
/// here" section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) executable: Option<PathBuf>,
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
    pub(super) project_hooks: Option<bool>,
    /// Acknowledgement that this harness's blanket approval bypass has been
    /// shown to and accepted by the person running Glasshouse on this
    /// machine — see [`EffectiveConfig::bypass_acknowledged`] for why this
    /// field is read from the user layer only, never the project layer.
    /// `None` means never asked, which must be treated as not acknowledged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) bypass_acknowledged: Option<bool>,
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

    /// Whether the user has acknowledged this harness's blanket approval
    /// bypass. `None` means never asked — see the field's own doc.
    pub fn bypass_acknowledged(&self) -> Option<bool> {
        self.bypass_acknowledged
    }

    pub fn set_bypass_acknowledged(&mut self, acknowledged: bool) -> &mut Self {
        self.bypass_acknowledged = Some(acknowledged);
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
