//! `[context_firewall]` — Phase 57 map lines 1991-1996: whether and how a
//! launched Claude Code session gets the context firewall's `PostToolUse`
//! bridge, and the thresholds that mode carries. Map lines 1997-2003 (Phase
//! 57B) add the semantic reducer's own fields.
//!
//! Thresholds are configuration with defaults, never architectural
//! constants (design-decisions.md §Phase 57), so this table carries them
//! rather than baking them into `crate::firewall`. Map line 1992's guarantee
//! — that no mode may enable semantic reduction on its own — now holds
//! because `reducer` is a separate field nothing in [`FirewallMode`] sets:
//! an absent `reducer` disables the whole semantic stage regardless of mode,
//! exactly as an absent field did before this field existed at all.

use std::fmt;

use serde::{Deserialize, Serialize};

/// `context_firewall.mode` — map line 1991's four modes, and no fifth.
///
/// `Off` is not merely "reduction disabled": it means no `PostToolUse` hook
/// is registered at all, so a session's command line is byte-identical to
/// one built before this phase existed. `Shadow` registers the hook and
/// runs the whole deterministic pipeline — storage, telemetry, provenance —
/// but its registered command line never carries the flag that lets a
/// result's `updatedToolOutput` reach the harness, so Claude Code always
/// sees the original. `Safe` and `Aggressive` both emit reduced output;
/// they differ only in how conservative the passthrough threshold is, never
/// in mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirewallMode {
    Off,
    Shadow,
    Safe,
    Aggressive,
}

impl FirewallMode {
    /// Every variant, in declaration order.
    pub const ALL: &'static [Self] = &[Self::Off, Self::Shadow, Self::Safe, Self::Aggressive];

    /// The one spelling this value has on the wire, in configuration and on
    /// a terminal.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Shadow => "shadow",
            Self::Safe => "safe",
            Self::Aggressive => "aggressive",
        }
    }

    /// The inverse of [`Self::as_str`]. `None` is "a spelling this build
    /// does not know", never a neighbouring variant.
    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "shadow" => Some(Self::Shadow),
            "safe" => Some(Self::Safe),
            "aggressive" => Some(Self::Aggressive),
            _ => None,
        }
    }

    /// The vocabulary, for a refusal message.
    pub fn spellings() -> String {
        Self::ALL
            .iter()
            .map(|value| format!("`{}`", value.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl fmt::Display for FirewallMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

impl Serialize for FirewallMode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for FirewallMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::from_stored(&value).ok_or_else(|| {
            serde::de::Error::custom(format!("`{value}` is not one of {}", Self::spellings()))
        })
    }
}

/// The conservative default for `safe` mode's passthrough threshold —
/// identical to `context-firewall hook --passthrough-tokens`'s own CLI
/// default (both name the same conservative starting point; this is
/// configuration's copy, not a second architectural constant).
pub const DEFAULT_PASSTHROUGH_TOKENS: u64 = 4000;

/// `aggressive` mode's default passthrough threshold. Lower than
/// [`DEFAULT_PASSTHROUGH_TOKENS`] on purpose — aggressive reduces smaller
/// results than safe does, which is the one difference map line 1991
/// permits between the two today. Both are ordinary configuration values, so
/// either may be overridden without touching this build's source.
pub const DEFAULT_AGGRESSIVE_PASSTHROUGH_TOKENS: u64 = 1500;

/// The conceptual default for `--min-semantic-tokens`: the deterministic
/// ladder's own forwarded size must exceed this before the semantic reducer
/// is ever asked, whatever mode or reducer is configured. Named "conceptual"
/// in map line 1997's package because it is ordinary configuration, not an
/// architectural constant — a user who wants semantic reduction to engage
/// sooner or later overrides it like any other threshold here.
pub const DEFAULT_MIN_SEMANTIC_TOKENS: u64 = 12_000;

/// Map lines 2023/2024: which shape of reduction policy a serving
/// entitlement's kind implies — a subscription pays in rate limits and
/// context window, a key in tokens, local inference in latency, so each gets
/// its own default thresholds.
///
/// Derived once, in `main.rs::install_context_firewall_hook`, from the
/// resolved entitlement — never carried by [`crate::firewall`] or the hook
/// subcommand, which stay entitlement-blind by construction: this type
/// exists only to select a sub-table here, and is not part of the value
/// that reaches a command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReductionPolicyKind {
    /// A harness's own first-party plan — `EntitlementKind::Claude`,
    /// `ChatGpt` or `Gemini`.
    Subscription,
    /// An API key — `EntitlementKind::ApiKey`.
    Metered,
    /// The entitlement's backing provider runs on this machine
    /// (`crate::provider::registry::Locality::Local`), or the session is
    /// served by such a provider directly with no entitlement naming it.
    Local,
}

