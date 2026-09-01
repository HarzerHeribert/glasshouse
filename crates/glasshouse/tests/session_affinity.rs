//! Phase 36, lines 1581–1588 — the session-affinity score as a breakdown of
//! named facets, entered the way production enters it.
//!
//! Every ranking test here goes through [`SessionRouter::choose`] with two
//! existing sessions differing in **one facet's input alone**, run in both
//! directions where the input has two sides — practice §35, and the reason
//! `tests/session_router.rs` is built the same way. The facet-level tests use
//! [`affinity_breakdown`], which is the same computation with the facets kept
//! apart, to pin the *unknown* arms: a facet whose signal did not arrive must
//! weigh nothing and say so, which a ranking test cannot see.
//!
//! The last test drives the shipped binary, because nothing above it can
//! fail on a build where `main.rs::routing_destinations` stops attaching what
//! it read (§35: a producer no test enters through is not a producer).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Instant;

use glasshouse::config::pairing::{WarmSession, WarmSessionState};
use glasshouse::harness::pairing::PairingOverrides;
use glasshouse::integrations::IntegrationId;
use glasshouse::provider::quota::CapacityBand;
use glasshouse::routing::classify::classify_heuristically;
use glasshouse::routing::free::FreePool;
use glasshouse::routing::pressure::CapacityFacts;
use glasshouse::routing::request::{
    AnswerProvenance, HeuristicReason, RouterAnswer, RoutingFingerprint, StickyClassification,
};
use glasshouse::routing::session::{
    Destination, RouterInputs, RoutingMoment, SessionContextFacts, SessionRouter, TaskRequirements,
    affinity_breakdown, paths_named_in,
};
use glasshouse::routing::{AssignedModel, Backend, Cost, CredentialId, ToolSemantics};
use glasshouse::secret::SecretRef;

// ---------------------------------------------------------------------------
// Fixtures — thin on purpose; every pair is built in its own test body.
// ---------------------------------------------------------------------------

fn backend(provider: &str, var: &str) -> Backend {
    Backend::new(
        provider,
        "anthropic-messages",
        AssignedModel::named("claude-opus-4"),
        CredentialId::new(
            provider,
            SecretRef::Environment {
                var: var.to_owned(),
            },
        ),
        Cost::Metered,
        ToolSemantics::Verified,
    )
}

fn live(idle_seconds: i64) -> WarmSession {
    WarmSession {
        state: WarmSessionState::Live,
        idle_seconds,
    }
}

/// An existing session on the one backend every pair shares.
fn session(id: &str, idle_seconds: i64) -> Destination {
    Destination::existing(
        id,
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "ANTHROPIC_API_KEY"),
        live(idle_seconds),
    )
}

struct Fixture {
    overrides: PairingOverrides,
    health: FreePool,
    now: Instant,
    requirements: TaskRequirements,
}

impl Fixture {
    fn new() -> Self {
        Self {
            overrides: PairingOverrides::from_parts(
                "no configuration",
                BTreeMap::new(),
                BTreeMap::new(),
            ),
            health: FreePool::new(),
            now: Instant::now(),
            requirements: TaskRequirements::default(),
        }
    }

    /// The same, with a task stated — classified the way a build with no
    /// routing model classifies it.
    fn with_task(task_text: &str) -> Self {
        Self {
            requirements: requirements_for(task_text),
            ..Self::new()
        }
    }

    fn inputs(&self) -> RouterInputs<'_> {
        RouterInputs {
            overrides: &self.overrides,
            health: &self.health,
            now: self.now,
            requirements: self.requirements.clone(),
        }
    }
}

fn requirements_for(task_text: &str) -> TaskRequirements {
    RouterAnswer::new(
        classify_heuristically(task_text),
        AnswerProvenance::Heuristic(HeuristicReason::NoRoutingModel),
    )
    .requirements()
}

/// Which of two destinations the router chooses at a session start.
fn winner(fixture: &Fixture, destinations: &[Destination]) -> String {
    SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            destinations,
            &fixture.inputs(),
        )
        .expect("two destinations were offered")
        .chosen()
        .id()
        .to_owned()
}

/// The `session affinity` contribution's magnitude in the winner's own
/// explanation.
fn affinity_of(fixture: &Fixture, destinations: &[Destination], id: &str) -> f64 {
    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            destinations,
            &fixture.inputs(),
        )
        .expect("two destinations were offered");
    routed
        .considered()
        .iter()
        .find(|(destination, _)| destination.id() == id)
        .and_then(|(_, explanation)| {
            explanation
                .contributions()
                .iter()
                .find(|c| c.name() == "session affinity")
        })
        .map(|c| c.magnitude())
        .unwrap_or_else(|| panic!("`{id}` was not scored, or carried no affinity term"))
}

