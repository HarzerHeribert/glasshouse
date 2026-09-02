//! Shared test fixtures for `config`'s split test modules, and the two halves themselves.
//!

use std::collections::BTreeMap;
use std::path::Path;

use crate::integrations::IntegrationId;
use crate::paths::RuntimePaths;
use crate::project::Project;

use super::*;
use super::{
    effective::*, entitlement::*, hooks::*, loading::*, profile::*, provider::*, routing_policy::*,
};

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
        .integrations_mut()
        .entry(IntegrationId::Hermes)
        .set_bypass_acknowledged(true);
    let mut profile = ProfileConfig::new(IntegrationId::ClaudeCode);
    profile.set_approval(ProfileApproval::AutomaticReview);
    config.profiles_mut().set("fast", profile);
    config.set_memory_extraction(Some(false));
    config
}

mod part_a;
mod part_b;
