//! GH-SUPPORT-WORK-ECONOMY — the economy of Glasshouse's own bounded support
//! work: which resource runs it, and whose reserve policy decides.
//!
//! # Why so much of this file spawns the binary
//!
//! Every claim here is about a *production* decision — practice §35's rule
//! that a caller every test bypasses is not a caller. The two support jobs
//! Glasshouse runs on its own behalf are reached only through the shipped
//! binary: memory extraction through `glasshouse hook` at the end of a turn
//! (`main.rs::disposable_extraction_model`), and the session router's
//! difficulty-conditioned ranking through `glasshouse route --task`. A unit
//! test that called `DisposableRouting::choose` itself would keep passing
//! with either wiring deleted, which is exactly what this file must not do.
//!
//! The one library-level test below (`a_local_free_candidate_…`) is marked as
//! such in its own doc: locality is a *classification* preference, and
//! `glasshouse classify`'s automatic mode needs a provider registry entry
//! this fixture cannot fabricate, so that half is proven at
//! `choose_for_automatic_classification` — the function
//! `main.rs::automatic_classification_choice` calls — rather than through the
//! binary. It says so rather than implying more.
//!
//! # The rationale is read back out of the database, not off a stream
//!
//! `main.rs::disposable_extraction_model` records the routing rationale it
//! *used* through `evaluation::record_disposable_route`
//! (`GH-DISPOSABLE-ROUTE-SINK`), so the decision survives the process that
//! made it and can be asserted on after the binary has exited. That is the
//! consumer capability map line 1577's background half needed.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Instant;

use clap::Parser as _;
use glasshouse::config::{ProviderConfig, ReservePoliciesConfig, UserConfig};
use glasshouse::evaluation::{EvaluationKind, EvaluationObservation, EvaluationObservations};
use glasshouse::provider::quota::{CapacityBand, ReserveDecisionInputs, evaluate_reserve_spend};
use glasshouse::routing::classify::WorkloadTier;
use glasshouse::routing::disposable::{
    AutomaticClassificationDecision, CandidateCapacity, ClassificationPolicy, DisposableCandidate,
    DisposableRouting, JobKind,
};
use glasshouse::routing::evidence::ClassificationRecord;
use glasshouse::routing::free::{FreePool, FreePreferences};
use glasshouse::routing::pressure::{ReservePolicies, ReservePolicy, ReserveScope};
use glasshouse::routing::{Cost, CredentialId};
use glasshouse::secret::SecretRef;
use glasshouse::session::{NewSession, ProjectSessions, SessionId, SessionLifecycle};
use glasshouse::{Cli, Runtime};

/// The provider every fixture configures. A name no catalogue contains, so a
/// rationale carrying it can only have come from this fixture's own decision.
const PROVIDER: &str = "support-economy-runner";
const METERED_MODEL: &str = "probe-metered-model-sw";
const FREE_MODEL: &str = "probe-free-model-sw";
const CREDENTIAL_VAR: &str = "GLASSHOUSE_TEST_ONLY_SUPPORT_ECONOMY_KEY";
const CREDENTIAL: &str = "sk-fabricated-support-economy-value-not-a-real-credential";

/// What the shipped binary prints — and stores — when every metered
/// candidate was refused by the protected-reserve gate.
const DENIED: &str = "protected-reserve policy denied every metered candidate";

/// The percentage headroom that lands a candidate in the reserve band under
/// the default thresholds (exhausted 2, reserve = `PremiumReservePercent`'s
/// default 20): ten of a hundred requests left.
const RESERVE_BAND_REMAINING: i64 = 10;
const RESERVE_BAND_LIMIT: i64 = 100;

/// Far enough out that `evaluate_reserve_spend`'s imminent-reset relief does
/// not apply — `provider::quota::RESET_DISTANT_SECONDS` is 3600.
const DISTANT_RESET_SECONDS: i64 = 7_200;

