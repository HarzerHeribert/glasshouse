use super::*;

fn test_project() -> (tempfile::TempDir, Project) {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join(".git")).unwrap();
    let project = Project::discover(tmp.path(), None, false).unwrap();
    (tmp, project)
}

// --- catalog integrity ---------------------------------------------

#[test]
fn every_integration_has_a_non_empty_slug_and_display_name() {
    for &id in IntegrationId::ALL {
        assert!(!id.slug().is_empty(), "{id:?} has an empty slug");
        assert!(
            !id.display_name().is_empty(),
            "{id:?} has an empty display name"
        );
        assert!(
            !id.executable_candidates().is_empty(),
            "{id:?} has no executable candidates"
        );
    }
}

#[test]
fn slugs_are_unique() {
    let mut slugs: Vec<&str> = IntegrationId::ALL.iter().map(|&id| id.slug()).collect();
    slugs.sort_unstable();
    let mut deduped = slugs.clone();
    deduped.dedup();
    assert_eq!(slugs, deduped, "duplicate slug found in catalog");
}

#[test]
fn no_minimum_version_is_declared_yet() {
    // Documents the deliberate current state; see minimum_version's doc
    // comment for why. If this ever legitimately changes, update this
    // test alongside the new minimum.
    for &id in IntegrationId::ALL {
        assert!(id.minimum_version().is_none());
    }
}

#[test]
fn no_integration_is_searched_for_under_a_guessed_abbreviation() {
    // This test used to assert that Antigravity was searched for as
    // `antigravity` and nothing else — a carefully reasoned guess, made
    // when no reference install existed, and simply wrong: the published
    // Antigravity CLI links its binary onto PATH as `agy`. Glasshouse
    // would never have found a real one.
    //
    // What was right about the original is the hazard it guarded, so that
    // is what survives here. `ag` is the-silver-searcher on a great many
    // machines; resolving it would start an unrelated program as a coding
    // harness, and a confident wrong detection is worse than a missed one.
    // Names come from real installs now — never from abbreviating a
    // product's name and hoping.
    for &id in IntegrationId::ALL {
        for &name in id.executable_candidates() {
            assert_ne!(
                name,
                "ag",
                "{} would resolve the-silver-searcher as a harness",
                id.slug()
            );
        }
    }
    assert_eq!(
        IntegrationId::Antigravity.executable_candidates(),
        &["agy", "antigravity"]
    );
}

// --- status display ---------------------------------------------------

#[test]
fn status_display_has_no_debug_artifacts() {
    for status in [
        IntegrationStatus::Available,
        IntegrationStatus::Configured,
        IntegrationStatus::Unconfigured,
        IntegrationStatus::UnsupportedVersion,
        IntegrationStatus::NotFound,
        IntegrationStatus::Unknown,
    ] {
        let s = status.to_string();
        assert!(!s.contains("Integration"));
        assert!(!s.is_empty());
    }
}

#[test]
fn not_found_and_unknown_render_distinct_labels() {
    assert_eq!(IntegrationStatus::NotFound.to_string(), "not found");
    assert_eq!(IntegrationStatus::Unknown.to_string(), "unknown");
    assert_ne!(
        IntegrationStatus::NotFound.to_string(),
        IntegrationStatus::Unknown.to_string()
    );
}

// --- config_evidence ----------------------------------------------

#[test]
fn claude_code_evidence_detects_claude_dir() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".claude")).unwrap();
    let (result, notes) = config_evidence(IntegrationId::ClaudeCode, Some(dir.path()));
    assert_eq!(result, ConfigEvidence::Configured);
    assert!(!notes.is_empty());
}

#[test]
fn claude_code_evidence_is_unconfigured_with_nothing_present() {
    let dir = tempfile::tempdir().unwrap();
    let (result, _) = config_evidence(IntegrationId::ClaudeCode, Some(dir.path()));
    assert_eq!(result, ConfigEvidence::Unconfigured);
}

