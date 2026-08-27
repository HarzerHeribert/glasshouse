//! Phase 9K, from the outside: the response profile a person actually
//! resolves, against real configuration files on a real filesystem, and the
//! arguments a real launch composes from it.
//!
//! Every test here enters through one of the two functions the shipped binary
//! calls — `config::response::report`, which is the one line `main.rs`'s
//! `response` arm runs, and `HarnessSelection::install_session_document`,
//! which is what `main.rs`'s launch path runs. That is deliberate and it is
//! §35: a test that built its own `PrecedenceStack` and asked `resolve`
//! directly would pass against a build whose configuration layering had been
//! deleted, because the report would have nothing to say and nothing would
//! fail. So the profiles below are *written into TOML files*, loaded by
//! `UserConfig::load` and `load_project_config`, and the assertions are on the
//! text the binary prints and the argv it composes.

use clap::Parser;

use glasshouse::config::response::{ResponseProfileEntry, ResponseRequest};
use glasshouse::config::{self, EffectiveConfig, ProjectConfig, UserConfig};
use glasshouse::harness::HookCommand;
use glasshouse::profile::response::{Dimension, Role};
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

/// One project on disk, with whatever configuration it was given.
struct Fixture {
    workspace: tempfile::TempDir,
    data: tempfile::TempDir,
}

impl Fixture {
    fn new(user_toml: &str, project_toml: Option<&str>) -> Self {
        let workspace = new_workspace();
        let data = tempfile::tempdir().unwrap();
        std::fs::write(data.path().join("config.toml"), user_toml).unwrap();
        if let Some(project_toml) = project_toml {
            let dir = workspace.path().join(".glasshouse");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("config.toml"), project_toml).unwrap();
        }
        Self { workspace, data }
    }

    /// The report the binary would print for `request`.
    fn report(&self, request: &ResponseRequest) -> String {
        let runtime = runtime_for(self.workspace.path(), self.data.path());
        let user = UserConfig::load(runtime.paths()).unwrap();
        let project: Option<ProjectConfig> =
            config::load_project_config(runtime.project()).unwrap();
        let effective = EffectiveConfig::new(&user, project.as_ref());
        config::response::report(&effective, request)
    }

    /// The arguments `main.rs`'s launch path would put in front of a Claude
    /// Code session, and the settings document it would write.
    ///
    /// This goes through `session::select::select` and
    /// `HarnessSelection::install_session_document` — the two functions the
    /// binary calls — rather than composing the argv by hand.
    fn launch_args(&self, request: &ResponseRequest) -> (Vec<String>, Option<String>) {
        let runtime = runtime_for(self.workspace.path(), self.data.path());
        let user = UserConfig::load(runtime.paths()).unwrap();
        let project: Option<ProjectConfig> =
            config::load_project_config(runtime.project()).unwrap();
        let effective = EffectiveConfig::new(&user, project.as_ref());

        let selection =
            glasshouse::session::select::select(Some("claude-code"), effective).unwrap();
        let resolution = effective.response_profile(request);
        let application =
            glasshouse::harness::response::apply(selection.adapter(), resolution.resolved());

        let session = "test-session";
        let report = HookCommand::new(
            std::path::PathBuf::from("/nonexistent/glasshouse"),
            session,
            runtime.session_dir(session),
            runtime.project().root(),
            runtime.paths().data_dir(),
            runtime.paths().config_dir(),
        );
        let document = selection
            .install_session_document(&report, false, &application)
            .unwrap();
        let args = document.args;

        let settings =
            std::fs::read_to_string(runtime.session_dir(session).join("claude-settings.json")).ok();
        (
            args.iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
            settings,
        )
    }
}

/// A user configuration enabling Claude Code with an executable that exists,
/// so `select` resolves without needing one installed on the machine.
///
/// `/bin/sh` is never run by these tests — `install_session_document` writes a
/// file and composes arguments, and starts nothing.
fn enabled_claude_code(extra: &str) -> String {
    let executable = if cfg!(windows) {
        "C:\\\\Windows\\\\System32\\\\cmd.exe"
    } else {
        "/bin/sh"
    };
    format!(
        "version = 1\n\n[integrations.claude-code]\nenabled = true\nexecutable = \"{executable}\"\n\n\
         {extra}"
    )
}

fn request() -> ResponseRequest {
    ResponseRequest::default()
}

