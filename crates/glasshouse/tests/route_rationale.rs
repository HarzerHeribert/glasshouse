//! Phase 47 — lines 1757 and 1766, `GH-ROUTE-RATIONALE-SINK`: the session
//! router's rationale, durable.
//!
//! - **1757** *"Add a debug view showing why the router chose a session or
//!   resource."*
//! - **1766** *"Show the strongest measured factors behind the most recent
//!   routing decision in concise text."*
//!
//! Every test drives the shipped binary — practice §35: a caller every test
//! bypasses is not a caller, and the whole claim of these two lines is that
//! `main.rs::launch_session`'s two routed exits, `sessions show` and
//! `status` now do something they did not before. The fixture below is
//! copied from `tests/evaluation_observations.rs`'s `LaunchFixture`, which
//! that file's own header says is copied for the same reason.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;

use glasshouse::evaluation::{EvaluationKind, EvaluationObservations};
use glasshouse::session::{NewSession, ProjectSessions};
use glasshouse::{Cli, Runtime, bootstrap};

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// A distinctive, never-otherwise-occurring string planted as `--task`, so
/// test (f) can assert it never reaches a `SessionRouteDecided` row's
/// `detail` — the row carries names, magnitudes and evidence, never task
/// text.
const PLANTED_TASK: &str = "ROUTE-RATIONALE-SINK-PLANTED-TASK-9f3c1a";

