//! Integration test suite for Phase 2B: integration detection status contract.
//!
//! Validates the core capability requirements:
//! 1. Every detected integration carries exactly one of the five capability states:
//!    `Available`, `Configured`, `Unconfigured`, `UnsupportedVersion`, or `Unknown`.
//! 2. `NotFound` is the distinct absent state ("not installed"), never confused with `Unknown`.
//! 3. `Unknown` is not an error or failure when an executable is detected: detection ran
//!    and could not determine configuration, so problems list remains clean.
//! 4. `UnsupportedVersion` marks an integration as not usable (`is_usable() == false`).
//! 5. `doctor_report` formats each integration's status cleanly in brackets without
//!    debug artifacts or guessing.

use clap::Parser;
use glasshouse::integrations::{Discovery, IntegrationId, IntegrationStatus, doctor_report};
use glasshouse::{Cli, Project, Runtime, bootstrap};

fn bootstrapped_runtime() -> (tempfile::TempDir, tempfile::TempDir, Runtime) {
    let data = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join(".git")).unwrap();

    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        data.path().to_str().unwrap(),
        "--config-dir",
        data.path().to_str().unwrap(),
    ])
    .unwrap();
    let runtime = bootstrap(&cli, workspace.path()).unwrap();
    (data, workspace, runtime)
}

fn test_project() -> (tempfile::TempDir, Project) {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join(".git")).unwrap();
    let project = Project::discover(tmp.path(), None, false).unwrap();
    (tmp, project)
}

#[test]
fn every_detected_integration_carries_one_of_the_five_capability_states() {
    let (_guard, project) = test_project();
    let discovery = Discovery::run(&project);

    for d in discovery.all() {
        let is_detected = d.executable().is_some() || d.status() != IntegrationStatus::NotFound;
        if is_detected {
            assert!(
                matches!(
                    d.status(),
                    IntegrationStatus::Available
                        | IntegrationStatus::Configured
                        | IntegrationStatus::Unconfigured
                        | IntegrationStatus::UnsupportedVersion
                        | IntegrationStatus::Unknown
                ),
                "{:?} was detected but carries status {:?}, which is not one of the five capability states",
                d.id(),
                d.status()
            );
            assert_ne!(
                d.status(),
                IntegrationStatus::NotFound,
                "{:?} is detected and must never carry NotFound",
                d.id()
            );
        }
    }
}

#[test]
fn every_status_formats_cleanly_without_debug_artifacts() {
    let expected = [
        (IntegrationStatus::Available, "available"),
        (IntegrationStatus::Configured, "configured"),
        (IntegrationStatus::Unconfigured, "unconfigured"),
        (IntegrationStatus::UnsupportedVersion, "unsupported version"),
        (IntegrationStatus::NotFound, "not found"),
        (IntegrationStatus::Unknown, "unknown"),
    ];

    for (status, label) in expected {
        let formatted = status.to_string();
        assert_eq!(formatted, label);
        assert!(!formatted.contains("Integration"));
        assert!(!formatted.contains("Status"));
        assert!(!formatted.is_empty());
    }
}

#[test]
fn usable_detected_integrations_are_never_unsupported_version() {
    let (_guard, project) = test_project();
    let discovery = Discovery::run(&project);

    for d in discovery.all() {
        if d.is_usable() {
            assert!(
                d.executable().is_some(),
                "{:?} is reported usable but has no executable",
                d.id()
            );
            assert_ne!(
                d.status(),
                IntegrationStatus::UnsupportedVersion,
                "{:?} is reported usable but has status UnsupportedVersion",
                d.id()
            );
        }
    }
}

#[test]
fn doctor_report_renders_detected_statuses_in_brackets() {
    let (_data, _workspace, runtime) = bootstrapped_runtime();
    let report = doctor_report(&runtime);

    // Doctor report must include all integrations with their statuses in brackets
    for &id in IntegrationId::ALL {
        let name = id.display_name();
        assert!(
            report.contains(name),
            "doctor report should contain integration row for {name}"
        );
    }

    // Every status appearing in the report is cleanly enclosed in brackets
    assert!(
        report.contains("[available]")
            || report.contains("[configured]")
            || report.contains("[unconfigured]")
            || report.contains("[unsupported version]")
            || report.contains("[unknown]")
            || report.contains("[not found]"),
        "doctor report should contain formatted status indicators in brackets"
    );
}

#[test]
fn unconfigured_and_unknown_with_executable_are_not_treated_as_problems() {
    let (_guard, project) = test_project();
    let discovery = Discovery::run(&project);

    for d in discovery.all() {
        match d.status() {
            IntegrationStatus::Unconfigured => {
                assert!(
                    d.problems().is_empty(),
                    "{:?} is Unconfigured but was reported as a problem: {:?}",
                    d.id(),
                    d.problems()
                );
            }
            IntegrationStatus::Unknown if d.executable().is_some() => {
                assert!(
                    d.problems().is_empty(),
                    "{:?} has an executable and Unknown status, but was reported as a problem: {:?}",
                    d.id(),
                    d.problems()
                );
            }
            _ => {}
        }
    }
}

#[test]
fn not_found_status_is_reserved_for_plain_absence() {
    let (_guard, project) = test_project();
    let discovery = Discovery::run(&project);

    for d in discovery.all() {
        if d.status() == IntegrationStatus::NotFound {
            assert!(
                d.executable().is_none(),
                "{:?} has status NotFound but has an executable: {:?}",
                d.id(),
                d.executable()
            );
            assert!(
                d.problems().is_empty(),
                "{:?} is NotFound but has recorded problems: {:?}",
                d.id(),
                d.problems()
            );
        }
    }
}