fn with_role(role: Role) -> ResponseRequest {
    ResponseRequest {
        role: Some(role),
        ..ResponseRequest::default()
    }
}

/// The value the report gives for one axis, and the layer it came from.
fn axis_line(report: &str, dimension: Dimension) -> String {
    report
        .lines()
        .skip_while(|line| !line.starts_with("Resolved profile"))
        .find(|line| line.trim_start().starts_with(dimension.slug()))
        .unwrap_or_else(|| panic!("no `{dimension}` line in:\n{report}"))
        .trim()
        .to_owned()
}

// ---------------------------------------------------------------- line 596

#[test]
fn every_one_of_the_six_precedence_layers_can_win_in_the_binary() {
    // Six layers, six answers, all of them resolved by the same function the
    // shipped binary calls. Written as one test because the interesting claim
    // is not that any layer works — it is that each one wins in turn as the
    // ones above it fall silent, which is the whole content of line 596.
    //
    // Verbosity is the axis under test throughout, so a layer that stopped
    // being consulted shows up as a value from the wrong layer rather than as
    // a missing line.

    // 6. Nothing configured at all: the harness default, and nothing applied.
    let bare = Fixture::new(&enabled_claude_code(""), None);
    assert!(
        axis_line(&bare.report(&request()), Dimension::Verbosity)
            .ends_with("from the harness default"),
        "an unconfigured Glasshouse must resolve to the harness's own default"
    );

    // 5. The user default.
    let user_only = Fixture::new(
        &enabled_claude_code("[response]\nverbosity = \"terse\"\n"),
        None,
    );
    assert_eq!(
        axis_line(&user_only.report(&request()), Dimension::Verbosity),
        "verbosity  terse          from the user default"
    );

    // 4. The project, over the user.
    let project = Fixture::new(
        &enabled_claude_code("[response]\nverbosity = \"terse\"\n"),
        Some("version = 1\n\n[response]\nverbosity = \"elaborate\"\n"),
    );
    assert_eq!(
        axis_line(&project.report(&request()), Dimension::Verbosity),
        "verbosity  elaborate      from the project"
    );

    // 3. The role, over the project.
    let role = Fixture::new(
        &enabled_claude_code("[response]\nverbosity = \"terse\"\n"),
        Some(
            "version = 1\n\n[response]\nverbosity = \"elaborate\"\n\n\
             [response.roles.worker]\nverbosity = \"standard\"\n",
        ),
    );
    assert_eq!(
        axis_line(&role.report(&with_role(Role::Worker)), Dimension::Verbosity),
        "verbosity  standard       from the role"
    );

    // 2. The session, over the role.
    let mut session_request = with_role(Role::Worker);
    session_request.session_preset = Some("brief".to_owned());
    assert_eq!(
        axis_line(&role.report(&session_request), Dimension::Verbosity),
        "verbosity  terse          from the session"
    );

    // 1. The task override, over everything.
    let mut task_request = session_request.clone();
    task_request
        .task
        .set_axis(Dimension::Verbosity, Some("elaborate".to_owned()));
    assert_eq!(
        axis_line(&role.report(&task_request), Dimension::Verbosity),
        "verbosity  elaborate      from the task override"
    );
}

#[test]
fn a_layer_that_sets_one_axis_leaves_the_other_four_to_the_layers_below() {
    let fixture = Fixture::new(
        &enabled_claude_code("[response]\npreset = \"brief\"\n"),
        Some("version = 1\n\n[response]\nnarration = \"detailed\"\n"),
    );
    let report = fixture.report(&request());
    assert!(
        axis_line(&report, Dimension::Narration).contains("detailed"),
        "{report}"
    );
    assert!(
        axis_line(&report, Dimension::Narration).ends_with("from the project"),
        "{report}"
    );
    assert!(
        axis_line(&report, Dimension::Format).ends_with("from the user default"),
        "the four axes the project said nothing about must come from below it:\n{report}"
    );
}

// ---------------------------------------------------------------- line 597