const REPO_TASK: &str = "fix the panic in src/routing/session.rs when the store is empty";
const PLAIN_QUESTION: &str = "what is the difference between a mutex and a semaphore";

// ---------------------------------------------------------------------------
// Line 1582 — the same task.
// ---------------------------------------------------------------------------

/// Two sessions identical in every respect but which of them the sticky
/// classification cache recorded the current task's classification against.
/// Mirrored: the one it names wins, whichever one that is.
#[test]
fn the_session_the_last_matching_classification_was_made_for_wins_on_same_task() {
    let fixture = Fixture::with_task(REPO_TASK);
    let same = classify_heuristically(REPO_TASK);

    let alpha = session("alpha", 60)
        .with_session_context(SessionContextFacts::UNREAD.with_last_task(Some(same.clone())));
    let beta = session("beta", 60);
    assert_eq!(winner(&fixture, &[alpha, beta]), "alpha");

    let alpha = session("alpha", 60);
    let beta = session("beta", 60)
        .with_session_context(SessionContextFacts::UNREAD.with_last_task(Some(same)));
    assert_eq!(
        winner(&fixture, &[alpha, beta]),
        "beta",
        "the caller's order must not be what decides this pair"
    );
}

/// The facet needs both sides. No task stated, or no record for this session,
/// is *unknown*: it weighs nothing and says which side is missing.
#[test]
fn same_task_is_unknown_without_a_stated_task_or_a_recorded_one() {
    let recorded = session("alpha", 60).with_session_context(
        SessionContextFacts::UNREAD.with_last_task(Some(classify_heuristically(REPO_TASK))),
    );
    let facet = affinity_breakdown(&recorded, None, &TaskRequirements::default())
        .expect("an existing session has a breakdown")
        .same_task;
    assert!(!facet.is_known(), "{}", facet.evidence());
    assert_eq!(facet.magnitude(), 0.0);
    assert!(
        facet.evidence().contains("stated no task"),
        "{}",
        facet.evidence()
    );

    let unrecorded = session("alpha", 60);
    let facet = affinity_breakdown(&unrecorded, None, &requirements_for(REPO_TASK))
        .expect("an existing session has a breakdown")
        .same_task;
    assert!(!facet.is_known(), "{}", facet.evidence());
    assert_eq!(facet.magnitude(), 0.0);
    assert!(
        facet.evidence().contains("no classified task is recorded"),
        "{}",
        facet.evidence()
    );
}

// ---------------------------------------------------------------------------
// Line 1583 — recently touched relevant files.
// ---------------------------------------------------------------------------

/// Two sessions whose checkpoints list different files; the task names one of
/// them. Mirrored on which session touched it.
#[test]
fn the_session_whose_checkpoint_lists_the_file_the_task_names_wins() {
    let fixture = Fixture::with_task(REPO_TASK);
    let named = Some(paths_named_in(REPO_TASK));
    assert_eq!(
        named.as_deref(),
        Some(&["src/routing/session.rs".to_owned()][..]),
        "the extractor must find exactly the one path the task names"
    );
    let touched_it = || {
        SessionContextFacts::UNREAD
            .with_touched_files(Some(vec!["src/routing/session.rs".to_owned()]))
            .with_task_named_paths(named.clone())
    };
    // The other session has **no checkpoint** — unknown, not "touched other
    // files": a session whose checkpoint lists files the task does not name
    // is priced by line 1586's penalty, and a pair built that way would be
    // separated by the penalty on one side rather than the credit on the
    // other. (Found by the mutation that zeroed the credit and survived.)
    let unknown = || SessionContextFacts::UNREAD.with_task_named_paths(named.clone());

    let alpha = session("alpha", 60).with_session_context(touched_it());
    let beta = session("beta", 60).with_session_context(unknown());
    assert_eq!(winner(&fixture, &[alpha.clone(), beta]), "alpha");
    let facet = affinity_breakdown(&alpha, None, &fixture.requirements)
        .unwrap()
        .touched_files;
    assert!(
        facet.is_known() && facet.magnitude() > 0.0,
        "{}",
        facet.evidence()
    );
    assert!(
        facet.evidence().contains("lists 1 of the 1 path"),
        "{}",
        facet.evidence()
    );

    let alpha = session("alpha", 60).with_session_context(unknown());
    let beta = session("beta", 60).with_session_context(touched_it());
    assert_eq!(
        winner(&fixture, &[alpha, beta]),
        "beta",
        "the caller's order must not be what decides this pair"
    );
}

