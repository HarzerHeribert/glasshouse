//! Phase 51 — a routing decision's outcome is the harness's own verdict.
//!
//! - **1835** *"Measure how often a low-cost or free route succeeds compared
//!   with the premium route it displaced."*
//! - **1845** *"Measure native versus cross-vendor harness-model pairings by
//!   task success ..."* — the task-success quantity only.
//! - **1854** *"Measure how often sparse, stale, or incorrectly segmented
//!   evidence causes a poor routing decision."* — the sparse half.
//!
//! **Everything here goes through the shipped binary**, twice over, because
//! the whole claim is about two production paths meeting: `glasshouse launch`
//! attributes a route to the session it produced, and `glasshouse hook` — a
//! *separate process the harness spawns* — attributes that session's turn
//! outcome back to it. Practice §35: a caller every test bypasses is not a
//! caller, and calling `record_routing_outcome` from a test would prove
//! nothing about either path.
//!
//! The one thing deliberately **not** proved here is that a quiet or exited
//! process produces an outcome, because it must not: the silent-exit case is
//! an assertion of absence, and it is written as one.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;
use rusqlite::Connection;

use glasshouse::evaluation::{EvaluationKind, EvaluationObservations};
use glasshouse::session::{NewSession, ProjectSessions, SessionLifecycle};
use glasshouse::{Cli, Runtime};

/// The credential variable the fixture's provider names, planted in the
/// child's environment only — a test process must not export a value every
/// other test in the binary would then see.
const CREDENTIAL_VAR: &str = "GLASSHOUSE_ROUTING_OUTCOME_KEY";

/// The model the fixture marks free, and the one it does not. Both are named
/// by a launch profile, so the class each launch records comes from
/// `ProviderConfig::cost_of` on the production path rather than from a
/// constant a test planted in the ledger.
const FREE_MODEL: &str = "probe/free-model";
const METERED_MODEL: &str = "probe/premium-model";

/// A task `classify_heuristically` reads as **standard**-tier code
/// modification — the same text `tests/tier_escalation.rs`'s
/// `STANDARD_REPO_TASK` uses, written out so a classifier change fails these
/// tests rather than rescaling with them.
const STANDARD_TASK: &str = "refactor the launch profile handling in this project";

/// A task the same heuristics read as **heavy** (`run ` is a shell keyword),
/// from heuristics and no model call — `tests/tier_escalation.rs`'s own
/// `HEAVY_SHELL_TASK`. This fixture's destinations are direct-provider
/// backends with no subscription quota reading, so a launch classified at
/// this tier is the case line 1837 calls `unknown`.
const HEAVY_TASK: &str = "run the tests in this project";

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// A project wired with a fake harness and two direct-provider launch
/// profiles, so `glasshouse launch` runs end to end and its two destinations
/// genuinely differ in cost class.
///
/// The same shape `tests/evaluation_observations.rs`'s `LaunchFixture` uses,
/// with the provider block added: line 1835 is about a class, and a fixture
/// whose every destination fell in one bucket would report a tautology
/// whatever the reader did.
struct Fixture {
    base: PathBuf,
    runtime: Runtime,
}

impl Fixture {
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
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
                 [providers.probe]\ntemplate = \"anthropic-compatible\"\n\
                 base_url = \"http://127.0.0.1:9/\"\n\
                 credential_env = [\"{CREDENTIAL_VAR}\"]\n\
                 free_models = [\"{FREE_MODEL}\"]\n\n\
                 [profiles.freebie]\nharness = \"claude-code\"\nmodel = \"{FREE_MODEL}\"\n\n\
                 [profiles.freebie.backend]\nkind = \"direct-provider\"\nprovider = \"probe\"\n\n\
                 [profiles.premium]\nharness = \"claude-code\"\nmodel = \"{METERED_MODEL}\"\n\n\
                 [profiles.premium.backend]\nkind = \"direct-provider\"\nprovider = \"probe\"\n"
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
        let runtime = glasshouse::bootstrap(&cli, &root).unwrap();

