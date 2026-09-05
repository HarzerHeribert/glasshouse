//! `glasshouse rate-route` — map line 1846's design note, *"The routing half
//! of RC-B: an explicit route rating when given, the turn-outcome proxy
//! otherwise"* (2026-09-05).
//!
//! **Everything here goes through the shipped binary**, exactly as
//! `tests/routing_outcome.rs` does and for the same reason: the claim is
//! about the door (`rate-route`), the reader (`route_outcomes_by` and
//! `route_outcomes_by_pairing_class`), and the readout (`glasshouse route`)
//! agreeing, and calling `record_route_rating` from a test would prove
//! nothing about the door or the readers.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;
use rusqlite::Connection;

use glasshouse::evaluation::{EvaluationKind, EvaluationObservations, EvaluationOutcome};
use glasshouse::{Cli, Runtime};

/// The credential variable the fixture's provider names, planted in the
/// child's environment only.
const CREDENTIAL_VAR: &str = "GLASSHOUSE_ROUTE_RATING_KEY";

const FREE_MODEL: &str = "probe/free-model";

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// A project wired with a fake harness and one direct-provider launch
/// profile, so `glasshouse launch` and `glasshouse hook` run end to end —
/// the same shape `tests/routing_outcome.rs::Fixture` uses, with a `name`
/// so two fixtures can share one `base` as two real, isolated projects.
struct Fixture {
    base: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &Path, name: &str) -> Self {
        let root = base.join("workspace").join(name);
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
                 [profiles.freebie.backend]\nkind = \"direct-provider\"\nprovider = \"probe\"\n"
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

    fn launch(&self) -> String {
        let before = self.session_ids();
        let launched = self.glasshouse(&[
            "launch",
            "claude-code",
            "--headless",
            "--profile",
            "freebie",
        ]);
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
        assert_eq!(created.len(), 1, "a launch must create exactly one session");
        created.remove(0)
    }

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
            .write_all(b"{}")
            .expect("write the hook payload");
        let output = child.wait_with_output().expect("the hook must exit");
        assert!(
            output.status.success(),
            "a hook always exits zero:\n{}",
            both_streams(&output)
        );
        output
    }

    fn rate_route(&self, session: &str, verdict: &str, note: Option<&str>) -> std::process::Output {
        let mut args = vec!["rate-route", session, verdict];
        if let Some(note) = note {
            args.push("--note");
            args.push(note);
        }
        self.glasshouse(&args)
    }

    fn route_report(&self) -> String {
        let output = self.glasshouse(&["route"]);
        assert!(
            output.status.success(),
            "`glasshouse route` failed:\n{}",
            both_streams(&output)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn ledger(&self) -> EvaluationObservations {
        EvaluationObservations::open(&self.runtime).unwrap()
    }

    fn db(&self) -> Connection {
        Connection::open(self.runtime.database_path()).unwrap()
    }

    fn session_ids(&self) -> Vec<String> {
        let conn = self.db();
        let mut statement = conn.prepare("SELECT id FROM sessions").unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn ratings_of(
        &self,
        session: &str,
    ) -> Vec<(Option<String>, EvaluationOutcome, Option<String>)> {
        self.ledger()
            .recent_of_kind(EvaluationKind::RoutingRated, 50)
            .unwrap()
            .into_iter()
            .filter(|row| row.session_id.as_deref() == Some(session))
            .map(|row| (row.subject, row.outcome, row.detail))
            .collect()
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

/// **Behaviour 1.** Rating a routed session appends one `routing_rated` row
/// naming the destination it was routed to, carries the note, and the
/// command prints the row it wrote.
#[test]
fn rate_route_useful_appends_a_routing_rated_row() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let session = fixture.launch();
    fixture.hook(&session, "Stop");

    let output = fixture.rate_route(&session, "useful", Some("shipped the feature"));
    assert!(
        output.status.success(),
        "`rate-route` on a routed session must succeed:\n{}",
        both_streams(&output)
    );
    let printed = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        printed.contains(&session) && printed.contains("useful"),
        "the command must print the session and the verdict it recorded:\n{printed}"
    );

    let ratings = fixture.ratings_of(&session);
    assert_eq!(ratings.len(), 1, "one rating, one row");
    assert_eq!(
        ratings[0].0.as_deref(),
        Some("fresh:claude-code:freebie"),
        "subject is the destination the session was routed to"
    );
    assert_eq!(ratings[0].1, EvaluationOutcome::Useful);
    assert_eq!(
        ratings[0].2.as_deref(),
        Some("shipped the feature"),
        "the note lands in detail, unparsed"
    );
}

/// **Behaviour 2.** A session with no routing decision, and an unknown
/// session id, are both refused: nothing is written and the command exits
/// non-zero with a sentence saying why.
#[test]
fn rate_route_refuses_a_session_with_no_routing_decision() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let output = fixture.rate_route("not-a-real-session-id", "useful", None);
    assert!(
        !output.status.success(),
        "rating an unrouted or unknown session must fail"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("never taken")
            || String::from_utf8_lossy(&output.stderr).contains("no recorded route"),
        "the refusal must say why:\n{}",
        both_streams(&output)
    );
    assert!(
        fixture
            .ledger()
            .recent_of_kind(EvaluationKind::RoutingRated, 10)
            .unwrap()
            .is_empty(),
        "a refused rating must record nothing"
    );
}

/// `useful`/`not-useful` are the only two words this door accepts — every
/// other word from `memory rate`'s own eight-word vocabulary is refused by
/// name, `unknown` included.
#[test]
fn rate_route_refuses_every_word_outside_the_two_it_accepts() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let session = fixture.launch();

    for word in ["unknown", "prevented-repetition", "caused-complexity"] {
        let output = fixture.rate_route(&session, word, None);
        assert!(
            !output.status.success(),
            "`{word}` must be refused as a route rating verdict"
        );
    }
    assert!(
        fixture.ratings_of(&session).is_empty(),
        "no refused word may write a row"
    );
}