/// The facet is unknown — nothing, and says so — when the task names no
/// path, when no task was stated, and when no checkpoint records the
/// session's files. Three absences, three different sentences.
#[test]
fn touched_files_is_unknown_when_either_operand_is_missing() {
    let facts_with_files = SessionContextFacts::UNREAD
        .with_touched_files(Some(vec!["src/routing/session.rs".to_owned()]));

    let no_task = session("alpha", 60).with_session_context(facts_with_files.clone());
    let facet = affinity_breakdown(&no_task, None, &TaskRequirements::default())
        .unwrap()
        .touched_files;
    assert!(!facet.is_known() && facet.magnitude() == 0.0);
    assert!(
        facet.evidence().contains("no task was stated"),
        "{}",
        facet.evidence()
    );

    let no_path = session("alpha", 60).with_session_context(
        facts_with_files
            .clone()
            .with_task_named_paths(Some(paths_named_in(PLAIN_QUESTION))),
    );
    let facet = affinity_breakdown(&no_path, None, &requirements_for(PLAIN_QUESTION))
        .unwrap()
        .touched_files;
    assert!(!facet.is_known() && facet.magnitude() == 0.0);
    assert!(
        facet.evidence().contains("names no path"),
        "{}",
        facet.evidence()
    );

    let no_checkpoint = session("alpha", 60).with_session_context(
        SessionContextFacts::UNREAD.with_task_named_paths(Some(paths_named_in(REPO_TASK))),
    );
    let facet = affinity_breakdown(&no_checkpoint, None, &requirements_for(REPO_TASK))
        .unwrap()
        .touched_files;
    assert!(!facet.is_known() && facet.magnitude() == 0.0);
    assert!(
        facet.evidence().contains("no checkpoint records"),
        "{}",
        facet.evidence()
    );
}

/// The extractor is a spelling test: paths and dotted file names get in,
/// prose, URLs, abbreviations and version numbers stay out.
#[test]
fn paths_named_in_reads_paths_and_not_prose() {
    assert_eq!(
        paths_named_in(
            "look at `src/main.rs`, Cargo.toml and ./docs/notes.md (e.g. v1.2, i.e. the Ph.D. \
             draft at https://example.com/x/y.md); src/main.rs again"
        ),
        vec![
            "src/main.rs".to_owned(),
            "Cargo.toml".to_owned(),
            "./docs/notes.md".to_owned(),
        ]
    );
    assert!(paths_named_in(PLAIN_QUESTION).is_empty());
}

// ---------------------------------------------------------------------------
// Lines 1584 and 1586 — the native context, intact or compacted into noise.
// ---------------------------------------------------------------------------

/// Two sessions differing only in their observed compaction count: never
/// compacted against compacted five times. Mirrored.
#[test]
fn a_never_compacted_session_beats_a_repeatedly_compacted_one() {
    let fixture = Fixture::new();
    let intact = || SessionContextFacts::UNREAD.with_observed_compactions(Some(0));
    let noisy = || SessionContextFacts::UNREAD.with_observed_compactions(Some(5));

    let alpha = session("alpha", 60).with_session_context(intact());
    let beta = session("beta", 60).with_session_context(noisy());
    assert_eq!(winner(&fixture, &[alpha, beta]), "alpha");

    let alpha = session("alpha", 60).with_session_context(noisy());
    let beta = session("beta", 60).with_session_context(intact());
    assert_eq!(winner(&fixture, &[alpha.clone(), beta.clone()]), "beta");

    // And the margin is line 1586's penalty, not only line 1584's credit:
    // the compacted session must trail by more than the intact one's
    // native-context facet alone accounts for.
    let noisy = affinity_breakdown(&alpha, None, &fixture.requirements).unwrap();
    let intact = affinity_breakdown(&beta, None, &fixture.requirements).unwrap();
    assert!(
        noisy.noise.is_known() && noisy.noise.magnitude() < 0.0,
        "{}",
        noisy.noise.evidence()
    );
    assert!(
        noisy.noise.evidence().contains("compacted 5 times"),
        "{}",
        noisy.noise.evidence()
    );
    assert!(
        noisy.total() < intact.total() - intact.native_context.magnitude(),
        "the compacted session trails only by the intact one's credit ({} vs {}): line 1586's \
         penalty is not being applied",
        noisy.total(),
        intact.total()
    );
}