impl ReductionPolicyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Subscription => "subscription",
            Self::Metered => "metered",
            Self::Local => "local",
        }
    }
}

impl fmt::Display for ReductionPolicyKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// The four fields a kind's sub-table, a launch profile, or an entitlement
/// may override — map lines 2023/2024. Shared shape rather than three
/// separate types, so `[context_firewall.<kind>]`,
/// `[profiles.<name>.context_firewall]` and
/// `[entitlements.<name>.context_firewall]` read and layer identically.
///
/// Deliberately narrower than [`ContextFirewallConfig`] itself: the reducer
/// name and model are a *resource*, not a *policy* — map line 1997's own
/// distinction — so they stay flat and have no place here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextFirewallOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<FirewallMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    passthrough_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    aggressive_passthrough_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    min_semantic_tokens: Option<u64>,
}

impl ContextFirewallOverride {
    pub fn is_unset(&self) -> bool {
        self.mode.is_none()
            && self.passthrough_tokens.is_none()
            && self.aggressive_passthrough_tokens.is_none()
            && self.min_semantic_tokens.is_none()
    }

    pub fn mode(&self) -> Option<FirewallMode> {
        self.mode
    }

    pub fn set_mode(&mut self, mode: Option<FirewallMode>) -> &mut Self {
        self.mode = mode;
        self
    }

    pub fn passthrough_tokens(&self) -> Option<u64> {
        self.passthrough_tokens
    }

    pub fn set_passthrough_tokens(&mut self, value: Option<u64>) -> &mut Self {
        self.passthrough_tokens = value;
        self
    }

    pub fn aggressive_passthrough_tokens(&self) -> Option<u64> {
        self.aggressive_passthrough_tokens
    }

    pub fn set_aggressive_passthrough_tokens(&mut self, value: Option<u64>) -> &mut Self {
        self.aggressive_passthrough_tokens = value;
        self
    }

    /// Reads a different field per mode, exactly like
    /// [`crate::config::EffectiveConfig::context_firewall_passthrough_tokens`]:
    /// `aggressive_passthrough_tokens` under [`FirewallMode::Aggressive`],
    /// `passthrough_tokens` under every other mode.
    pub fn passthrough_tokens_for(&self, mode: FirewallMode) -> Option<u64> {
        if mode == FirewallMode::Aggressive {
            self.aggressive_passthrough_tokens
        } else {
            self.passthrough_tokens
        }
    }

    pub fn min_semantic_tokens(&self) -> Option<u64> {
        self.min_semantic_tokens
    }

    pub fn set_min_semantic_tokens(&mut self, value: Option<u64>) -> &mut Self {
        self.min_semantic_tokens = value;
        self
    }
}