#[test]
fn config_evidence_distinguishes_tools_needing_no_config_from_unknown_harness() {
    let dir = tempfile::tempdir().unwrap();
    // cmux and llama.cpp need no user credentials -> Available
    for id in [IntegrationId::Cmux, IntegrationId::LlamaCpp] {
        let (result, notes) = config_evidence(id, Some(dir.path()));
        assert_eq!(result, ConfigEvidence::Available);
        assert!(notes.is_empty());
    }
    // Antigravity needs credentials/setup but has no reliable signal -> Unknown
    let (result, notes) = config_evidence(IntegrationId::Antigravity, Some(dir.path()));
    assert_eq!(result, ConfigEvidence::Unknown);
    assert!(!notes.is_empty());
    assert!(
        notes
            .iter()
            .any(|n| n.contains("no reliable configuration signal"))
    );
}

#[test]
fn detected_harness_with_indeterminate_config_is_unknown_and_usable() {
    let (_guard, project) = test_project();
    let exe = exec::resolve("sh").expect("sh on PATH");
    let d = detect_one_with_prober(
        IntegrationId::Antigravity,
        None,
        &project,
        |_| Ok(exe.clone()),
        |_| Vec::new(),
        |_, _, _| Ok(None),
        |_| None,
    );
    assert_eq!(d.status(), IntegrationStatus::Unknown);
    assert!(d.executable().is_some());
    assert!(
        d.is_usable(),
        "detected unknown harness must still be launchable"
    );
    assert!(
        d.problems().is_empty(),
        "indeterminate config is not an error/problem"
    );
    assert!(
        d.evidence()
            .iter()
            .any(|e| e.contains("no reliable configuration signal"))
    );
}

#[test]
fn detected_tool_requiring_no_config_is_available_and_usable() {
    let (_guard, project) = test_project();
    let exe = exec::resolve("sh").expect("sh on PATH");
    let d = detect_one_with_prober(
        IntegrationId::Cmux,
        None,
        &project,
        |_| Ok(exe.clone()),
        |_| Vec::new(),
        |_, _, _| Ok(None),
        |_| None,
    );
    assert_eq!(d.status(), IntegrationStatus::Available);
    assert!(d.executable().is_some());
    assert!(d.is_usable());
    assert!(d.problems().is_empty());
}

#[test]
fn detected_harness_with_probed_version_below_minimum_is_unsupported_version_and_not_usable() {
    let (_guard, project) = test_project();
    let exe = exec::resolve("sh").expect("sh on PATH");
    let probed_ver = version::parse_version("1.0.0").unwrap();
    let min_ver = version::parse_version("2.0.0").unwrap();
    let d = detect_one_with_prober(
        IntegrationId::ClaudeCode,
        None,
        &project,
        |_| Ok(exe.clone()),
        |_| Vec::new(),
        move |_, _, _| Ok(Some(probed_ver.clone())),
        move |_| Some(min_ver.clone()),
    );
    assert_eq!(d.status(), IntegrationStatus::UnsupportedVersion);
    assert!(
        !d.is_usable(),
        "unsupported version must never be reported as usable"
    );
    assert_eq!(d.problems().len(), 1);
    assert!(d.problems()[0].contains("below the minimum supported version"));
}

#[test]
fn detected_harness_with_probed_version_satisfying_minimum_preserves_status_and_is_usable() {
    let (_guard, project) = test_project();
    let exe = exec::resolve("sh").expect("sh on PATH");
    let probed_ver = version::parse_version("2.5.0").unwrap();
    let min_ver = version::parse_version("2.0.0").unwrap();
    let d = detect_one_with_prober(
        IntegrationId::ClaudeCode,
        None,
        &project,
        |_| Ok(exe.clone()),
        |_| Vec::new(),
        move |_, _, _| Ok(Some(probed_ver.clone())),
        move |_| Some(min_ver.clone()),
    );
    assert_eq!(d.status(), IntegrationStatus::Unconfigured);
    assert_ne!(d.status(), IntegrationStatus::UnsupportedVersion);
    assert!(d.is_usable());
    assert!(d.problems().is_empty());
}