// ---------------------------------------------------------------------------
// A project, and the binary run against it.
// ---------------------------------------------------------------------------

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");
        let runtime = bootstrap(&base, &root);
        Self {
            _tmp: tmp,
            base,
            root,
            runtime,
        }
    }

    /// The configuration a person writes for a project whose only routable
    /// resource is a metered one, plus the two reserve policies.
    ///
    /// Deliberately **no** `memory_extraction_model`: naming one takes
    /// `disposable_extraction_model`'s early return, where no disposable
    /// routing decision is made at all. This fixture is about the branch that
    /// does decide.
    fn configure(&self, free: &[&str], metered: &[&str], reserve: ReservePoliciesConfig) {
        let mut user = UserConfig::load(self.runtime.paths()).unwrap();
        let mut provider = ProviderConfig::new("openai-compatible");
        provider.set_credential_env(vec![CREDENTIAL_VAR.to_owned()]);
        provider.set_free_models(free.iter().map(|m| (*m).to_owned()).collect());
        provider.set_metered_models(metered.iter().map(|m| (*m).to_owned()).collect());
        user.providers_mut().set(PROVIDER, provider);
        user.routing_mut().set_reserve(Some(reserve));
        // A harness the binary can actually see, so `glasshouse route` has
        // somewhere to rank rather than refusing for want of a destination.
        // It is never executed by anything in this file; it exists so the
        // destination set is non-empty.
        let harness = install_fake_harness(&self.base.join("bin"));
        let integration = user
            .integrations_mut()
            .entry(glasshouse::integrations::IntegrationId::ClaudeCode);
        integration.set_enabled(true);
        integration.set_executable(Some(harness));
        user.onboarding_mut()
            .mark_completed(glasshouse::VERSION.to_owned());
        user.save(self.runtime.paths()).unwrap();
    }

    /// Plant a gateway quota reading where `GatewayQuotaCache::new` resolves
    /// one from this run's `--data-dir`, and prove it landed — the same
    /// technique `tests/subscription_pressure.rs::Binary::plant_quota` uses,
    /// because it is the only way a band reaches the shipped binary without
    /// a network.
    fn plant_reserve_band(&self) {
        let cache = glasshouse::provider::telemetry::GatewayQuotaCache::at(
            self.base.join("data").join("gateway-quota"),
        );
        let limit = RESERVE_BAND_LIMIT.to_string();
        let remaining = RESERVE_BAND_REMAINING.to_string();
        let reset = DISTANT_RESET_SECONDS.to_string();
        cache.store(
            PROVIDER,
            &glasshouse::provider::telemetry::RateLimitHeaders::read(vec![
                ("ratelimit-limit", limit.as_str()),
                ("ratelimit-remaining", remaining.as_str()),
                ("ratelimit-reset", reset.as_str()),
            ]),
            glasshouse::provider::cache::now_unix_seconds(),
        );
        assert!(
            cache.load(PROVIDER).is_some(),
            "the planted reading for `{PROVIDER}` must be on disk and readable"
        );
    }

    fn running_session(&self) -> SessionId {
        let sessions = ProjectSessions::open(&self.runtime).unwrap();
        let store = sessions.store();
        let record = store.create(NewSession::embedded("claude-code")).unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Running)
            .unwrap();
        record.id
    }

    /// Run `glasshouse hook`, exactly as a harness runs it at the end of a
    /// turn. A hook may never fail a person's turn, so a non-zero exit is a
    /// defect in every test here rather than a result any of them wants.
    fn completed_turn(&self, session: &SessionId) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .env(CREDENTIAL_VAR, CREDENTIAL)
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("hook")
            .arg("--session")
            .arg(session.as_str())
            .arg("--event")
            .arg("Stop")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the glasshouse binary must be runnable");
        child
            .stdin
            .as_mut()
            .expect("stdin was piped")
            .write_all(PAYLOAD.as_bytes())
            .expect("the handler must read its payload rather than closing the pipe");
        let output = child.wait_with_output().expect("the hook must finish");
        assert!(
            output.status.success(),
            "a hook must exit zero whatever routing decided: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// The rationale of the one disposable-routing decision this project has
    /// recorded, read back through the store's own door.
    fn one_rationale(&self) -> String {
        let decisions = self.decisions();
        assert_eq!(
            decisions.len(),
            1,
            "one completed turn is one routing decision: {decisions:#?}"
        );
        decisions[0]
            .detail
            .clone()
            .expect("a decision with no rationale is what GH-DISPOSABLE-ROUTE-SINK prevents")
    }

    fn decisions(&self) -> Vec<EvaluationObservation> {
        let ledger = EvaluationObservations::open(&self.runtime).expect("open the ledger");
        let rows = ledger
            .recent_of_kind(EvaluationKind::DisposableRouteDecided, 20)
            .expect("read the ledger");
        // Dropped before anything else opens the project database — practice
        // §65, and the same rule `tests/disposable_route_sink.rs` follows.
        drop(ledger);
        rows
    }

    /// One completed turn under `reserve`, against a single reserve-band
    /// metered candidate, and the rationale it recorded.
    fn extraction_rationale_under(reserve: ReservePoliciesConfig) -> String {
        let fixture = Fixture::new();
        fixture.configure(&[], &[METERED_MODEL], reserve);
        fixture.plant_reserve_band();
        let session = fixture.running_session();
        fixture.completed_turn(&session);
        fixture.one_rationale()
    }
}

/// A harness payload with the conversation in it. Never read by the hook, and
/// therefore never available to anything recorded here.
const PAYLOAD: &str = concat!(
    r#"{"session_id":"native-1","hook_event_name":"Stop","cwd":"/somewhere","#,
    r#""prompt":"PAYLOAD-PROMPT-MUST-NEVER-BE-STORED","#,
    r#""last_assistant_message":"PAYLOAD-REPLY-MUST-NEVER-BE-STORED"}"#
);

