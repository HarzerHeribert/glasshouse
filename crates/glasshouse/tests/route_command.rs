//! Phase 37 lines 1592–1602 — the session router's **production callers**.
//!
//! # Why this file exists at all, and why none of it is a unit test
//!
//! `tests/session_router.rs` already proves the ranking: eleven mutations,
//! eleven killed, every one of the six `Consider X` contributions shown
//! separating two destinations that differ in that axis alone. Not one of
//! them can fail on a build where **nothing calls the router**, and that is
//! exactly this project's most common defect (practice §35: *a caller you can
//! delete without a test noticing is, to the test suite, not a caller*).
//!
//! So every test here runs the shipped binary, and the load-bearing one is
//! `a_second_launch_continues_the_warm_session_rather_than_starting_another`:
//! delete `SessionRouter::choose` from `main.rs::launch_session` and it fails,
//! because a second session record appears where there should be one.
//!
//! # How a resume is observed, and why not from the session record
//!
//! The fake harness appends its own argv to a file on every invocation,
//! unconditionally. A fresh launch is `--session-id <uuid>`; a resume is
//! `--resume <uuid>` — the two adapter invocations Claude Code's own adapter
//! declares. Recording argv is independent of everything under test, which is
//! practice §80 case 5's requirement: a mutation must fail the assertion the
//! test is named for, not the fixture's ability to identify anything.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The fixture's provider credential variable. A name only — nothing here
/// resolves a value, and the router is handed the *name* for its explanation.
const CREDENTIAL_VAR: &str = "GLASSHOUSE_ROUTE_TEST_KEY";