/// **Behaviour 3 and the mutation target.** A rated session is counted by
/// its rating instead of its proxy in both `route_outcomes_by` and
/// `route_outcomes_by_pairing_class`, and the two counts print apart, never
/// summed.
///
/// Two sessions land in the same bucket (`free`); both harnesses report
/// `Stop` (a completed proxy turn); one session is then rated
/// `not-useful`. The mutation this test kills is "keep counting a rated
/// session by its proxy as well": with the exclusion dropped, the free
/// bucket's reported-turns count stays at 2 instead of dropping to 1, and
/// this test's first assertion fails.
#[test]
fn a_rated_session_is_counted_by_its_rating_and_the_proxy_count_drops_by_one() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let rated = fixture.launch();
    fixture.hook(&rated, "Stop");
    let unrated = fixture.launch();
    fixture.hook(&unrated, "Stop");

    fixture.rate_route(&rated, "not-useful", None);

    let report = fixture.route_report();
    let normalised = report.split_whitespace().collect::<Vec<_>>().join(" ");

    assert!(
        normalised.contains("free : 1 of 1 reported turns completed; 2 sessions routed"),
        "the rated session's proxy turn must drop out of the free bucket's reported turns, \
         leaving only the unrated session's — the mutation this line kills counts it twice:\n{report}"
    );
    assert!(
        normalised.contains("rated 0 useful / 1 not-useful"),
        "the rated count must print apart from the proxy figures, never summed into them:\n{report}"
    );
    assert!(
        !normalised.contains("2 of 2 reported turns completed"),
        "a rated session's proxy verdict must never be added back on top of its rating:\n{report}"
    );
}

/// A session rated twice is counted under its latest rating.
#[test]
fn rating_the_same_session_twice_takes_the_latest() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let session = fixture.launch();
    fixture.hook(&session, "Stop");

    fixture.rate_route(&session, "useful", None);
    fixture.rate_route(&session, "not-useful", None);

    assert_eq!(
        fixture.ratings_of(&session).len(),
        2,
        "a rating is a new row, never an edit of the one before it"
    );

    let report = fixture.route_report();
    let normalised = report.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        normalised.contains("rated 0 useful / 1 not-useful"),
        "the latest rating must win over the one it revised:\n{report}"
    );
}

/// **Behaviour 4.** With no ratings in the window, `glasshouse route`'s
/// outcomes section is byte-identical to today: no ` · rated` clause
/// appears anywhere.
#[test]
fn with_no_ratings_the_report_carries_no_rated_clause() {
    let tmp = tempdir();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let session = fixture.launch();
    fixture.hook(&session, "Stop");

    let report = fixture.route_report();
    assert!(
        !report.contains("rated"),
        "a window with no ratings must print nothing about them:\n{report}"
    );
}

/// A session id from another project is refused by the ledger's project
/// scope — the same isolation `tests/memory_rating.rs`'s
/// `a_memory_from_another_project_is_refused` proves for `memory rate`.
#[test]
fn a_session_from_another_project_is_refused() {
    let tmp = tempdir();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");

    let beta_session = beta.launch();
    beta.hook(&beta_session, "Stop");

    let output = alpha.rate_route(&beta_session, "useful", None);
    assert!(
        !output.status.success(),
        "rating another project's session must be refused"
    );
    assert!(
        alpha
            .ledger()
            .recent_of_kind(EvaluationKind::RoutingRated, 10)
            .unwrap()
            .is_empty(),
        "alpha's ledger must record nothing for a refused rating"
    );
    assert!(
        beta.ledger()
            .recent_of_kind(EvaluationKind::RoutingRated, 10)
            .unwrap()
            .is_empty(),
        "the command ran against alpha's project, not beta's, so beta must see nothing either"
    );
}