// --- resolve_first_usable / resolve_first_usable_with ------------------

#[test]
fn resolve_first_usable_reports_plain_not_found_with_no_candidates_present() {
    let outcome = resolve_first_usable_with(
        &["definitely-not-a-real-glasshouse-integration-xyz"],
        exec::resolve,
    );
    assert!(matches!(outcome, ResolveOutcome::NotFound));
}

#[test]
fn injected_not_found_resolver_yields_not_found_outcome() {
    let outcome = resolve_first_usable_with(&["codex"], |name| {
        Err(ResolveError::NotFound {
            name: name.to_string(),
        })
    });
    assert!(matches!(outcome, ResolveOutcome::NotFound));
}

#[test]
fn injected_interop_only_resolver_yields_unusable_not_not_found() {
    let outcome = resolve_first_usable_with(&["codex"], |name| {
        Err(ResolveError::WindowsInteropOnly {
            name: name.to_string(),
            found_at: vec![PathBuf::from("/mnt/c/codex.exe")],
        })
    });
    assert!(matches!(outcome, ResolveOutcome::Unusable(_)));
}

#[test]
fn unusable_hit_takes_priority_over_a_later_plain_miss() {
    // First candidate is interop-only, second is genuinely absent: the
    // more specific, more actionable finding must win.
    let outcome = resolve_first_usable_with(&["llama-server", "llama-cli"], |name| {
        if name == "llama-server" {
            Err(ResolveError::WindowsInteropOnly {
                name: name.to_string(),
                found_at: vec![PathBuf::from("/mnt/c/llama-server.exe")],
            })
        } else {
            Err(ResolveError::NotFound {
                name: name.to_string(),
            })
        }
    });
    assert!(matches!(outcome, ResolveOutcome::Unusable(_)));
}

// --- detect_one_with -----------------------------------------------

#[test]
fn not_found_produces_no_problem_but_records_what_was_tried() {
    let (_guard, project) = test_project();
    let d = detect_one_with(
        IntegrationId::Codex,
        None,
        &project,
        |name| {
            Err(ResolveError::NotFound {
                name: name.to_string(),
            })
        },
        |_| Vec::new(),
    );
    assert_eq!(d.status(), IntegrationStatus::NotFound);
    assert!(d.executable().is_none());
    assert!(
        d.problems().is_empty(),
        "plain absence must not be reported as a problem, got: {:?}",
        d.problems()
    );
    assert!(d.evidence().iter().any(|e| e.contains("codex")));
}

#[test]
fn interop_only_hit_is_unknown_with_an_actionable_problem() {
    let (_guard, project) = test_project();
    let d = detect_one_with(
        IntegrationId::Codex,
        None,
        &project,
        |name| {
            Err(ResolveError::WindowsInteropOnly {
                name: name.to_string(),
                found_at: vec![PathBuf::from("/mnt/c/codex.exe")],
            })
        },
        |_| Vec::new(),
    );
    assert_eq!(d.status(), IntegrationStatus::Unknown);
    assert!(d.executable().is_none());
    assert_eq!(d.problems().len(), 1);
}

// --- presence_without_executable_with --------------------------------

#[test]
fn cmux_socket_path_set_yields_evidence_naming_it() {
    let notes = presence_without_executable_with(IntegrationId::Cmux, |name| match name {
        "CMUX_SOCKET_PATH" => Some("/tmp/cmux-socket".to_string()),
        _ => None,
    });
    assert!(!notes.is_empty());
    assert!(notes.iter().any(|n| n.contains("CMUX_SOCKET_PATH")));
}

#[test]
fn cmux_corroborating_variables_are_also_named() {
    let notes = presence_without_executable_with(IntegrationId::Cmux, |name| match name {
        "CMUX_SOCKET_PATH" => Some("/tmp/cmux-socket".to_string()),
        "CMUX_SURFACE_ID" => Some("surf".to_string()),
        _ => None,
    });
    assert!(notes.iter().any(|n| n.contains("CMUX_SOCKET_PATH")));
    assert!(notes.iter().any(|n| n.contains("CMUX_SURFACE_ID")));
}