#[test]
fn a_projects_response_profile_does_not_reach_another_project() {
    // Two project roots, one user configuration file, one of the two carrying
    // a `[response]` table. The other must resolve as if that file did not
    // exist — which is line 597 stated as an experiment rather than as a
    // property of a struct.
    let data = tempfile::tempdir().unwrap();
    std::fs::write(
        data.path().join("config.toml"),
        enabled_claude_code("[response]\nverbosity = \"standard\"\n"),
    )
    .unwrap();

    let configured = new_workspace();
    let dir = configured.path().join(".glasshouse");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.toml"),
        "version = 1\n\n[response]\nverbosity = \"terse\"\naudience = \"executive\"\n",
    )
    .unwrap();

    let untouched = new_workspace();

    let report_for = |root: &std::path::Path| {
        let runtime = runtime_for(root, data.path());
        let user = UserConfig::load(runtime.paths()).unwrap();
        let project: Option<ProjectConfig> =
            config::load_project_config(runtime.project()).unwrap();
        let effective = EffectiveConfig::new(&user, project.as_ref());
        config::response::report(&effective, &request())
    };

    let here = report_for(configured.path());
    assert_eq!(
        axis_line(&here, Dimension::Verbosity),
        "verbosity  terse          from the project"
    );
    assert!(axis_line(&here, Dimension::Audience).contains("executive"));

    let there = report_for(untouched.path());
    assert_eq!(
        axis_line(&there, Dimension::Verbosity),
        "verbosity  standard       from the user default"
    );
    assert!(
        !axis_line(&there, Dimension::Audience).contains("executive"),
        "another project's audience reached this one:\n{there}"
    );
}

// ---------------------------------------------------------- lines 588–594

#[test]
fn no_configured_profile_can_stop_the_binary_reporting_the_four_that_matter() {
    // Line 594 and the phase's second fixed architectural requirement, tested
    // where a user could actually break it: through configuration. Every
    // preset, and the terse/silent/minimal corner that would be the natural
    // place for a concision setting to eat the diagnostics.
    let corners = [
        "preset = \"brief\"",
        "preset = \"concise-technical\"",
        "verbosity = \"terse\"\nnarration = \"silent\"\nevidence = \"minimal\"",
    ];
    for corner in corners {
        let fixture = Fixture::new(
            &enabled_claude_code(&format!("[response]\n{corner}\n")),
            None,
        );
        let report = fixture.report(&request());
        for required in ["changed files", "verification", "risks", "blockers"] {
            assert!(
                report.contains(required),
                "`{corner}` produced a report without `{required}`:\n{report}"
            );
        }
        let (args, _) = fixture.launch_args(&request());
        let joined = args.join(" ");
        for required in ["changed files", "verification", "risks", "blockers"] {
            assert!(
                joined.contains(required),
                "`{corner}` composed a launch without `{required}`:\n{joined}"
            );
        }
    }
}

#[test]
fn the_five_axes_are_reported_separately_and_one_never_moves_another() {
    // Set one axis in configuration, at the strongest layer a file can reach,
    // and check the other four stay where they were. A build in which terse
    // silently implied silent narration, or minimal evidence, dies here — and
    // it dies against configuration a user could actually write.
    let base = Fixture::new(
        &enabled_claude_code("[response]\npreset = \"standard\"\n"),
        None,
    );
    let baseline: Vec<String> = Dimension::ALL
        .iter()
        .map(|dimension| axis_line(&base.report(&request()), *dimension))
        .collect();

    let moved = Fixture::new(
        &enabled_claude_code("[response]\npreset = \"standard\"\n"),
        Some("version = 1\n\n[response]\nverbosity = \"terse\"\n"),
    );
    let after: Vec<String> = Dimension::ALL
        .iter()
        .map(|dimension| axis_line(&moved.report(&request()), *dimension))
        .collect();

    assert_ne!(baseline[0], after[0], "verbosity must have moved");
    assert_eq!(
        &baseline[1..],
        &after[1..],
        "no other axis may move with verbosity"
    );
}

// ---------------------------------------------------------------- line 595

#[test]
fn each_role_resolves_to_its_own_default_through_the_binary() {
    let fixture = Fixture::new(&enabled_claude_code(""), None);
    let mut seen: Vec<String> = Vec::new();
    for role in Role::ALL {
        let report = fixture.report(&with_role(role));
        assert!(
            report.contains(&format!("Resolved profile — role `{role}`")),
            "{report}"
        );
        seen.push(
            Dimension::ALL
                .iter()
                .map(|dimension| axis_line(&report, *dimension))
                .collect::<Vec<_>>()
                .join("|"),
        );
    }
    let distinct: std::collections::BTreeSet<&String> = seen.iter().collect();
    assert!(
        distinct.len() >= 4,
        "five roles resolved to {} distinct profiles",
        distinct.len()
    );
}

