//! User-level and optional project-level Glasshouse configuration.
//! Two files, same small shape: `<config_dir>/config.toml` (user-level,
//! loaded by every run, never auto-created — [`UserConfig::load`]) and
//! `<project root>/.glasshouse/config.toml` (optional, layered over the
//! user file — [`EffectiveConfig`] — never written except by explicit user
//! consent, [`write_project_config_with_consent`]).
//! The schema is deliberately tiny (Phase 49): a field belongs here once a
//! user can actually make the decision it records. [`RoutingConfig`] and
//! [`ProfileTable`] store only inert selections — never a resolved overlay,
//! a health observation, a live price, or a credential; resolving happens
//! in [`crate::profile`] and the later router, not here.
//! No secrets here, structurally: [`IntegrationConfig`], [`ProfileConfig`]
//! and [`ProviderConfig`] hold only names and references
//! ([`StoredCredentialRef`]), never an API key or token (Phase 9E) —
//! resolving one is `SecretStore`'s job, guarded structurally by
//! `tests::serialized_form_has_no_secret_capable_field`.
//! Phase 59 split this directory by concern: [`hooks`], [`profile`],
//! [`provider`], [`entitlement`], [`routing_policy`], [`loading`] and
//! [`effective`]; this file keeps only wiring, [`ConfigError`], re-exports.
// History: design-decisions.md, "Trims: api, events, harness and config module docs, second packet", crates/glasshouse/src/config/mod.rs module doc.

pub mod capability;
pub mod effective;
pub mod entitlement;
pub mod firewall;
pub mod hooks;
pub mod loading;
pub mod pairing;
pub mod profile;
pub mod provider;
pub mod response;
pub mod routing_policy;

use std::io;
use std::path::PathBuf;

use crate::project::ScopeError;

// Internal cross-concern visibility: every concern file's items stay reachable
// from every other (and from `tests`, a descendant of this module) exactly as
// they were when all of them lived in one file's scope.
use entitlement::*;
use profile::*;
use provider::*;

pub use effective::{EffectiveConfig, ProfileDisabled};
pub use entitlement::{
    EntitlementBacking, EntitlementConfig, EntitlementCredential, EntitlementKind,
    EntitlementLookupError, EntitlementModels, EntitlementTelemetry, EntitlementVendor,
    ResolvedEntitlement, TelemetryScope,
};
pub use hooks::{IntegrationConfig, IntegrationTable};
pub use loading::{
    Layer, Layered, ProjectConfig, UserConfig, load_project_config,
    write_project_config_with_consent,
};
pub use profile::{ProfileApproval, ProfileBackend, ProfileConfig, ProfileTable};
pub use provider::{
    BudgetPeriod, ConfiguredWorkloadTier, ExtractionModelRef, FreeResourceRef, MonetaryBudget,
    ProviderConfig, ProviderTable, QuotaOverride, QuotaStaleAfterSeconds, StoredCredentialRef,
};
pub use routing_policy::{
    CapacityBandThresholdsConfig, PremiumReservePercent, ReservePoliciesConfig, RouterCostMicroUsd,
    RouterLatencyMs, RoutingConfig, RoutingFallback, RoutingModelChoice, RoutingModelResolution,
    ScoreWeightsConfig,
};

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
    ///
    /// The rendering goes through [`crate::secret::redact`], and the inner
    /// error is deliberately **not** `#[source]`: `toml`'s own `Display`
    /// quotes the whole offending line of the file under a caret, and
    /// `main.rs` prints this with `{err:#}`, which walks the chain. A file
    /// that carried a pasted key on the line that failed to parse would
    /// otherwise copy it to stderr and into `glasshouse.log` — the case
    /// `crate::secret::redact` documents itself as existing for.
    #[error(
        "configuration file `{path}` is not valid TOML: {}",
        crate::secret::redact(&.source.to_string())
    )]
    Parse {
        path: PathBuf,
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
    /// (see [`UserConfig::load`] / [`load_project_config`]) still succeeds — refusing to
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

/// A `bool` that is only worth writing when it is `true`.
///
/// Used by `serde`'s `skip_serializing_if` so that an unpinned profile — which
/// is every profile that has never been pinned — serialises to exactly what it
/// did before the field existed.
fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests;