/// The `[context_firewall]` table.
///
/// Every field is optional so a layer that never touched this table has
/// none of it on disk (see [`ContextFirewallConfig::is_unset`]) — the same
/// three-state reasoning `GuardrailsConfig` already uses: `None` here means
/// "this layer never decided", not "off".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextFirewallConfig {
    /// `off`, `shadow`, `safe` or `aggressive`. `None` means this layer
    /// never decided; [`crate::config::EffectiveConfig::context_firewall_mode`]
    /// resolves the missing case to `off`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<FirewallMode>,
    /// Overrides [`DEFAULT_PASSTHROUGH_TOKENS`] for `safe` mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    passthrough_tokens: Option<u64>,
    /// Overrides [`DEFAULT_AGGRESSIVE_PASSTHROUGH_TOKENS`] for `aggressive`
    /// mode. A distinct field from `passthrough_tokens` rather than one
    /// number two modes share, because line 1991 lets aggressive move its
    /// own threshold without safe's changing underneath it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    aggressive_passthrough_tokens: Option<u64>,
    /// Map line 1997: the provider or `[entitlements.<name>]` reference the
    /// semantic reducer is routed through. `None` — the only state a user who
    /// never touched this table has — disables semantic reduction entirely,
    /// in every mode; see this module's own header for why that is map line
    /// 1992's guarantee rather than a second one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reducer: Option<String>,
    /// Pins the reducer to exactly this model on `reducer`'s resource.
    /// `None` lets [`crate::routing::disposable::DisposableRouting`] choose
    /// among whatever `reducer` names — map line 2002's free-router aliases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reducer_model: Option<String>,
    /// Overrides [`DEFAULT_MIN_SEMANTIC_TOKENS`] — map line 1997's
    /// `--min-semantic-tokens` gate, as configuration rather than only a CLI
    /// flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    min_semantic_tokens: Option<u64>,
    /// Map line 2000: `aggressive` mode keeps an `uncertain` candidate by
    /// default, exactly like `safe`. Only an explicit `true` here lets
    /// `aggressive` drop them — the recall trade this field's own name states
    /// in plain words, per the box's own requirement that the trade never be
    /// silent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    aggressive_drops_uncertain: Option<bool>,
    /// Map line 2003's "local-only operation": when `true`, the reducer is
    /// chosen only from candidates the provider registry states run locally
    /// (`crate::provider::registry::Locality::Local`) — a remote-only
    /// configuration then has no reducer at all, which is
    /// [`ContextFirewallConfig::reducer`]'s own "disabled" state, never a
    /// silent fallback to a remote one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reducer_local_only: Option<bool>,
    /// `[context_firewall.subscription]` — map line 2023's per-kind
    /// thresholds for a serving entitlement classified
    /// [`ReductionPolicyKind::Subscription`]. An unset field here falls
    /// through to the flat field above it and then to the constant, exactly
    /// like every other layer on this type.
    #[serde(default, skip_serializing_if = "ContextFirewallOverride::is_unset")]
    subscription: ContextFirewallOverride,
    /// `[context_firewall.metered]` — [`ReductionPolicyKind::Metered`]'s own
    /// thresholds, same fallthrough as [`Self::subscription`].
    #[serde(default, skip_serializing_if = "ContextFirewallOverride::is_unset")]
    metered: ContextFirewallOverride,
    /// `[context_firewall.local]` — [`ReductionPolicyKind::Local`]'s own
    /// thresholds, same fallthrough as [`Self::subscription`].
    #[serde(default, skip_serializing_if = "ContextFirewallOverride::is_unset")]
    local: ContextFirewallOverride,
}

impl ContextFirewallConfig {
    /// Whether this layer recorded nothing at all — the
    /// `skip_serializing_if` predicate, so a user who never touched the
    /// context firewall has no `[context_firewall]` table in their file.
    pub fn is_unset(&self) -> bool {
        self.mode.is_none()
            && self.passthrough_tokens.is_none()
            && self.aggressive_passthrough_tokens.is_none()
            && self.reducer.is_none()
            && self.reducer_model.is_none()
            && self.min_semantic_tokens.is_none()
            && self.aggressive_drops_uncertain.is_none()
            && self.reducer_local_only.is_none()
            && self.subscription.is_unset()
            && self.metered.is_unset()
            && self.local.is_unset()
    }

    pub fn mode(&self) -> Option<FirewallMode> {
        self.mode
    }

    pub fn set_mode(&mut self, mode: Option<FirewallMode>) -> &mut Self {
        self.mode = mode;
        self
    }

    pub fn passthrough_tokens(&self) -> Option<u64> {
        self.passthrough_tokens
    }