#[test]
fn a_worker_gets_its_profile_explicitly_rather_than_inheriting_one() {
    // Line 605. Nothing in the resolution has an "inherit" state: a worker
    // session names its role and every axis is resolved and attributed. The
    // check that matters is that the worker's answer differs from the
    // interactive one, so "explicit" is not a synonym for "whatever the
    // parent had".
    let fixture = Fixture::new(&enabled_claude_code(""), None);
    let worker = fixture.report(&with_role(Role::Worker));
    let interactive = fixture.report(&with_role(Role::Interactive));
    assert_ne!(
        axis_line(&worker, Dimension::Evidence),
        axis_line(&interactive, Dimension::Evidence)
    );
    assert!(axis_line(&worker, Dimension::Evidence).ends_with("from the role"));
}

// ------------------------------------------------------------ lines 601–607

#[test]
fn the_report_names_the_mechanism_each_harness_would_apply() {
    let fixture = Fixture::new(
        &enabled_claude_code("[response]\npreset = \"concise-technical\"\n"),
        None,
    );
    let report = fixture.report(&request());
    assert!(
        report.contains("Claude Code — native"),
        "Claude Code declares an output style and must prefer it:\n{report}"
    );
    assert!(
        report.contains("set to `Concise`"),
        "the harness's own vocabulary, not Glasshouse's:\n{report}"
    );
    assert!(
        report.contains("Codex — none"),
        "a harness nobody read a mechanism from must say so:\n{report}"
    );
}

#[test]
fn a_profile_no_native_style_expresses_is_recorded_as_additive_not_as_native() {
    let fixture = Fixture::new(
        &enabled_claude_code("[response]\npreset = \"audit\"\n"),
        None,
    );
    let report = fixture.report(&request());
    assert!(report.contains("Claude Code — additive"), "{report}");
    assert!(
        report.contains("--append-system-prompt"),
        "the record must name the mechanism that actually ran:\n{report}"
    );
}

#[test]
fn the_launch_carries_exactly_one_settings_flag_and_keeps_the_lifecycle_hooks() {
    // The hazard this composer exists for, verified on Claude Code 2.1.247:
    // `claude --settings A --settings B` honours only `B`. A response profile
    // that appended its own would have silently switched off every lifecycle
    // hook in the session.
    let fixture = Fixture::new(
        &enabled_claude_code("[response]\npreset = \"concise-technical\"\n"),
        None,
    );
    let (args, document) = fixture.launch_args(&request());
    assert_eq!(
        args.iter().filter(|arg| *arg == "--settings").count(),
        1,
        "a second --settings would discard the first: {args:?}"
    );
    let document = document.expect("the session document must be written");
    assert!(
        document.contains("\"outputStyle\": \"Concise\""),
        "{document}"
    );
    assert!(
        document.contains("UserPromptSubmit"),
        "the lifecycle hooks must survive the merge:\n{document}"
    );
}

#[test]
fn the_launch_appends_to_the_system_prompt_and_never_replaces_it() {
    // Line 607. `claude --help` documents both `--system-prompt` and
    // `--append-system-prompt`; only one of them is safe, and the unsafe one
    // must never appear on an argv Glasshouse composed.
    for extra in [
        "[response]\npreset = \"concise-technical\"\n",
        "[response]\npreset = \"audit\"\n",
        "[response]\npreset = \"explainer\"\n",
        "",
    ] {
        let fixture = Fixture::new(&enabled_claude_code(extra), None);
        let (args, _) = fixture.launch_args(&request());
        assert!(
            !args.iter().any(|arg| arg == "--system-prompt"),
            "`{extra}` composed a launch that replaces the system prompt: {args:?}"
        );
    }
}

#[test]
fn an_unconfigured_project_composes_a_launch_that_says_nothing_about_communication() {
    let fixture = Fixture::new(&enabled_claude_code(""), None);
    let (args, document) = fixture.launch_args(&request());
    assert!(
        !args.iter().any(|arg| arg == "--append-system-prompt"),
        "nobody asked for a response profile: {args:?}"
    );
    let document = document.expect("the hook document is still written");
    assert!(
        !document.contains("outputStyle"),
        "an unconfigured Glasshouse must leave the harness's own style alone:\n{document}"
    );
    let report = fixture.report(&request());
    assert!(
        report.contains("Claude Code — none"),
        "and it must say so:\n{report}"
    );
}