/// A harness executable that records nothing and exits zero. Only its
/// existence matters here: `glasshouse route` refuses to rank when no
/// harness is installed, and that refusal would be the thing under test
/// rather than the ranking.
#[cfg(unix)]
fn install_fake_harness(bin_dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(bin_dir).expect("create bin dir");
    let path = bin_dir.join("fake-claude-code");
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_fake_harness(bin_dir: &Path) -> PathBuf {
    std::fs::create_dir_all(bin_dir).expect("create bin dir");
    let path = bin_dir.join("fake-claude-code.cmd");
    std::fs::write(&path, "@echo off\r\nexit /b 0\r\n").expect("write fake harness");
    path
}

fn bootstrap(base: &Path, root: &Path) -> Runtime {
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        base.join("data").to_str().unwrap(),
        "--config-dir",
        base.join("config").to_str().unwrap(),
    ])
    .unwrap();
    glasshouse::bootstrap(&cli, root).unwrap()
}

fn policies(interactive: ReservePolicy, background: ReservePolicy) -> ReservePoliciesConfig {
    let mut config = ReservePoliciesConfig::default();
    config.set_interactive(Some(interactive));
    config.set_background(Some(background));
    config
}

// ---------------------------------------------------------------------------
// 1577 — the background reserve policy, on the path that acts.
// ---------------------------------------------------------------------------

/// **Capability map line 1577, background half.**
///
/// The same project, the same planted reserve-band reading, the same job —
/// three configurations, and only `[routing.reserve]` differs.
///
/// - `background = "protect"` (and the default) denies, exactly as before
///   this line had a reader;
/// - `background = "spend"` admits the reserve-band candidate and the
///   recorded rationale says the background policy did it;
/// - `interactive = "spend"` with `background = "protect"` still denies —
///   which is the half that makes the two policies *independent* rather than
///   one value read twice. This is the assertion that dies if
///   `main.rs`'s call site selects `ReserveScope::Interactive`.
#[test]
fn the_background_reserve_policy_decides_a_support_job_and_the_interactive_one_does_not() {
    let protecting = Fixture::extraction_rationale_under(policies(
        ReservePolicy::Protect,
        ReservePolicy::Protect,
    ));
    assert!(
        protecting.contains(DENIED),
        "a reserve-band candidate must be denied to a leaf-tier support job under `protect`:\n\
         {protecting}"
    );

    let spending =
        Fixture::extraction_rationale_under(policies(ReservePolicy::Protect, ReservePolicy::Spend));
    assert!(
        !spending.contains(DENIED),
        "`background = \"spend\"` must remove the denial:\n{spending}"
    );
    assert!(
        spending.contains(METERED_MODEL),
        "the admitted candidate must be the one chosen:\n{spending}"
    );
    assert!(
        spending.contains("background reserve policy"),
        "the rationale must name the policy that admitted this candidate:\n{spending}"
    );
    assert!(
        spending.contains("admitted this candidate in the reserve band anyway"),
        "the rationale must say the policy admitted it, and from which band:\n{spending}"
    );
    assert!(
        spending.contains("map line 1550"),
        "spend removes the denial, not the pressure: the reserve decision's own reason must \
         still be rendered:\n{spending}"
    );

    // The half that proves the scopes are independent rather than aliases.
    let other_scope =
        Fixture::extraction_rationale_under(policies(ReservePolicy::Spend, ReservePolicy::Protect));
    assert!(
        other_scope.contains(DENIED),
        "a person's own session policy must not decide a background support job:\n{other_scope}"
    );

    // No credential and no conversation reached the durable rationale.
    for rationale in [&protecting, &spending, &other_scope] {
        assert!(!rationale.contains(CREDENTIAL), "{rationale}");
        assert!(!rationale.contains(CREDENTIAL_VAR), "{rationale}");
        assert!(
            !rationale.contains("PAYLOAD-PROMPT-MUST-NEVER-BE-STORED"),
            "{rationale}"
        );
    }
}

/// The default, written out: a project that says nothing about
/// `[routing.reserve]` gets `protect`, which is what a spending protection
/// must default to. Separate from the test above because "the default is
/// protect" and "protect denies" are two claims, and a fixture that only ever
/// wrote a policy could not tell them apart.
#[test]
fn an_unconfigured_project_protects_the_reserve_for_its_own_bookkeeping() {
    let rationale = Fixture::extraction_rationale_under(ReservePoliciesConfig::default());
    assert!(
        rationale.contains(DENIED),
        "an unconfigured reserve policy must fail closed:\n{rationale}"
    );
}

// ---------------------------------------------------------------------------
// 1607 — prefer local or free resources for trivial classification and
// extraction work.
// ---------------------------------------------------------------------------

fn credential(provider: &str) -> CredentialId {
    CredentialId::new(
        provider,
        SecretRef::Environment {
            var: format!("{}_API_KEY", provider.to_uppercase()),
        },
    )
}

