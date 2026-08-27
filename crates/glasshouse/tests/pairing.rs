//! Phase 9J, from the outside: the pairing report a person actually runs,
//! against real configuration files on a real filesystem.
//!
//! Every test here enters through `config::pairing::report`, which is the one
//! line `main.rs`'s `pairing` arm calls. That is deliberate and it is §35:
//! a test that built its own `PairingQuery` and asked `classify` directly
//! would pass against a build whose configuration resolution had been deleted
//! — the report would have nothing to say and nothing would fail. So the
//! corrections below are *written into a TOML file*, loaded by
//! `UserConfig::load`, and the assertion is on the text the binary prints.

use clap::Parser;

use glasshouse::config::{self, EffectiveConfig, ProjectConfig, UserConfig};
use glasshouse::{Cli, Runtime, bootstrap};

fn new_workspace() -> tempfile::TempDir {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join(".git")).unwrap();
    workspace
}

fn runtime_for(workspace: &std::path::Path, data: &std::path::Path) -> Runtime {
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        data.to_str().unwrap(),
        "--config-dir",
        data.to_str().unwrap(),
    ])
    .unwrap();
    bootstrap(&cli, workspace).unwrap()
}

/// Write `contents` as the user-level configuration file and return the
/// report the binary would print for it.
fn report_for(user_toml: &str, project_toml: Option<&str>) -> String {
    let workspace = new_workspace();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("config.toml"), user_toml).unwrap();
    if let Some(project_toml) = project_toml {
        let dir = workspace.path().join(".glasshouse");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), project_toml).unwrap();
    }

    let runtime = runtime_for(workspace.path(), data.path());
    let user = UserConfig::load(runtime.paths()).unwrap();
    let project: Option<ProjectConfig> = config::load_project_config(runtime.project()).unwrap();
    let effective = EffectiveConfig::new(&user, project.as_ref());
    config::pairing::report(&effective, None, None)
}

/// A launch profile pointing Claude Code at a model on OpenRouter that
/// nothing has attributed.
const RESOLD_UNKNOWN_MODEL: &str = r#"
version = 1

[providers.openrouter]
template = "openrouter"

[profiles.glm]
harness = "claude-code"
model = "z-ai/glm-4.6"
expected_protocol = "anthropic-messages"

[profiles.glm.backend]
kind = "direct-provider"
provider = "openrouter"
"#;

/// The block of the report describing one profile.
fn profile_block<'a>(report: &'a str, name: &str) -> &'a str {
    let start = report
        .find(&format!("profile `{name}`"))
        .unwrap_or_else(|| panic!("no block for profile `{name}` in:\n{report}"));
    let rest = &report[start..];
    match rest[1..].find("\n  profile `") {
        Some(end) => &rest[..end + 1],
        None => rest,
    }
}

/// A field's value inside one profile block.
fn field(block: &str, label: &str) -> String {
    block
        .lines()
        .find(|line| line.trim_start().starts_with(label))
        .unwrap_or_else(|| panic!("no `{label}` line in:\n{block}"))
        .trim_start()
        .trim_start_matches(label)
        .trim()
        .to_owned()
}

/// Line 554, in the surface a person reads: six facts, six lines, and the
/// class beside them rather than instead of them.
#[test]
fn the_report_keeps_publisher_developer_and_server_apart() {
    let report = report_for(RESOLD_UNKNOWN_MODEL, None);
    let block = profile_block(&report, "glm");

    assert_eq!(
        field(block, "harness:"),
        "Claude Code (publisher Anthropic)"
    );
    assert_eq!(field(block, "developer:"), "unknown");
    assert_eq!(field(block, "family:"), "unknown");
    assert_eq!(field(block, "serving provider:"), "openrouter");
    assert_eq!(field(block, "gateway:"), "none");
    assert_eq!(field(block, "wire protocol:"), "anthropic-messages");
}

/// Line 560, through the whole stack: an unattributed model reaches the user
/// as `unknown`, and the wire it happens to travel over does not promote it.
#[test]
fn an_unattributed_model_reaches_the_user_as_unknown() {
    let report = report_for(RESOLD_UNKNOWN_MODEL, None);
    let block = profile_block(&report, "glm");

    assert_eq!(field(block, "pairing class:"), "unknown");
    assert_eq!(
        field(block, "protocol fit:"),
        "native",
        "the wire is still described — it is the *pairing* that is unknown"
    );
}