        Fixture {
            base: base.to_path_buf(),
            runtime,
        }
    }

    fn glasshouse(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .current_dir(self.runtime.project().root())
            .env(CREDENTIAL_VAR, "not-a-real-key")
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .output()
            .expect("the glasshouse binary must run")
    }

    /// Launch under `profile`, and fail loudly with both streams if it did
    /// not start — a launch that quietly refused would make every assertion
    /// below vacuous.
    fn launch(&self, profile: &str) -> String {
        self.launch_args(&["launch", "claude-code", "--headless", "--profile", profile])
    }

    /// Launch under `profile`, classified by `task` — the same fixture,
    /// with a `--task` argument the heuristic classifier reads without a
    /// model call, exactly as `tests/tier_escalation.rs`'s `HEAVY_SHELL_TASK`
    /// does.
    fn launch_with_task(&self, profile: &str, task: &str) -> String {
        self.launch_args(&[
            "launch",
            "claude-code",
            "--headless",
            "--profile",
            profile,
            "--task",
            task,
        ])
    }

    fn launch_args(&self, args: &[&str]) -> String {
        let before = self.session_ids();
        let launched = self.glasshouse(args);
        assert!(
            launched.status.success(),
            "the launch must succeed:\n{}",
            both_streams(&launched)
        );
        let mut created: Vec<String> = self
            .session_ids()
            .into_iter()
            .filter(|id| !before.contains(id))
            .collect();
        assert_eq!(
            created.len(),
            1,
            "a launch must create exactly one session; before: {before:?}"
        );
        created.remove(0)
    }

    /// Run `glasshouse hook`, exactly as a harness runs it: a separate
    /// process, the event on its argv, a payload on its standard input.
    fn hook(&self, session: &str, event: &str) -> std::process::Output {
        use std::io::Write as _;
        use std::process::Stdio;

        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .current_dir(self.runtime.project().root())
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("hook")
            .arg("--session")
            .arg(session)
            .arg("--event")
            .arg(event)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the glasshouse binary must be runnable");
        child
            .stdin
            .as_mut()
            .expect("stdin was piped")
            .write_all(b"{\"prompt\":\"PROMPT-MARKER-7f3a9c\"}")
            .expect("write the hook payload");
        let output = child.wait_with_output().expect("the hook must exit");
        assert!(
            output.status.success(),
            "a hook always exits zero:\n{}",
            both_streams(&output)
        );
        output
    }

    fn ledger(&self) -> EvaluationObservations {
        EvaluationObservations::open(&self.runtime).unwrap()
    }

    fn db(&self) -> Connection {
        Connection::open(self.runtime.database_path()).unwrap()
    }

    /// Every recorded session's id.
    ///
    /// Read as a set and differenced across a launch rather than taken as
    /// "the newest row": `created_at` is a whole second, and two launches in
    /// one second tie, so an `ORDER BY created_at DESC LIMIT 1` silently
    /// hands back the wrong session — which is a test that passes while
    /// asserting about a session it did not mean.
    fn session_ids(&self) -> Vec<String> {
        let conn = self.db();
        let mut statement = conn.prepare("SELECT id FROM sessions").unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    /// Every recorded outcome, as `(session_id, subject, detail)`, newest
    /// first.
    fn outcomes(&self) -> Vec<(Option<String>, Option<String>, Option<String>)> {
        self.ledger()
            .recent_of_kind(EvaluationKind::RoutingOutcomeObserved, 50)
            .unwrap()
            .into_iter()
            .map(|row| (row.session_id, row.subject, row.detail))
            .collect()
    }

    /// The cost class recorded for one session, or `None` when no route was
    /// attributed to it.
    fn cost_class_of(&self, session: &str) -> Option<String> {
        self.ledger()
            .recent_of_kind(EvaluationKind::RoutingCostClassObserved, 50)
            .unwrap()
            .into_iter()
            .find(|row| row.session_id.as_deref() == Some(session))
            .and_then(|row| row.subject)
    }

    /// The `(subject, detail)` a [`EvaluationKind::ReserveAvailabilityObserved`]
    /// row recorded for one session, or `None` when no such row exists.
    fn reserve_availability_of(&self, session: &str) -> Option<(Option<String>, Option<String>)> {
        self.ledger()
            .recent_of_kind(EvaluationKind::ReserveAvailabilityObserved, 50)
            .unwrap()
            .into_iter()
            .find(|row| row.session_id.as_deref() == Some(session))
            .map(|row| (row.subject, row.detail))
    }
}