/// `Some(0)` is a counted clean history and `None` is nobody counting. The
/// two must not score alike: an uncounted history earns nothing and says so.
#[test]
fn a_counted_clean_history_outscores_an_uncounted_one_and_says_why() {
    let counted = session("alpha", 60)
        .with_session_context(SessionContextFacts::UNREAD.with_observed_compactions(Some(0)));
    let uncounted = session("beta", 60);
    let empty = TaskRequirements::default();

    let counted = affinity_breakdown(&counted, None, &empty).unwrap();
    let uncounted = affinity_breakdown(&uncounted, None, &empty).unwrap();
    assert!(counted.native_context.is_known());
    assert!(counted.native_context.magnitude() > 0.0);
    assert!(!uncounted.native_context.is_known());
    assert_eq!(uncounted.native_context.magnitude(), 0.0);
    assert!(
        uncounted
            .native_context
            .evidence()
            .contains("nobody counted"),
        "{}",
        uncounted.native_context.evidence()
    );
    assert!(counted.total() > uncounted.total());
}

/// Line 1586's other two halves: a session whose last classified task differs
/// from this one, and one whose checkpoint lists files the task does not name,
/// each lose to an otherwise identical session about which nothing is known.
#[test]
fn an_unrelated_task_or_unrelated_files_cost_a_session_against_an_unknown_one() {
    let fixture = Fixture::with_task(REPO_TASK);

    let unrelated_task = session("alpha", 60).with_session_context(
        SessionContextFacts::UNREAD.with_last_task(Some(classify_heuristically(PLAIN_QUESTION))),
    );
    let unknown = session("beta", 60);
    assert_eq!(
        winner(&fixture, &[unrelated_task.clone(), unknown.clone()]),
        "beta",
        "a session last seen doing something classed differently must lose to one nothing \
         is known about"
    );
    let noise = affinity_breakdown(&unrelated_task, None, &fixture.requirements)
        .unwrap()
        .noise;
    assert!(noise.magnitude() < 0.0, "{}", noise.evidence());
    assert!(
        noise.evidence().contains("classed differently"),
        "{}",
        noise.evidence()
    );

    let unrelated_files = session("alpha", 60).with_session_context(
        SessionContextFacts::UNREAD
            .with_touched_files(Some(vec!["src/gateway/upstream.rs".to_owned()]))
            .with_task_named_paths(Some(paths_named_in(REPO_TASK))),
    );
    assert_eq!(
        winner(&fixture, &[unrelated_files.clone(), unknown]),
        "beta",
        "a session whose checkpoint lists files the task does not name must lose to one \
         nothing is known about"
    );
    let noise = affinity_breakdown(&unrelated_files, None, &fixture.requirements)
        .unwrap()
        .noise;
    assert!(noise.magnitude() < 0.0, "{}", noise.evidence());
    assert!(
        noise.evidence().contains("names paths"),
        "{}",
        noise.evidence()
    );
}

// ---------------------------------------------------------------------------
// Line 1585 — the prompt cache is likely hot.
// ---------------------------------------------------------------------------

/// The facet steps at the published lifetime: one second inside it earns the
/// term, one second past it earns nothing — and at a task boundary, moving
/// off the backend that built the prefix earns nothing however recent.
#[test]
fn the_prompt_cache_facet_steps_at_the_published_lifetime_and_off_the_backend() {
    let empty = TaskRequirements::default();
    let inside = affinity_breakdown(&session("alpha", 299), None, &empty)
        .unwrap()
        .prompt_cache;
    let past = affinity_breakdown(&session("alpha", 301), None, &empty)
        .unwrap()
        .prompt_cache;
    assert!(
        inside.is_known() && inside.magnitude() > 0.0,
        "{}",
        inside.evidence()
    );
    assert!(
        past.is_known() && past.magnitude() == 0.0,
        "{}",
        past.evidence()
    );
    assert!(
        inside.evidence().contains("likely hot"),
        "{}",
        inside.evidence()
    );

    let current = Destination::existing(
        "current",
        IntegrationId::ClaudeCode,
        "default",
        backend("openai", "OPENAI_API_KEY"),
        live(0),
    );
    let moved = affinity_breakdown(&session("alpha", 1), Some(&current), &empty)
        .unwrap()
        .prompt_cache;
    assert!(
        moved.is_known() && moved.magnitude() == 0.0,
        "{}",
        moved.evidence()
    );
    assert!(
        moved.evidence().contains("moving off"),
        "{}",
        moved.evidence()
    );
}