// ---------------------------------------------------------------- line 609

#[test]
fn a_configured_backend_prompt_transformation_is_surfaced_with_its_warning() {
    let fixture = Fixture::new(
        &enabled_claude_code(
            "[providers.my-gateway]\ntemplate = \"anthropic-compatible\"\n\
             base_url = \"http://127.0.0.1:9000\"\n\
             prompt_transform = \"rewrites the system prompt to add house style\"\n",
        ),
        None,
    );
    let report = fixture.report(&request());
    assert!(
        report.contains("rewrites the system prompt to add house style"),
        "{report}"
    );
    assert!(
        report.contains("may"),
        "the report must warn that it can interact with harness instructions:\n{report}"
    );
    assert!(report.contains("provider `my-gateway`"), "{report}");
}

#[test]
fn glasshouse_never_offers_a_gateway_rewrite_as_the_way_to_apply_a_profile() {
    // Line 608. With no backend transformation configured, the report says so
    // in the words that matter, and the launch below carries only the
    // harness's own mechanisms.
    let fixture = Fixture::new(
        &enabled_claude_code("[response]\npreset = \"concise-technical\"\n"),
        None,
    );
    let report = fixture.report(&request());
    assert!(
        report.contains("Glasshouse never rewrites a prompt at the gateway"),
        "{report}"
    );
    let (args, _) = fixture.launch_args(&request());
    assert!(
        args.iter()
            .all(|arg| !arg.contains("gateway") || arg.ends_with("claude-settings.json")),
        "{args:?}"
    );
}

// -------------------------------------------------- degrading visibly

#[test]
fn a_value_this_build_does_not_know_is_reported_rather_than_guessed_at() {
    let fixture = Fixture::new(
        &enabled_claude_code("[response]\nverbosity = \"chatty\"\npreset = \"verbose\"\n"),
        None,
    );
    let report = fixture.report(&request());
    assert!(report.contains("`chatty`"), "{report}");
    assert!(report.contains("`verbose`"), "{report}");
    assert!(
        axis_line(&report, Dimension::Verbosity).ends_with("from the harness default"),
        "an unreadable value must not become a silent default:\n{report}"
    );
}

#[test]
fn a_role_table_this_build_does_not_know_is_named_rather_than_ignored() {
    let fixture = Fixture::new(
        &enabled_claude_code("[response.roles.architect]\npreset = \"audit\"\n"),
        None,
    );
    let report = fixture.report(&request());
    assert!(report.contains("architect"), "{report}");
    assert!(
        report.contains("orchestrator, worker, reviewer"),
        "{report}"
    );
}

#[test]
fn an_entry_that_names_both_a_preset_and_an_axis_takes_the_axis() {
    let fixture = Fixture::new(
        &enabled_claude_code("[response]\npreset = \"brief\"\nverbosity = \"elaborate\"\n"),
        None,
    );
    let report = fixture.report(&request());
    assert!(axis_line(&report, Dimension::Verbosity).contains("elaborate"));
    assert!(
        axis_line(&report, Dimension::Format).contains("bullets"),
        "the rest of the preset must still apply:\n{report}"
    );
}

#[test]
fn a_session_preset_the_build_does_not_know_is_refused_by_name() {
    let fixture = Fixture::new(&enabled_claude_code(""), None);
    let mut request = request();
    request.session_preset = Some("laconic".to_owned());
    let report = fixture.report(&request);
    assert!(report.contains("`laconic`"), "{report}");
    assert!(report.contains("concise-technical"), "{report}");
}

#[test]
fn the_task_override_entry_is_not_configuration() {
    // A task override lives for one invocation. This is the type-level claim
    // stated as a test: `ResponseProfileEntry` is what a file holds, and the
    // request carries one that no `save` ever writes.
    let mut entry = ResponseProfileEntry::default();
    entry.set_axis(Dimension::Verbosity, Some("terse".to_owned()));
    assert_eq!(entry.axis(Dimension::Verbosity), Some("terse"));
    assert_eq!(entry.axis(Dimension::Format), None);
}