#[test]
fn empty_cmux_socket_path_counts_as_unset() {
    let notes = presence_without_executable_with(IntegrationId::Cmux, |name| match name {
        "CMUX_SOCKET_PATH" => Some(String::new()),
        _ => None,
    });
    assert!(
        notes.is_empty(),
        "an empty variable must count as unset, got: {notes:?}"
    );
}

#[test]
fn no_cmux_variables_yields_no_evidence() {
    let notes = presence_without_executable_with(IntegrationId::Cmux, |_| None);
    assert!(notes.is_empty());
}

#[test]
fn ollama_host_set_unset_and_empty() {
    let set = presence_without_executable_with(IntegrationId::Ollama, |name| match name {
        "OLLAMA_HOST" => Some("http://127.0.0.1:11434".to_string()),
        _ => None,
    });
    assert!(!set.is_empty());
    assert!(set.iter().any(|n| n.contains("OLLAMA_HOST")));

    assert!(presence_without_executable_with(IntegrationId::Ollama, |_| None).is_empty());

    let empty = presence_without_executable_with(IntegrationId::Ollama, |name| match name {
        "OLLAMA_HOST" => Some(String::new()),
        _ => None,
    });
    assert!(
        empty.is_empty(),
        "empty must count as unset, got: {empty:?}"
    );
}

#[test]
fn evidence_notes_never_contain_a_value_only_names() {
    // The security-critical assertion: with unmistakable sentinel values
    // in every variable this function may consult, no produced note may
    // contain any of them anywhere.
    let sentinels = [
        "SECRET-SOCKET-VALUE-12345",
        "SECRET-SURFACE-VALUE-67890",
        "SECRET-WORKSPACE-VALUE-24680",
        "SECRET-ENDPOINT-VALUE-13579",
    ];
    let lookup = |name: &str| match name {
        "CMUX_SOCKET_PATH" => Some("SECRET-SOCKET-VALUE-12345".to_string()),
        "CMUX_SURFACE_ID" => Some("SECRET-SURFACE-VALUE-67890".to_string()),
        "CMUX_WORKSPACE_ID" => Some("SECRET-WORKSPACE-VALUE-24680".to_string()),
        "OLLAMA_HOST" => Some("SECRET-ENDPOINT-VALUE-13579".to_string()),
        _ => None,
    };
    for id in [IntegrationId::Cmux, IntegrationId::Ollama] {
        for note in presence_without_executable_with(id, lookup) {
            for sentinel in sentinels {
                assert!(
                    !note.contains(sentinel),
                    "note leaked a value ({sentinel}): {note:?}"
                );
            }
        }
    }
}

// --- detect_one_with x presence wiring -------------------------------

#[test]
fn absent_executable_but_presence_evidence_is_configured_not_launchable() {
    let (_guard, project) = test_project();
    let d = detect_one_with(
        IntegrationId::Ollama,
        None,
        &project,
        |name| {
            Err(ResolveError::NotFound {
                name: name.to_string(),
            })
        },
        |id| {
            assert_eq!(id, IntegrationId::Ollama);
            vec!["OLLAMA_HOST is set".to_string()]
        },
    );
    assert_eq!(d.status(), IntegrationStatus::Configured);
    assert!(d.executable().is_none(), "no executable was resolved");
    assert!(d.version().is_none());
    assert!(
        !d.is_usable(),
        "detected-but-unlaunchable must never be mistaken for launchable"
    );
    assert!(d.problems().is_empty());
    // Evidence shows BOTH the failed PATH search and why it is present.
    assert!(d.evidence().iter().any(|e| e.contains("candidates tried")));
    assert!(d.evidence().iter().any(|e| e.contains("OLLAMA_HOST")));
}

