//! Map line 1846's crossover question — `EvaluationObservations::pairing_prior_crossover`
//! and `commands::route::render_pairing_prior_crossover` — *"Measure how
//! quickly local pairing evidence becomes more predictive than the initial
//! same-vendor prior."*
//!
//! Rows are planted through the real ledger APIs in-process — a real session
//! record for `sessions.pairing_class`, and `RoutingCostClassObserved`,
//! `RoutingOutcomeObserved` and `RoutingRated` rows through the ledger's own
//! `record` — the same allowance `tests/responsiveness_terms.rs`'s own
//! header gives its fixture: the producers themselves are proven elsewhere
//! (`tests/evaluation_producers.rs`, `tests/routing_outcome.rs`,
//! `tests/route_rating.rs`), and what is new here is the reader's arithmetic
//! and the readout, over rows a real launch cannot conveniently place
//! twenty-plus of. Every test reads the result through the shipped binary's
//! `glasshouse route`, which is `main.rs::route_report`'s real caller and
//! therefore this package's own production entry point (practice §35).

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;

use glasshouse::evaluation::{
    EvaluationKind, EvaluationObservations, EvaluationOutcome, NewObservation, now_unix,
};
use glasshouse::session::{NewSession, ProjectSessions, SessionPairingClass};
use glasshouse::{Cli, Runtime};