/// A project with a fake `claude-code`, a direct-provider profile, and a log
/// of every argv the harness was ever started with.
struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    argv_log: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let argv_log = base.join("argv.log");
        let harness = install_fake_harness(&bin_dir, &argv_log);
        let escaped = harness.display().to_string().replace('\\', "\\\\");

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
                 [providers.route-probe]\ntemplate = \"openrouter\"\n\
                 credential_env = [\"{CREDENTIAL_VAR}\"]\n\n\
                 [profiles.direct]\nharness = \"claude-code\"\n\
                 expected_protocol = \"openai-chat\"\n\n\
                 [profiles.direct.backend]\nkind = \"direct-provider\"\n\
                 provider = \"route-probe\"\n\n\
                 [profiles.metered]\nharness = \"claude-code\"\n\
                 expected_protocol = \"anthropic-messages\"\n\n\
                 [profiles.metered.backend]\nkind = \"direct-provider\"\n\
                 provider = \"route-probe\"\n"
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

    fn glasshouse(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .env(CREDENTIAL_VAR, "planted-opaque-route-value-37")
            .env("PATH", self.base.join("empty-path"))
            .output()
            .expect("the glasshouse binary must be runnable")
    }

    fn stdout(&self, args: &[&str]) -> String {
        String::from_utf8_lossy(&self.glasshouse(args).stdout).into_owned()
    }

    fn both_streams(output: &Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    /// Every argv the harness has been started with, oldest first.
    fn harness_invocations(&self) -> Vec<String> {
        match std::fs::read_to_string(&self.argv_log) {
            Ok(log) => log.lines().map(str::to_owned).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// The identifiers `glasshouse sessions` lists, one per recorded session.
    fn recorded_sessions(&self) -> Vec<String> {
        let listing = self.stdout(&["sessions"]);
        if listing.contains("No sessions recorded") {
            return Vec::new();
        }
        listing
            .lines()
            .skip(1)
            .filter_map(|line| line.split_whitespace().next())
            .filter(|id| id.len() >= 8)
            .map(str::to_owned)
            .collect()
    }
}

#[cfg(unix)]
fn install_fake_harness(bin_dir: &Path, argv_log: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join("fake-claude-code");
    // Exit 0, deliberately: a non-zero exit makes the session `Failed`, and a
    // failed session is not a warm one — there would be nothing to route back
    // into, and every test below would pass for the wrong reason.
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
            argv_log.display()
        ),
    )
    .expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_fake_harness(bin_dir: &Path, argv_log: &Path) -> PathBuf {
    let path = bin_dir.join("fake-claude-code.cmd");
    std::fs::write(
        &path,
        format!(
            "@echo off\r\necho %*>>\"{}\"\r\nexit /b 0\r\n",
            argv_log.display()
        ),
    )
    .expect("write fake harness");
    path
}

// --- line 1601: the explanation, and the fact that it decides nothing -------

/// Line 1601. Every contribution the router weighed, the alternatives it did
/// not choose, and — the half a scoring test cannot assert — that asking the
/// question started nothing.
#[test]
fn route_explains_the_ranking_and_starts_nothing() {
    let fixture = Fixture::new();

    let report = fixture.stdout(&["route"]);
    for term in [
        "harness capability fit",
        "session affinity",
        "prompt-cache state",
        "known quota pressure",
        "provider health",
        "switching and bootstrap cost",
    ] {
        assert!(
            report.contains(term),
            "line 1601 asks for an inspectable explanation, and `{term}` is one of the six \
             things it weighed:\n{report}"
        );
    }
    assert!(
        report.contains("alternatives"),
        "\"why this one\" is unanswerable without \"and what the others scored\":\n{report}"
    );

    // It decided nothing and started nothing — the whole claim of a
    // diagnostic, and the one no amount of rendering proves.
    assert!(
        fixture.recorded_sessions().is_empty(),
        "`glasshouse route` must record no session"
    );
    assert!(
        fixture.harness_invocations().is_empty(),
        "`glasshouse route` must start no harness, and it started {:?}",
        fixture.harness_invocations()
    );
}

/// The `REQUIRED BEHAVIOR` clause a report is easiest to leave out: a project
/// with nothing to go on still answers, and **says** what it had nothing to go
/// on rather than presenting silence as agreement.
#[test]
fn a_project_with_no_sessions_and_no_telemetry_says_what_it_had_nothing_to_go_on() {
    let fixture = Fixture::new();
    let report = fixture.stdout(&["route"]);

    assert!(
        report.contains("what this ranking could not see"),
        "an explanation whose silent terms are invisible cannot be told from one that \
         weighed them and found them equal:\n{report}"
    );
    assert!(
        report.contains("the health pool is filled by a running gateway"),
        "provider health is 0.0 here because nothing filled the pool, and that is a fact \
         about this command rather than about the providers:\n{report}"
    );
    assert!(
        report.contains("no quota reading has been cached"),
        "an unread quota must not read as a quota of zero:\n{report}"
    );
    assert!(
        report.contains("recorded no session that is still warm"),
        "with no warm session, session affinity separates nothing and the report should say \
         so:\n{report}"
    );
}

// --- §4.1: the input that is easy to drop ----------------------------------

/// **The trap `report-gh-router.md` §4.1 names, as a test.**
///
/// `route-probe` is an `openrouter` template, so it serves three protocols;
/// the `direct` profile routes over `openai-chat`, which Claude Code does not
/// speak. `ProtocolFit::Compatible` — *"not this protocol, but the provider
/// serves another one the harness does speak"* — is reachable only because
/// the caller passes `Destination::with_provider_protocols` every protocol the
/// provider declares a base URL for.
///
/// Drop that one builder call and the destination's protocol list collapses to
/// the backend's own single entry, `protocol_fit` answers `Incompatible`, and
/// a hard constraint removes the destination outright. So this test fails in a
/// very specific way — the profile moves from the ranking into `rejected` —
/// which is what makes it evidence about item 3 and not about scoring.
#[test]
fn a_direct_provider_destination_is_scored_rather_than_rejected_outright() {
    let fixture = Fixture::new();
    let report = fixture.stdout(&["route"]);

    let (ranked, rejected) = match report.split_once("\nrejected\n") {
        Some((ranked, rejected)) => (ranked, rejected),
        None => (report.as_str(), ""),
    };
    assert!(
        !rejected.contains("fresh:claude-code:direct"),
        "a provider serving a protocol the harness speaks must be scored, not removed by a \
         hard constraint — see §4.1, `with_provider_protocols` is what makes \
         `ProtocolFit::Compatible` reachable:\n{report}"
    );
    assert!(
        ranked.contains("fresh:claude-code:direct"),
        "the direct-provider profile must appear in the ranking:\n{report}"
    );
}

// --- line 1602: the override, on the path that reports and the one that acts

/// Line 1602 on the diagnostic. An override that wins says what it displaced —
/// a router whose whole product is an explanation must not quietly agree with
/// whoever asked last.
#[test]
fn an_override_wins_and_the_report_says_what_the_ranking_would_have_chosen() {
    let fixture = Fixture::new();

    let report = fixture.stdout(&["route", "--to", "fresh:claude-code:direct"]);
    assert!(
        report.starts_with("destination  fresh:claude-code:direct"),
        "`--to` must decide the destination:\n{report}"
    );
    assert!(
        report.contains("the ranking would have chosen"),
        "an override that silently replaced the automatic answer would leave a reader unable \
         to tell that it had:\n{report}"
    );

    // And an override naming nothing is refused out loud rather than swallowed.
    let refused = fixture.stdout(&["route", "--to", "no-such-destination"]);
    assert!(
        refused.contains("not one of the destinations offered"),
        "a user who asked for a destination and silently got another one has been lied \
         to:\n{refused}"
    );
}

// --- line 1592: the boundary gate ------------------------------------------

/// Line 1592: routing is taken at task or session boundaries, and **not**
/// between turns — with the one thing that lifts it, which is a person asking.
#[test]
fn routing_is_not_taken_mid_turn_unless_the_user_asks_for_it() {
    let fixture = Fixture::new();
    // A warm session, so there is something for the gate to hold the work on.
    fixture.glasshouse(&["launch", "claude-code", "--headless"]);

    let held = fixture.stdout(&["route", "--moment", "mid-turn"]);
    assert!(
        held.contains("routing is not taken here"),
        "line 1592 forbids re-deciding between turns:\n{held}"
    );
    assert!(
        held.contains("routing boundary"),
        "the explanation must name the term that held the work, not merely omit the \
         others:\n{held}"
    );

    let decided = fixture.stdout(&["route", "--moment", "mid-turn", "--now"]);
    assert!(
        decided.contains("routing was taken here"),
        "a person asking for a decision mid-turn is the opposite of the blind switching line \
         1592 forbids, and `--now` is how they ask:\n{decided}"
    );

    // The other half of line 1592's "task **or** session boundaries": a task
    // boundary re-decides without anyone having to ask.
    let boundary = fixture.stdout(&["route", "--moment", "task-boundary"]);
    assert!(
        boundary.contains("task boundary — routing was taken here"),
        "a task boundary is one of the two moments the line names:\n{boundary}"
    );
}

// --- line 1597: the term that needs a `from` -------------------------------

/// Line 1597, and the reason it is a *task boundary* test rather than a
/// session-start one.
///
/// `prompt_cache_state` is defined as `CacheLocality::between(from, to)`. At a
/// session start there is no `from` — the router's own report §4 records that
/// correction — so the term is honestly inert there and says so. At a task
/// boundary the work is already somewhere, and the caller has to supply that
/// somewhere or the term stays inert for a second reason nobody chose.
///
/// So: a warm session exists, and the report must show the cache term
/// *comparing* rather than reporting an absence. Passing `None` for `current`
/// at this moment puts the session-start wording back and fails this.
#[test]
fn at_a_task_boundary_the_cache_term_compares_against_where_the_work_is() {
    let fixture = Fixture::new();
    fixture.glasshouse(&["launch", "claude-code", "--headless"]);

    let boundary = fixture.stdout(&["route", "--moment", "task-boundary"]);
    // The winner's own block only. Alternatives include fresh destinations,
    // and "a fresh session has no cached prefix" is the right thing to say
    // about those — the claim here is about the chosen one.
    let winner = boundary
        .split("\nalternatives\n")
        .next()
        .expect("split always yields at least one part");
    assert!(
        winner.contains("prompt-cache state"),
        "line 1597's term must appear at all:\n{boundary}"
    );
    assert!(
        !winner.contains("a fresh session starts with no cached prefix anywhere"),
        "that is the session-start wording, and it is what the term degrades to when the \
         caller supplies no `from` to compare against:\n{boundary}"
    );
    assert!(
        winner.contains("provider-side prompt caching is unaffected"),
        "at a task boundary the term must be a comparison against where the work is, which \
         is what supplying `current` buys:\n{boundary}"
    );
}

// --- line 1600: the bootstrap half -----------------------------------------

/// Line 1600's bootstrap half. A project with a checkpoint prices a fresh
/// session differently from one with nothing to boot from, and the caller is
/// what reads the checkpoint — the router is forbidden to look one up.
#[test]
fn a_checkpoint_changes_what_a_fresh_session_costs_to_bootstrap() {
    let fixture = Fixture::new();

    let before = fixture.stdout(&["route"]);
    assert!(
        before.contains("with no checkpoint to boot from"),
        "with no checkpoint, a fresh session starts from nothing:\n{before}"
    );

    fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    let saved = fixture.glasshouse(&[
        "checkpoint",
        "save",
        "--objective",
        "wire the session router to a production caller",
        "--state",
        "the diagnostic exists and the launch path routes",
        "--next",
        "run the mutation on the call rather than the callee",
    ]);
    assert!(
        saved.status.success(),
        "the checkpoint must save:\n{}",
        Fixture::both_streams(&saved)
    );

    let after = fixture.stdout(&["route"]);
    assert!(
        !after.contains("with no checkpoint to boot from"),
        "a checkpoint with next actions is exactly what line 1600 prices a bootstrap by, and \
         `latest_checkpoint_quality` is the caller that reads it:\n{after}"
    );
}

// --- 6b: the launch path, and the mutation this whole file is for ----------

/// **The one that fails when `launch_session` stops calling `choose`.**
///
/// Line 1593: *prefer an existing relevant session when its affinity outweighs
/// the benefit of starting a new session.* The first launch leaves a warm,
/// resumable session behind. The second launch must land in it — one session
/// record, and a harness started with `--resume`, not `--session-id`.
///
/// On a build with the routing call deleted, `launch_session` does what it did
/// before this batch: mints a second identifier and records a second session.
/// Both assertions below fail, and they fail on their own terms rather than
/// through a fixture that could no longer identify anything (§80 case 5).
#[test]
fn a_second_launch_continues_the_warm_session_rather_than_starting_another() {
    let fixture = Fixture::new();

    let first = fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    assert!(
        first.status.success(),
        "the first launch must succeed:\n{}",
        Fixture::both_streams(&first)
    );
    let after_first = fixture.recorded_sessions();
    assert_eq!(
        after_first.len(),
        1,
        "the first launch records exactly one session"
    );

    let second = fixture.glasshouse(&["run", "claude-code", "--headless"]);
    let said = Fixture::both_streams(&second);

    // The behavioural claim first, deliberately. A mutation that removed the
    // routing call would also remove the announcement below, and a KILLED
    // credited to a missing *message* would say nothing about where the work
    // went — practice §80's rule that a verdict must be read for which
    // assertion produced it.
    assert_eq!(
        fixture.recorded_sessions(),
        after_first,
        "the second launch must continue the warm session, not record a second one — this is \
         the assertion that fails when `launch_session` stops calling \
         `SessionRouter::choose`:\n{said}"
    );

    let invocations = fixture.harness_invocations();
    assert_eq!(
        invocations.len(),
        2,
        "the harness must have been started twice:\n{invocations:?}"
    );
    assert!(
        invocations[0].contains("--session-id"),
        "the first launch starts a new conversation:\n{invocations:?}"
    );
    assert!(
        invocations[1].contains("--resume"),
        "the second launch must reopen the first session's own conversation, which is what \
         routing into an existing destination *means*:\n{invocations:?}"
    );

    // And it said so on the way in, while `--fresh` was still an answer.
    assert!(
        said.contains("continuing session"),
        "a launch that continues an existing session must not do it silently:\n{said}"
    );
}

/// Line 1602 on the path that acts rather than the one that reports. The same
/// flag, meaning the same thing, on the command that starts something.
#[test]
fn fresh_overrides_the_ranking_on_the_launch_path() {
    let fixture = Fixture::new();
    fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    assert_eq!(fixture.recorded_sessions().len(), 1);

    let second = fixture.glasshouse(&["launch", "claude-code", "--headless", "--fresh"]);
    assert!(
        second.status.success(),
        "`--fresh` must start a session:\n{}",
        Fixture::both_streams(&second)
    );
    assert_eq!(
        fixture.recorded_sessions().len(),
        2,
        "`--fresh` must start a new session however good the warm one looked"
    );

    let invocations = fixture.harness_invocations();
    assert!(
        invocations.iter().all(|argv| !argv.contains("--resume")),
        "nothing was resumed under `--fresh`:\n{invocations:?}"
    );
}

/// An explicitly named `--profile` is a statement about a **new** session, and
/// a router that answered it by reopening an old one would be overruling the
/// person who typed it.
#[test]
fn naming_a_profile_explicitly_starts_a_fresh_session_under_it() {
    let fixture = Fixture::new();
    fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    assert_eq!(fixture.recorded_sessions().len(), 1);

    let second = fixture.glasshouse(&[
        "launch",
        "claude-code",
        "--headless",
        "--profile",
        "metered",
    ]);
    assert!(
        second.status.success(),
        "a named profile must start:\n{}",
        Fixture::both_streams(&second)
    );
    assert_eq!(
        fixture.recorded_sessions().len(),
        2,
        "`--profile metered` names the profile a new session runs under"
    );

    let listing = fixture.stdout(&["sessions"]);
    assert!(
        listing.contains("metered"),
        "and the new session must actually be running that profile:\n{listing}"
    );
}

/// `--to` on the launch path takes the same identifiers `glasshouse route`
/// prints, which is what makes the diagnostic worth reading: an answer can be
/// pasted into the command that acts.
#[test]
fn to_on_the_launch_path_takes_the_identifier_route_printed() {
    let fixture = Fixture::new();

    let report = fixture.stdout(&["route"]);
    assert!(
        report.contains("fresh:claude-code:metered"),
        "the report must name the destination this test is about to ask for:\n{report}"
    );

    let launched = fixture.glasshouse(&[
        "launch",
        "claude-code",
        "--headless",
        "--to",
        "fresh:claude-code:metered",
    ]);
    assert!(
        launched.status.success(),
        "an identifier `route` printed must be usable:\n{}",
        Fixture::both_streams(&launched)
    );

    let listing = fixture.stdout(&["sessions"]);
    assert!(
        listing.contains("metered"),
        "`--to` must decide which profile the session actually ran under:\n{listing}"
    );
}

// --- GH-ROUTER-TASK-INPUT: `--task`, and the capability registry it wires --
//
// Phase 34's registry (`src/routing/capability.rs`) was built, tested and
// integrated with all ten of its boxes deliberately held open —
// `docs/product/evidence/phase-34.md` — because no production entry point
// ever had request text to classify, so `TaskRequirements::hard_capabilities`
// was always empty and `capability_fit` always took its first-statement
// early return. `route_report`'s new `task` parameter is that one missing
// argument; the tests below are the executable proof it is load-bearing, not
// only present.

/// Acceptance test 1: the no-`--task` contract. `route` with no `--task`
/// must render `capability_fit`'s own early-return evidence string —
/// `TaskRequirements::default()`'s `hard_capabilities` is empty, byte for
/// byte the same as before this packet, because `src/routing/session.rs` was
/// not touched to produce it.
#[test]
fn omitting_task_leaves_the_capability_term_at_its_pre_packet_default() {
    let fixture = Fixture::new();
    let report = fixture.stdout(&["route"]);
    assert!(
        report.contains(
            "+0.000  capability fit — the task named no hard capability requirement, so this \
             resource's capability description contributes nothing"
        ),
        "with no `--task`, the capability term must read exactly the empty-`hard_capabilities` \
         evidence string `capability_fit` has always produced for `TaskRequirements::default()`:\
             \n{report}"
    );
}

/// Acceptance test 2. A `--task` describing browser work names browser use
/// in the capability contribution; a `--task` describing plain text work
/// does not — the same rendered term, driven by what the text says rather
/// than by whether `--task` was passed at all (that half is test 1's).
#[test]
fn a_browser_task_names_browser_interaction_and_a_plain_one_does_not() {
    let fixture = Fixture::new();

    let browser = fixture.stdout(&[
        "route",
        "--task",
        "open the browser, navigate to the page and take a screenshot",
    ]);
    assert!(
        browser.contains("needs browser interaction"),
        "a task this heuristic reads as browser work must show up in the capability \
         contribution's evidence:\n{browser}"
    );

    let plain = fixture.stdout(&[
        "route",
        "--task",
        "what is the difference between a fresh and an existing session",
    ]);
    assert!(
        !plain.contains("needs browser interaction"),
        "a task with no browser signal must not claim one:\n{plain}"
    );
    assert!(
        plain.contains("the task named no hard capability requirement"),
        "a pure question with no repository reference classifies to no hard capability at \
         all (`classify_heuristically`'s own fail-open case for a question), so the term must \
         read exactly like the no-`--task` case:\n{plain}"
    );
}

/// Acceptance test 5. Empty or whitespace-only `--task` text must behave as
/// if `--task` were absent — never classified as some default class — so
/// this asserts byte-for-byte identity with the no-`--task` report, not
/// merely "no capability was named".
#[test]
fn empty_or_whitespace_only_task_text_behaves_as_absent() {
    let fixture = Fixture::new();
    let absent = fixture.stdout(&["route"]);
    let empty = fixture.stdout(&["route", "--task", ""]);
    let whitespace = fixture.stdout(&["route", "--task", "   \t  "]);
    assert_eq!(
        absent, empty,
        "an empty `--task` must reproduce the no-`--task` report exactly"
    );
    assert_eq!(
        absent, whitespace,
        "a whitespace-only `--task` must reproduce the no-`--task` report exactly"
    );
}

/// A project with two harnesses behind the same provider, differing only in
/// what map line 1382's registry establishes about `browser-use`:
/// `claude-code` declares it present (`claude --help`'s `--chrome`); `codex`
/// declares nothing either way. `direct-codex`'s `expected_protocol` is
/// `openai-chat`, which Codex does not speak
/// (`harness::codex::PROTOCOLS = &[WireProtocol::OpenAiResponses]`), so it
/// scores `ProtocolFit::Compatible` (`+0.4`) where `direct-cc` — no
/// `expected_protocol`, so its route has none to compare against — scores
/// `ProtocolFit::Unknown` (`+0.0`). Every other term ties: both fresh, no
/// checkpoint, no cached quota, no health pool, `moment == SessionStart` so
/// there is no `current` to price a switch against.
///
/// That `+0.4` gap is exactly `CAPABILITY_ESTABLISHED_PRESENT`
/// (`session.rs`), chosen so a task naming only browser interaction —
/// phrased as a question so `classify_heuristically` does not also add
/// repository access, which both harnesses declare present and which would
/// cancel out — closes it: `direct-cc` gains `+0.4` on the axis `direct-codex`
/// cannot, exactly erasing the protocol-fit gap.
struct TwoHarnessFixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl TwoHarnessFixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let argv_log = base.join("argv.log");
        let claude_code = install_named_fake_harness(&bin_dir, "fake-claude-code", &argv_log);
        let codex = install_named_fake_harness(&bin_dir, "fake-codex", &argv_log);
        let escape = |p: &Path| p.display().to_string().replace('\\', "\\\\");

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n\n\
                 [integrations.codex]\nenabled = true\nexecutable = \"{}\"\n\n\
                 [providers.route-probe]\ntemplate = \"openrouter\"\n\
                 credential_env = [\"{CREDENTIAL_VAR}\"]\n\n\
                 [profiles.direct-cc]\nharness = \"claude-code\"\n\n\
                 [profiles.direct-cc.backend]\nkind = \"direct-provider\"\n\
                 provider = \"route-probe\"\n\n\
                 [profiles.direct-codex]\nharness = \"codex\"\n\
                 expected_protocol = \"openai-chat\"\n\n\
                 [profiles.direct-codex.backend]\nkind = \"direct-provider\"\n\
                 provider = \"route-probe\"\n",
                escape(&claude_code),
                escape(&codex),
            ),
        )
        .expect("write user config");

        Self {
            _tmp: tmp,
            base,
            root,
        }
    }

    fn stdout(&self, args: &[&str]) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .env(CREDENTIAL_VAR, "planted-opaque-route-value-37")
            .env("PATH", self.base.join("empty-path"))
            .output()
            .expect("the glasshouse binary must be runnable");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