/// A free candidate on `provider`, running where `locality` says.
fn free_candidate(
    provider: &str,
    model: &str,
    locality: glasshouse::provider::registry::Locality,
) -> DisposableCandidate {
    DisposableCandidate::new(provider, model, credential(provider), Cost::Free)
        .with_locality(locality)
}

/// **Capability map line 1607, classification half — library level.**
///
/// Two free candidates that are adequate in every measured way, differing
/// only in where they run. `choose_for_automatic_classification` — the
/// function `main.rs::automatic_classification_choice` calls — must pick the
/// local one.
///
/// This is not run through the binary: reaching automatic classification
/// needs a provider the registry knows as local (`ResourceKind::locality`
/// reads the `ollama` / `llama-cpp` slugs), and a fixture cannot invent a
/// registry entry. The production caller of this function is proven by
/// `tests/routing_economics.rs`; what is proven here is the preference it
/// applies.
#[test]
fn a_local_free_candidate_is_preferred_for_trivial_classification_and_extraction() {
    use glasshouse::provider::registry::Locality;

    let routing = DisposableRouting::for_support_work(true, FreePreferences::new())
        .with_classification_policy(ClassificationPolicy::new());
    // Remote first in the caller's order, so a policy that merely preserved
    // the order it was handed would pick the remote one.
    let candidates = [
        free_candidate("remote-runner", "a-remote-model", Locality::Remote),
        free_candidate("local-runner", "a-local-model", Locality::Local),
    ];
    let decision = routing
        .choose_for_automatic_classification(
            &candidates,
            &FreePool::new(),
            Instant::now(),
            1_800_000_000,
            None,
            None,
        )
        .expect("two healthy free candidates must yield a choice");
    let AutomaticClassificationDecision::Fresh(choice, _) = decision else {
        panic!("no pick was retained, so the decision must be a fresh one");
    };
    assert_eq!(
        choice.model(),
        "a-local-model",
        "a local candidate must be preferred over an equally adequate remote one: {}",
        choice.explanation().render()
    );
    assert!(
        choice.explanation().render().contains("local inference"),
        "the rationale must name locality as the reason: {}",
        choice.explanation().render()
    );
}

/// **Capability map line 1607, extraction half — on the shipped binary.**
///
/// A project with one free model and one metered model, and a completed turn.
/// The free one is chosen, and the rationale says why.
///
/// The metered candidate here is **not** in a reserve band: nothing is
/// planted, so the reserve gate never runs and cannot be what kept the free
/// one. The only thing left to explain the outcome is `choose`'s free-first
/// order.
///
/// **What this does not prove**: extraction has no *locality* preference.
/// `choose`'s free loop walks the user's own free-resource order and consults
/// no score, so `classification_preferences`' locality term reaches an
/// extraction rationale as text and never as a tiebreak. Line 1607 is a
/// disjunction — *local or free* — and free-first satisfies it; the local
/// half of the disjunction is classification's, above.
#[test]
fn free_capacity_is_preferred_for_extraction_on_the_shipped_binary() {
    let fixture = Fixture::new();
    fixture.configure(
        &[FREE_MODEL],
        &[METERED_MODEL],
        ReservePoliciesConfig::default(),
    );
    let session = fixture.running_session();
    fixture.completed_turn(&session);

    let rationale = fixture.one_rationale();
    assert!(
        rationale.contains(FREE_MODEL),
        "the free model must be the one chosen:\n{rationale}"
    );
    assert!(
        !rationale.contains(METERED_MODEL),
        "no metered candidate may appear as the chosen resource when a free one can serve:\n\
         {rationale}"
    );
    assert!(
        rationale.contains("line 530 prefers free capacity"),
        "the rationale must name the reason, not only the outcome:\n{rationale}"
    );
}

// ---------------------------------------------------------------------------
// 1611 — no premium capacity for Glasshouse's own bookkeeping when a cheap
// resource can do it reliably.
// ---------------------------------------------------------------------------

/// A classification record with `parsed` of `outcomes` successes, enough
/// observations to clear `CLASSIFICATION_RELIABILITY_MIN_OBSERVATIONS`.
fn record(provider: &str, model: &str, parsed: usize, outcomes: usize) -> ClassificationRecord {
    ClassificationRecord {
        provider: provider.to_owned(),
        model: model.to_owned(),
        parsed,
        outcomes_recorded: outcomes,
        timed: 0,
        median_duration_ms: None,
    }
}