/// Line 561, and the box most likely to be faked: the correction is a TOML
/// table a person types, it is parsed by the ordinary configuration loader,
/// and the class the binary prints changes because of it. No router code is
/// involved.
#[test]
fn a_correction_in_the_configuration_file_changes_the_class_the_binary_prints() {
    let before = report_for(RESOLD_UNKNOWN_MODEL, None);
    assert_eq!(
        field(profile_block(&before, "glm"), "pairing class:"),
        "unknown"
    );

    let corrected = format!(
        "{RESOLD_UNKNOWN_MODEL}\n[pairing.models.\"z-ai/glm-4.6\"]\ndeveloper = \"zhipu-ai\"\n\
         family = \"glm\"\n"
    );
    let after = report_for(&corrected, None);
    let block = profile_block(&after, "glm");

    assert_eq!(field(block, "pairing class:"), "protocol-native");
    assert_eq!(field(block, "developer:"), "zhipu-ai");
    assert_eq!(field(block, "family:"), "glm");
    assert!(
        after.contains("model `z-ai/glm-4.6` (the user configuration)"),
        "a correction in effect must be visible as one, so a surprising verdict can be \
         traced to the file that caused it:\n{after}"
    );
}

/// A correction that makes Glasshouse *less* certain is a correction too. A
/// person who knows an attribution is wrong must be able to withdraw it
/// rather than being stuck with a confident error.
#[test]
fn a_correction_can_withdraw_an_attribution_glasshouse_got_wrong() {
    let toml = "version = 1\n\n[profiles.native-opus]\nharness = \"claude-code\"\nmodel = \
                \"opus\"\n";
    let plain = report_for(toml, None);
    assert_eq!(
        field(profile_block(&plain, "native-opus"), "pairing class:"),
        "vendor-native"
    );

    let withdrawn = format!("{toml}\n[pairing.models.opus]\ndeveloper = \"\"\n");
    let withdrawn = report_for(&withdrawn, None);
    let block = profile_block(&withdrawn, "native-opus");
    assert_eq!(field(block, "developer:"), "unknown");
    assert_eq!(field(block, "pairing class:"), "unknown");
}

/// A project's correction wins over the user's for the same model, and the
/// report says which file it came from — the ordinary layering every other
/// lookup on `EffectiveConfig` uses.
#[test]
fn a_projects_correction_wins_over_the_users_and_says_so() {
    let user = format!(
        "{RESOLD_UNKNOWN_MODEL}\n[pairing.models.\"z-ai/glm-4.6\"]\ndeveloper = \"wrong-org\"\n"
    );
    let project = "version = 1\n\n[pairing.models.\"z-ai/glm-4.6\"]\ndeveloper = \"zhipu-ai\"\n";
    let report = report_for(&user, Some(project));

    assert_eq!(
        field(profile_block(&report, "glm"), "developer:"),
        "zhipu-ai"
    );
    assert!(
        report.contains("model `z-ai/glm-4.6` (this project's configuration)"),
        "the project layer's correction must be shown as the project's:\n{report}"
    );
}

/// Line 559 as a user sees it: three lines, three answers, and a native wire
/// does not fill the other two in.
#[test]
fn the_three_compatibility_axes_are_three_lines() {
    let report = report_for(RESOLD_UNKNOWN_MODEL, None);
    let block = profile_block(&report, "glm");

    assert_eq!(field(block, "protocol fit:"), "native");
    assert_eq!(field(block, "model behaviour:"), "unverified");
    assert_eq!(field(block, "tool semantics:"), "unverified");
}

/// Line 557: a vendor-native pairing, end to end, from a profile that names
/// one of Anthropic's own families in Anthropic's own harness.
#[test]
fn a_first_party_profile_is_reported_as_vendor_native() {
    let toml = "version = 1\n\n[profiles.native-opus]\nharness = \"claude-code\"\nmodel = \
                \"opus\"\n";
    let report = report_for(toml, None);
    let block = profile_block(&report, "native-opus");

    assert_eq!(field(block, "pairing class:"), "vendor-native");
    assert_eq!(field(block, "developer:"), "anthropic");
    assert_eq!(
        field(block, "serving provider:"),
        "the harness's own first-party service"
    );
}