#[cfg(unix)]
fn install_named_fake_harness(bin_dir: &Path, name: &str, argv_log: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join(name);
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
            argv_log.display()
        ),
    )
    .expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_named_fake_harness(bin_dir: &Path, name: &str, argv_log: &Path) -> PathBuf {
    let path = bin_dir.join(format!("{name}.cmd"));
    std::fs::write(
        &path,
        format!(
            "@echo off\r\necho %*>>\"{}\"\r\nexit /b 0\r\n",
            argv_log.display()
        ),
    )
    .expect("write fake harness");
    path
}

// --- GH-PROFILE-SELECTION: box 372 — does the router select among ----------
// --- *enabled* launch profiles when *automatic routing* is enabled? --------
//
// The refusal register's Cluster D marked 372 open on a stale premise (`grep
// 'fn score\|Score'` is not empty — `SessionRouter` genuinely ranks multiple
// profiles). The two clauses below are what actually decide it, proven
// separately per practice §36's rule: a caller reaching the ranking code is
// not the same as a caller that *acts* on it.

/// A project with `claude-code`, a `route-probe` provider (`openrouter`
/// template, so it serves `anthropic-messages` among others), one profile
/// that out-ranks the implied `native` profile on protocol fit (`better`
/// declares `anthropic-messages`, which claude-code speaks natively — `+1.000`
/// against `native`'s undeclared `+0.000`), and one profile that is
/// otherwise identical to `better` but for `enabled = false`
/// (`disabled-one`).
///
/// [`ProfileSelectionFixture::with_all_profiles_enabled`] writes the **same**
/// configuration with that one `enabled = false` line removed and nothing
/// else changed, which is what makes the no-regression test a controlled
/// comparison rather than two unrelated fixtures that happen to agree.
struct ProfileSelectionFixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl ProfileSelectionFixture {
    fn new() -> Self {
        Self::build("enabled = false\n")
    }