/// And it decides a ranking: two sessions two seconds apart in idle time on
/// either side of the lifetime. Warmth alone separates them by `1.5 * 2 /
/// 28800`, far less than the term — so the assertion is that removing the
/// prompt-cache facet would have left the pair inside that margin.
#[test]
fn the_prompt_cache_step_is_what_separates_two_sessions_either_side_of_it() {
    let fixture = Fixture::new();
    let inside = session("inside", 299);
    let past = session("past", 301);
    let destinations = [past.clone(), inside.clone()];
    assert_eq!(winner(&fixture, &destinations), "inside");

    let inside_affinity = affinity_of(&fixture, &destinations, "inside");
    let past_affinity = affinity_of(&fixture, &destinations, "past");
    let step = affinity_breakdown(&inside, None, &fixture.requirements)
        .unwrap()
        .prompt_cache
        .magnitude();
    assert!(
        inside_affinity - step < past_affinity + 0.001,
        "without the prompt-cache facet the two would be within warmth's two-second margin \
         ({inside_affinity} vs {past_affinity}, step {step}); the facet is what decided this"
    );
}

// ---------------------------------------------------------------------------
// Line 1587 — significant quota pressure.
// ---------------------------------------------------------------------------

/// Two sessions with the **same** capacity reading and different bands — the
/// band is the reading against each provider's own thresholds and reserve —
/// where one band is significant pressure and the other is not.
#[test]
fn the_reserve_band_costs_a_session_affinity_and_the_healthy_band_does_not() {
    let empty = TaskRequirements::default();
    let reserve = session("alpha", 60)
        .with_capacity_facts(CapacityFacts::new(Some(CapacityBand::Reserve), None));
    let healthy = session("beta", 60)
        .with_capacity_facts(CapacityFacts::new(Some(CapacityBand::Healthy), None));
    let unread = session("gamma", 60);

    let reserve = affinity_breakdown(&reserve, None, &empty)
        .unwrap()
        .quota_pressure;
    let healthy = affinity_breakdown(&healthy, None, &empty)
        .unwrap()
        .quota_pressure;
    let unread = affinity_breakdown(&unread, None, &empty)
        .unwrap()
        .quota_pressure;
    assert!(
        reserve.is_known() && reserve.magnitude() < 0.0,
        "{}",
        reserve.evidence()
    );
    assert!(
        reserve.evidence().contains("significant pressure"),
        "{}",
        reserve.evidence()
    );
    assert!(
        healthy.is_known() && healthy.magnitude() == 0.0,
        "{}",
        healthy.evidence()
    );
    assert!(
        !unread.is_known() && unread.magnitude() == 0.0,
        "{}",
        unread.evidence()
    );
}

// ---------------------------------------------------------------------------
// Lines 1581 and 1588 — the struct is the score and its Display is the
// explanation.
// ---------------------------------------------------------------------------

/// The `session affinity` contribution's magnitude is the breakdown's total,
/// and its evidence names every facet with its line, its signed magnitude and
/// its sentence — including which ones read nothing.
#[test]
fn the_affinity_contribution_is_the_breakdown_and_its_explanation_names_every_facet() {
    let fixture = Fixture::with_task(REPO_TASK);
    let alpha = session("alpha", 60).with_session_context(
        SessionContextFacts::UNREAD
            .with_observed_compactions(Some(0))
            .with_touched_files(Some(vec!["src/routing/session.rs".to_owned()]))
            .with_task_named_paths(Some(paths_named_in(REPO_TASK))),
    );
    let breakdown = affinity_breakdown(&alpha, None, &fixture.requirements).unwrap();
    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[alpha],
            &fixture.inputs(),
        )
        .unwrap();
    let term = routed
        .explanation()
        .contributions()
        .iter()
        .find(|c| c.name() == "session affinity")
        .expect("the affinity term is present");

    assert!((term.magnitude() - breakdown.total()).abs() < 1e-9);
    assert_eq!(term.evidence(), breakdown.to_string());
    for line in [1596u16, 1582, 1583, 1584, 1585, 1586, 1587] {
        let facet = breakdown
            .for_line(line)
            .unwrap_or_else(|| panic!("no facet for {line}"));
        assert!(
            term.evidence().contains(&format!(
                "{:+.3}  {} (line {line}",
                facet.magnitude(),
                facet.name()
            )),
            "line {line}'s facet is not in the rendered explanation:\n{}",
            term.evidence()
        );
        assert!(term.evidence().contains(facet.evidence()));
    }
    assert!(
        term.evidence().contains("(line 1582, unknown)"),
        "an unread facet must be labelled unknown:\n{}",
        term.evidence()
    );
    let rendered = routed.render();
    assert!(rendered.contains("session affinity"), "{rendered}");
    assert!(
        rendered.contains("native context (line 1584)"),
        "{rendered}"
    );
}