fn both_streams(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

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

/// **Acceptance 1.** A harness saying its turn completed leaves exactly one
/// outcome row, against the session the launch routed and naming the
/// destination that decision chose.
///
/// This is the mutation target for the hook-path call: deleting it leaves the
/// launch's own rows in place and every other test about routing decisions
/// passing, and only this one fails.
#[test]
fn a_completed_turn_records_the_outcome_against_the_decision_that_routed_it() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path());

    let session = fixture.launch("freebie");
    assert!(
        fixture.outcomes().is_empty(),
        "a launch on its own says nothing about how a turn went"
    );

    fixture.hook(&session, "Stop");

    let outcomes = fixture.outcomes();
    assert_eq!(outcomes.len(), 1, "one turn end, one outcome row");
    assert_eq!(
        outcomes[0].0.as_deref(),
        Some(session.as_str()),
        "the outcome must be attributed to the session whose turn ended"
    );
    assert_eq!(
        outcomes[0].1.as_deref(),
        Some("completed"),
        "`Stop` is the harness stating a completed turn"
    );
    assert_eq!(
        outcomes[0].2.as_deref(),
        Some("fresh:claude-code:freebie"),
        "the outcome names the destination the routing decision chose, so a reader can key \
         a success ratio on it without a second join"
    );
    assert_eq!(
        fixture.cost_class_of(&session).as_deref(),
        Some("free"),
        "the route this launch took is the free one, from the provider's own `free_models`"
    );

    // The isolation invariant, read off the stored row rather than trusted:
    // every text cell either is an id this project already holds or is a word
    // from a closed vocabulary, and the four columns this producer has no
    // business filling are empty. The hook's payload carried a prompt, and
    // nothing anywhere below has it.
    let base = fixture.base.display().to_string();
    for row in fixture
        .ledger()
        .recent(20)
        .expect("read every stored observation")
    {
        assert_eq!(row.feature, None, "no A/B feature belongs on a routing row");
        assert_eq!(row.arm, None, "and no arm");
        assert_eq!(
            row.memory_id, None,
            "these rows are about routes, not memories"
        );
        assert_eq!(row.routing_seq, None, "nothing here points at a cost row");
        for cell in [&row.subject, &row.session_id, &row.detail]
            .into_iter()
            .flatten()
        {
            assert!(
                // The marker is a token no vocabulary can contain: the session
                // router's rationale row (`SessionRouteDecided`, wave 103)
                // legitimately says "an unread resource is neither preferred
                // nor withheld", and the plain word `unread` this test planted
                // before that row existed matched it in the trailing sweep.
                !cell.contains("PROMPT-MARKER-7f3a9c") && !cell.contains(&base),
                "a routing row carries ids and vocabulary words only — never the prompt the \
                 hook was handed, and never a filesystem path: `{cell}`"
            );
        }
    }
}

/// **Acceptance 2.** A turn that ended badly is recorded as badly ended, and
/// a session ending with no turn end recorded is recorded as nothing at all.
///
/// The second half is the standing rule this whole package is built around:
/// silence is not an outcome. `SessionEnd` is a real harness event that
/// `session::lifecycle::event_for` deliberately translates to nothing, so it
/// is the honest way to drive "the process went away" through the production
/// path.
#[test]
fn a_failed_turn_records_failed_and_a_silent_exit_records_nothing() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path());

    let session = fixture.launch("premium");
    fixture.hook(&session, "StopFailure");

    let outcomes = fixture.outcomes();
    assert_eq!(outcomes.len(), 1, "one turn end, one outcome row");
    assert_eq!(
        outcomes[0].1.as_deref(),
        Some("failed"),
        "`StopFailure` is the harness stating a turn that ended badly, and recording it as \
         `completed` would make every success ratio here a fabrication"
    );

    fixture.hook(&session, "SessionEnd");
    assert_eq!(
        fixture.outcomes().len(),
        1,
        "a session ending is not a verdict on a turn; it must add no row"
    );

    assert_eq!(
        fixture.cost_class_of(&session).as_deref(),
        Some("metered"),
        "a model the provider has not marked free is metered — the fail-closed direction"
    );
}