/// A bootstrapped project inside `base` — the same idiom
/// `tests/tier_outcomes.rs` and `tests/responsiveness_terms.rs` use.
struct Fixture {
    base: PathBuf,
    root: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &Path) -> Self {
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        std::fs::create_dir_all(base.join("config")).unwrap();

        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        let runtime = glasshouse::bootstrap(&cli, &root).unwrap();
        Fixture {
            base: base.to_path_buf(),
            root,
            runtime,
        }
    }

    /// The shipped binary, pointed at this project.
    fn glasshouse(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .output()
            .expect("the glasshouse binary must be runnable")
    }

    fn route(&self) -> String {
        let output = self.glasshouse(&["route"]);
        assert!(
            output.status.success(),
            "`glasshouse route` must succeed: stdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// A real session record, so `sessions.pairing_class` is a fact the
    /// session-id join can actually read — the same door
    /// `tests/responsiveness_terms.rs::Fixture::session` uses.
    fn session(&self, pairing_class: Option<SessionPairingClass>) -> String {
        let sessions = ProjectSessions::open(&self.runtime).unwrap();
        let record = sessions
            .store()
            .create(NewSession::embedded("claude-code").with_pairing_class(pairing_class))
            .unwrap();
        record.id.as_str().to_owned()
    }

    /// Plant one `RoutingCostClassObserved` decision row — the routing-row
    /// half of "a session with a routing row, a pairing class, and an
    /// outcome" the packet's contract asks for.
    fn plant_decision(&self, session_id: &str, observed_at_unix: i64) {
        let ledger = EvaluationObservations::open(&self.runtime).unwrap();
        ledger
            .record(
                NewObservation::new(EvaluationKind::RoutingCostClassObserved)
                    .with_subject("free")
                    .with_session_id(session_id)
                    .with_detail("fresh:claude-code:probe"),
                observed_at_unix,
            )
            .unwrap();
    }

    /// Plant one `RoutingOutcomeObserved` proxy row — the harness's own
    /// verdict on the routed session's turn.
    fn plant_proxy(&self, session_id: &str, completed: bool, observed_at_unix: i64) {
        let ledger = EvaluationObservations::open(&self.runtime).unwrap();
        ledger
            .record(
                NewObservation::new(EvaluationKind::RoutingOutcomeObserved)
                    .with_subject(if completed { "completed" } else { "failed" })
                    .with_session_id(session_id),
                observed_at_unix,
            )
            .unwrap();
    }

    /// Plant one `RoutingRated` row — an explicit verdict that replaces the
    /// proxy for this session.
    fn plant_rating(&self, session_id: &str, useful: bool, observed_at_unix: i64) {
        let ledger = EvaluationObservations::open(&self.runtime).unwrap();
        ledger
            .record(
                NewObservation::new(EvaluationKind::RoutingRated)
                    .with_subject("fresh:claude-code:probe")
                    .with_session_id(session_id)
                    .with_outcome(if useful {
                        EvaluationOutcome::Useful
                    } else {
                        EvaluationOutcome::NotUseful
                    }),
                observed_at_unix,
            )
            .unwrap();
    }

    /// Plant one routed, outcome-bearing session of `pairing_class`, `count`
    /// times, each with a strictly later routing timestamp than the one
    /// before it — the ordering
    /// `EvaluationObservations::pairing_prior_crossover` walks per class.
    fn plant_sessions(
        &self,
        pairing_class: SessionPairingClass,
        count: usize,
        success: bool,
        start_at: i64,
    ) {
        for i in 0..count {
            let session = self.session(Some(pairing_class));
            let at = start_at + (i as i64) * 10;
            self.plant_decision(&session, at);
            self.plant_proxy(&session, success, at + 1);
        }
    }
}

/// The `local pairing evidence vs the same-vendor prior (1846):` block —
/// everything from its own header to the end of the report, the same
/// slicing technique `tests/responsiveness_terms.rs::separation_section`
/// uses.
fn crossover_section(report: &str) -> String {
    let marker = "local pairing evidence vs the same-vendor prior (1846):";
    let start = report
        .find(marker)
        .unwrap_or_else(|| panic!("no map-line-1846 section in:\n{report}"));
    report[start..].to_owned()
}

/// **Behaviour 1, and the mutation target.** Twenty-five vendor-native
/// sessions that all succeed: the prior predicts success for every one of
/// them and is right every time, in every bucket. Local evidence is wrong
/// at `k = 0` (nothing to predict from) but right everywhere else, so it
/// only catches all the way up to the prior once a bucket's sessions all
/// carry `k >= 1` — bucket `5-9`, where both read `5/5`.
///
/// The mutation this test kills is "invert the prior's prediction (vendor-
/// native predicts failure)": with the prediction inverted, the prior would
/// be wrong in every bucket instead of right, flipping every `prior right`
/// count in this assertion.
#[test]
fn vendor_native_always_succeeds_prior_right_everywhere_local_catches_up_at_5_9() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let start = now_unix() - 3600;

    fixture.plant_sessions(SessionPairingClass::VendorNative, 25, true, start);

    let report = fixture.route();
    let section = crossover_section(&report);
    let normalised = section.split_whitespace().collect::<Vec<_>>().join(" ");

    assert!(
        normalised.contains("k 0-4: prior right 5/5 \u{b7} local right 4/5"),
        "k=0 is scored wrong even though every later session in the bucket succeeds: {section}"
    );
    assert!(
        normalised.contains("k 5-9: prior right 5/5 \u{b7} local right 5/5"),
        "from k=5 on, local has real history and matches the prior exactly: {section}"
    );
    assert!(
        normalised.contains("k 10-19: prior right 10/10 \u{b7} local right 10/10"),
        "{section}"
    );
    assert!(
        normalised.contains("k 20+: prior right 5/5 \u{b7} local right 5/5"),
        "{section}"
    );
    assert!(
        normalised.contains("local evidence at least as predictive from bucket 5-9"),
        "bucket 0-4 falls one short of the prior (4 of 5 vs 5 of 5), so the crossover must not \
         name it; 5-9 is the first bucket where local catches all the way up: {section}"
    );
}

