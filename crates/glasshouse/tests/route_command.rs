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
        Self::with_extra_config("")
    }

    /// The same project, with `extra` appended to the user config.
    ///
    /// Every test above this line calls [`Fixture::new`] and must keep seeing
    /// the configuration it was written against: adding a launch profile adds
    /// a *destination*, and several of those tests assert on how many
    /// destinations the ranking held. So the quota tests below get their own
    /// profiles through here rather than by widening the shared fixture.
    fn with_extra_config(extra: &str) -> Self {
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
                 provider = \"route-probe\"\n{extra}"
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

    /// The directory `--data-dir` names, which is what
    /// `crate::paths::RuntimePaths::data_dir` resolves to inside the binary
    /// and therefore the root `GatewayQuotaCache::new` and
    /// `GatewayHealthCache::new` build their caches under.
    fn data_dir(&self) -> PathBuf {
        self.base.join("data")
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
        report.contains("no gateway has yet persisted a health reading"),
        "provider health is 0.0 here because nothing has been persisted about these \
         credentials, and that is a fact about what was read rather than about the \
         providers. **This caveat is now conditional** — line 1599's bridge makes it \
         false whenever a reading was attributed — so it has to keep being *printed* in \
         the case it is still true, or an unread pool becomes indistinguishable from a \
         pool that was read and found healthy:\n{report}"
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

// --- GH-ROUTER-INPUT-PROOF: Phase 34D lines 1452/1453/1455/1456 -----------
//
// `report-router-schema-recon.md` called 1452/1453 REACHABLE/ALREADY TRUE
// with a diagnostic-only caveat (the chain runs under `glasshouse route`,
// which ranks and prints and starts nothing) and called 1455/1456 ALREADY
// TRUE by construction. Phase 34D's own heading is "Router request schema"
// — every line in it, 1452/1453 included, is a claim about what the
// router's *input* contains, not about what a routing decision then does.
// The diagnostic-only caveat is therefore not disqualifying here: `--task`
// genuinely builds `RouterInputs` and hands it to `SessionRouter::choose`
// (`main.rs:1170-1206`), which is the router this phase's lines are about.
// It would be disqualifying for a line asking whether a *launch* sees the
// signal — it does not (`launch_session`/`report_task_boundary_routing`
// still pass `TaskRequirements::default()`, `main.rs:1431`/`3820`) — but
// that is a different claim than these two lines make.
#[test]
fn a_task_naming_repository_work_puts_repository_access_in_the_router_input_and_a_plain_question_does_not()
 {
    let fixture = Fixture::new();

    let repo_task = fixture.stdout(&[
        "route",
        "--task",
        "check how the router handles this repo's classification logic",
    ]);
    assert!(
        repo_task.contains("needs repository access"),
        "a declarative task naming this repository must put the repository-exploration \
         signal in the router's input — `capability_fit`'s evidence string is the \
         observable trace of `TaskRequirements.hard_capabilities` (line 1452):\n{repo_task}"
    );

    let plain_question = fixture.stdout(&[
        "route",
        "--task",
        "what is the difference between a fresh and an existing session",
    ]);
    assert!(
        !plain_question.contains("needs repository access"),
        "a pure question with no repository reference and no code-modification signal must \
         not carry the repository-exploration signal:\n{plain_question}"
    );
    assert!(
        plain_question.contains("the task named no hard capability requirement"),
        "and must classify to no hard capability at all, the same evidence string \
         `TaskRequirements::default()` produces:\n{plain_question}"
    );
}

/// Lines 1455/1456: the router's actual input, not the CLI report.
///
/// The CLI report (`render_route_recommendation`) never echoes `--task`
/// text or any Debug form of `RouterInputs`/`TaskRequirements` — it renders
/// only the named `Contribution`s. So a report-text assertion cannot tell
/// "the router received a bounded classification" from "the router received
/// the raw text and nothing happens to print it". The claim has to be checked
/// against the value the router actually receives, so these tests build it
/// through the same public `RouterAnswer::requirements()` `main.rs`'s
/// `heuristic_answer` produces on every path that asks no model (the
/// functions themselves are private to the binary and unreachable from an
/// integration test). Since Phase 34D the request a routing *model* is shown
/// is bounded separately — `routing::request::TASK_TEXT_CEILING_BYTES` and
/// `tests/launch_classification.rs` cover that half against the wire.
mod bounded_router_input {
    use glasshouse::routing::classify::classify_heuristically;
    use glasshouse::routing::request::{AnswerProvenance, HeuristicReason, RouterAnswer};
    use glasshouse::routing::session::TaskRequirements;

    /// What `main.rs`'s `heuristic_answer(..).requirements()` builds from a
    /// task string — the producer `classify_for_routing` uses on every path
    /// that asks no model — reproduced here through the same public
    /// `RouterAnswer` because those functions are private to the binary.
    fn router_input_for(task_text: &str) -> TaskRequirements {
        RouterAnswer::new(
            classify_heuristically(task_text),
            AnswerProvenance::Heuristic(HeuristicReason::NoRoutingModel),
        )
        .requirements()
    }

    /// Line 1455. A task description built to look like a real file's
    /// worth of source — many lines, `.rs` references, the shape a person
    /// pasting repository content into `--task` would actually produce —
    /// must not make the router's input grow. `HardCapability` is a
    /// three-variant, data-free enum (`classify.rs:241-245`) and
    /// `TaskRequirements` is exactly `{ needs_tool_calls: bool,
    /// hard_capabilities: Vec<HardCapability> }` (`session.rs:379-386`), so
    /// there is structurally nowhere for the repository content itself to
    /// go — this test is the executable form of that reading.
    #[test]
    fn the_router_input_stays_small_when_the_task_text_looks_like_repository_contents() {
        let repo_shaped = format!(
            "check how this repo's router.rs handles this: {}",
            "fn handle_request(req: &Request) -> Response { todo!() }\n".repeat(4_000)
        );
        assert!(
            repo_shaped.len() > 200_000,
            "the fixture text must actually be repository-content-sized"
        );

        let requirements = router_input_for(&repo_shaped);
        let rendered = format!("{requirements:?}");
        assert!(
            rendered.len() < 1_024,
            "the router's input must stay a small structured value however large a task \
             description that looks like repository contents is — it rendered to {} bytes \
             for a {}-byte task, which would mean the repository content itself reached the \
             router:\n{rendered}",
            rendered.len(),
            repo_shaped.len()
        );
    }

    /// Line 1456. Same claim, shaped like a session transcript rather than
    /// source code — many `user:`/`assistant:` turns, the shape a person
    /// pasting a prior conversation into `--task` would produce.
    #[test]
    fn the_router_input_stays_small_when_the_task_text_looks_like_a_session_transcript() {
        let transcript_shaped = format!(
            "continue this session: {}",
            "user: can you look at this repo\nassistant: sure, one moment\n".repeat(4_000)
        );
        assert!(
            transcript_shaped.len() > 200_000,
            "the fixture text must actually be transcript-sized"
        );

        let requirements = router_input_for(&transcript_shaped);
        let rendered = format!("{requirements:?}");
        assert!(
            rendered.len() < 1_024,
            "the router's input must stay small however large a task description that looks \
             like a session transcript is — it rendered to {} bytes for a {}-byte task, which \
             would mean the transcript itself reached the router:\n{rendered}",
            rendered.len(),
            transcript_shaped.len()
        );
    }

    /// Acceptance test 5: the general case, independent of content shape —
    /// "small structured input" as an executable bound rather than a
    /// description. A task body an order of magnitude larger than either
    /// fixture above, of arbitrary content, still renders to a bounded
    /// value.
    #[test]
    fn a_very_long_task_body_of_arbitrary_content_still_produces_a_small_structured_input() {
        let huge = "the quick brown fox jumps over the lazy dog, repeatedly, ".repeat(20_000);
        assert!(
            huge.len() > 1_000_000,
            "the fixture text must be over a megabyte"
        );

        let requirements = router_input_for(&huge);
        let rendered = format!("{requirements:?}");
        assert!(
            rendered.len() < 1_024,
            "a megabyte-scale task description must still produce a router input that \
             renders to a small, bounded value — it rendered to {} bytes:\n{rendered}",
            rendered.len()
        );
    }
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

// ===========================================================================
// Lines 1598 and 1599 — the two routing inputs, on the path that *acts*
// ===========================================================================
//
// `tests/session_router.rs` already shows `quota_pressure` and
// `provider_health` each separating two destinations. Both of those tests
// build their `Destination`s by hand and hand them straight to
// `SessionRouter::choose`, so neither can fail on a build where
// `routing_destinations` never reads a capacity at all — practice §35's exact
// shape, and why `docs/product/evidence/phase-37.md` returned 1598 and 1599
// open rather than closing them on those tests.
//
// Everything below enters through `glasshouse launch`, which is
// `main.rs::launch_session` — the caller that starts a session, not
// `route_recommendation`, which ranks and prints and starts nothing.
//
// **No production seam was needed for the quota half, and none was added.**
// `routing_destinations` already reads its telemetry from the on-disk gateway
// quota cache — `GatheredTelemetry::gather_gateway_quota(&GatewayQuotaCache::
// new(runtime.paths()))` — and that cache is a real production producer,
// written by the gateway from responses it forwards anyway. Planting a
// reading in it is what `tests/provider_discovery.rs` already does for
// `glasshouse resources`; the binary reads it through the code it always runs.

/// Two launch profiles that differ in **nothing** the router scores except
/// the provider whose quota cache they read.
///
/// Same harness, same declared wire protocol, same provider template, same
/// credential variable — so `harness capability fit`, `provider health`,
/// `model behaviour`, `switching cost` and `prompt-cache state` are equal
/// across the pair by construction.
const QUOTA_PROFILES: &str = "\n\
     [providers.alpha-probe]\ntemplate = \"openrouter\"\n\
     credential_env = [\"GLASSHOUSE_ROUTE_TEST_KEY\"]\n\n\
     [providers.beta-probe]\ntemplate = \"openrouter\"\n\
     credential_env = [\"GLASSHOUSE_ROUTE_TEST_KEY\"]\n\n\
     [profiles.alpha]\nharness = \"claude-code\"\n\
     expected_protocol = \"anthropic-messages\"\n\n\
     [profiles.alpha.backend]\nkind = \"direct-provider\"\n\
     provider = \"alpha-probe\"\n\n\
     [profiles.beta]\nharness = \"claude-code\"\n\
     expected_protocol = \"anthropic-messages\"\n\n\
     [profiles.beta.backend]\nkind = \"direct-provider\"\n\
     provider = \"beta-probe\"\n";

/// [`QUOTA_PROFILES`] with **a second credential on `alpha-probe`**.
///
/// `destination_backend` names a direct provider's credential from
/// `credential_env.first()`, so `alpha`'s destination is still keyed by
/// `GLASSHOUSE_ROUTE_TEST_KEY` and every label above is unchanged. What this
/// adds is a *sibling* key on the same provider — a second entry in the same
/// `gateway-health` file — which is the only configuration in which line
/// 1599's identity hazard can actually be observed. `CredentialId`'s own doc
/// calls two keys for one provider *"two separate allowances"*, and one being
/// refused says nothing about the other.
const SIBLING_KEY_PROFILES: &str = "\n\
     [providers.alpha-probe]\ntemplate = \"openrouter\"\n\
     credential_env = [\"GLASSHOUSE_ROUTE_TEST_KEY\", \"GLASSHOUSE_ROUTE_SIBLING_KEY\"]\n\n\
     [providers.beta-probe]\ntemplate = \"openrouter\"\n\
     credential_env = [\"GLASSHOUSE_ROUTE_TEST_KEY\"]\n\n\
     [profiles.alpha]\nharness = \"claude-code\"\n\
     expected_protocol = \"anthropic-messages\"\n\n\
     [profiles.alpha.backend]\nkind = \"direct-provider\"\n\
     provider = \"alpha-probe\"\n\n\
     [profiles.beta]\nharness = \"claude-code\"\n\
     expected_protocol = \"anthropic-messages\"\n\n\
     [profiles.beta.backend]\nkind = \"direct-provider\"\n\
     provider = \"beta-probe\"\n";

/// Wall-clock now: the binary reads these caches with its own real clock and
/// there is no injectable one (`mod@glasshouse::provider::quota`'s own rule).
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the system clock is after 1970")
        .as_secs() as i64
}

/// Plant a gateway quota reading exactly where `GatewayQuotaCache::new`
/// resolves one from this run's `--data-dir`, and prove it landed.
fn plant_quota(fixture: &Fixture, provider: &str, remaining: i64, limit: i64) {
    let cache = glasshouse::provider::telemetry::GatewayQuotaCache::at(
        fixture.data_dir().join("gateway-quota"),
    );
    cache.store(
        provider,
        &glasshouse::provider::telemetry::RateLimitHeaders::read(vec![
            ("ratelimit-limit", limit.to_string().as_str()),
            ("ratelimit-remaining", remaining.to_string().as_str()),
        ]),
        now_unix(),
    );
    assert!(
        cache.load(provider).is_some(),
        "the planted reading for `{provider}` must be on disk and readable, or the assertion \
         it supports would be about a misplaced file rather than about routing"
    );
}

/// The model every destination in these fixtures carries.
///
/// None of the profiles above names a `model`, so `session_pairing` answers
/// `AssignedModel::HarnessDefault` and `AssignedModel::label` renders it as
/// this. It is written out rather than computed because the bridge under test
/// matches a persisted reading's `model` field against exactly this string:
/// a test that derived it from the same call the production code makes would
/// rescale with any mutation of that call and could not detect one
/// (practice §80 case 6).
///
/// If this ever stops matching, `glasshouse route` prints it — the `provider
/// health` line names the model between backticks.
const HARNESS_DEFAULT_MODEL: &str = "the harness's own default";

/// One persisted gateway health reading, in the shape the write side
/// (`gateway::session::SessionRouting::health_readings_for`) produces: the
/// credential's **rendered label**, the model's own label, and an absolute
/// unix deadline.
fn health_reading(
    credential_label: &str,
    consecutive_failures: u32,
    cooling_down_until_unix: Option<i64>,
    credential_rejected: bool,
) -> glasshouse::provider::telemetry::GatewayHealthReading {
    glasshouse::provider::telemetry::GatewayHealthReading {
        credential_label: credential_label.to_owned(),
        model: HARNESS_DEFAULT_MODEL.to_owned(),
        consecutive_failures,
        cooling_down_until_unix,
        credential_rejected,
    }
}

/// Plant gateway health readings exactly where `GatewayHealthCache::new`
/// resolves them from this run's `--data-dir`, and prove they landed.
///
/// The same shape `plant_quota` uses, and for the same reason: an assertion
/// resting on a file nobody checked is an assertion about a path, not about
/// routing.
fn plant_health(
    fixture: &Fixture,
    provider: &str,
    readings: &[glasshouse::provider::telemetry::GatewayHealthReading],
) {
    let cache = glasshouse::provider::telemetry::GatewayHealthCache::at(
        fixture.data_dir().join("gateway-health"),
    );
    cache.store(provider, readings, now_unix());
    assert_eq!(
        cache.load(provider).len(),
        readings.len(),
        "the planted readings for `{provider}` must be on disk and readable through the same \
         reader production uses"
    );
}

/// The session identifier the fake harness was started with, from one logged
/// argv line. A fresh launch is `--session-id <uuid>`; a resume is
/// `--resume <uuid>`.
fn session_arg(argv: &str, flag: &str) -> String {
    let mut tokens = argv.split_whitespace();
    while let Some(token) = tokens.next() {
        if token == flag {
            return tokens
                .next()
                .unwrap_or_else(|| panic!("`{flag}` carried no identifier in `{argv}`"))
                .to_owned();
        }
    }
    panic!("no `{flag}` in `{argv}`")
}

/// Start one session under each of the two profiles and return
/// `(alpha_session, beta_session)`, read off the harness's own argv log
/// rather than off any listing this package also renders (§80 case 5).
fn two_sessions(fixture: &Fixture) -> (String, String) {
    for profile in ["alpha", "beta"] {
        let out =
            fixture.glasshouse(&["launch", "claude-code", "--headless", "--profile", profile]);
        assert!(
            out.status.success(),
            "launching under `{profile}` must succeed:\n{}",
            Fixture::both_streams(&out)
        );
    }
    let invocations = fixture.harness_invocations();
    assert_eq!(
        invocations.len(),
        2,
        "each `--profile` launch starts its own session:\n{invocations:?}"
    );
    let alpha = session_arg(&invocations[0], "--session-id");
    let beta = session_arg(&invocations[1], "--session-id");
    assert_ne!(alpha, beta, "the two launches must be two sessions");
    (alpha, beta)
}

/// Launch with no destination flags and return the session that was resumed.
fn launch_and_read_resumed(fixture: &Fixture) -> String {
    let out = fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    let said = Fixture::both_streams(&out);
    assert!(
        out.status.success(),
        "the deciding launch must succeed:\n{said}"
    );
    let invocations = fixture.harness_invocations();
    assert_eq!(
        invocations.len(),
        3,
        "the deciding launch must have continued one of the two existing sessions rather \
         than started a third:\n{invocations:?}\n{said}"
    );
    session_arg(&invocations[2], "--resume")
}

/// **Line 1598, through the acting path — as a mirrored pair.**
///
/// Two existing sessions, equal on every axis the router scores except what
/// the gateway quota cache says about their providers. The pair is run twice
/// with the readings swapped, and the winner has to follow the reading both
/// times.
///
/// Mirroring is what makes this airtight rather than merely green. A single
/// direction could be satisfied by any fixed ordering that happened to agree
/// with it — the caller's order, the store's, or a sub-second recency gap
/// between the two launches, which is worth `1.5/28800` per second against
/// quota's `0.8` and is decided by which side of a clock tick each launch
/// landed on. **No ordering can produce both halves below.** Only a value the
/// binary actually read can.
///
/// A build where `routing_destinations` stops calling `destination_capacity`,
/// or passes `None` for it, fails here — and nothing in
/// `tests/session_router.rs` can keep it passing, because nothing below
/// `launch_session` is entered.
#[test]
fn known_quota_pressure_decides_which_session_the_launch_path_continues() {
    // Half one: the room is on `alpha`.
    let roomy_alpha = Fixture::with_extra_config(QUOTA_PROFILES);
    let (alpha, beta) = two_sessions(&roomy_alpha);
    plant_quota(&roomy_alpha, "alpha-probe", 95, 100);
    plant_quota(&roomy_alpha, "beta-probe", 5, 100);
    assert_eq!(
        launch_and_read_resumed(&roomy_alpha),
        alpha,
        "the session on the provider with 95% remaining must win over the one on 5%. \
         alpha={alpha} beta={beta}"
    );

    // Half two: the same project, the same two profiles, the readings
    // swapped. This is the half a fixed ordering cannot also satisfy — and
    // the assertion that fails when the binary stops supplying
    // `Destination::capacity`, which no test that constructs a `Destination`
    // by hand can notice.
    let roomy_beta = Fixture::with_extra_config(QUOTA_PROFILES);
    let (alpha, beta) = two_sessions(&roomy_beta);
    plant_quota(&roomy_beta, "alpha-probe", 5, 100);
    plant_quota(&roomy_beta, "beta-probe", 95, 100);
    assert_eq!(
        launch_and_read_resumed(&roomy_beta),
        beta,
        "swapping the readings must swap the destination; a ranking that answers `alpha` \
         both times is reading its own order, not the quota cache. alpha={alpha} beta={beta}"
    );

    // And the explanation a person reads carries the planted reading itself,
    // not a placeholder. `route` renders the same contributions from the same
    // producers; the claim under test is the launch above, this is its
    // readable trace.
    let explained = roomy_beta.stdout(&["route"]);
    assert!(
        explained.contains("known quota pressure"),
        "the ranking must name the term:\n{explained}"
    );
    assert!(
        explained.contains("95% remaining") && explained.contains("5% remaining"),
        "and must carry both planted readings:\n{explained}"
    );
}

/// **The negative control.**
///
/// With nothing read about either provider, line 1598's term must be present
/// and worth exactly nothing — `quota_pressure`'s `None` arm, which is
/// neither "assume full" nor "assume empty" — and the launch must still
/// continue one of the two existing sessions rather than the ranking
/// collapsing into something else.
///
/// Without this, the pair above could pass because reading the cache changed
/// the shape of the candidate set rather than because a reading was weighed.
/// Note what is deliberately *not* asserted: **which** of the two wins. With
/// the term inert the two destinations are equal to within a sub-second
/// recency gap, so pinning a winner here would be pinning a clock tick.
#[test]
fn with_no_quota_reading_the_term_is_present_and_weighs_nothing() {
    let fixture = Fixture::with_extra_config(QUOTA_PROFILES);
    let (alpha, beta) = two_sessions(&fixture);

    let explained = fixture.stdout(&["route"]);
    assert!(
        explained.contains("known quota pressure"),
        "the term must still be in the explanation when nothing was read — a term that \
         vanishes cannot be told from one that was never computed:\n{explained}"
    );
    assert!(
        explained.contains("nothing has been read about `alpha-probe/GLASSHOUSE_ROUTE_TEST_KEY`")
            && explained
                .contains("nothing has been read about `beta-probe/GLASSHOUSE_ROUTE_TEST_KEY`"),
        "and must say so about both providers rather than inventing a number:\n{explained}"
    );

    let resumed = launch_and_read_resumed(&fixture);
    assert!(
        resumed == alpha || resumed == beta,
        "with the term inert the launch must still continue one of the two existing \
         sessions: resumed={resumed} alpha={alpha} beta={beta}"
    );
}

/// **Line 1599, through the acting path — the bridge, as a mirrored pair.**
///
/// Two existing sessions, and in each half the one whose provider refused the
/// credential is also the one quota favours. The pair is run twice with **both**
/// readings moved to the other provider, and the winner has to follow the health
/// reading both times — against the quota advantage, both times.
///
/// **Mirroring is what makes this airtight rather than merely green**, exactly
/// as it is for line 1598 above: a single direction could be satisfied by any
/// fixed ordering that happened to agree with it — the caller's order, the
/// store's, or the sub-second recency gap between the two launches. No ordering
/// produces both halves.
///
/// # Why quota is here rather than two bare destinations
///
/// **The first version of this test had no fulcrum and it survived severing the
/// bridge.** With no reading attributed the two sessions are equal to within a
/// sub-second recency gap, so each half was decided by a tie-break, and a
/// tie-break is not required to answer the same way in two separately-built
/// fixtures — so both halves could pass against a build where `launch_session`
/// read an empty pool. That is precisely practice §35's *"a caller you can
/// delete without a test noticing is, to the test suite, not a caller"*, found
/// by running the mutation this package requires rather than by reading the
/// test.
///
/// Planting quota removes the tie: with the bridge severed, quota alone decides
/// and names the **opposite** session in each half, so a severed build fails
/// here twice. `known_quota_pressure_decides_which_session_the_launch_path_continues`
/// is what makes that a fulcrum rather than an assumption — it proves the `0.72`
/// gap genuinely decides on its own — and `provider_health`'s `-1.5` for a
/// refused credential is what overturns it.
#[test]
fn observed_provider_health_decides_which_session_the_launch_path_continues() {
    // Half one: quota favours `alpha`, and `alpha` is the one whose credential
    // was refused — so `beta` must win, against the quota.
    let sick_alpha = Fixture::with_extra_config(QUOTA_PROFILES);
    let (alpha, beta) = two_sessions(&sick_alpha);
    plant_quota(&sick_alpha, "alpha-probe", 95, 100);
    plant_quota(&sick_alpha, "beta-probe", 5, 100);
    plant_health(
        &sick_alpha,
        "alpha-probe",
        &[health_reading(
            "alpha-probe/GLASSHOUSE_ROUTE_TEST_KEY",
            9,
            None,
            true,
        )],
    );
    assert_eq!(
        launch_and_read_resumed(&sick_alpha),
        beta,
        "a session whose provider refused the credential must lose to one nothing has been \
         observed against, even holding a 90-point quota advantage. A build that reads an \
         empty pool answers `alpha` here, on the quota alone. alpha={alpha} beta={beta}"
    );

    // Half two: the same project, the same two profiles, both readings moved to
    // the other provider.
    let sick_beta = Fixture::with_extra_config(QUOTA_PROFILES);
    let (alpha, beta) = two_sessions(&sick_beta);
    plant_quota(&sick_beta, "alpha-probe", 5, 100);
    plant_quota(&sick_beta, "beta-probe", 95, 100);
    plant_health(
        &sick_beta,
        "beta-probe",
        &[health_reading(
            "beta-probe/GLASSHOUSE_ROUTE_TEST_KEY",
            9,
            None,
            true,
        )],
    );
    assert_eq!(
        launch_and_read_resumed(&sick_beta),
        alpha,
        "moving the readings must move the destination; a ranking that answers the same \
         session both times is reading its own order, not the health cache — and one that \
         answers `beta` here is reading the quota and stopping there. alpha={alpha} \
         beta={beta}"
    );

    // And the explanation a person reads carries the planted reading itself.
    let explained = sick_beta.stdout(&["route"]);
    assert!(
        explained.contains("`beta-probe/GLASSHOUSE_ROUTE_TEST_KEY` was refused by its provider"),
        "the ranking must name what it read and attribute it to the credential it was \
         actually planted against — the identity hazard line 1599 turns on:\n{explained}"
    );
    assert!(
        !explained.contains("`alpha-probe/GLASSHOUSE_ROUTE_TEST_KEY` was refused"),
        "and must not smear it onto the other provider, which is configured with the very \
         same credential *variable* and is a separate allowance:\n{explained}"
    );
}

/// **Hazard 2: a persisted deadline that has already passed is not a
/// cooldown.**
///
/// `ResourceHealth::cooling_down_until` is an `Instant` — a monotonic clock
/// with no epoch — while a reading carries unix seconds, so the bridge has to
/// convert. The conversion that matters is the one for a deadline already in
/// the past: it must become *"not cooling down"*, never an `Instant`
/// manufactured to carry a value.
///
/// Both destinations carry a reading with the **same** `consecutive_failures`,
/// so the only difference between them is which side of `now` the deadline
/// falls on. Holding the failure count constant is deliberate: it is what
/// stops this passing because one side simply had more evidence against it.
///
/// - an expired deadline scores the graded failure penalty (`-0.9`, the floor)
///   and stays choosable;
/// - a live deadline scores `HEALTH_UNAVAILABLE_PENALTY` (`-1.5`).
///
/// A bridge that converted an elapsed deadline into a future `Instant` — the
/// obvious way to get the arithmetic wrong — suppresses *both* and this fails.
#[test]
fn an_already_elapsed_persisted_cooldown_does_not_suppress_a_destination() {
    // Half one: `alpha`'s cooldown is over, `beta`'s is not.
    let alpha_recovered = Fixture::with_extra_config(QUOTA_PROFILES);
    let (alpha, beta) = two_sessions(&alpha_recovered);
    plant_health(
        &alpha_recovered,
        "alpha-probe",
        &[health_reading(
            "alpha-probe/GLASSHOUSE_ROUTE_TEST_KEY",
            4,
            Some(now_unix() - 3_600),
            false,
        )],
    );
    plant_health(
        &alpha_recovered,
        "beta-probe",
        &[health_reading(
            "beta-probe/GLASSHOUSE_ROUTE_TEST_KEY",
            4,
            Some(now_unix() + 3_600),
            false,
        )],
    );
    assert_eq!(
        launch_and_read_resumed(&alpha_recovered),
        alpha,
        "a cooldown that elapsed an hour ago must not withhold a destination, while one that \
         has an hour left must. alpha={alpha} beta={beta}"
    );

    // Half two, swapped — the same reason line 1598's pair is mirrored.
    let beta_recovered = Fixture::with_extra_config(QUOTA_PROFILES);
    let (alpha, beta) = two_sessions(&beta_recovered);
    plant_health(
        &beta_recovered,
        "alpha-probe",
        &[health_reading(
            "alpha-probe/GLASSHOUSE_ROUTE_TEST_KEY",
            4,
            Some(now_unix() + 3_600),
            false,
        )],
    );
    plant_health(
        &beta_recovered,
        "beta-probe",
        &[health_reading(
            "beta-probe/GLASSHOUSE_ROUTE_TEST_KEY",
            4,
            Some(now_unix() - 3_600),
            false,
        )],
    );
    assert_eq!(
        launch_and_read_resumed(&beta_recovered),
        beta,
        "swapping which deadline has passed must swap the destination. alpha={alpha} \
         beta={beta}"
    );

    let explained = beta_recovered.stdout(&["route"]);
    assert!(
        explained.contains(
            "`alpha-probe/GLASSHOUSE_ROUTE_TEST_KEY` is still cooling down after 4 consecutive \
             observed failures"
        ),
        "the live deadline must read as a cooldown:\n{explained}"
    );
    assert!(
        explained.contains(
            "4 consecutive observed failures on `beta-probe/GLASSHOUSE_ROUTE_TEST_KEY` that \
             have not yet earned a cooldown"
        ),
        "and the elapsed one must read as failures *without* a cooldown — the same four \
         failures, scored the other way, which is the whole of hazard 2:\n{explained}"
    );
}

/// **The negative control.**
///
/// With nothing persisted about either provider, line 1599's term must be
/// present and worth exactly nothing — `provider_health`'s zero-failure arm,
/// which is *"not a health claim, the absence of one"* — and the launch must
/// still continue one of the two existing sessions.
///
/// Without this, the pairs above could pass because reading the cache changed
/// the shape of the candidate set rather than because a reading was weighed.
/// Note what is deliberately *not* asserted: **which** of the two wins. With
/// the term inert the two are equal to within a sub-second recency gap, so
/// pinning a winner here would be pinning a clock tick.
#[test]
fn with_no_health_reading_the_term_is_present_and_weighs_nothing() {
    let fixture = Fixture::with_extra_config(QUOTA_PROFILES);
    let (alpha, beta) = two_sessions(&fixture);

    let explained = fixture.stdout(&["route"]);
    assert!(
        explained.contains(
            "nothing has been observed against `the harness's own default` on \
             `alpha-probe/GLASSHOUSE_ROUTE_TEST_KEY`"
        ) && explained.contains(
            "nothing has been observed against `the harness's own default` on \
             `beta-probe/GLASSHOUSE_ROUTE_TEST_KEY`"
        ),
        "the term must still be in the explanation when nothing was read, and must say so \
         about both providers rather than inventing a health claim:\n{explained}"
    );
    assert!(
        explained.contains("provider health   nothing observed — no gateway has yet persisted"),
        "and the caveat block must say the pool was empty rather than letting a reader think \
         health was weighed and found equal:\n{explained}"
    );

    let resumed = launch_and_read_resumed(&fixture);
    assert!(
        resumed == alpha || resumed == beta,
        "with the term inert the launch must still continue one of the two existing sessions: \
         resumed={resumed} alpha={alpha} beta={beta}"
    );
}

/// **The tripwire, inverted — line 1599 is CLOSED and this is the executable
/// form of it.**
///
/// This test was
/// `a_persisted_provider_health_reading_reaches_the_binary_but_never_the_launch_paths_router`,
/// and it asserted the opposite: that a persisted `GatewayHealthReading`
/// reached the binary, was rendered by `glasshouse resources`, and stopped
/// there — because `launch_session` built `FreePool::new()` and no production
/// code converted a reading into a pool. Its own doc said *"the day someone
/// bridges `GatewayHealthCache` into `RouterInputs.health`, this test fails —
/// which is the signal to re-open line 1599, not to relax the assertion."*
///
/// `main.rs::observed_provider_health` is that bridge. **The assertion is
/// inverted rather than relaxed**, and it keeps its fulcrum, which is what
/// makes it worth more than a plain "the reading arrives" test:
///
/// - quota is planted so `alpha` leads by `0.9 × 0.8 = 0.72` — a gap the
///   mirrored pair for line 1598 proves is genuinely decisive on its own;
/// - a credential-rejected reading is planted against `alpha`, worth `-1.5`.
///
/// So `beta` wins only if the health reading was read **and weighed against a
/// quota advantage that would otherwise have carried the ranking**. A bridge
/// that fired but contributed nothing leaves `alpha` winning, and this fails.
#[test]
fn a_persisted_provider_health_reading_reaches_the_launch_paths_router() {
    let fixture = Fixture::with_extra_config(QUOTA_PROFILES);
    let (alpha, beta) = two_sessions(&fixture);
    plant_quota(&fixture, "alpha-probe", 95, 100);
    plant_quota(&fixture, "beta-probe", 5, 100);
    plant_health(
        &fixture,
        "alpha-probe",
        &[health_reading(
            "alpha-probe/GLASSHOUSE_ROUTE_TEST_KEY",
            9,
            Some(now_unix() + 3_600),
            true,
        )],
    );

    // The fixture is not broken, proven on its own terms (§80 case 5 — a test
    // must fail on its subject, never on its fixture's ability to plant). The
    // shipped binary really does read *this* directory in *this* fixture:
    // `glasshouse resources` describes the built-in registry rather than a
    // config-only provider, so that half is planted under a registry name —
    // same cache, same run — and it comes back rendered.
    let cache = glasshouse::provider::telemetry::GatewayHealthCache::at(
        fixture.data_dir().join("gateway-health"),
    );
    cache.store(
        "anyrouter",
        &[health_reading(
            "anyrouter/ANYROUTER_API_KEY",
            9,
            Some(now_unix() + 3_600),
            true,
        )],
        now_unix(),
    );
    let resources = fixture.stdout(&["resources", "--no-harness"]);
    assert!(
        resources.contains("credential rejected"),
        "the binary must read the very directory this test plants into, or the assertion \
         below would be about a misplaced file rather than about routing:\n{resources}"
    );

    let resumed = launch_and_read_resumed(&fixture);
    assert_eq!(
        resumed, beta,
        "line 1599 is CLOSED: a provider whose credential the gateway watched being \
         *rejected* must lose even to a provider with a large quota advantage, because \
         `observed_provider_health` puts the persisted reading into the very `FreePool` \
         `provider_health` reads. If this fails while the quota pair still passes, the bridge \
         was severed. alpha={alpha} beta={beta}"
    );
}

/// **Hazard 1, as an assertion: one credential's health is not another's.**
///
/// This is the test the whole design of `observed_provider_health` exists to
/// pass, and the only one in this file that can fail while every other one
/// stays green.
///
/// `alpha-probe` is configured with **two** credentials. The destination is
/// keyed by the first (`destination_backend` reads `credential_env.first()`);
/// the rejected reading planted in that provider's health file names the
/// **second**. Both live in `alpha-probe.json`, so a bridge that filtered by
/// provider and then took whatever it found — the obvious shortcut, and the
/// one a label that cannot be reversed invites — attributes a sibling key's
/// refusal to a destination that does not use it.
///
/// The fulcrum is quota, proven decisive on its own by line 1598's mirrored
/// pair: `alpha` leads by `0.9 × 0.8 = 0.72`, and the misattributed `-1.5`
/// would overturn it. So the two behaviours give opposite answers:
///
/// - **attributing by credential** (correct): nothing is known about
///   `alpha`'s own key, the health term is `0.0` for both, quota decides, and
///   `alpha` wins;
/// - **attributing by provider alone** (the hazard): `alpha` is suppressed on
///   its sibling's evidence and `beta` wins.
///
/// Map line 1294's rule is why this is worth a test of its own — *"a
/// fabricated value here does not degrade the policy, it inverts it"*. A
/// router that avoids a healthy resource on another key's refusal is worse
/// than one that reads no health at all, because it is confidently wrong.
#[test]
fn a_sibling_credentials_refusal_is_not_attributed_to_the_key_the_destination_uses() {
    let fixture = Fixture::with_extra_config(SIBLING_KEY_PROFILES);
    let (alpha, beta) = two_sessions(&fixture);
    plant_quota(&fixture, "alpha-probe", 95, 100);
    plant_quota(&fixture, "beta-probe", 5, 100);

    // In `alpha-probe`'s own file, and refused — but against the key this
    // destination does not use.
    plant_health(
        &fixture,
        "alpha-probe",
        &[health_reading(
            "alpha-probe/GLASSHOUSE_ROUTE_SIBLING_KEY",
            9,
            Some(now_unix() + 3_600),
            true,
        )],
    );

    assert_eq!(
        launch_and_read_resumed(&fixture),
        alpha,
        "a refusal recorded against `GLASSHOUSE_ROUTE_SIBLING_KEY` must not withhold the \
         destination keyed by `GLASSHOUSE_ROUTE_TEST_KEY`: they are two separate \
         allowances, and the quota advantage `alpha` holds must therefore still decide. A \
         `beta` here means the bridge attributed by provider rather than by credential. \
         alpha={alpha} beta={beta}"
    );

    let explained = fixture.stdout(&["route"]);
    assert!(
        explained.contains(
            "nothing has been observed against `the harness's own default` on \
             `alpha-probe/GLASSHOUSE_ROUTE_TEST_KEY`"
        ),
        "the key the destination actually uses must still read as unobserved — a reading \
         filed under the same provider is not a reading about this credential:\n{explained}"
    );
    assert!(
        !explained.contains("`alpha-probe/GLASSHOUSE_ROUTE_TEST_KEY` was refused"),
        "and must never be reported as refused on its sibling's evidence:\n{explained}"
    );
}

/// **Hazard 1's other half: two readings that name one resource and disagree
/// are not a reading.**
///
/// `health_readings_for` maps over a pool already keyed by `FreeResource`, so
/// a file *this program wrote* cannot contain two entries for one credential
/// and model. A file it did not write can — and so, in principle, can a
/// genuine label collision, because `CredentialId::label` renders
/// `provider/var` for a `SecretRef::Environment` and
/// `provider/service:account` for a `SecretRef::OsCredential`, and those two
/// spellings can coincide. Both arrive as the same thing: one rendered name,
/// two different claims.
///
/// **Picking one is the failure mode this whole design refuses.** Which is
/// chosen would be an artefact of file order, and the router would then avoid
/// a healthy resource on evidence that may belong to a different credential
/// entirely. The rule is that a resource two readings disagree about is
/// unobserved, which is the same inert `0.0` an empty cache produces.
///
/// Quota is the fulcrum again: `alpha` leads by `0.72`, and the `-1.5` of the
/// refusal below would overturn it if either reading were adopted.
#[test]
fn two_readings_that_disagree_about_one_resource_leave_it_unobserved() {
    let fixture = Fixture::with_extra_config(QUOTA_PROFILES);
    let (alpha, beta) = two_sessions(&fixture);
    plant_quota(&fixture, "alpha-probe", 95, 100);
    plant_quota(&fixture, "beta-probe", 5, 100);

    // The same credential, the same model, contradicting each other about
    // whether the provider refused the key.
    plant_health(
        &fixture,
        "alpha-probe",
        &[
            health_reading(
                "alpha-probe/GLASSHOUSE_ROUTE_TEST_KEY",
                9,
                Some(now_unix() + 3_600),
                true,
            ),
            health_reading("alpha-probe/GLASSHOUSE_ROUTE_TEST_KEY", 0, None, false),
        ],
    );

    assert_eq!(
        launch_and_read_resumed(&fixture),
        alpha,
        "with the contradiction withheld the quota advantage decides, exactly as it does \
         with an empty cache. A `beta` here means one of the two readings was picked — and \
         a bridge that picks between contradictory claims is choosing by file order. \
         alpha={alpha} beta={beta}"
    );

    let explained = fixture.stdout(&["route"]);
    assert!(
        explained.contains(
            "nothing has been observed against `the harness's own default` on \
             `alpha-probe/GLASSHOUSE_ROUTE_TEST_KEY`"
        ),
        "a resource two readings disagree about must read as unobserved, not as whichever \
         of them the file happened to list first:\n{explained}"
    );
}