/// A fresh destination has no breakdown and keeps the sentence it always had.
#[test]
fn a_fresh_destination_has_no_breakdown_and_is_not_penalised() {
    let fresh = Destination::fresh(
        "fresh:claude-code:default",
        IntegrationId::ClaudeCode,
        "default",
        backend("anthropic", "ANTHROPIC_API_KEY"),
        None,
    );
    assert!(affinity_breakdown(&fresh, None, &TaskRequirements::default()).is_none());
    let fixture = Fixture::new();
    let routed = SessionRouter::new()
        .choose(
            RoutingMoment::SessionStart,
            None,
            &[fresh],
            &fixture.inputs(),
        )
        .unwrap();
    let term = routed
        .explanation()
        .contributions()
        .iter()
        .find(|c| c.name() == "session affinity")
        .unwrap();
    assert_eq!(term.magnitude(), 0.0);
    assert!(term.evidence().contains("not a penalty"));
}

// ---------------------------------------------------------------------------
// The production caller — through the shipped binary.
// ---------------------------------------------------------------------------

const CREDENTIAL_VAR: &str = "GLASSHOUSE_AFFINITY_TEST_KEY";

/// A project with a fake `claude-code` that logs its argv and exits 0, so a
/// launch leaves a warm, resumable session behind.
struct BinaryFixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    argv_log: PathBuf,
}

impl BinaryFixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");
        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let argv_log = base.join("argv.log");
        let harness = install_fake_harness(&bin_dir);
        let escaped = harness.display().to_string().replace('\\', "\\\\");
        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
                 [providers.affinity-probe]\ntemplate = \"openrouter\"\n\
                 credential_env = [\"{CREDENTIAL_VAR}\"]\n"
            ),
        )
        .expect("write user config");
        Self {
            _tmp: tmp,
            base,
            root,
            argv_log,
        }
    }

    /// Launch one headless session, prove the fake harness ran, and return
    /// the **Glasshouse** session id — the one a `Destination` carries.
    ///
    /// Not the `--session-id` in the harness's argv: that is the
    /// harness-native UUID (`claude_code.rs:342`), a different identifier
    /// from the record id the router names. The record id is read off
    /// `glasshouse route`'s own `destination` line, which renders
    /// `Destination::label` and therefore `Destination::id` verbatim.
    fn launch_one(&self) -> String {
        let launched = self.glasshouse(&["launch", "claude-code", "--headless"]);
        assert!(
            launched.status.success(),
            "the launch must succeed:\n{}",
            Self::both_streams(&launched)
        );
        let log = std::fs::read_to_string(&self.argv_log).expect("the harness logged its argv");
        assert!(
            log.contains("--session-id"),
            "the fake harness must have been started with a session:\n{log}"
        );
        let report = String::from_utf8_lossy(&self.glasshouse(&["route"]).stdout).into_owned();
        report
            .lines()
            .find_map(|line| {
                let mut tokens = line.split_whitespace();
                (tokens.next() == Some("destination"))
                    .then(|| tokens.next())
                    .flatten()
            })
            .unwrap_or_else(|| panic!("no `destination` line in the route report:\n{report}"))
            .to_owned()
    }

    /// The one project state directory this run created under `--data-dir`
    /// — where the binary keeps `routing-classification.json`.
    fn project_state_dir(&self) -> PathBuf {
        let projects = self.base.join("data").join("projects");
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&projects)
            .expect("the launch created the projects directory")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_dir())
            .collect();
        assert_eq!(dirs.len(), 1, "exactly one project state dir: {dirs:?}");
        dirs.remove(0)
    }

    fn glasshouse(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .env(CREDENTIAL_VAR, "planted-opaque-affinity-value-36")
            .env(ARGV_LOG_VAR, &self.argv_log)
            .env("PATH", self.base.join("empty-path"))
            .output()
            .expect("the glasshouse binary must be runnable")
    }

    fn both_streams(output: &Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }
}

/// The env var each spawned harness reads its argv-log destination from,
/// set per spawn by [`BinaryFixture::glasshouse`] rather than baked into
/// the script bytes — see [`shared_fixture`]'s doc for why.
const ARGV_LOG_VAR: &str = "GLASSHOUSE_TEST_ARGV_LOG";