/// **Behaviour 2.** Twelve vendor-native sessions that all fail: the prior
/// predicts success for every one and is wrong every time — `0` prior-right
/// in every bucket. Local evidence is wrong only at `k = 0`; from `k = 1` on,
/// a pure-failure history correctly predicts failure. Since the prior is
/// never right, `local right >= prior right` holds as soon as any bucket
/// clears the sample floor, which is the first bucket in order: `0-4`.
#[test]
fn vendor_native_always_fails_prior_wrong_everywhere_crossover_is_first_bucket_at_the_floor() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let start = now_unix() - 3600;

    fixture.plant_sessions(SessionPairingClass::VendorNative, 12, false, start);

    let report = fixture.route();
    let section = crossover_section(&report);
    let normalised = section.split_whitespace().collect::<Vec<_>>().join(" ");

    assert!(
        normalised.contains("k 0-4: prior right 0/5 \u{b7} local right 4/5"),
        "the prior predicts success for a vendor-native pairing and is wrong on every failure; \
         local is wrong only at k=0: {section}"
    );
    assert!(
        normalised.contains("k 5-9: prior right 0/5 \u{b7} local right 5/5"),
        "{section}"
    );
    assert!(
        normalised.contains("k 10-19: prior right 0/2 \u{b7} local right 2/2"),
        "only two sessions reach k=10 or k=11 with twelve seeded: {section}"
    );
    assert!(
        normalised.contains("local evidence at least as predictive from bucket 0-4"),
        "bucket 0-4 already clears the 5-session floor and the prior is never right, so it is \
         the first qualifying bucket: {section}"
    );
}

/// **Behaviour 3.** A session whose harness reported `completed` but whose
/// operator rated it `not-useful` is scored as a failure, not a success —
/// the rating replaces the proxy in the outcome this reader uses, exactly as
/// it does for [`glasshouse::evaluation::EvaluationObservations::route_outcomes_by`].
///
/// Four vendor-native sessions succeed (building a 4/4 local success
/// history); a fifth reports `completed` but is rated `not-useful`. If the
/// proxy were used instead of the rating, the prior and local predictions
/// would both read success and both would be right for that session,
/// printing `prior right 5/5 · local right 4/5`. Because the rating
/// overrides the proxy, the fifth session is a failure: the prior (which
/// always predicts success for a vendor-native pairing) is wrong on it, and
/// local — with a 4/4 success history — also predicts success and is wrong
/// on it too.
#[test]
fn a_rated_session_overrides_its_completed_proxy() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let start = now_unix() - 3600;

    for i in 0..4 {
        let session = fixture.session(Some(SessionPairingClass::VendorNative));
        let at = start + i * 10;
        fixture.plant_decision(&session, at);
        fixture.plant_proxy(&session, true, at + 1);
    }
    let rated_session = fixture.session(Some(SessionPairingClass::VendorNative));
    let at = start + 4 * 10;
    fixture.plant_decision(&rated_session, at);
    fixture.plant_proxy(&rated_session, true, at + 1);
    fixture.plant_rating(&rated_session, false, at + 2);

    let report = fixture.route();
    let section = crossover_section(&report);
    let normalised = section.split_whitespace().collect::<Vec<_>>().join(" ");

    assert!(
        normalised.contains("k 0-4: prior right 4/5 \u{b7} local right 3/5"),
        "the rated session's proxy `completed` must not count as a success once it carries a \
         `not-useful` rating — the mutation this line would catch counts it as a success and \
         reads `prior right 5/5 · local right 4/5` instead: {section}"
    );
}

/// **Behaviour 4.** Fewer than the sample floor anywhere prints the *not
/// yet* line, buckets with zero sessions print `none`, and the reader never
/// divides by zero.
#[test]
fn fewer_than_the_floor_prints_not_yet_and_never_divides_by_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let start = now_unix() - 3600;

    fixture.plant_sessions(SessionPairingClass::VendorNative, 3, true, start);

    let report = fixture.route();
    let section = crossover_section(&report);
    let normalised = section.split_whitespace().collect::<Vec<_>>().join(" ");

    assert!(
        normalised.contains("k 0-4: prior right 3/3 \u{b7} local right 2/3"),
        "{section}"
    );
    assert!(normalised.contains("k 5-9: none"), "{section}");
    assert!(normalised.contains("k 10-19: none"), "{section}");
    assert!(normalised.contains("k 20+: none"), "{section}");
    assert!(
        normalised.contains(
            "not yet: no bucket with at least 5 sessions where local evidence matches the prior"
        ),
        "no bucket clears the sample floor, so the section must say so rather than name one: \
         {section}"
    );
}