/// Line 555, in the form that would actually bite: a provider *named* after
/// a model's developer is still a service, and naming it must not attribute
/// anything.
#[test]
fn a_provider_named_after_a_developer_attributes_nothing() {
    let toml = r#"
version = 1

[providers.anthropic]
template = "openrouter"

[profiles.resold]
harness = "claude-code"
model = "some-unlisted-model"
expected_protocol = "anthropic-messages"

[profiles.resold.backend]
kind = "direct-provider"
provider = "anthropic"
"#;
    let report = report_for(toml, None);
    let block = profile_block(&report, "resold");

    assert_eq!(field(block, "serving provider:"), "anthropic");
    assert_eq!(field(block, "developer:"), "unknown");
    assert_eq!(field(block, "pairing class:"), "unknown");
}

/// A profile that names no model. The harness's own default serves the
/// session, and the report says that rather than filling in the publisher's
/// model line.
#[test]
fn a_profile_that_names_no_model_reports_no_developer() {
    let toml = "version = 1\n\n[profiles.plain]\nharness = \"claude-code\"\n";
    let report = report_for(toml, None);
    let block = profile_block(&report, "plain");

    assert_eq!(field(block, "model:"), "the harness's own default");
    assert_eq!(field(block, "developer:"), "unknown");
    assert_eq!(field(block, "pairing class:"), "unknown");
    assert_eq!(
        field(block, "harness:"),
        "Claude Code (publisher Anthropic)",
        "the publisher is known; it is simply not an attribution"
    );
}

/// Line 562: the declarative metadata is in the report, with the artifact it
/// was read from beside it, so a wrong entry can be caught by reading rather
/// than by a surprise at launch.
#[test]
fn every_harnesss_official_support_is_shown_with_its_evidence() {
    let report = report_for("version = 1\n", None);

    assert!(report.contains("Antigravity — publisher: Google"));
    assert!(
        report.contains(
            "supported models: claude-sonnet-4-6, claude-opus-4-6-thinking, \
                         gpt-oss-120b-medium"
        ),
        "Antigravity's cross-vendor support list is the evidence line 558 rests on:\n{report}"
    );
    assert!(
        report.contains("`agy models` (Antigravity CLI 1.1.21, run 2026-08-27)"),
        "a declaration without its citation is the thing this project refuses to ship:\n{report}"
    );
    assert!(
        report.contains("native families : unverified — nobody read this list"),
        "a harness nobody read a model list for must say so rather than reading as \
         `supports nothing`:\n{report}"
    );
}

/// Line 558 through the report: a model a harness vendor explicitly supports
/// but nobody attributed is vendor-supported *and* has an unknown developer.
/// The two answers are independent, and a build that let the support list
/// fill in a developer would fail here.
#[test]
fn a_vendor_supported_model_can_still_have_an_unknown_developer() {
    let workspace = new_workspace();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("config.toml"), "version = 1\n").unwrap();
    let runtime = runtime_for(workspace.path(), data.path());
    let user = UserConfig::load(runtime.paths()).unwrap();
    let effective = EffectiveConfig::new(&user, None);

    let report =
        config::pairing::report(&effective, Some("gpt-oss-120b-medium"), Some("antigravity"));
    assert!(
        report.contains("in Antigravity: vendor-supported"),
        "Google lists this model in `agy models`, so the pairing is supported:\n{report}"
    );
    assert!(
        report.contains("whose developer is unknown"),
        "a vendor's support list says nothing about who wrote the weights:\n{report}"
    );
}

/// A harness name the build does not know is answered, not ignored.
#[test]
fn an_unknown_harness_name_is_refused_by_name() {
    let workspace = new_workspace();
    let data = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("config.toml"), "version = 1\n").unwrap();
    let runtime = runtime_for(workspace.path(), data.path());
    let user = UserConfig::load(runtime.paths()).unwrap();
    let effective = EffectiveConfig::new(&user, None);

    let report = config::pairing::report(&effective, Some("opus"), Some("claud-code"));
    assert!(
        report.contains("`claud-code` is not a harness Glasshouse knows"),
        "{report}"
    );
    assert!(
        report.contains("claude-code"),
        "and it lists the real ones:\n{report}"
    );
}