#[test]
fn absent_executable_with_no_presence_evidence_stays_not_found() {
    let (_guard, project) = test_project();
    let d = detect_one_with(
        IntegrationId::Codex,
        None,
        &project,
        |name| {
            Err(ResolveError::NotFound {
                name: name.to_string(),
            })
        },
        |_| Vec::new(),
    );
    assert_eq!(d.status(), IntegrationStatus::NotFound);
    assert!(d.executable().is_none());
    assert!(d.problems().is_empty());
}

// --- Discovery::run ------------------------------------------------

#[test]
fn discovery_runs_without_panicking_and_covers_the_whole_catalog() {
    let (_guard, project) = test_project();
    let discovery = Discovery::run(&project);
    assert_eq!(discovery.all().len(), IntegrationId::ALL.len());
    for &id in IntegrationId::ALL {
        assert!(discovery.get(id).is_some());
    }
    // A `NotFound` entry must never carry a problem (plain absence is
    // not a problem); an `Unknown` entry (found but unusable) must.
    for d in discovery.all() {
        match d.status() {
            IntegrationStatus::NotFound => assert!(
                d.problems().is_empty(),
                "{:?} is NotFound but has problems: {:?}",
                d.id(),
                d.problems()
            ),
            IntegrationStatus::Unknown if d.executable().is_none() => assert!(
                !d.problems().is_empty(),
                "{:?} is Unknown-and-absent but recorded no problem",
                d.id()
            ),
            _ => {}
        }
    }
}

// --- Discovery::problems ---------------------------------------------

#[test]
fn no_harness_detected_produces_exactly_one_discovery_level_problem() {
    let integrations: Vec<DetectedIntegration> = IntegrationId::ALL
        .iter()
        .map(|&id| DetectedIntegration {
            id,
            status: IntegrationStatus::NotFound,
            executable: None,
            version: None,
            evidence: Vec::new(),
            problems: Vec::new(),
        })
        .collect();
    let discovery = Discovery {
        integrations,
        providers: ProviderSignals::default(),
    };
    let problems = discovery.problems();
    assert_eq!(
        problems.len(),
        1,
        "expected exactly one problem: {problems:?}"
    );
    assert!(problems[0].contains("no supported coding-agent harness"));
}

#[test]
fn discovery_level_problem_absent_once_any_harness_is_detected() {
    let Ok(exe) = exec::resolve("sh") else {
        eprintln!("skipping: `sh` is not on PATH");
        return;
    };
    let mut integrations: Vec<DetectedIntegration> = IntegrationId::ALL
        .iter()
        .map(|&id| DetectedIntegration {
            id,
            status: IntegrationStatus::NotFound,
            executable: None,
            version: None,
            evidence: Vec::new(),
            problems: Vec::new(),
        })
        .collect();
    integrations[0] = DetectedIntegration {
        id: IntegrationId::ClaudeCode,
        status: IntegrationStatus::Available,
        executable: Some(exe),
        version: None,
        evidence: Vec::new(),
        problems: Vec::new(),
    };
    let discovery = Discovery {
        integrations,
        providers: ProviderSignals::default(),
    };
    assert!(discovery.problems().is_empty());
}

#[test]
fn harnesses_and_available_harnesses_are_consistent() {
    let (_guard, project) = test_project();
    let discovery = Discovery::run(&project);
    let harness_ids: Vec<_> = discovery.harnesses().map(|d| d.id()).collect();
    assert!(harness_ids.contains(&IntegrationId::ClaudeCode));
    assert!(harness_ids.contains(&IntegrationId::Codex));
    assert!(harness_ids.contains(&IntegrationId::Antigravity));
    assert!(harness_ids.contains(&IntegrationId::OpenCode));
    assert!(!harness_ids.contains(&IntegrationId::Cmux));

    for d in discovery.available_harnesses() {
        assert!(d.is_usable());
        assert_eq!(d.kind(), IntegrationKind::Harness);
    }
}

// --- doctor_report ---------------------------------------------------