/// **Capability map line 1611, on the shipped binary and at the reliability
/// floor.**
///
/// Two halves, because the line has two conditions and only one of them has a
/// producer for both jobs.
///
/// *"Avoid spending premium capacity"* — the metered gate — is proven on the
/// binary for extraction: a free adequate candidate exists and the metered
/// one is never reached. *"when a cheap resource can perform it reliably"* is
/// proven for **classification only**, where `ClassificationRecord` measures
/// reliability: a free candidate the ledger shows unreliable is excluded, and
/// the metered candidate is then used rather than the cheap-but-unreliable
/// one. There is no reliability record for extraction — `routing_observations`
/// rows carry no purpose for an extraction call — so for extraction the
/// verdict rests on the metered gate alone, and this test says so rather than
/// implying a measurement that does not exist.
#[test]
fn premium_capacity_is_not_spent_on_bookkeeping_when_a_cheap_reliable_resource_exists() {
    use glasshouse::provider::registry::Locality;

    // Half one — the metered gate, through the binary. A free model beside a
    // metered one, and the metered one is never spent.
    let fixture = Fixture::new();
    fixture.configure(
        &[FREE_MODEL],
        &[METERED_MODEL],
        ReservePoliciesConfig::default(),
    );
    let session = fixture.running_session();
    fixture.completed_turn(&session);
    let rationale = fixture.one_rationale();
    assert!(
        rationale.contains(FREE_MODEL) && !rationale.contains(METERED_MODEL),
        "Glasshouse's own bookkeeping must not reach a metered resource while a free one can \
         serve it:\n{rationale}"
    );

    // Half two — reliability, where it is measured. A free candidate the
    // ledger shows below the floor is excluded from classification, and the
    // metered candidate is used instead. The cheap resource is preferred
    // *when it can perform reliably*, which is the line's own condition, not
    // unconditionally.
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new())
        .with_classification_policy(ClassificationPolicy::new());
    let unreliable_free = free_candidate("cheap-runner", "an-unreliable-model", Locality::Remote)
        .with_classification_record(Some(record("cheap-runner", "an-unreliable-model", 1, 10)));
    let metered = DisposableCandidate::new(
        "paid-runner",
        METERED_MODEL,
        credential("paid-runner"),
        Cost::Metered,
    )
    .with_locality(Locality::Remote);
    let decision = routing
        .choose_for_automatic_classification(
            &[unreliable_free.clone(), metered.clone()],
            &FreePool::new(),
            Instant::now(),
            1_800_000_000,
            None,
            None,
        )
        .expect("the metered candidate can still serve");
    let AutomaticClassificationDecision::Fresh(choice, _) = decision else {
        panic!("no pick was retained, so the decision must be a fresh one");
    };
    assert_eq!(
        choice.model(),
        METERED_MODEL,
        "a cheap resource below the reliability floor is not one that can perform the job \
         reliably: {}",
        choice.explanation().render()
    );

    // And the control that makes the assertion above about *reliability*
    // rather than about the free candidate being unusable at all: the same
    // candidate, reliable, is chosen over the same metered one.
    let reliable_free = free_candidate("cheap-runner", "an-unreliable-model", Locality::Remote)
        .with_classification_record(Some(record("cheap-runner", "an-unreliable-model", 10, 10)));
    let decision = routing
        .choose_for_automatic_classification(
            &[reliable_free, metered],
            &FreePool::new(),
            Instant::now(),
            1_800_000_000,
            None,
            None,
        )
        .expect("a reliable free candidate must serve");
    let AutomaticClassificationDecision::Fresh(choice, _) = decision else {
        panic!("no pick was retained, so the decision must be a fresh one");
    };
    assert_eq!(
        choice.model(),
        "an-unreliable-model",
        "the same candidate, now reliable, must keep the job off the metered one: {}",
        choice.explanation().render()
    );
}

// ---------------------------------------------------------------------------
// 1609 — a difficult task keeps its warm premium session.
// ---------------------------------------------------------------------------

/// Two task descriptions the deterministic heuristic classes at opposite
/// ends, asserted here so a change to the heuristic fails this file rather
/// than silently making the test below compare two identical tiers.
const HEAVY_TASK: &str = "run cargo test and fix whatever fails";
const LEAF_TASK: &str = "what is a mutex";

#[test]
fn the_two_task_descriptions_this_file_relies_on_really_do_classify_differently() {
    use glasshouse::routing::classify::classify_heuristically;
    assert_eq!(
        classify_heuristically(HEAVY_TASK).conservative_workload_tier(),
        WorkloadTier::Heavy,
        "the heavy fixture text must classify heavy, or the routing test below proves nothing"
    );
    assert_eq!(
        classify_heuristically(LEAF_TASK).conservative_workload_tier(),
        WorkloadTier::Leaf
    );
}