    /// The same fixture with no `enabled` key anywhere — the behaviour every
    /// configuration written before the field was read has to keep.
    fn with_all_profiles_enabled() -> Self {
        Self::build("")
    }

    /// Write a project-level configuration into the fixture's own project
    /// root, at the path `EffectiveConfig`'s project layer reads.
    fn write_project_config(&self, toml: &str) {
        let dir = self.root.join(".glasshouse");
        std::fs::create_dir_all(&dir).expect("create project config dir");
        std::fs::write(dir.join("config.toml"), toml).expect("write project config");
    }

    fn build(disabled_line: &str) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let argv_log = base.join("argv.log");
        let harness = install_fake_harness(&bin_dir, &argv_log);
        let escaped = harness.display().to_string().replace('\\', "\\\\");

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
                 [providers.route-probe]\ntemplate = \"openrouter\"\n\
                 credential_env = [\"{CREDENTIAL_VAR}\"]\n\n\
                 [profiles.better]\nharness = \"claude-code\"\n\
                 expected_protocol = \"anthropic-messages\"\n\n\
                 [profiles.better.backend]\nkind = \"direct-provider\"\n\
                 provider = \"route-probe\"\n\n\
                 [profiles.disabled-one]\nharness = \"claude-code\"\n{disabled_line}\
                 expected_protocol = \"anthropic-messages\"\n\n\
                 [profiles.disabled-one.backend]\nkind = \"direct-provider\"\n\
                 provider = \"route-probe\"\n"
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
            .env(CREDENTIAL_VAR, "planted-opaque-route-value-37")
            .env("PATH", self.base.join("empty-path"))
            .output()
            .expect("the glasshouse binary must be runnable")
    }

    fn stdout(&self, args: &[&str]) -> String {
        String::from_utf8_lossy(&self.glasshouse(args).stdout).into_owned()
    }
}