#[test]
fn doctor_report_includes_project_identity_and_never_panics() {
    use clap::Parser;

    let data = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join(".git")).unwrap();

    let cli = crate::Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        data.path().to_str().unwrap(),
        "--config-dir",
        data.path().to_str().unwrap(),
    ])
    .unwrap();
    let runtime = crate::bootstrap(&cli, workspace.path()).unwrap();

    let report = doctor_report(&runtime);
    assert!(report.contains(&runtime.project().name()));
    assert!(report.contains("Harnesses"));
    assert!(report.contains("Optional integrations"));
    assert!(report.contains("Provider signals"));
    assert!(report.contains("Problems"));
}

/// `doctor` is where an adapter's declarations become visible to a user,
/// and so it is the production caller that keeps them from being a
/// write-only data structure.
///
/// Asserted against the specific rows for one harness rather than against
/// the whole report: a `contains` over a screenful of text passes for
/// reasons that have nothing to do with the thing under test.
#[test]
fn the_doctor_report_shows_each_adapters_declarations() {
    use clap::Parser;

    let data = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join(".git")).unwrap();

    let cli = crate::Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        data.path().to_str().unwrap(),
        "--config-dir",
        data.path().to_str().unwrap(),
    ])
    .unwrap();
    let runtime = crate::bootstrap(&cli, workspace.path()).unwrap();
    let report = doctor_report(&runtime);

    assert!(report.contains("Harness adapters"));

    // The Claude Code block: its heading row, then the rows under it, each
    // located on its own rather than searched for anywhere in the report.
    let adapters_section = report
        .split("Harness adapters")
        .nth(1)
        .expect("a harness adapters section");
    let block: Vec<&str> = adapters_section
        .lines()
        .skip_while(|line| !line.trim_start().starts_with("Claude Code"))
        // Nine: the heading plus eight declaration rows. `edit intent:`
        // (Phase 60 map line 2404) is the ninth line and the newest.
        .take(9)
        .collect();
    assert!(
        !block.is_empty(),
        "the report has no Claude Code adapter block"
    );

    let heading = block[0];
    assert!(
        heading.contains("Anthropic"),
        "adapter heading does not name the vendor: {heading:?}"
    );
    assert!(
        heading.contains("`claude`"),
        "adapter heading does not name the executable: {heading:?}"
    );

    let row = |label: &str| {
        block
            .iter()
            .find(|line| line.trim_start().starts_with(label))
            .unwrap_or_else(|| panic!("no `{label}` row in {block:?}"))
    };
    assert!(row("resume:").contains("--resume"));
    assert!(row("session ids:").contains("--session-id"));
    assert!(row("hooks:").contains("settings"));
    // Both halves: the argv that actually selects the mode, and the
    // absence of the `auto-mode` subcommand this row used to name. That
    // subcommand inspects the classifier's configuration and would not
    // have started a session at all.
    assert!(row("approvals:").contains("--permission-mode auto"));
    assert!(
        !row("approvals:").contains("auto-mode"),
        "the approvals row must not name the `auto-mode` subcommand: {}",
        row("approvals:")
    );
    assert!(row("capabilities:").contains("MCP"));
    assert!(row("protocols:").contains("anthropic-messages"));
    assert!(row("model:").contains("--model"));
    // Map line 2404: Claude Code is the one harness with a verified
    // structured pre-tool hook, and the report says which event it is.
    assert!(row("edit intent:").contains("`PreToolUse`"));
    assert!(row("edit intent:").contains("available"));
}

/// Every harness gets a block, not only the ones that happen to be
/// installed on the machine running the tests.
#[test]
fn the_doctor_report_describes_every_harness_adapter() {
    use clap::Parser;

    let data = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join(".git")).unwrap();

    let cli = crate::Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        data.path().to_str().unwrap(),
        "--config-dir",
        data.path().to_str().unwrap(),
    ])
    .unwrap();
    let runtime = crate::bootstrap(&cli, workspace.path()).unwrap();
    let report = doctor_report(&runtime);

    let adapters_section = report
        .split("Harness adapters")
        .nth(1)
        .expect("a harness adapters section");
    for adapter in crate::harness::all() {
        let name = adapter.id().display_name();
        assert!(
            adapters_section.contains(name),
            "{name} has an adapter but no block in the doctor report"
        );
    }
}