/// **Capability map line 1609, at the gate that makes it difficulty-conditioned.**
///
/// `evaluate_reserve_spend` is what the session router's reserve arm and the
/// disposable router both consult, and it is the only term in this build
/// whose answer depends on how hard the work is. Given a premium resource in
/// the reserve band with a cheaper adequate alternative available, a heavy
/// task keeps it and a leaf task does not.
///
/// Paired with `session_affinity` — which is a positive term for an existing
/// session and absent for a fresh one — that is the whole of "prefer premium
/// warm sessions for difficult tasks" this build can compute.
///
/// **What it does not prove**: *"that benefit strongly from existing context"*
/// is not measured per task. `routing::session::session_affinity`'s own doc
/// records that Phase 36's same-task, touched-file and semantic-quality
/// signals have no producer here, so warmth is the whole affinity signal and
/// the preference applies to difficult tasks generally rather than to the
/// subset the line names.
#[test]
fn a_heavy_task_keeps_its_warm_premium_session() {
    let inputs = |tier: WorkloadTier| ReserveDecisionInputs {
        band: CapacityBand::Reserve,
        tier,
        cheaper_adequate_resource_exists: true,
        user_override: false,
        seconds_until_reset: Some(DISTANT_RESET_SECONDS),
        task_nearly_complete: false,
    };
    let heavy = evaluate_reserve_spend(inputs(WorkloadTier::Heavy));
    assert!(
        heavy.is_allowed(),
        "a difficult task must keep the premium resource it is on: {}",
        heavy.reason()
    );
    let leaf = evaluate_reserve_spend(inputs(WorkloadTier::Leaf));
    assert!(
        !leaf.is_allowed(),
        "the same premium resource must not be kept for trivial work, or the preference is not \
         conditioned on difficulty at all: {}",
        leaf.reason()
    );
}

/// **Capability map line 1609, on the shipped binary.**
///
/// `glasshouse route --task` classifies the text and hands the tier to the
/// session router, which is the production path a launch takes too
/// (`main.rs::classify_for_routing`, called from both). With a warm session
/// on a premium destination in the reserve band, the heavy task's ranking
/// keeps it and names the tier it acted on.
///
/// The assertion is on the **explanation**, not on the winner, and
/// deliberately: `route` ranks whatever destinations this machine's
/// configuration produces, and a fixture that pinned the winner would be
/// asserting about the fixture's destination set rather than about the term.
/// What must be true is that the classification reached the ranking and that
/// the reserve arm rendered its verdict for the tier — the two links line
/// 1609 needs and the two a mutation can break.
#[test]
fn the_route_command_carries_a_tasks_difficulty_into_the_ranking() {
    let fixture = Fixture::new();
    fixture.configure(
        &[FREE_MODEL],
        &[METERED_MODEL],
        ReservePoliciesConfig::default(),
    );
    fixture.plant_reserve_band();

    let heavy = fixture.route(HEAVY_TASK);
    assert!(
        heavy.contains("heavy"),
        "the ranking must say what the task was classed as:\n{heavy}"
    );
    assert!(
        heavy.contains("session affinity"),
        "warmth is the affinity term line 1609's `warm` half rests on, and it must be on every \
         explanation:\n{heavy}"
    );

    let leaf = fixture.route(LEAF_TASK);
    assert_ne!(
        heavy, leaf,
        "two tasks at opposite tiers must not produce the identical explanation, or nothing in \
         the ranking read the classification"
    );
}

impl Fixture {
    /// `glasshouse route --task <text>`, both streams, so the explanation is
    /// captured wherever the binary chose to print it.
    fn route(&self, task: &str) -> String {
        let output: Output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .env(CREDENTIAL_VAR, CREDENTIAL)
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("route")
            .arg("--task")
            .arg(task)
            .output()
            .expect("the glasshouse binary must be runnable");
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }
}

// ---------------------------------------------------------------------------
// 1608 — refused. The tripwire, not a closure.
// ---------------------------------------------------------------------------

/// **Capability map line 1608 is REFUSED — refusal register, Cluster Q.**
///
/// *"Prefer cheap resources for simple repository summarization when no
/// valuable warm session already exists."* There is no repository
/// summarization job in this build: [`JobKind`] is `Classification`,
/// `MemoryExtraction`, `Reranking` and `Evaluation`, and no production code
/// path summarises a repository. A test that passed by preferring a cheap
/// resource for a job that does not exist would be exactly the shape Cluster
/// Q refuses — *"a test that passes because the feature is absent is not
/// evidence of restraint."*
///
/// So this asserts the **absence** instead, as a tripwire: the day a
/// summarization job kind arrives, this test fails and 1608 becomes a real
/// question rather than a quietly-ticked box. It does not close the line.
#[test]
fn no_repository_summarization_job_exists_to_route_cheaply_yet() {
    let kinds = [
        JobKind::Classification,
        JobKind::MemoryExtraction,
        JobKind::Reranking,
        JobKind::Evaluation,
        JobKind::ContextReduction,
    ];
    for kind in kinds {
        assert!(
            !kind.as_str().contains("summar"),
            "a summarization job kind exists now: capability map line 1608 is no longer a \
             Cluster Q refusal and must be packaged"
        );
    }
    // Exhaustiveness: if a variant is added, this match stops compiling and
    // the list above stops being a claim about all of them.
    //
    // GH-FIREWALL-REDUCER added `JobKind::ContextReduction` (Phase 57B, map
    // line 1997) and is updating this arm as that addition's own tripwire —
    // this line does not by itself bear on 1608's summarization refusal,
    // which stays open.
    for kind in kinds {
        match kind {
            JobKind::Classification
            | JobKind::MemoryExtraction
            | JobKind::Reranking
            | JobKind::Evaluation
            | JobKind::ContextReduction => {}
        }
    }
}