/// **Acceptance 3.** The reader prints every ratio with its denominator, and
/// the classes stay apart.
///
/// Both halves of the comparison line 1835 asks for are produced by real
/// launches through the shipped binary, so this test also proves that the
/// cost class is read from a production fact rather than from the constant
/// `destination_backend` puts on every `Backend`.
#[test]
fn free_and_metered_route_success_is_reported_with_denominators() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path());

    let free = fixture.launch("freebie");
    fixture.hook(&free, "Stop");

    let metered = fixture.launch("premium");
    fixture.hook(&metered, "Stop");
    fixture.hook(&metered, "StopFailure");

    let unreported = fixture.launch("freebie");
    assert_ne!(unreported, free, "a third launch is a third session");

    let report = fixture.glasshouse(&["route"]);
    let printed = both_streams(&report);
    // Column padding is a rendering detail; the bucket-to-sentence binding is
    // not, so the assertions below normalise runs of spaces rather than
    // pinning a width that has nothing to do with what is being proved.
    let normalised = printed.split_whitespace().collect::<Vec<_>>().join(" ");

    assert!(
        printed.contains("Past routes in this project, last 30 days"),
        "the outcome section must reach a person:\n{printed}"
    );
    assert!(
        normalised.contains(
            "free : 1 of 1 reported turns completed; 2 sessions routed, 1 with no turn end \
             reported"
        ),
        "a free route's success must print its denominator and its unknown bucket:\n{printed}"
    );
    assert!(
        normalised.contains("metered : 1 of 2 reported turns completed; 1 session routed"),
        "a metered route's success must print the turns it is out of:\n{printed}"
    );
    assert!(
        !printed.contains('%'),
        "no ratio here may print as a bare percentage:\n{printed}"
    );
    assert!(
        printed.contains("by evidence held about the destination when it was chosen"),
        "line 1854's sparse half must be rendered beside the outcome:\n{printed}"
    );
    assert!(
        normalised.contains(
            "absent : 2 of 3 reported turns completed; 3 sessions routed, 1 with no turn end \
             reported"
        ),
        "no gateway has ever observed these destinations, so every route here was chosen with \
         absent evidence — and that whole bucket, with both its denominators, is what line \
         1854 asks about:\n{printed}"
    );
    assert!(
        printed.contains("by pairing class"),
        "line 1845's task-success quantity is keyed on the session's own pairing class:\n{printed}"
    );
    assert!(
        normalised.contains(
            "unknown task success : 2 of 3 reported turns completed; 3 sessions routed, 1 \
             with no turn end reported"
        ),
        "nothing has attributed this fixture's model, so its pairing class is honestly \
         `unknown` — and line 1845's ratio must print under that bucket rather than being \
         folded into a neighbouring one:\n{printed}"
    );
    // `GH-RESPONSIVENESS-TERMS`, map line 1845's other five quantities: this
    // fixture's rows carry no tool rounds, no ms offsets and no override, so
    // every one of the five is honestly *not enough evidence*, never a
    // fabricated number.
    for label in [
        "usable tool calls",
        "repair loops",
        "effective TTFC",
        "reliability",
        "user overrides",
    ] {
        assert!(
            normalised.contains(&format!("{label} : not enough evidence")),
            "{label} must print `not enough evidence` on this fixture's rows, not a fabricated \
             number:\n{printed}"
        );
    }
}