/// Acceptance test 1: two enabled profiles differing on protocol fit rank
/// differently, and the higher-scoring one is what the diagnostic
/// recommends — the half of 372 that is true. `SessionRouter::choose` really
/// does rank multiple launch profiles against each other; see
/// `automatic_launch_never_selects_the_higher_ranked_profile_it_did_not_ask_for`
/// for the half that is not.
#[test]
fn two_enabled_profiles_differing_on_protocol_fit_are_ranked_and_the_higher_one_wins() {
    let fixture = ProfileSelectionFixture::new();
    let report = fixture.stdout(&["route"]);
    assert!(
        report.starts_with("destination  fresh:claude-code:better"),
        "`better` declares the protocol claude-code speaks natively (protocol fit \
         `native`, +1.000) and must outrank the implied `native` profile, which declares \
         none (protocol fit `unknown`, +0.000):\n{report}"
    );
    assert!(
        report.contains("fresh:claude-code:native"),
        "the implied native profile must still be offered as a ranked alternative — this \
         is what a mutation offering only the top-ranked name would remove:\n{report}"
    );
}

/// Acceptance test 1 (GH-PROFILE-ENABLED), and the inversion of the test
/// this replaces.
///
/// `GH-PROFILE-SELECTION` pinned the defect here as
/// `a_disabled_profile_is_still_offered_as_a_fresh_destination`, and said in
/// its own doc that *"if clause 1 is ever closed, this test's assertions
/// invert along with it"*. This is that inversion: `disabled-one` sets
/// `enabled = false` — the field `config/mod.rs` defines and the settings
/// screen round-trips — and it is no longer a routing candidate.
///
/// # Why the other two assertions are here
///
/// "Does not contain a string" is true of an empty report, of a crashed
/// binary, and of a filter that removed every profile rather than one. Both
/// survivors are asserted, in their ranked order, so the exclusion is a real
/// one — the same shape
/// `a_profile_configured_for_another_harness_is_never_offered_under_the_wrong_one`
/// below uses, and practice §17's lesson about absence assertions.
///
/// `native` in particular must survive: the filter reads
/// `EffectiveConfig::profile_enabled`, which answers `true` for the implied
/// Native profile without consulting configuration, and that is what makes
/// the enabled candidate set impossible to empty.
#[test]
fn a_disabled_profile_is_not_offered_as_a_fresh_destination() {
    let fixture = ProfileSelectionFixture::new();
    let report = fixture.stdout(&["route"]);
    assert!(
        !report.contains("fresh:claude-code:disabled-one"),
        "`enabled = false` must remove a profile from the routing candidate set:\n{report}"
    );
    assert!(
        report.starts_with("destination  fresh:claude-code:better"),
        "the enabled profile that outranks `native` must still win — without this the \
         assertion above would also pass against a build that offered nothing at \
         all:\n{report}"
    );
    assert!(
        report.contains("fresh:claude-code:native"),
        "and the implied Native profile must still be offered, so the filter is removing \
         one profile rather than emptying the set:\n{report}"
    );
}

