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

fn no_path_lookup(what: &'static str) -> impl Fn(&str) -> Result<ResolvedExecutable, ResolveError> {
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

/// Map line 1514 (acting path): `resolve_executable` (`:554-586`) is
/// asked before `launch_session` ever calls `routing_destinations`, and
/// a harness with no configured executable and no candidate name found
/// on `PATH` is refused with `SelectionError::NotInstalled` — never a
/// resolved executable a routing candidate could then be built from. The
/// census's mutation (bypass this check and fabricate a resolved
/// executable) would let a not-installed harness's candidates reach
/// generation.
#[test]
fn an_uninstalled_harness_is_refused_before_any_routing_candidate_could_exist_1514() {
    let user = UserConfig::default();

    let err = select_with(
        Some("claude-code"),
        EffectiveConfig::new(&user, None),
        no_configured_lookup("no path is configured for this harness"),
        |name| {
            Err(ResolveError::NotFound {
                name: name.to_owned(),
            })
        },
    )
    .unwrap_err();

    match err {
        SelectionError::NotInstalled { id } => {
            assert_eq!(id, IntegrationId::ClaudeCode);
        }
        other => panic!("expected NotInstalled, got {other:?}"),
    }
    assert!(err.to_string().contains("is not installed"), "{err}");
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