/// **Acceptance 4.** A session Glasshouse never routed gets no outcome, and
/// the hook still exits zero.
///
/// This is the pre-existing-session case the objective names: a session
/// recorded by an older build, or by any path that does not route, has no
/// decision for an outcome to be attributed to, and inventing one would put a
/// row in the ledger pointing at a route nobody chose.
#[test]
fn a_session_with_no_routing_decision_records_no_outcome() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path());

    let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
    let store = sessions.store();
    let record = store.create(NewSession::embedded("claude-code")).unwrap();
    store
        .set_lifecycle(&record.id, SessionLifecycle::Running)
        .unwrap();
    drop(sessions);

    fixture.hook(record.id.as_str(), "Stop");

    assert!(
        fixture.outcomes().is_empty(),
        "a session with no routing decision has nothing for an outcome to be about"
    );
    assert!(
        fixture.cost_class_of(record.id.as_str()).is_none(),
        "and nothing attributed a route to it either"
    );
}

/// **Acceptance 5** (map line 1837). A launch classified above the routine
/// tier records the destination's capacity band as its protected-quota
/// reading — `unknown` here, because this fixture's direct-provider
/// destinations carry no subscription capacity reading at all.
#[test]
fn a_heavy_tier_launch_records_reserve_availability_as_unknown_with_no_reading() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path());

    let session = fixture.launch_with_task("freebie", HEAVY_TASK);

    let (subject, detail) = fixture
        .reserve_availability_of(&session)
        .expect("a heavy-tier launch must record a protected-quota reading");
    assert_eq!(
        subject.as_deref(),
        Some("unknown"),
        "a destination with no capacity reading must record `unknown`, never a fabricated band"
    );
    assert_eq!(
        detail.as_deref(),
        Some("heavy"),
        "detail is the tier word the routing decision used"
    );
}

/// **Acceptance 6** (map line 1837). A launch at or below the routine tier,
/// and a launch that states no task at all, write no protected-quota row —
/// *needed* is the line's own word.
#[test]
fn a_standard_or_unclassified_launch_writes_no_reserve_availability_row() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path());

    let standard = fixture.launch_with_task("freebie", STANDARD_TASK);
    assert!(
        fixture.reserve_availability_of(&standard).is_none(),
        "a standard-tier launch is routine support work, and 1837 asks about the reserve the \
         routine tier does not draw on"
    );

    let unclassified = fixture.launch("freebie");
    assert!(
        fixture.reserve_availability_of(&unclassified).is_none(),
        "a launch with no `--task` classified nothing, so there is no tier for 1837 to ask about"
    );
}

/// **Acceptance 7** (map line 1837). `glasshouse route` prints the
/// denominator-carrying summary once there are enough high-tier launches to
/// summarise, and the honest *not enough* sentence below that floor.
#[test]
fn route_outcomes_section_prints_the_protected_quota_line() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path());

    // Below `MIN_SAMPLE_FOR_SUMMARY`: one high-tier launch is not enough to
    // summarise as a rate.
    fixture.launch_with_task("freebie", HEAVY_TASK);
    let sparse = both_streams(&fixture.glasshouse(&["route"]));
    assert!(
        sparse.contains(
            "protected quota for high-tier tasks (1837): not enough high-tier \
                          launches (1)"
        ),
        "below the sample floor, the line must say so rather than print a ratio nobody could \
         act on:\n{sparse}"
    );

    // Four more take the window to five, `MIN_SAMPLE_FOR_SUMMARY`'s own
    // floor — every one of them lands on the same unread destination, so
    // the whole count is `unknown` and none is `available`, `at reserve` or
    // `exhausted`.
    for _ in 0..4 {
        fixture.launch_with_task("freebie", HEAVY_TASK);
    }
    let report = both_streams(&fixture.glasshouse(&["route"]));
    let normalised = report.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        normalised.contains(
            "protected quota for high-tier tasks (1837): available 0 · at reserve 0 · \
             exhausted 0 · unknown 5 of 5 high-tier launches"
        ),
        "the summary must carry all four counts and their shared denominator:\n{report}"
    );
    assert!(
        !report.contains("(1837): not enough"),
        "at the floor the summary form must replace the not-enough sentence, not sit beside \
         it:\n{report}"
    );
}