// --- Configured providers (Phase 9C/9D) -------------------------------

/// Bootstrap a `Runtime` over fresh, isolated data/config/workspace
/// directories — the shared setup every doctor test below needs.
fn bootstrapped_runtime() -> (tempfile::TempDir, tempfile::TempDir, crate::Runtime) {
    use clap::Parser;

    let data = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join(".git")).unwrap();

    let cli = crate::Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        data.path().to_str().unwrap(),
        "--config-dir",
        data.path().to_str().unwrap(),
    ])
    .unwrap();
    let runtime = crate::bootstrap(&cli, workspace.path()).unwrap();
    (data, workspace, runtime)
}

#[test]
fn the_doctor_report_says_none_configured_with_no_providers_set_up() {
    let (_data, _workspace, runtime) = bootstrapped_runtime();

    let report = doctor_report(&runtime);
    assert!(report.contains("Configured providers"));
    let section = report
        .split("Configured providers")
        .nth(1)
        .expect("a configured providers section");
    let first_line = section
        .lines()
        .find(|l| !l.trim().is_empty())
        .expect("at least one line after the heading");
    assert!(first_line.contains("none configured"), "{first_line:?}");
}

/// `doctor` is where a configured provider's resolved shape becomes
/// visible to a user: which protocol, at which base URL (including an
/// override), and which credential variable names to set. Asserted
/// against the specific block rather than the whole report, for the same
/// reason `the_doctor_report_shows_each_adapters_declarations` is.
#[test]
fn the_doctor_report_shows_a_configured_providers_protocol_and_base_url() {
    let (_data, _workspace, runtime) = bootstrapped_runtime();

    let mut user = crate::config::UserConfig::load(runtime.paths()).unwrap();
    let mut provider = crate::config::ProviderConfig::new("openrouter");
    provider.set_base_url(Some("https://mirror.example.com/v1".to_owned()));
    user.providers_mut().set("my-router", provider);
    user.save(runtime.paths()).unwrap();

    let report = doctor_report(&runtime);
    let section = report
        .split("Configured providers")
        .nth(1)
        .expect("a configured providers section");
    // Not a fixed line count: openrouter now declares more than one
    // protocol (line 353), so its block is longer than it used to be.
    // This test configures the only provider in the section, so taking
    // every line up to the section's own trailing blank line is exactly
    // this provider's block, however many protocols it grows to.
    let block: Vec<&str> = section
        .lines()
        .skip_while(|line| !line.trim_start().starts_with("my-router"))
        .take_while(|line| !line.trim().is_empty())
        .collect();
    assert!(!block.is_empty(), "no `my-router` block in the report");

    assert!(block[0].contains("layer: user"), "{block:?}");
    let protocol_line = block
        .iter()
        .find(|l| l.contains("openai-chat"))
        .unwrap_or_else(|| panic!("no openai-chat row in {block:?}"));
    assert!(
        protocol_line.contains("https://mirror.example.com/v1"),
        "{protocol_line:?}"
    );
    let credential_line = block
        .iter()
        .find(|l| l.contains("credential env"))
        .unwrap_or_else(|| panic!("no credential env row in {block:?}"));
    assert!(
        credential_line.contains("OPENROUTER_API_KEY"),
        "{credential_line:?}"
    );
}