#[cfg(unix)]
fn install_fake_harness(bin_dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join("fake-claude-code");
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_fake_harness(bin_dir: &Path) -> PathBuf {
    let path = bin_dir.join("fake-claude-code.cmd");
    std::fs::write(&path, "@echo off\r\nexit /b 0\r\n").expect("write fake harness");
    path
}

/// A project wired with a fake harness, so `glasshouse launch` runs end to
/// end — copied from `tests/evaluation_observations.rs`'s `LaunchFixture`.
struct LaunchFixture {
    base: PathBuf,
    runtime: Runtime,
}

impl LaunchFixture {
    fn new(base: &Path) -> Self {
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let harness = install_fake_harness(&bin_dir);
        let escaped = harness.display().to_string().replace('\\', "\\\\");

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n[integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n"
            ),
        )
        .expect("write user config");

        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            config_dir.to_str().unwrap(),
        ])
        .unwrap();
        let runtime = bootstrap(&cli, &root).unwrap();

        LaunchFixture {
            base: base.to_path_buf(),
            runtime,
        }
    }

    fn glasshouse(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .current_dir(self.runtime.project().root())
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .output()
            .expect("the glasshouse binary must run")
    }

    fn both_streams(output: &std::process::Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    fn stdout(output: &std::process::Output) -> String {
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn ledger(&self) -> EvaluationObservations {
        EvaluationObservations::open(&self.runtime).unwrap()
    }

    /// Every recorded session's id, oldest first — the same door
    /// `tests/evaluation_observations.rs`'s `LaunchFixture::session_ids`
    /// uses.
    fn session_ids(&self) -> Vec<String> {
        let conn = rusqlite::Connection::open(self.runtime.database_path()).unwrap();
        let mut statement = conn
            .prepare("SELECT id FROM sessions ORDER BY created_at")
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
    }
}

/// A `routing rationale`-block reader for `sessions show` output: the
/// trimmed text after the `routing rationale` label on its own line, and
/// every following indented contribution line up to the next unindented
/// line (or the end of the report).
struct RationaleBlock {
    heading: String,
    contribution_lines: Vec<String>,
}

fn parse_rationale_block(output: &str) -> RationaleBlock {
    let mut lines = output.lines();
    let heading_line = lines
        .by_ref()
        .find(|line| line.starts_with("routing rationale"))
        .expect("`sessions show` must print a `routing rationale` line");
    let heading = heading_line
        .trim_start_matches("routing rationale")
        .trim()
        .to_owned();

    let mut contribution_lines = Vec::new();
    for line in lines {
        if line.is_empty() || !line.starts_with(' ') {
            break;
        }
        contribution_lines.push(line.trim().to_owned());
    }
    RationaleBlock {
        heading,
        contribution_lines,
    }
}

/// Split one rendered contribution line — `<name>  <+/-X.XXX>  <evidence>` —
/// into its name and its signed-magnitude-plus-evidence remainder, by
/// finding the first `[+-]<digit>` run rather than assuming a fixed column,
/// since a contribution's own name may contain spaces or hyphens.
fn split_contribution_line(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    for i in 0..bytes.len().saturating_sub(1) {
        let sign = bytes[i];
        let digit = bytes[i + 1];
        if (sign == b'+' || sign == b'-') && digit.is_ascii_digit() {
            return Some((line[..i].trim_end(), line[i..].trim_start()));
        }
    }
    None
}

/// Parse `status`'s `last routing decision: ...` line into its destination
/// and its `name +m` factors, in the order printed.
fn parse_status_routing_line(output: &str) -> (String, Vec<String>) {
    let line = output
        .lines()
        .find(|line| line.starts_with("last routing decision:"))
        .expect("`status` must print a `last routing decision:` line");
    let rest = line
        .trim_start_matches("last routing decision:")
        .trim()
        .trim_end_matches(')');
    // `<destination> — <factors> (<age>, session <id>`, and a destination
    // with no factors omits the `— <factors>` segment entirely (never
    // observed here, since a real launch always scores well over three
    // contributions, but split defensively rather than assume).
    let (before_paren, _paren) = rest.rsplit_once(" (").expect("a trailing parenthetical");
    match before_paren.split_once(" — ") {
        Some((destination, factors)) => (
            destination.to_owned(),
            factors.split(", ").map(str::to_owned).collect(),
        ),
        None => (before_paren.to_owned(), Vec::new()),
    }
}

/// **Acceptance (a).** A fresh launch's rationale is durable and readable:
/// `sessions show <id>` names at least one contribution `glasshouse route`
/// prints for the same task, with a signed magnitude.
#[test]
fn a_fresh_launch_shows_a_rationale_route_also_names() {
    let tmp = tempdir();
    let fixture = LaunchFixture::new(tmp.path());

    let launched = fixture.glasshouse(&[
        "launch",
        "claude-code",
        "--headless",
        "--task",
        PLANTED_TASK,
    ]);
    assert!(
        launched.status.success(),
        "the launch must succeed:\n{}",
        LaunchFixture::both_streams(&launched)
    );
    let session = fixture.session_ids();
    assert_eq!(session.len(), 1, "one fresh launch, one session");

    let shown = fixture.glasshouse(&["sessions", "show", &session[0]]);
    assert!(
        shown.status.success(),
        "sessions show must succeed:\n{}",
        LaunchFixture::both_streams(&shown)
    );
    let block = parse_rationale_block(&LaunchFixture::stdout(&shown));
    assert!(
        !block.heading.is_empty() && block.heading != "-",
        "a routed session must not show `-`"
    );
    assert!(
        !block.contribution_lines.is_empty(),
        "a real launch's explanation scores more than zero contributions"
    );
    let (name, magnitude_and_evidence) = split_contribution_line(&block.contribution_lines[0])
        .expect("the first contribution line must carry a signed magnitude");
    assert!(
        magnitude_and_evidence.starts_with('+') || magnitude_and_evidence.starts_with('-'),
        "got: {magnitude_and_evidence}"
    );

    let routed = fixture.glasshouse(&["route", "--task", PLANTED_TASK]);
    assert!(
        routed.status.success(),
        "route must succeed:\n{}",
        LaunchFixture::both_streams(&routed)
    );
    let route_text = LaunchFixture::stdout(&routed);
    assert!(
        route_text.contains(name),
        "`glasshouse route` must print the same contribution `{name}` the recorded rationale \
         names; got:\n{route_text}"
    );
}

/// **Acceptance (b).** `status` names the newest routed session's
/// destination and lists no more than three factors, the three largest by
/// absolute magnitude, most first.
#[test]
fn status_lists_at_most_three_factors_by_absolute_magnitude() {
    let tmp = tempdir();
    let fixture = LaunchFixture::new(tmp.path());

    let first = fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    assert!(
        first.status.success(),
        "the first launch must succeed:\n{}",
        LaunchFixture::both_streams(&first)
    );
    // `--fresh` guarantees a second, distinct destination, so the test can
    // tell "the newest row" from "the only row".
    let second = fixture.glasshouse(&["launch", "claude-code", "--headless", "--fresh"]);
    assert!(
        second.status.success(),
        "the second launch must succeed:\n{}",
        LaunchFixture::both_streams(&second)
    );
    let sessions = fixture.session_ids();
    assert_eq!(sessions.len(), 2, "two launches, two sessions");

    // The independent oracle for "the second launch's own destination":
    // `record_routed_session` — existing production code this packet does
    // not touch — writes the very same `destination_id` as
    // `RoutingCostClassObserved::detail`, at the same instant as the
    // `SessionRouteDecided` row `status` reads from. Comparing against a
    // session id would be wrong for a fresh launch, whose destination id is
    // a profile slug like `fresh:claude-code:native`, never a session id.
    let cost_class_rows = fixture
        .ledger()
        .recent_of_kind(EvaluationKind::RoutingCostClassObserved, 10)
        .unwrap();
    assert_eq!(cost_class_rows.len(), 2, "one row per launch");
    let expected_destination = cost_class_rows[0]
        .detail
        .clone()
        .expect("a cost-class row always carries the destination id");

    let status = fixture.glasshouse(&["status"]);
    assert!(
        status.status.success(),
        "status must succeed:\n{}",
        LaunchFixture::both_streams(&status)
    );
    let (destination, factors) = parse_status_routing_line(&LaunchFixture::stdout(&status));
    assert_eq!(
        destination, expected_destination,
        "status must name the second launch's own destination"
    );
    assert!(
        factors.len() <= 3,
        "no more than three factors, got {}: {factors:?}",
        factors.len()
    );
    assert_eq!(
        factors.len(),
        3,
        "a real launch scores well over three contributions, so exactly three must be shown: \
         {factors:?}"
    );

    // An internal-ordering-only check is not enough here: a routing
    // explanation carries several `0.0`-magnitude informational
    // contributions (legitimate, by `Contribution`'s own doc), so a sort
    // reversed to smallest-first can still pick three that are tied with
    // each other and pass a bare "each pair is non-increasing" check. The
    // real test is that these are the three LARGEST of the row's own full
    // contribution list — computed independently here from the same
    // `SessionRouteDecided` row `status` reads, never by re-deriving from
    // `routing`'s scoring itself.
    let newest_row = fixture
        .ledger()
        .latest_session_route()
        .unwrap()
        .expect("the newest launch must have recorded a rationale row");
    let mut all_contributions =
        glasshouse::evaluation::route_contributions(newest_row.detail.as_deref().unwrap_or(""));
    assert!(
        all_contributions.len() > 3,
        "a real launch scores well over three contributions, got {}",
        all_contributions.len()
    );
    all_contributions.sort_by(|a, b| b.magnitude.abs().total_cmp(&a.magnitude.abs()));
    let expected: Vec<String> = all_contributions
        .iter()
        .take(3)
        .map(|contribution| format!("{} {:+.3}", contribution.name, contribution.magnitude))
        .collect();
    assert_eq!(
        factors, expected,
        "status must show the three largest contributions by absolute magnitude, most first"
    );
}

/// **Acceptance (c).** A session with no `SessionRouteDecided` row — the
/// machine door, which does not route — shows `-`, never a crash and never
/// a fabricated block.
#[test]
fn an_unrouted_session_shows_a_dash() {
    let tmp = tempdir();
    let fixture = LaunchFixture::new(tmp.path());

    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    let record = sessions
        .store()
        .create(NewSession::embedded("claude-code"))
        .unwrap();
    drop(sessions);

    let shown = fixture.glasshouse(&["sessions", "show", record.id.as_str()]);
    assert!(
        shown.status.success(),
        "sessions show must succeed:\n{}",
        LaunchFixture::both_streams(&shown)
    );
    let block = parse_rationale_block(&LaunchFixture::stdout(&shown));
    assert_eq!(block.heading, "-", "an unrouted session must show `-`");
    assert!(
        block.contribution_lines.is_empty(),
        "a `-` row prints no contribution lines"
    );
}

/// **Acceptance (d).** A project with no routed launch shows *none
/// recorded*, never a crash and never `status`'s absence read as zero.
#[test]
fn a_project_with_no_launches_shows_none_recorded() {
    let tmp = tempdir();
    let fixture = LaunchFixture::new(tmp.path());

    let status = fixture.glasshouse(&["status"]);
    assert!(
        status.status.success(),
        "status must succeed:\n{}",
        LaunchFixture::both_streams(&status)
    );
    let text = LaunchFixture::stdout(&status);
    assert!(
        text.contains("last routing decision: none recorded"),
        "got:\n{text}"
    );
}

/// **Acceptance (e).** The continued-session branch records its rationale
/// against the continued session's own id, not a fresh one.
#[test]
fn a_continued_launch_records_its_rationale_against_the_continued_session() {
    let tmp = tempdir();
    let fixture = LaunchFixture::new(tmp.path());

    let first = fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    assert!(
        first.status.success(),
        "the first launch must succeed:\n{}",
        LaunchFixture::both_streams(&first)
    );
    let warm_id = fixture.session_ids().remove(0);

    // No `--fresh`: the ranking continues the warm session from the first
    // launch, the same shape
    // `tests/evaluation_observations.rs`'s
    // `a_continued_warm_session_and_a_fresh_one_are_distinguishable_in_the_ledger`
    // drives.
    let second = fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    assert!(
        second.status.success(),
        "the second launch must succeed:\n{}",
        LaunchFixture::both_streams(&second)
    );
    assert_eq!(
        fixture.session_ids().len(),
        1,
        "the second launch must have continued the warm session rather than starting another"
    );

    let rows = fixture
        .ledger()
        .recent_of_kind(EvaluationKind::SessionRouteDecided, 10)
        .unwrap();
    assert_eq!(rows.len(), 2, "one rationale row per launch");
    assert!(
        rows.iter()
            .all(|row| row.session_id.as_deref() == Some(warm_id.as_str())),
        "both launches routed onto the one warm session, so both rows must name it: {rows:?}"
    );

    let for_session = fixture
        .ledger()
        .session_route_for(&warm_id)
        .unwrap()
        .expect("the continued session must have a rationale row");
    assert_eq!(for_session.session_id.as_deref(), Some(warm_id.as_str()));
}

/// **Acceptance (f).** The planted task text never reaches a
/// `SessionRouteDecided` row's `detail` — the row carries names, magnitudes
/// and evidence, never the task.
#[test]
fn the_task_text_never_reaches_a_rationale_rows_detail() {
    let tmp = tempdir();
    let fixture = LaunchFixture::new(tmp.path());

    let launched = fixture.glasshouse(&[
        "launch",
        "claude-code",
        "--headless",
        "--task",
        PLANTED_TASK,
    ]);
    assert!(
        launched.status.success(),
        "the launch must succeed:\n{}",
        LaunchFixture::both_streams(&launched)
    );

    let rows = fixture
        .ledger()
        .recent_of_kind(EvaluationKind::SessionRouteDecided, 10)
        .unwrap();
    assert!(!rows.is_empty(), "the launch must have recorded a row");
    for row in &rows {
        let detail = row.detail.as_deref().unwrap_or_default();
        assert!(
            !detail.contains(PLANTED_TASK),
            "a rationale row's detail must never carry the task text; got: {detail}"
        );
    }
}