    pub fn set_passthrough_tokens(&mut self, value: Option<u64>) -> &mut Self {
        self.passthrough_tokens = value;
        self
    }

    pub fn aggressive_passthrough_tokens(&self) -> Option<u64> {
        self.aggressive_passthrough_tokens
    }

    pub fn set_aggressive_passthrough_tokens(&mut self, value: Option<u64>) -> &mut Self {
        self.aggressive_passthrough_tokens = value;
        self
    }

    pub fn reducer(&self) -> Option<&str> {
        self.reducer.as_deref()
    }

    pub fn set_reducer(&mut self, value: Option<String>) -> &mut Self {
        self.reducer = value;
        self
    }

    pub fn reducer_model(&self) -> Option<&str> {
        self.reducer_model.as_deref()
    }

    pub fn set_reducer_model(&mut self, value: Option<String>) -> &mut Self {
        self.reducer_model = value;
        self
    }

    pub fn min_semantic_tokens(&self) -> Option<u64> {
        self.min_semantic_tokens
    }

    pub fn set_min_semantic_tokens(&mut self, value: Option<u64>) -> &mut Self {
        self.min_semantic_tokens = value;
        self
    }

    pub fn aggressive_drops_uncertain(&self) -> Option<bool> {
        self.aggressive_drops_uncertain
    }

    pub fn set_aggressive_drops_uncertain(&mut self, value: Option<bool>) -> &mut Self {
        self.aggressive_drops_uncertain = value;
        self
    }

    pub fn reducer_local_only(&self) -> Option<bool> {
        self.reducer_local_only
    }

    pub fn set_reducer_local_only(&mut self, value: Option<bool>) -> &mut Self {
        self.reducer_local_only = value;
        self
    }

    pub fn subscription(&self) -> &ContextFirewallOverride {
        &self.subscription
    }

    pub fn subscription_mut(&mut self) -> &mut ContextFirewallOverride {
        &mut self.subscription
    }

    pub fn metered(&self) -> &ContextFirewallOverride {
        &self.metered
    }

    pub fn metered_mut(&mut self) -> &mut ContextFirewallOverride {
        &mut self.metered
    }

    pub fn local(&self) -> &ContextFirewallOverride {
        &self.local
    }

    pub fn local_mut(&mut self) -> &mut ContextFirewallOverride {
        &mut self.local
    }