// ---------------------------------------------------------------------------
// The scope selection itself, at the one function that makes it.
// ---------------------------------------------------------------------------

/// `ReservePolicies::for_scope` is the only place a scope maps to a field,
/// and `main.rs` calls it with `Background` for both support jobs. Held here
/// so that a call site changed to the other scope has a unit-level witness
/// beside the binary-level one above.
#[test]
fn the_background_scope_selects_the_background_field() {
    let policies = ReservePolicies {
        interactive: ReservePolicy::Spend,
        background: ReservePolicy::Protect,
    };
    assert_eq!(
        policies.for_scope(ReserveScope::Background),
        ReservePolicy::Protect
    );
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new())
        .with_reserve_policy(policies.for_scope(ReserveScope::Background));
    assert_eq!(routing.reserve_policy(), ReservePolicy::Protect);
}

/// Omitting the builder is `protect`, so every caller that predates line 1577
/// keeps the behaviour it had. Separate from the binary test because "the
/// default is protect" is a claim about the type, not about configuration
/// layering.
#[test]
fn a_router_built_without_a_reserve_policy_protects() {
    let routing = DisposableRouting::for_support_work(true, FreePreferences::new());
    assert_eq!(routing.reserve_policy(), ReservePolicy::Protect);
}

/// The gate itself, at library level: the same reserve-band candidate, the
/// same leaf-tier job, and only the policy differs. This is the assertion the
/// `background policy ignored` mutation dies on without spawning a process.
#[test]
fn the_background_policy_removes_the_denial_at_the_disposable_gate() {
    let candidate = DisposableCandidate::new(
        "paid-runner",
        METERED_MODEL,
        credential("paid-runner"),
        Cost::Metered,
    )
    .with_capacity(
        CandidateCapacity::new()
            .with_band(Some(CapacityBand::Reserve))
            .with_seconds_until_reset(Some(DISTANT_RESET_SECONDS)),
    );

    let protecting = DisposableRouting::for_support_work(true, FreePreferences::new())
        .with_reserve_policy(ReservePolicy::Protect);
    assert!(
        protecting
            .choose(
                JobKind::MemoryExtraction,
                std::slice::from_ref(&candidate),
                &FreePool::new(),
                Instant::now(),
                None,
            )
            .is_err(),
        "`protect` must leave the reserve denial standing"
    );

    let spending = DisposableRouting::for_support_work(true, FreePreferences::new())
        .with_reserve_policy(ReservePolicy::Spend);
    let choice = spending
        .choose(
            JobKind::MemoryExtraction,
            std::slice::from_ref(&candidate),
            &FreePool::new(),
            Instant::now(),
            None,
        )
        .expect("`spend` must remove the denial");
    let explanation = choice.explanation().render();
    assert!(
        explanation.contains("background reserve policy"),
        "{explanation}"
    );
    assert!(
        explanation.contains("admitted this candidate in the reserve band anyway"),
        "{explanation}"
    );
    assert!(
        explanation.contains("map line 1550"),
        "the pressure the policy did not remove must still be rendered: {explanation}"
    );
}

// ---------------------------------------------------------------------------
// 1609 on the shipped binary, with a real warm premium destination.
//
// The fixture above writes its configuration through `UserConfig`, which has
// no setter for a launch profile. This one writes `config.toml` directly, the
// same way `tests/subscription_pressure.rs::Binary` does and for the same
// reason: a *profile* bound to a direct provider is what makes a destination
// premium or cheap, and that is the whole subject here.
// ---------------------------------------------------------------------------

/// A premium destination in the reserve band and a zero-cost one beside it.
///
/// Both profiles name the same harness, so nothing about harness capability
/// separates them; the only differences are the provider each is bound to
/// and, once a session has been started under one of them, which one is warm.
const TWO_PROFILES: &str = "\n\
     [providers.premium-probe]\ntemplate = \"openrouter\"\n\
     credential_env = [\"GLASSHOUSE_TEST_ONLY_SUPPORT_ECONOMY_KEY\"]\n\n\
     [providers.cheap-probe]\ntemplate = \"openrouter\"\n\
     credential_env = [\"GLASSHOUSE_TEST_ONLY_SUPPORT_ECONOMY_KEY\"]\n\
     free_models = [\"the-cheap-model\"]\n\n\
     [profiles.premium]\nharness = \"claude-code\"\n\
     expected_protocol = \"anthropic-messages\"\nmodel = \"the-premium-model\"\n\n\
     [profiles.premium.backend]\nkind = \"direct-provider\"\n\
     provider = \"premium-probe\"\n\n\
     [profiles.cheap]\nharness = \"claude-code\"\n\
     expected_protocol = \"anthropic-messages\"\nmodel = \"the-cheap-model\"\n\n\
     [profiles.cheap.backend]\nkind = \"direct-provider\"\n\
     provider = \"cheap-probe\"\n";