/// Acceptance test 3: an explicit `--profile` naming a disabled profile is
/// refused, and the refusal names the profile and how to undo it.
///
/// The candidate filter above only stops *offering* the profile. Without
/// this, `--profile disabled-one` would still start a session under it —
/// `DestinationScope::Launchable` takes the already-chosen name and never
/// consults the enabled set — and `enabled` would mean nothing on the one
/// path that actually starts anything.
///
/// `launch_preflight.rs`'s
/// `an_explicitly_named_disabled_profile_is_refused_before_anything_is_probed_or_recorded`
/// is the other half: that the refusal arrives early enough to cost nothing.
/// This one is about what the person reads.
#[test]
fn explicitly_launching_a_disabled_profile_is_refused_and_says_how_to_undo_it() {
    let fixture = ProfileSelectionFixture::new();
    let launched = fixture.glasshouse(&[
        "launch",
        "claude-code",
        "--headless",
        "--profile",
        "disabled-one",
    ]);
    let streams = Fixture::both_streams(&launched);

    assert!(
        streams.contains("disabled-one"),
        "the refusal must name the profile that was refused:\n{streams}"
    );
    assert!(
        streams.contains("enabled = true"),
        "and must say what to change to undo it — a refusal a person cannot act on is the \
         same silence this packet removed:\n{streams}"
    );
    assert!(
        !launched.status.success(),
        "a disabled profile named explicitly must not start a session:\n{streams}"
    );

    // The same configuration with that one line gone launches, so the
    // refusal is about `enabled` and not about anything else in the profile.
    let enabled = ProfileSelectionFixture::with_all_profiles_enabled();
    let launched = enabled.glasshouse(&[
        "launch",
        "claude-code",
        "--headless",
        "--profile",
        "disabled-one",
    ]);
    assert!(
        launched.status.success(),
        "the same profile without `enabled = false` must launch — otherwise the refusal \
         above could be about the profile's backend, its protocol, or anything else:\n{}",
        Fixture::both_streams(&launched)
    );
}

/// A name nobody configured is reported as **unknown**, not as disabled —
/// and the list of valid names it prints still contains the disabled
/// profile.
///
/// # This test exists because a mutation survived
///
/// Practice §80: flipping `profile_enabled`'s final fallback from `true` to
/// `false` passed the whole of this file. The verdict held up under §80's
/// three questions — the target ran, nothing was filtered out, and the line
/// really is on a path a person reaches, because `--profile <typo>` is
/// exactly the case where neither layer has the name. Nothing watched it.
///
/// What the mutation actually produced, run against the shipped binary:
/// *"launch profile `no-such-profile` is disabled by default; re-enable it
/// ... or set `enabled = true`"* — advice for a profile that does not exist,
/// about a key there is nowhere to put. "Disabled" and "never configured"
/// are different facts and a typo must not be answered with the wrong one.
///
/// # And it is the second listing surface
///
/// `ProfileLookupError::Unknown` builds its `valid names are:` list from
/// `EffectiveConfig::profile_names`, which is the accessor this packet was
/// told **not** to filter. `disabled-one` appearing here is that ruling
/// being load-bearing rather than stylistic: had the filter gone into
/// `profile_names`, a person who disabled a profile and then mistyped its
/// name would be told the profile does not exist.
#[test]
fn an_unconfigured_profile_name_is_unknown_rather_than_disabled() {
    let fixture = ProfileSelectionFixture::new();
    let output = fixture.glasshouse(&[
        "launch",
        "claude-code",
        "--headless",
        "--profile",
        "no-such-profile",
    ]);
    let streams = Fixture::both_streams(&output);

    assert!(
        streams.contains("is not a known launch profile"),
        "a name nobody configured must be reported as unknown:\n{streams}"
    );
    // `ProfileDisabled`'s own phrase, not the bare word: `disabled-one` is
    // in the valid-names list below, so "does not contain `disabled`" is a
    // condition this fixture can never satisfy and would be a test that only
    // looked like one.
    assert!(
        !streams.contains("is disabled"),
        "and never as disabled — there is no `enabled` key to set on a profile that does \
         not exist, so that advice cannot be followed:\n{streams}"
    );
    assert!(
        streams.contains("disabled-one"),
        "the valid-names list comes from `profile_names`, which this packet deliberately \
         does not filter — so a disabled profile is still named here, and a person who \
         disabled one and then mistyped it is told it exists:\n{streams}"
    );
    assert!(
        !output.status.success(),
        "and an unknown profile still does not start a session:\n{streams}"
    );
}