/// Write each distinct fixture executable once per test binary instead of
/// once per test, so macOS Gatekeeper (`syspolicyd`/XProtect) validates it
/// once per run instead of once per test — see the project memory
/// `gatekeeper-scans-make-pty-fixtures-flaky` and GH-FIXTURE-REUSE /
/// GH-ARGV-LOG-HOIST. The argv-log destination used to be interpolated into
/// the script bytes, which made every call's content distinct; it is now
/// read from `ARGV_LOG_VAR` at spawn time (set by the caller's `Command`),
/// so the script bytes are constant and every call below collapses onto the
/// one file the first caller writes.
///
/// Sharing is keyed by content, never by the caller's requested name, so a
/// name never causes two distinct fixtures to collide, and a repeated name
/// with the same bytes never causes a second write. Race-free the way
/// `provider/cache.rs::write_json_atomically` is: one process-wide mutex
/// serialises the check-and-write, and the write itself lands in a
/// same-directory temporary name before an atomic rename.
fn shared_fixture(unique_name: &str, contents: &str) -> PathBuf {
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};
    use std::sync::{Mutex, OnceLock};

    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    static CACHE: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();

    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().expect("shared fixture cache poisoned");
    if let Some(path) = guard.get(contents) {
        return path.clone();
    }

    let dir = DIR.get_or_init(|| tempfile::tempdir().expect("shared fixture dir"));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    contents.hash(&mut hasher);
    let digest = format!("{:016x}", hasher.finish());
    let named = Path::new(unique_name);
    let stem = named
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(unique_name);
    let filename = match named.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem}-{digest}.{ext}"),
        None => format!("{stem}-{digest}"),
    };
    let path = dir.path().join(&filename);
    let temporary = dir.path().join(format!("{filename}.writing"));
    std::fs::write(&temporary, contents).expect("write shared fixture");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&temporary).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&temporary, perms).unwrap();
    }
    std::fs::rename(&temporary, &path).expect("rename shared fixture into place");
    guard.insert(contents.to_string(), path.clone());
    path
}

#[cfg(unix)]
fn install_fake_harness(_bin_dir: &Path) -> PathBuf {
    shared_fixture(
        "fake-claude-code",
        &format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"${ARGV_LOG_VAR}\"\nexit 0\n"),
    )
}

#[cfg(windows)]
fn install_fake_harness(_bin_dir: &Path) -> PathBuf {
    shared_fixture(
        "fake-claude-code.cmd",
        &format!("@echo off\r\necho %*>>\"%{ARGV_LOG_VAR}%\"\r\nexit /b 0\r\n"),
    )
}

#[cfg(test)]
mod shared_fixture_proof {
    use super::{ARGV_LOG_VAR, BinaryFixture, install_fake_harness};

    /// **The once-per-binary proof, through the real caller.** Every test in
    /// this file that spawns the harness goes through `BinaryFixture::new`,
    /// which unconditionally calls `install_fake_harness` — so two
    /// independent per-test tempdirs asking for it, the ordinary shape this
    /// binary runs under, must collapse to one file rather than each
    /// writing its own.
    #[test]
    fn two_tempdirs_installing_the_fake_harness_get_one_shared_file() {
        let tmp_a = tempfile::tempdir().expect("tempdir a");
        let tmp_b = tempfile::tempdir().expect("tempdir b");
        let a = install_fake_harness(tmp_a.path());
        let meta_before = std::fs::metadata(&a).expect("fixture exists after first install");

        let b = install_fake_harness(tmp_b.path());
        assert_eq!(
            a, b,
            "two different tempdirs installing the fixture must share one file"
        );
        assert!(
            !a.starts_with(tmp_a.path()) && !a.starts_with(tmp_b.path()),
            "the shared file must live in the per-binary fixture dir, not either \
             test's own tempdir: {a:?}"
        );

        let meta_after = std::fs::metadata(&b).expect("fixture exists after second install");
        assert_eq!(
            meta_before.modified().unwrap(),
            meta_after.modified().unwrap(),
            "a second install of the same fixture must not rewrite the file"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                meta_before.ino(),
                meta_after.ino(),
                "a second install of the same fixture must return the same inode, \
                 not a second copy"
            );
        }
    }

    /// **Bytes constant.** The shared fixture's bytes read the argv-log
    /// destination from `ARGV_LOG_VAR` rather than embedding a per-test
    /// path, so the script text is the same regardless of which tempdir
    /// asked for it.
    #[cfg(unix)]
    #[test]
    fn the_shared_fixture_reads_its_log_path_from_the_env_var_not_the_script() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = install_fake_harness(tmp.path());
        let content = std::fs::read_to_string(&path).expect("read shared fixture");
        assert_eq!(
            content,
            format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"${ARGV_LOG_VAR}\"\nexit 0\n"),
            "the shared fixture's bytes must read the log destination from the env var, \
             not have a path baked in"
        );
    }

    /// **End-to-end, through the real caller.** The env var the fixture
    /// reads is exactly the one `BinaryFixture::glasshouse` sets per spawn
    /// — [`BinaryFixture::launch_one`] already asserts the argv log
    /// contains `--session-id` on every one of the three tests below, so
    /// this only needs to prove it once more, directly, as its own
    /// regression rather than riding on those.
    #[test]
    fn a_real_launch_through_the_shared_fixture_writes_its_argv_to_the_requested_log() {
        let fixture = BinaryFixture::new();
        let session = fixture.launch_one();
        assert!(!session.is_empty(), "a destination id must be reported");
        let log = std::fs::read_to_string(&fixture.argv_log).expect("read argv log");
        assert!(
            log.contains("--session-id"),
            "the shared, env-driven fixture must still log the harness's argv into \
             this fixture's own argv log:\n{log}"
        );
    }
}