    /// The sub-table [`ReductionPolicyKind`] selects — map lines 2023/2024's
    /// per-kind lookup, in one place so a caller never matches on the kind
    /// itself to pick a field.
    pub fn kind_override(&self, kind: ReductionPolicyKind) -> &ContextFirewallOverride {
        match kind {
            ReductionPolicyKind::Subscription => &self.subscription,
            ReductionPolicyKind::Metered => &self.metered,
            ReductionPolicyKind::Local => &self.local,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_round_trips_through_its_stored_spelling() {
        for mode in FirewallMode::ALL {
            assert_eq!(FirewallMode::from_stored(mode.as_str()), Some(*mode));
        }
    }

    #[test]
    fn an_unrecognized_spelling_is_none_not_a_neighbouring_variant() {
        assert_eq!(FirewallMode::from_stored("bypass"), None);
        assert_eq!(FirewallMode::from_stored(""), None);
    }

    #[test]
    fn a_freshly_defaulted_table_is_unset() {
        assert!(ContextFirewallConfig::default().is_unset());
    }

    #[test]
    fn setting_any_field_marks_the_table_no_longer_unset() {
        let mut config = ContextFirewallConfig::default();
        config.set_mode(Some(FirewallMode::Shadow));
        assert!(!config.is_unset());
    }

    #[test]
    fn the_table_serializes_to_json_and_back_unchanged() {
        let mut config = ContextFirewallConfig::default();
        config
            .set_mode(Some(FirewallMode::Aggressive))
            .set_passthrough_tokens(Some(9000))
            .set_aggressive_passthrough_tokens(Some(500))
            .set_reducer(Some("openrouter".to_owned()))
            .set_reducer_model(Some("a-free-model".to_owned()))
            .set_min_semantic_tokens(Some(20_000))
            .set_aggressive_drops_uncertain(Some(true))
            .set_reducer_local_only(Some(true));
        let json = serde_json::to_string(&config).unwrap();
        let restored: ContextFirewallConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, restored);
    }

    /// Map line 1992's guarantee, restated for the reducer fields: a table
    /// with `reducer` unset has none of Phase 57B's fields either, so a user
    /// who never configured a reducer writes no `[context_firewall]` table
    /// on their reducer's account alone.
    #[test]
    fn setting_only_a_reducer_field_marks_the_table_no_longer_unset() {
        let mut config = ContextFirewallConfig::default();
        config.set_reducer(Some("openrouter".to_owned()));
        assert!(!config.is_unset());
    }

    #[test]
    fn an_unknown_mode_spelling_is_a_load_error_naming_the_vocabulary() {
        let err = serde_json::from_str::<ContextFirewallConfig>(r#"{"mode":"stealth"}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("stealth"));
        assert!(err.contains("off"));
        assert!(err.contains("aggressive"));
    }

    #[test]
    fn a_freshly_defaulted_override_is_unset() {
        assert!(ContextFirewallOverride::default().is_unset());
    }

    #[test]
    fn setting_any_override_field_marks_it_no_longer_unset() {
        let mut over = ContextFirewallOverride::default();
        over.set_min_semantic_tokens(Some(1200));
        assert!(!over.is_unset());
    }

    #[test]
    fn a_kind_subtable_marks_the_table_no_longer_unset_without_touching_the_flat_fields() {
        let mut config = ContextFirewallConfig::default();
        config.metered_mut().set_passthrough_tokens(Some(900));
        assert!(!config.is_unset());
        assert_eq!(
            config.passthrough_tokens(),
            None,
            "the flat field is untouched"
        );
        assert_eq!(config.metered().passthrough_tokens(), Some(900));
    }

    #[test]
    fn kind_override_selects_the_matching_subtable() {
        let mut config = ContextFirewallConfig::default();
        config.subscription_mut().set_mode(Some(FirewallMode::Safe));
        config
            .metered_mut()
            .set_mode(Some(FirewallMode::Aggressive));
        config.local_mut().set_mode(Some(FirewallMode::Shadow));

        assert_eq!(
            config
                .kind_override(ReductionPolicyKind::Subscription)
                .mode(),
            Some(FirewallMode::Safe)
        );
        assert_eq!(
            config.kind_override(ReductionPolicyKind::Metered).mode(),
            Some(FirewallMode::Aggressive)
        );
        assert_eq!(
            config.kind_override(ReductionPolicyKind::Local).mode(),
            Some(FirewallMode::Shadow)
        );
    }

    #[test]
    fn passthrough_tokens_for_reads_the_aggressive_field_only_under_aggressive_mode() {
        let mut over = ContextFirewallOverride::default();
        over.set_passthrough_tokens(Some(4000))
            .set_aggressive_passthrough_tokens(Some(1500));
        assert_eq!(over.passthrough_tokens_for(FirewallMode::Safe), Some(4000));
        assert_eq!(
            over.passthrough_tokens_for(FirewallMode::Shadow),
            Some(4000)
        );
        assert_eq!(
            over.passthrough_tokens_for(FirewallMode::Aggressive),
            Some(1500)
        );
    }

    #[test]
    fn the_table_with_kind_subtables_serializes_to_json_and_back_unchanged() {
        let mut config = ContextFirewallConfig::default();
        config
            .set_mode(Some(FirewallMode::Safe))
            .set_passthrough_tokens(Some(4000));
        config.subscription_mut().set_passthrough_tokens(Some(6000));
        config.metered_mut().set_min_semantic_tokens(Some(2000));
        config.local_mut().set_mode(Some(FirewallMode::Aggressive));
        let json = serde_json::to_string(&config).unwrap();
        let restored: ContextFirewallConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, restored);
    }
}