/// Acceptance test 4 — the no-regression, and the reason the fixture has two
/// constructors that differ by exactly one line of TOML.
///
/// `enabled_by_default` is `true`, and a configuration file written before
/// anything read the field has no `enabled` key at all. Every such profile
/// must be offered exactly as it was before this packet. Captured from the
/// shipped binary before the change and asserted here: three destinations,
/// `better` first.
///
/// This is the test mutation (c) — `enabled_by_default` returning `false` —
/// has to fail, and it fails loudly rather than quietly: under it `better`
/// and `disabled-one` both vanish and the report opens on `native` instead.
#[test]
fn a_configuration_with_no_enabled_key_offers_every_profile_exactly_as_before() {
    let fixture = ProfileSelectionFixture::with_all_profiles_enabled();
    let report = fixture.stdout(&["route"]);

    assert!(
        report.starts_with("destination  fresh:claude-code:better"),
        "with no `enabled` key anywhere the ranking must be what it was before the field \
         was read — `better` first:\n{report}"
    );
    for name in ["better", "disabled-one", "native"] {
        assert!(
            report.contains(&format!("fresh:claude-code:{name}")),
            "`{name}` has no `enabled` key, so it must still be offered:\n{report}"
        );
    }
}

/// Ruling 4: the implied Native profile cannot be disabled, and a
/// `[profiles.native]` table saying otherwise changes nothing.
///
/// `EffectiveConfig::launch_profile` already short-circuits this name before
/// any table lookup — the Native profile exists for every harness by
/// construction, and `ProfileTable` never stores it — so `profile_enabled`
/// answers the same way rather than inventing a second rule. That is what
/// makes "you have disabled every profile and have nowhere to launch"
/// unreachable instead of merely unwritten: this one always survives the
/// filter.
#[test]
fn the_implied_native_profile_cannot_be_disabled() {
    let fixture = ProfileSelectionFixture::new();
    fixture.write_project_config(
        "version = 1\n\n[profiles.native]\nharness = \"claude-code\"\nenabled = false\n",
    );

    let report = fixture.stdout(&["route"]);
    assert!(
        report.contains("fresh:claude-code:native"),
        "`[profiles.native] enabled = false` must not remove the implied Native profile — \
         it is not a configuration entry and there is nothing there to disable:\n{report}"
    );

    let launched =
        fixture.glasshouse(&["launch", "claude-code", "--headless", "--profile", "native"]);
    assert!(
        launched.status.success(),
        "and naming it explicitly must still launch:\n{}",
        Fixture::both_streams(&launched)
    );
}

/// Acceptance test 5: project and user disagreeing about `enabled` resolve
/// the way every other lookup on `EffectiveConfig` resolves — project first,
/// then user — asserted in **both** directions through the shipped binary.
///
/// Both directions matter, and a mutation that inverts the precedence has to
/// fail on one of them whichever way it inverts. A single-direction test
/// would pass against a build that simply ignored the project layer, or one
/// that simply ignored the user layer.
///
/// The layer is chosen for the profile as a whole, not for this field alone:
/// `launch_profile` already takes the winning layer's `ProfileConfig` entire,
/// so a project that redefines a name supplies its `enabled` too. See
/// `EffectiveConfig::profile_enabled`'s own doc for why resolving the field
/// separately would build a profile neither layer wrote.
#[test]
fn project_and_user_disagreeing_about_enabled_resolve_project_first() {
    // The user disabled it; the project redefines the name and says nothing
    // about `enabled`, which is `true`. Project wins, so it is offered.
    let user_disabled = ProfileSelectionFixture::new();
    user_disabled.write_project_config(
        "version = 1\n\n\
         [profiles.disabled-one]\nharness = \"claude-code\"\n\
         expected_protocol = \"anthropic-messages\"\n\n\
         [profiles.disabled-one.backend]\nkind = \"direct-provider\"\n\
         provider = \"route-probe\"\n",
    );
    let report = user_disabled.stdout(&["route"]);
    assert!(
        report.contains("fresh:claude-code:disabled-one"),
        "the project layer redefines this profile and does not disable it, so it wins over \
         the user layer's `enabled = false`:\n{report}"
    );

    // And the other way: nothing disabled at the user layer, the project
    // disables it. Project wins again, so it is gone.
    let project_disabled = ProfileSelectionFixture::with_all_profiles_enabled();
    project_disabled.write_project_config(
        "version = 1\n\n\
         [profiles.disabled-one]\nharness = \"claude-code\"\nenabled = false\n\
         expected_protocol = \"anthropic-messages\"\n\n\
         [profiles.disabled-one.backend]\nkind = \"direct-provider\"\n\
         provider = \"route-probe\"\n",
    );
    let report = project_disabled.stdout(&["route"]);
    assert!(
        !report.contains("fresh:claude-code:disabled-one"),
        "the project layer disables it and the user layer says nothing, so it must be \
         gone:\n{report}"
    );
    assert!(
        report.starts_with("destination  fresh:claude-code:better"),
        "and the rest of the ranking is untouched, so the assertion above is an exclusion \
         rather than an empty report:\n{report}"
    );

    // The refusal reads the same resolution, and says which layer decided.
    let launched = project_disabled.glasshouse(&[
        "launch",
        "claude-code",
        "--headless",
        "--profile",
        "disabled-one",
    ]);
    let streams = Fixture::both_streams(&launched);
    assert!(
        streams.contains("this project's configuration"),
        "a person told their profile is disabled needs to know which file to open:\n{streams}"
    );
}