struct RouteBinary {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl RouteBinary {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");
        let harness = install_fake_harness(&base.join("bin"));
        let escaped = harness.display().to_string().replace('\\', "\\\\");
        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\
                 {TWO_PROFILES}"
            ),
        )
        .expect("write user config");
        Self {
            _tmp: tmp,
            base,
            root,
        }
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
            .env(CREDENTIAL_VAR, CREDENTIAL)
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

    /// Start one session under `profile`, so that profile has a warm
    /// destination the router can keep.
    fn warm_session_under(&self, profile: &str) {
        let out = self.glasshouse(&["launch", "claude-code", "--headless", "--profile", profile]);
        assert!(
            out.status.success(),
            "launching under `{profile}` must succeed:\n{}",
            Self::both_streams(&out)
        );
    }

    fn plant_reserve_band(&self, provider: &str) {
        let cache = glasshouse::provider::telemetry::GatewayQuotaCache::at(
            self.base.join("data").join("gateway-quota"),
        );
        let limit = RESERVE_BAND_LIMIT.to_string();
        let remaining = RESERVE_BAND_REMAINING.to_string();
        let reset = DISTANT_RESET_SECONDS.to_string();
        cache.store(
            provider,
            &glasshouse::provider::telemetry::RateLimitHeaders::read(vec![
                ("ratelimit-limit", limit.as_str()),
                ("ratelimit-remaining", remaining.as_str()),
                ("ratelimit-reset", reset.as_str()),
            ]),
            glasshouse::provider::cache::now_unix_seconds(),
        );
        assert!(cache.load(provider).is_some());
    }

    fn route(&self, task: &str) -> String {
        Self::both_streams(&self.glasshouse(&["route", "--task", task]))
    }
}

/// The destination `glasshouse route` said the work should go to — the first
/// line's identifier, which is the decision the report is about.
fn chosen_destination(report: &str) -> &str {
    let line = report
        .lines()
        .find(|line| line.starts_with("destination"))
        .unwrap_or_else(|| panic!("no ranking was printed:\n{report}"));
    line.split_whitespace()
        .nth(1)
        .unwrap_or_else(|| panic!("the destination line names nothing:\n{report}"))
}

#[test]
#[cfg_attr(
    windows,
    ignore = "the fake harness launch path is proven on unix here"
)]
fn a_heavy_task_keeps_its_warm_premium_session_on_the_shipped_binary() {
    let binary = RouteBinary::new();
    binary.warm_session_under("premium");
    binary.plant_reserve_band("premium-probe");

    // The difficult task. The warm premium session is in the reserve band
    // and a zero-cost fresh destination is sitting beside it, and the
    // ranking keeps the warm premium one anyway.
    let heavy = binary.route(HEAVY_TASK);
    let kept = chosen_destination(&heavy).to_owned();
    assert!(
        !kept.starts_with("fresh:"),
        "a difficult task must keep the warm premium session rather than start somewhere \
         cheaper and cold, got `{kept}`:\n{heavy}"
    );
    assert!(
        heavy.contains("tier heavy"),
        "the ranking must have acted on the task's difficulty:\n{heavy}"
    );
    assert!(
        heavy.contains("justifies spending protected reserve"),
        "the term that makes the preference conditional on difficulty must be the one that \
         kept it:\n{heavy}"
    );
    assert!(
        heavy.contains("fresh:claude-code:cheap"),
        "the cheaper alternative must have been ranked and lost, not been absent:\n{heavy}"
    );

    // The attribution. Same project, same planted band, same destinations —
    // only the task text differs, and the work moves off the premium session
    // to the cheap fresh one. Warmth alone cannot explain either outcome,
    // because warmth is identical in both runs.
    let leaf = binary.route(LEAF_TASK);
    assert_eq!(
        chosen_destination(&leaf),
        "fresh:claude-code:cheap",
        "trivial work must not keep premium capacity it does not need:\n{leaf}"
    );
    assert!(
        leaf.contains("which denies the spend"),
        "the same gate must deny at the lower tier, or nothing conditioned the preference on \
         difficulty:\n{leaf}"
    );
    assert!(
        leaf.contains(&kept) && leaf.contains("session affinity"),
        "the warm session must still be a ranked candidate at the lower tier — it lost the \
         ranking, it was not removed from it:\n{leaf}"
    );
}