/// **The wiring, through the binary.** A session this build creates starts
/// with `observed_compactions = Some(0)` — a *counted* clean history — and
/// the router can only know that if `routing_destinations` attached it. So
/// the report must read "no compaction has been observed", and must **not**
/// read "nobody counted", which is what the facet says on a build where the
/// facts never left `main.rs` (sever `with_session_context` and this fails).
///
/// The task states a path, so the touched-files facet must report the
/// checkpoint side as the missing operand — a launch leaves no checkpoint —
/// which proves the task's named paths reached the router too.
#[test]
fn the_launch_paths_router_reads_the_compaction_count_and_the_tasks_named_paths() {
    let fixture = BinaryFixture::new();
    fixture.launch_one();

    let report = fixture.glasshouse(&["route", "--task", REPO_TASK]);
    let report = BinaryFixture::both_streams(&report);
    assert!(
        report.contains("session affinity"),
        "the ranking must carry the affinity term:\n{report}"
    );
    assert!(
        report.contains("native context (line 1584)")
            && report.contains("no compaction has been observed"),
        "the binary must hand the router the session's counted-clean compaction history:\n\
         {report}"
    );
    assert!(
        !report.contains("nobody counted"),
        "a session this build created is counted, so `nobody counted` means the facts never \
         reached the router:\n{report}"
    );
    assert!(
        report.contains("touched files (line 1583, unknown)")
            && report.contains("no checkpoint records which files"),
        "the task named a path, so the missing operand must be the checkpoint side:\n{report}"
    );
}

/// **Line 1583's link, through the binary.** A checkpoint saved for the
/// session — through `glasshouse checkpoint save`, the same writer a person
/// uses — lists a file; a task naming that file must find it in the
/// session's affinity. Sever `routing_destinations`' call to
/// `session_touched_files` and this fails.
#[test]
fn the_launch_paths_router_reads_the_files_the_sessions_own_checkpoint_lists() {
    let fixture = BinaryFixture::new();
    fixture.launch_one();
    let saved = fixture.glasshouse(&[
        "checkpoint",
        "save",
        "--objective",
        "fix the empty-store panic",
        "--state",
        "reproduced",
        "--file",
        "src/routing/session.rs::choose — the match on `destinations`",
        "--next",
        "guard the empty case",
    ]);
    assert!(
        saved.status.success(),
        "the checkpoint must save:\n{}",
        BinaryFixture::both_streams(&saved)
    );

    let report = fixture.glasshouse(&["route", "--task", REPO_TASK]);
    let report = BinaryFixture::both_streams(&report);
    assert!(
        report.contains("touched files (line 1583)") && report.contains("lists 1 of the 1 path"),
        "the binary must hand the router the file its own checkpoint lists:\n{report}"
    );
}

/// **Line 1582's link, through the binary.** The sticky classification cache
/// is written by the binary only when a routing model answered, which this
/// fixture has none of — so the record is planted exactly where
/// `ClassificationStickyCache::new` reads it, in the shape the binary writes
/// (`StickyClassification::to_json`), naming the launched session. A task
/// classed the same way must then find the session on the same task. Sever
/// `routing_destinations`' `with_last_task` and this fails.
#[test]
fn the_launch_paths_router_reads_the_sticky_caches_last_task_for_the_session() {
    let fixture = BinaryFixture::new();
    let session = fixture.launch_one();
    let record = StickyClassification::new(
        session.as_str(),
        RoutingFingerprint::new(None, &[], std::iter::empty()),
        &classify_heuristically(REPO_TASK),
        1_800_000_000,
    );
    std::fs::write(
        fixture
            .project_state_dir()
            .join("routing-classification.json"),
        record.to_json().expect("the record serialises"),
    )
    .expect("plant the sticky record");

    let report = fixture.glasshouse(&["route", "--task", REPO_TASK]);
    let report = BinaryFixture::both_streams(&report);
    assert!(
        report.contains("same task (line 1582)")
            && report.contains("was classed the way this one is"),
        "the binary must hand the router the sticky cache's last task for the session:\n{report}"
    );
    assert!(
        !report.contains("no classified task is recorded"),
        "the planted record names the session, so this sentence means the facts never left \
         main.rs:\n{report}"
    );
}