/// Acceptance test 4, and the evidence for clause 2 of the refusal: **the
/// launch path never reaches the candidate set the previous two tests rank.**
///
/// `better` outranks the implied `native` profile in `glasshouse route`. If
/// box 372's second clause held — "the router selects among launch profiles
/// when automatic routing is enabled" — a launch with no `--profile` flag
/// would start under whichever profile the ranking actually prefers. It does
/// not: `routing_destinations`'s `offered` set under
/// `DestinationScope::Launchable` (`main.rs`, the match feeding the "one
/// fresh destination per configured launch profile" loop) is a single
/// already-chosen name, never `effective.profile_names()`. The two
/// production callers that *do* build the multi-profile candidate set —
/// `route_recommendation` (reached only by `glasshouse route`, which starts
/// nothing, see `route_explains_the_ranking_and_starts_nothing` above) and
/// `report_task_boundary_routing` (called from `resume_session`, which
/// forces `RoutingOverride::to` the resumed session and uses the ranking
/// only to print what it *would* have chosen on stderr) — neither acts on
/// the ranking. `launch_session`, the one caller that does act, never builds
/// this candidate set at all.
#[test]
fn automatic_launch_never_selects_the_higher_ranked_profile_it_did_not_ask_for() {
    let fixture = ProfileSelectionFixture::new();

    let report = fixture.stdout(&["route"]);
    assert!(
        report.starts_with("destination  fresh:claude-code:better"),
        "the fixture's own baseline — `better` must outrank the implied `native` profile \
         before this test's claim about the launch path means anything:\n{report}"
    );

    let launched = fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    assert!(
        launched.status.success(),
        "the launch must succeed:\n{}",
        Fixture::both_streams(&launched)
    );

    let listing = fixture.stdout(&["sessions"]);
    assert!(
        !listing.contains("better"),
        "if the router selected among launch profiles when automatic routing is enabled, a \
         launch naming no profile would start under `better`, the one the ranking actually \
         prefers:\n{listing}"
    );
    assert!(
        listing.contains("native"),
        "instead the launch starts under the implied native profile — the one already \
         chosen by `launch_session`'s own default, before the router ever runs:\n{listing}"
    );
}

/// Acceptance test 3, and box 1382's own executable proof: two runs
/// differing **only** in `--task` text choose differently, because the
/// registry the task's classification reaches actually separates the two
/// candidates. Under mutation (a) — `TaskRequirements::default()` at
/// `route_report`'s call site instead of the derived value — the browser-task
/// run would score identically to the no-task run and this assertion fails.
#[test]
fn a_task_naming_a_capability_flips_which_candidate_the_ranking_prefers() {
    let fixture = TwoHarnessFixture::new();

    let without_task = fixture.stdout(&["route"]);
    assert!(
        without_task.starts_with("destination  fresh:codex:direct-codex"),
        "without a task, `direct-codex`'s `+0.4` protocol-fit edge over `direct-cc`'s `+0.0` \
         must be what wins — the fixture's own baseline, verified before the task text can \
         change anything:\n{without_task}"
    );

    let with_browser_task = fixture.stdout(&[
        "route",
        "--task",
        "explain what chrome browser screenshot support looks like",
    ]);
    assert!(
        with_browser_task.starts_with("destination  fresh:claude-code:direct-cc"),
        "a task naming only browser interaction must close `direct-codex`'s protocol-fit \
         lead with `direct-cc`'s own `+0.4` capability contribution and change which \
         candidate wins:\n{with_browser_task}"
    );
}

/// Acceptance test 3 (GH-PROFILE-SELECTION, box 372): a profile configured
/// for another harness is not offered as a destination under the wrong one.
/// `main.rs`'s own comment on the "one fresh destination per configured
/// launch profile" loop names this ("a profile configured for another
/// harness is not a destination for this launch"), and
/// `EffectiveConfig::launch_profile` (`config/mod.rs:2730`) refuses a
/// harness mismatch (`ProfileLookupError::HarnessMismatch`) rather than
/// substituting. Already true before this packet — pinned here as a
/// regression rather than left proven only by reading the source.
///
/// `glasshouse route` with no `--harness` ranks across every enabled harness
/// in one combined report (`route_recommendation` loops `IntegrationId::ALL`),
/// so `TwoHarnessFixture`'s single report already carries both harnesses'
/// candidates — exactly what this test needs.
#[test]
fn a_profile_configured_for_another_harness_is_never_offered_under_the_wrong_one() {
    let fixture = TwoHarnessFixture::new();
    let report = fixture.stdout(&["route"]);

    assert!(
        !report.contains("fresh:claude-code:direct-codex"),
        "`direct-codex` names harness `codex` and must never appear as a claude-code \
         destination:\n{report}"
    );
    assert!(
        !report.contains("fresh:codex:direct-cc"),
        "`direct-cc` names harness `claude-code` and must never appear as a codex \
         destination:\n{report}"
    );
    // And both still appear under their own harness, so the two assertions
    // above are checking a real exclusion rather than a typo nothing renders.
    assert!(
        report.contains("fresh:claude-code:direct-cc"),
        "direct-cc must still be offered under its own harness:\n{report}"
    );
    assert!(
        report.contains("fresh:codex:direct-codex"),
        "direct-codex must still be offered under its own harness:\n{report}"
    );
}