/// The one test this file exists to make pass for providers: a credential
/// variable set to an unmistakable secret-shaped value in the test
/// process must never appear in the report, while its name must.
#[test]
fn the_doctor_report_names_variable_names_and_never_values() {
    const VAR_NAME: &str = "GLASSHOUSE_DOCTOR_TEST_ONLY_SECRET_VAR";
    const SECRET_VALUE: &str = "sk-doctor-test-totally-real-looking-secret-xyz123";

    let (_data, _workspace, runtime) = bootstrapped_runtime();

    let mut user = crate::config::UserConfig::load(runtime.paths()).unwrap();
    let mut provider = crate::config::ProviderConfig::new("openrouter");
    provider.set_credential_env(vec![VAR_NAME.to_owned()]);
    user.providers_mut().set("secret-test", provider);
    user.save(runtime.paths()).unwrap();

    // SAFETY: `VAR_NAME` is unique to this test and is always removed
    // again before returning, including on the panic paths below, so no
    // other test can observe it set.
    unsafe {
        std::env::set_var(VAR_NAME, SECRET_VALUE);
    }
    let report = doctor_report(&runtime);
    unsafe {
        std::env::remove_var(VAR_NAME);
    }

    assert!(
        !report.contains(SECRET_VALUE),
        "the doctor report must never contain a credential's value"
    );
    assert!(
        report.contains(VAR_NAME),
        "the doctor report must name the credential variable"
    );
    assert!(
        report.contains(&format!("{VAR_NAME} (set")),
        "the doctor report must say the variable is set: {report}"
    );
}

/// Phase 9E line 2 at the one surface a user runs to find out what
/// Glasshouse believes: the report says **which** store a credential
/// would be read from, and names the fallback when there is no native
/// one. A user must never have to guess whether their key is in the
/// Keychain or in a shell profile.
#[test]
fn the_doctor_report_says_which_secret_store_credentials_come_from() {
    const VAR_NAME: &str = "GLASSHOUSE_DOCTOR_TEST_ONLY_STORE_LABEL_VAR";
    const SECRET_VALUE: &str = "sk-doctor-store-label-test-0123456789abcd";

    let (_data, _workspace, runtime) = bootstrapped_runtime();

    let mut user = crate::config::UserConfig::load(runtime.paths()).unwrap();
    let mut provider = crate::config::ProviderConfig::new("openrouter");
    provider
        .set_credential_env(vec![VAR_NAME.to_owned()])
        .set_credential_store(Some(crate::config::StoredCredentialRef::new(
            crate::secret::native::SERVICE,
            VAR_NAME,
        )));
    user.providers_mut().set("store-label-test", provider);
    user.save(runtime.paths()).unwrap();

    // SAFETY: `VAR_NAME` is unique to this test and is removed again
    // before any assertion that could fail.
    unsafe {
        std::env::set_var(VAR_NAME, SECRET_VALUE);
    }
    let report = doctor_report(&runtime);
    unsafe {
        std::env::remove_var(VAR_NAME);
    }

    // The value is never in the report, whichever store answered.
    assert!(
        !report.contains(SECRET_VALUE),
        "the doctor report must never contain a credential's value"
    );

    assert!(
        report.contains("Secret storage"),
        "the report must have a secret-storage section: {report}"
    );
    // Whichever of the three arrangements is in force on the machine
    // running this, the report must print that arrangement's own label —
    // never nothing, and never a label for a different one.
    let store = crate::secret::native::PreferNativeSecretStore::detect();
    let label = crate::secret::SecretStore::describe(&store);
    assert!(
        report.contains(&format!("credentials resolve from: {label}")),
        "the report must name the store that answers: {report}"
    );
    assert!(
        [
            crate::secret::native::NATIVE_FIRST_LABEL,
            crate::secret::native::UNSUPPORTED_PLATFORM_LABEL,
            crate::secret::native::STORE_UNREACHABLE_LABEL,
        ]
        .contains(&label),
        "`{label}` is not one of the three arrangements this store can be in"
    );

    // Per credential, too: the environment answered this one, and the
    // report says so rather than leaving the user to infer it.
    assert!(
        report.contains(&format!("{VAR_NAME} (set in process environment")),
        "the report must say which source answered: {report}"
    );

    // A configuration that records a stored credential the store does
    // not return is reported, not silently papered over with the
    // environment's copy.
    assert!(
        report.contains(&format!(
            "stored credential: {}/{VAR_NAME}",
            crate::secret::native::SERVICE
        )),
        "the recorded stored credential must be named: {report}"
    );
}
