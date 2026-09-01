//! `[context_firewall]` — Phase 57 map lines 1991-1996: whether and how a
//! launched Claude Code session gets the context firewall's `PostToolUse`
//! bridge, and the thresholds that mode carries.
//!
//! Thresholds are configuration with defaults, never architectural
//! constants (design-decisions.md §Phase 57), so this table carries them
//! rather than baking them into `crate::firewall`. What it never carries —
//! by construction, since no field exists for it — is a reducer name: map
//! line 1992 requires that no mode may enable semantic reduction until a
//! later package adds that field, and an absent field is a stronger
//! guarantee than a documentation note.

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
}

impl ContextFirewallConfig {
    /// Whether this layer recorded nothing at all — the
    /// `skip_serializing_if` predicate, so a user who never touched the
    /// context firewall has no `[context_firewall]` table in their file.
    pub fn is_unset(&self) -> bool {
        self.mode.is_none()
            && self.passthrough_tokens.is_none()
            && self.aggressive_passthrough_tokens.is_none()
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
            .set_aggressive_passthrough_tokens(Some(500));
        let json = serde_json::to_string(&config).unwrap();
        let restored: ContextFirewallConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, restored);
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
}
