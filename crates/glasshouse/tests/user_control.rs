//! Phase 40 lines 1712–1720 — **the person's controls over where work goes
//! and who may speak into a session.**
//!
//! # Why none of this is a unit test
//!
//! Every line in this block is a promise about what happens when a person
//! types a command or a machine speaks to a door. A unit test can prove the
//! rule and cannot prove that the shipped binary asks it, which is this
//! project's most expensive recurring defect (practice §35: *a caller you can
//! delete without a test noticing is, to the test suite, not a caller*). So
//! the launch-path tests here run `glasshouse launch` as a person would, and
//! the door tests start a real `glasshouse api serve`, spawn a real harness in
//! a real pseudo-terminal, and observe what that harness itself wrote down.
//!
//! The two fixtures below are deliberately the same shape as the ones in
//! `tests/route_command.rs` and `tests/worker_access.rs` — the first records
//! every argv its harness was started with, the second names its log files
//! after the session it is running — because those are the observations that
//! stay honest under a mutation: a harness that writes down its own argv is
//! independent of everything under test here.
//!
//! # What each test is for
//!
//! | line | test |
//! |---|---|
//! | 1712 | [`automatic_routing_can_be_turned_off_and_the_launch_says_so`], [`the_no_routing_flag_turns_the_ranking_off_for_one_launch`] |
//! | 1713 | [`pinning_a_harness_opens_that_harness_and_not_the_other_one`] |
//! | 1714, 1715 | [`to_and_fresh_override_a_ranking_that_would_have_chosen_otherwise`] |
//! | 1716 | [`checkpoint_first_leaves_a_checkpoint_for_the_session_being_left`], [`checkpoint_first_says_when_it_had_nothing_to_check_point`], [`checkpoint_first_on_a_resume_leaves_a_checkpoint_for_the_session_being_left`], [`checkpoint_first_on_a_resume_of_the_session_in_hand_says_it_had_nothing_to_do`] |
//! | 1717 | [`a_muted_session_refuses_machine_messages_but_not_interrupts`], [`a_mute_expires_on_its_own`] |
//! | 1718 | [`a_person_takes_over_an_orchestrated_worker_and_the_orchestrator_is_locked_out`] |
//! | 1719 | [`a_persons_keystroke_outranks_a_machine_message_to_the_same_session`] |
//! | 1720 | [`every_automated_move_is_announced_before_it_happens`] |

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// The launch-path fixture — lines 1712 to 1716, and 1720.
// ---------------------------------------------------------------------------

/// A project with two installed harnesses, each recording every argv it was
/// started with into a log of its own.
///
/// Two rather than one because line 1713 is *"pin a task to a specific
/// harness"*, and a project with one harness cannot tell a pin from a
/// default. Everything else here is `tests/route_command.rs`'s fixture: a
/// direct-provider profile so the ranking has more than one destination to
/// weigh, and argv logs as the observation, because what a harness was
/// started with is a fact no assertion in this file can influence.
struct Launcher {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    claude_log: PathBuf,
    codex_log: PathBuf,
}

/// The provider credential variable. A name only — nothing here resolves a
/// value, and the router is handed the *name* for its explanation.
const CREDENTIAL_VAR: &str = "GLASSHOUSE_USER_CONTROL_TEST_KEY";

impl Launcher {
    fn new() -> Self {
        Self::with_extra_config("")
    }

    fn with_extra_config(extra: &str) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let claude_log = base.join("claude-argv.log");
        let codex_log = base.join("codex-argv.log");
        let claude = install_argv_logging_harness(&bin_dir, "fake-claude-code", &claude_log);
        let codex = install_argv_logging_harness(&bin_dir, "fake-codex", &codex_log);

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n\n\
                 [integrations.codex]\nenabled = true\nexecutable = \"{}\"\n\n\
                 [providers.control-probe]\ntemplate = \"openrouter\"\n\
                 credential_env = [\"{CREDENTIAL_VAR}\"]\n\n\
                 [profiles.metered]\nharness = \"claude-code\"\n\
                 expected_protocol = \"anthropic-messages\"\n\n\
                 [profiles.metered.backend]\nkind = \"direct-provider\"\n\
                 provider = \"control-probe\"\n{extra}",
                escape(&claude),
                escape(&codex),
            ),
        )
        .expect("write user config");

        Self {
            _tmp: tmp,
            base,
            root,
            claude_log,
            codex_log,
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
            .env(CREDENTIAL_VAR, "planted-opaque-user-control-value")
            .env("PATH", self.base.join("empty-path"))
            .stdin(Stdio::null())
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

    fn stdout(&self, args: &[&str]) -> String {
        String::from_utf8_lossy(&self.glasshouse(args).stdout).into_owned()
    }

    fn invocations(log: &Path) -> Vec<String> {
        match std::fs::read_to_string(log) {
            Ok(text) => text.lines().map(str::to_owned).collect(),
            Err(_) => Vec::new(),
        }
    }

    fn claude_invocations(&self) -> Vec<String> {
        Self::invocations(&self.claude_log)
    }

    fn codex_invocations(&self) -> Vec<String> {
        Self::invocations(&self.codex_log)
    }

    /// The short identifiers `glasshouse sessions` prints, one per recorded
    /// session, most recently active first.
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

    /// Every **full** session identifier `glasshouse route` names, in the
    /// order it ranked them.
    ///
    /// `glasshouse sessions` prints the first twelve characters and `--to`
    /// compares against the whole identifier, so the report is where a person
    /// gets one they can paste — which is exactly what `--to`'s own help says
    /// ("by the identifier `glasshouse route` prints") and what this reads.
    fn ranked_session_ids(&self) -> Vec<String> {
        let report = self.stdout(&["route"]);
        let mut ids = Vec::new();
        for token in report.split(|c: char| !c.is_ascii_alphanumeric()) {
            if token.len() == 32
                && token.chars().all(|c| c.is_ascii_hexdigit())
                && !ids.iter().any(|seen| seen == token)
            {
                ids.push(token.to_owned());
            }
        }
        ids
    }

    fn checkpoint_listing(&self) -> String {
        self.stdout(&["checkpoint", "list"])
    }
}

fn escape(path: &Path) -> String {
    path.display().to_string().replace('\\', "\\\\")
}

/// A harness that appends its own argv to `log` and exits successfully.
///
/// Exit 0 deliberately: a non-zero exit makes the session `Failed`, and a
/// failed session is not a warm one — there would be nothing to route back
/// into, and every continuation test here would pass for the wrong reason.
fn install_argv_logging_harness(bin_dir: &Path, name: &str, log: &Path) -> PathBuf {
    let path = bin_dir.join(name);
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
            log.display()
        ),
    )
    .expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

// ---------------------------------------------------------------------------
// Line 1712 — the off switch
// ---------------------------------------------------------------------------

/// **Line 1712, through configuration.** *"Allow the user to disable
/// automatic routing for the current Glasshouse instance."*
///
/// The first launch leaves a warm, resumable session behind, which
/// `route_command::a_second_launch_continues_the_warm_session_rather_than_starting_another`
/// proves the ranking continues. With `automatic = false` under `[routing]`,
/// the second launch must **not** continue it: a second session is recorded
/// and the harness is started fresh rather than with `--resume`.
///
/// The behavioural assertion is first and the message second, on purpose
/// (practice §80): a mutation that removed the off switch would also remove
/// the sentence, and a KILLED credited to a missing *message* would say
/// nothing about where the work went.
#[test]
fn automatic_routing_can_be_turned_off_and_the_launch_says_so() {
    let fixture = Launcher::with_extra_config("\n[routing]\nautomatic = false\n");

    let first = fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    assert!(
        first.status.success(),
        "the first launch must succeed:\n{}",
        Launcher::both_streams(&first)
    );
    assert_eq!(fixture.recorded_sessions().len(), 1);

    let second = fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    let said = Launcher::both_streams(&second);
    assert!(
        second.status.success(),
        "the second launch must succeed:\n{said}"
    );

    assert_eq!(
        fixture.recorded_sessions().len(),
        2,
        "with automatic routing off, a launch takes no routing decision — so the warm session \
         this project has is not continued and a new one is recorded. This is the assertion \
         that fails when `launch_session` stops reading `EffectiveConfig::automatic_routing`:\n\
         {said}"
    );
    let invocations = fixture.claude_invocations();
    assert_eq!(
        invocations.len(),
        2,
        "the harness ran twice:\n{invocations:?}"
    );
    assert!(
        invocations.iter().all(|argv| !argv.contains("--resume")),
        "nothing was resumed with routing off:\n{invocations:?}"
    );

    assert!(
        said.contains("automatic routing is off"),
        "a launch that took no routing decision must say so — a person who turned the ranking \
         off still needs to know that is why nothing was continued:\n{said}"
    );
    assert!(
        said.contains("glasshouse route"),
        "and it must point at the command that answers `what would it have chosen`, because \
         this launch deliberately did not compute that:\n{said}"
    );
    assert!(
        !said.contains("continuing session"),
        "with routing off there is no continuation to announce:\n{said}"
    );
}

/// **Line 1712, for one launch.** The same switch as a flag, with the
/// configuration left alone: `--no-routing` turns the ranking off for this
/// invocation and the next launch without it behaves exactly as before.
///
/// The second half is what makes this a test of a *per-launch* control rather
/// than of the same code path twice.
#[test]
fn the_no_routing_flag_turns_the_ranking_off_for_one_launch() {
    let fixture = Launcher::new();

    fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    assert_eq!(fixture.recorded_sessions().len(), 1);

    let off = fixture.glasshouse(&["launch", "claude-code", "--headless", "--no-routing"]);
    let said = Launcher::both_streams(&off);
    assert!(off.status.success(), "`--no-routing` must launch:\n{said}");
    assert_eq!(
        fixture.recorded_sessions().len(),
        2,
        "`--no-routing` starts a session rather than continuing the warm one:\n{said}"
    );
    assert!(
        said.contains("--no-routing"),
        "the announcement must name the flag that turned the ranking off, not just the state, \
         so a person can tell a one-off from their configuration:\n{said}"
    );

    // And the very next launch, without the flag, routes again.
    let on = fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    let said_on = Launcher::both_streams(&on);
    assert_eq!(
        fixture.recorded_sessions().len(),
        2,
        "a launch without `--no-routing` continues a warm session again — the flag was for one \
         launch and changed nothing standing:\n{said_on}"
    );
    assert!(
        said_on.contains("continuing session"),
        "and it announces the continuation as it always did:\n{said_on}"
    );
}

// ---------------------------------------------------------------------------
// Line 1713 — pin a harness
// ---------------------------------------------------------------------------

/// **Line 1713.** *"Allow the user to pin a task to a specific harness."*
///
/// Two harnesses are installed and enabled, so nothing about this project
/// makes either the obvious answer — `glasshouse launch` with no harness
/// named refuses rather than guessing. Naming one starts that one's
/// executable and leaves the other's untouched, which the two argv logs
/// record independently of anything else here.
#[test]
fn pinning_a_harness_opens_that_harness_and_not_the_other_one() {
    let fixture = Launcher::new();

    // With two enabled, an unpinned launch is a refusal rather than a guess.
    // Asserted first because it is what makes the pin below mean something:
    // a project where one harness would have been chosen anyway cannot tell
    // a pin from a default.
    let unpinned = fixture.glasshouse(&["launch", "--headless"]);
    assert!(
        !unpinned.status.success(),
        "with two harnesses enabled Glasshouse must ask rather than guess:\n{}",
        Launcher::both_streams(&unpinned)
    );
    assert!(
        fixture.claude_invocations().is_empty() && fixture.codex_invocations().is_empty(),
        "and it must have started neither"
    );

    let pinned = fixture.glasshouse(&["launch", "codex", "--headless"]);
    assert!(
        pinned.status.success(),
        "a named harness must open:\n{}",
        Launcher::both_streams(&pinned)
    );
    assert_eq!(
        fixture.codex_invocations().len(),
        1,
        "the pinned harness is the one that ran"
    );
    assert!(
        fixture.claude_invocations().is_empty(),
        "and the other one was never started: {:?}",
        fixture.claude_invocations()
    );

    let listing = fixture.stdout(&["sessions"]);
    assert!(
        listing.contains("codex"),
        "and the recorded session must name the harness the person pinned:\n{listing}"
    );
}

// ---------------------------------------------------------------------------
// Lines 1714 and 1715 — pin a session, force a fresh one
// ---------------------------------------------------------------------------

/// **Lines 1714 and 1715, both against a ranking that would have chosen
/// otherwise.**
///
/// The point of both flags is that they beat an automatic answer, so a test
/// in which the ranking agreed with them would prove nothing. Two sessions
/// exist; the ranking's own answer is read out of `glasshouse route` first
/// and asserted to be the *other* one, and only then is `--to` given the
/// session it did not pick.
///
/// `--fresh` is the mirror image: the same ranking, and a new session anyway.
#[test]
fn to_and_fresh_override_a_ranking_that_would_have_chosen_otherwise() {
    let fixture = Launcher::new();

    fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    fixture.glasshouse(&["launch", "claude-code", "--headless", "--fresh"]);
    assert_eq!(
        fixture.recorded_sessions().len(),
        2,
        "two sessions must exist for a pin to be distinguishable from a default"
    );

    // What the ranking itself would do, read from the command that answers
    // without acting. The first identifier is the destination it chose.
    let ranked = fixture.ranked_session_ids();
    assert!(
        ranked.len() >= 2,
        "the report must rank both sessions, or `--to` has nothing to overrule: {ranked:?}"
    );
    let automatic = ranked[0].clone();
    let displaced = ranked[1].clone();

    let pinned = fixture.glasshouse(&["launch", "claude-code", "--headless", "--to", &displaced]);
    let said = Launcher::both_streams(&pinned);
    assert!(pinned.status.success(), "`--to` must launch:\n{said}");

    // Line 1714's behavioural claim: the session the person named is the one
    // that was continued, and no third session was recorded.
    assert_eq!(
        fixture.recorded_sessions().len(),
        2,
        "`--to <existing session>` continues that session rather than recording a third:\n{said}"
    );
    let invocations = fixture.claude_invocations();
    let resumed: Vec<&String> = invocations
        .iter()
        .filter(|argv| argv.contains("--resume"))
        .collect();
    assert_eq!(
        resumed.len(),
        1,
        "exactly one resume must have happened:\n{invocations:?}"
    );
    assert!(
        !resumed[0].contains(&automatic),
        "the resume must not be of the destination the ranking preferred (`{automatic}`) — that \
         is the whole of line 1714:\n{invocations:?}"
    );
    assert!(
        said.contains(&displaced) || said.contains("continuing session"),
        "and the launch must say where it went:\n{said}"
    );

    // Line 1715: the same ranking, and a new session anyway.
    let fresh = fixture.glasshouse(&["launch", "claude-code", "--headless", "--fresh"]);
    let said_fresh = Launcher::both_streams(&fresh);
    assert!(
        fresh.status.success(),
        "`--fresh` must launch:\n{said_fresh}"
    );
    assert_eq!(
        fixture.recorded_sessions().len(),
        3,
        "`--fresh` starts a new session however good the ranking's own answer looked:\n\
         {said_fresh}"
    );
    assert_eq!(
        fixture
            .claude_invocations()
            .iter()
            .filter(|argv| argv.contains("--resume"))
            .count(),
        1,
        "and nothing further was resumed"
    );
}

// ---------------------------------------------------------------------------
// Line 1716 — a checkpoint before the move
// ---------------------------------------------------------------------------

/// **Line 1716.** *"Allow the user to force a checkpoint before migration."*
///
/// Three sessions' worth of setup for one assertion, and every part of it is
/// load-bearing: the checkpoint has to be *for the session being left*, which
/// is only distinguishable from "for the session being entered" when the two
/// are different sessions and the project has been in one of them most
/// recently.
#[test]
fn checkpoint_first_leaves_a_checkpoint_for_the_session_being_left() {
    let fixture = Launcher::new();

    fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    fixture.glasshouse(&["launch", "claude-code", "--headless", "--fresh"]);
    let sessions = fixture.recorded_sessions();
    assert_eq!(sessions.len(), 2);
    // `glasshouse sessions` lists most recently active first, so this is the
    // session the work is currently in — the one a migration leaves.
    let leaving = sessions[0].clone();

    let ranked = fixture.ranked_session_ids();
    let elsewhere = ranked
        .iter()
        .find(|id| !id.starts_with(&leaving))
        .expect("a destination other than the session in hand")
        .clone();

    assert!(
        fixture.checkpoint_listing().contains("No checkpoints"),
        "nothing has been checkpointed yet:\n{}",
        fixture.checkpoint_listing()
    );

    let moved = fixture.glasshouse(&[
        "launch",
        "claude-code",
        "--headless",
        "--to",
        &elsewhere,
        "--checkpoint-first",
    ]);
    let said = Launcher::both_streams(&moved);
    assert!(
        moved.status.success(),
        "the migration must succeed:\n{said}"
    );

    let listing = fixture.checkpoint_listing();
    assert!(
        !listing.contains("No checkpoints"),
        "`--checkpoint-first` must have left a checkpoint behind:\n{listing}\n{said}"
    );
    assert!(
        listing.contains(&leaving),
        "and it must be for the session the work **left** (`{leaving}`), not the one it moved \
         into — a checkpoint filed against the destination would preserve nothing:\n{listing}"
    );
    assert!(
        said.contains("checkpoint") && said.contains(&leaving),
        "and the launch must announce it, naming the session it saved:\n{said}"
    );
}

/// **Line 1716's other half**, and the one a silent implementation would pass:
/// a flag that had nothing to do must say so rather than pass quietly.
///
/// A launch that starts a fresh session leaves nothing behind, so there is
/// nothing to check point. The failure this rules out is a `--checkpoint-first`
/// that writes no checkpoint and says nothing, which is indistinguishable
/// from one that wrote one — practice §68's shape, in a new costume.
#[test]
fn checkpoint_first_says_when_it_had_nothing_to_check_point() {
    let fixture = Launcher::new();

    let first = fixture.glasshouse(&["launch", "claude-code", "--headless", "--checkpoint-first"]);
    let said = Launcher::both_streams(&first);
    assert!(
        first.status.success(),
        "the launch must still succeed:\n{said}"
    );
    assert!(
        said.contains("--checkpoint-first had nothing to check point"),
        "a flag that did nothing must say so:\n{said}"
    );
    assert!(
        fixture.checkpoint_listing().contains("No checkpoints"),
        "and it must not have invented one:\n{}",
        fixture.checkpoint_listing()
    );

    // The second case: a launch that continues the session it was already in
    // is not a migration either.
    let second = fixture.glasshouse(&["launch", "claude-code", "--headless", "--checkpoint-first"]);
    let said_second = Launcher::both_streams(&second);
    assert!(
        said_second.contains("already where this work is")
            || said_second.contains("nothing to check point"),
        "continuing the session in hand leaves nothing behind, and the flag must say which of \
         the reasons applied:\n{said_second}"
    );
    assert!(
        fixture.checkpoint_listing().contains("No checkpoints"),
        "still no checkpoint:\n{}",
        fixture.checkpoint_listing()
    );
}

/// **Line 1716 on the resume path**, which is a production call site of its own
/// and would otherwise be one every test in this file goes around (practice
/// §35: *a caller you can delete without a test noticing is, to the test
/// suite, not a caller*).
///
/// `glasshouse resume <other>` is the same migration as `launch --to <other>`
/// wearing a different command name: the work leaves whichever session this
/// project was most recently in. Same flag, same no-op rules, same
/// announcement.
///
/// # Why the resume itself is not asserted to succeed
///
/// `glasshouse resume` has no `--headless`, so it wants a terminal on both
/// standard input and standard output and refuses without one — which a test
/// harness never has. That refusal happens **after** the checkpoint, which is
/// the ordering under test: the flag says *before the move*, and a move that
/// then did not happen must still have left the checkpoint behind. So this
/// asserts what the flag did, not what the resume did, and the failure of the
/// resume is quoted in the message so a future reader is not left guessing.
#[test]
fn checkpoint_first_on_a_resume_leaves_a_checkpoint_for_the_session_being_left() {
    let fixture = Launcher::new();

    fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    fixture.glasshouse(&["launch", "claude-code", "--headless", "--fresh"]);
    let sessions = fixture.recorded_sessions();
    assert_eq!(sessions.len(), 2);
    // Most recently active first: the work is in `leaving`, and `going_to`
    // is the older session the resume moves it into.
    let leaving = sessions[0].clone();
    let going_to = sessions[1].clone();

    assert!(
        fixture.checkpoint_listing().contains("No checkpoints"),
        "nothing has been checkpointed yet"
    );

    let resumed = fixture.glasshouse(&["resume", &going_to, "--checkpoint-first"]);
    let said = Launcher::both_streams(&resumed);

    let listing = fixture.checkpoint_listing();
    assert!(
        listing.contains(&leaving),
        "the checkpoint must be for the session the work **left** (`{leaving}`), not the one \
         being resumed (`{going_to}`):\n{listing}\n{said}"
    );
    assert!(
        said.contains("checkpoint") && said.contains(&leaving),
        "and the resume must announce it, naming the session it saved:\n{said}"
    );
}

/// **Line 1716's no-op on the resume path.** A project with one session has
/// nowhere to move work *from*, so resuming it is not a migration and the flag
/// says so rather than writing a checkpoint of a session against itself.
#[test]
fn checkpoint_first_on_a_resume_of_the_session_in_hand_says_it_had_nothing_to_do() {
    let fixture = Launcher::new();

    fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    let only = fixture.recorded_sessions();
    assert_eq!(only.len(), 1);

    let resumed = fixture.glasshouse(&["resume", &only[0], "--checkpoint-first"]);
    let said = Launcher::both_streams(&resumed);
    assert!(
        said.contains("already where this work is"),
        "resuming the session the work is already in is not a migration, and the flag must say \
         which of its reasons applied:\n{said}"
    );
    assert!(
        fixture.checkpoint_listing().contains("No checkpoints"),
        "and it must not have invented one:\n{}",
        fixture.checkpoint_listing()
    );
}

// ---------------------------------------------------------------------------
// Line 1720 — no automated move is silent
// ---------------------------------------------------------------------------

/// **Line 1720.** *"Surface automation decisions instead of silently moving
/// work between sessions."*
///
/// One test over every sentence `launch_session` can reach, because the claim
/// is about coverage rather than about any one message: an announcement block
/// that covers three of four decisions is one silent move away from the
/// failure this line names.
///
/// Each section names the decision it is about and asserts the sentence for
/// it. What is deliberately **not** asserted is a fresh session the *ranking*
/// chose over one it could have continued: see this test's own note below.
#[test]
fn every_automated_move_is_announced_before_it_happens() {
    let fixture = Launcher::new();

    // 1. A continuation. The ranking moved the work into a session the person
    //    did not name, which is the case line 1720 is written against.
    fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    let continued = fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    let said = Launcher::both_streams(&continued);
    assert!(
        said.contains("continuing session"),
        "a launch that continues an existing session must not do it silently:\n{said}"
    );

    // 2. An override that was honoured. The person's flag won, and what it
    //    displaced is named — otherwise the person cannot tell that their
    //    flag changed anything.
    let overridden = fixture.glasshouse(&["launch", "claude-code", "--headless", "--fresh"]);
    let said = Launcher::both_streams(&overridden);
    assert!(
        said.contains("because you named it") && said.contains("would have chosen"),
        "an honoured override must say what the ranking would have done instead:\n{said}"
    );

    // 3. An override that was refused. A destination that does not exist is
    //    not a destination, and a launch that silently used the ranking's own
    //    answer instead would have moved the work somewhere nobody asked for.
    let refused = fixture.glasshouse(&[
        "launch",
        "claude-code",
        "--headless",
        "--to",
        "fresh:claude-code:not-a-profile-anybody-configured",
    ]);
    let said = Launcher::both_streams(&refused);
    assert!(
        said.contains("was not applied"),
        "a refused override must say it was refused:\n{said}"
    );

    // 4. Routing off. Covered by its own test for behaviour; asserted here
    //    for presence in the same block, because the claim of this test is
    //    that the block covers every case rather than that each case works.
    let off = fixture.glasshouse(&["launch", "claude-code", "--headless", "--no-routing"]);
    let said = Launcher::both_streams(&off);
    assert!(
        said.contains("automatic routing is off"),
        "a launch that took no routing decision must say so:\n{said}"
    );

    // 5. A forced checkpoint, in both of its outcomes.
    let no_op = fixture.glasshouse(&[
        "launch",
        "claude-code",
        "--headless",
        "--fresh",
        "--checkpoint-first",
    ]);
    let said = Launcher::both_streams(&no_op);
    assert!(
        said.contains("--checkpoint-first had nothing to check point"),
        "a checkpoint that was not needed must say so rather than pass silently:\n{said}"
    );
}

// ---------------------------------------------------------------------------
// The door fixture — lines 1717, 1718 and 1719.
// ---------------------------------------------------------------------------

/// The harness the door tests run: it names its log files after the session it
/// was started for, echoes every line it reads, and survives a real `SIGINT`.
///
/// Byte for byte the shape `tests/worker_access.rs` uses, and for its reasons:
/// the session tag comes from the `--settings <state>/sessions/<id>/…`
/// argument the lifecycle-hook installation adds, so a door that stopped
/// installing hooks would fail these tests rather than quietly pass them
/// against an unattributable log; and the read loop distinguishes an
/// interrupted `read` from a real end of input, because the shells disagree
/// about what an interrupted `read` returns and the one-line form turns this
/// into a kill test on Linux.
const ECHOING_HARNESS: &str = "#!/bin/sh\n\
     tag=unknown\n\
     prev=\"\"\n\
     for a in \"$@\"; do\n\
     if [ \"$prev\" = \"--settings\" ]; then tag=$(basename \"$(dirname \"$a\")\"); fi\n\
     prev=\"$a\"\n\
     done\n\
     echo \"$@\" > \"$PWD/argv-$tag.log\"\n\
     interrupted=0\n\
     trap 'echo interrupted >> \"$PWD/interrupted-$tag.log\"; interrupted=1' INT\n\
     echo READY\n\
     while :; do\n\
     if IFS= read -r line; then\n\
     printf '%s\\n' \"$line\" >> \"$PWD/received-$tag.log\"\n\
     echo \"got:$line\"\n\
     elif [ \"$interrupted\" = 1 ]; then\n\
     interrupted=0\n\
     else\n\
     break\n\
     fi\n\
     done\n";

struct Door {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl Door {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let harness = bin_dir.join("echoing-harness");
        std::fs::write(&harness, ECHOING_HARNESS).expect("write the echoing harness");
        let mut perms = std::fs::metadata(&harness).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&harness, perms).unwrap();

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n[integrations.claude-code]\nenabled = true\n\
                 executable = \"{}\"\n",
                escape(&harness)
            ),
        )
        .expect("write user config");

        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        Self {
            _tmp: tmp,
            base,
            root,
        }
    }

    /// Everything the harness running `session` has read from its terminal.
    fn received(&self, session: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(format!("received-{session}.log"))).ok()
    }

    /// Whether the harness running `session` has handled a real `SIGINT`.
    ///
    /// Written by the harness's own `trap`, so it is the worker's account of
    /// what reached it rather than the door's account of what it sent.
    fn reacted_to_interrupt(&self, session: &str) -> bool {
        self.root
            .join(format!("interrupted-{session}.log"))
            .exists()
    }

    fn argv(&self, session: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(format!("argv-{session}.log"))).ok()
    }

    /// Run the shipped binary as a person would, against this project.
    fn client(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run the glasshouse client")
    }
}

/// A running `glasshouse api serve`, killed on drop.
///
/// `Child::drop` does not kill, and this project has accumulated runaway
/// harness sessions before; the explicit kill is what stops a failed assertion
/// from leaving a pty behind.
struct Serving {
    child: Child,
    socket: PathBuf,
}

impl Serving {
    fn start(door: &Door) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&door.root)
            .arg("--data-dir")
            .arg(door.base.join("data"))
            .arg("--config-dir")
            .arg(door.base.join("config"))
            .arg("api")
            .arg("serve")
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn `glasshouse api serve`");

        let stderr = child.stderr.take().expect("captured stderr");
        let mut reader = BufReader::new(stderr);
        let deadline = Instant::now() + TIMEOUT;
        let socket = loop {
            let mut line = String::new();
            let read = reader.read_line(&mut line).expect("read server stderr");
            assert!(read > 0, "the server exited before announcing its socket");
            if let Some(path) = line
                .trim_end()
                .strip_prefix("glasshouse: control API listening on ")
            {
                break PathBuf::from(path);
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the server to announce its socket"
            );
        };
        Self { child, socket }
    }

    /// One request, one answer — the orchestrator's own path into the door,
    /// speaking the protocol rather than going through the client.
    fn call(&self, request: serde_json::Value) -> serde_json::Value {
        let deadline = Instant::now() + TIMEOUT;
        let mut stream = loop {
            match UnixStream::connect(&self.socket) {
                Ok(stream) => break stream,
                Err(err) => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out connecting to the control socket: {err}"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };
        let mut payload = serde_json::to_string(&request).expect("encode request");
        payload.push('\n');
        stream.write_all(payload.as_bytes()).expect("write request");
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read response");
        serde_json::from_str(line.trim_end()).expect("parse response")
    }

    fn spawn_worker(&self) -> String {
        let response = self.call(serde_json::json!({
            "op": "spawn_session",
            "harness": "claude-code",
            "role": "worker",
        }));
        assert_eq!(response["status"], "ok", "{response}");
        response["result"]["session"]
            .as_str()
            .expect("a session id")
            .to_owned()
    }

    /// A machine-originated line, exactly as an orchestrator sends one: no
    /// `origin` field at all, which the protocol reads as `machine`.
    fn machine_send(&self, session: &str, text: &str) -> serde_json::Value {
        self.call(serde_json::json!({
            "op": "send_message",
            "session": session,
            "text": text,
        }))
    }
}

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait_for<F: FnMut() -> bool>(what: &str, mut done: F) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if done() {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn error_message(response: &serde_json::Value) -> String {
    response["message"].as_str().unwrap_or_default().to_owned()
}

// ---------------------------------------------------------------------------
// Line 1717 — mute
// ---------------------------------------------------------------------------

/// **Line 1717.** *"Allow the user to prevent a session from receiving
/// orchestrator-generated messages temporarily."*
///
/// Six steps, and the order is chosen so that each one is observable without
/// waiting for anything:
///
/// 1. muted, and a machine message is refused **with the remaining time**;
/// 2. an interrupt still reaches the worker — asserted from the worker's own
///    `trap`, so it is a real `SIGINT` and not the door's opinion of one;
/// 3. unmuting reports that it lifted something;
/// 4. a machine message now **arrives**, which is the delivery a different
///    error message could not have proven;
/// 5. muted again, a *person's* message still arrives — a mute is about the
///    orchestrator and never about them;
/// 6. unmute is idempotent and says which case it was.
#[test]
fn a_muted_session_refuses_machine_messages_but_not_interrupts() {
    let door = Door::new();
    let server = Serving::start(&door);
    let worker = server.spawn_worker();
    wait_for("the worker's harness to start", || {
        door.argv(&worker).is_some()
    });

    let muted = server.call(serde_json::json!({
        "op": "mute_session",
        "session": worker,
        "seconds": 600,
    }));
    assert_eq!(muted["status"], "ok", "{muted}");
    assert_eq!(
        muted["result"]["muted_for_seconds"], 600,
        "the door must say what it granted: {muted}"
    );

    // 1. The refusal, and what it has to contain.
    let refused = server.machine_send(&worker, "orchestrator-line-one");
    assert_eq!(
        refused["status"], "error",
        "a muted session must refuse an orchestrator's message: {refused}"
    );
    let message = error_message(&refused);
    assert!(
        message.contains("muted"),
        "the refusal must say the session is muted: {message}"
    );
    assert!(
        message.contains('s') && message.contains("another"),
        "and it must name the remaining time, because the caller's next question is when to \
         try again: {message}"
    );
    assert!(
        !message.contains("orchestrator-line-one"),
        "the refusal must not carry the text it refused: {message}"
    );
    assert!(
        door.received(&worker)
            .is_none_or(|read| !read.contains("orchestrator-line-one")),
        "and nothing reached the worker"
    );

    // 2. An interrupt is never muted.
    let interrupted = server.call(serde_json::json!({
        "op": "interrupt",
        "session": worker,
    }));
    assert_eq!(
        interrupted["status"], "ok",
        "a mute must never make a runaway worker unstoppable: {interrupted}"
    );
    wait_for("the worker to handle a real SIGINT", || {
        door.reacted_to_interrupt(&worker)
    });

    // 3 and 4. Unmuting lifts it, and the next machine message is delivered.
    let lifted = server.call(serde_json::json!({
        "op": "unmute_session",
        "session": worker,
    }));
    assert_eq!(lifted["status"], "ok", "{lifted}");
    assert_eq!(
        lifted["result"]["was_muted"], true,
        "unmute must report that it lifted something: {lifted}"
    );

    let delivered = server.machine_send(&worker, "orchestrator-line-two");
    assert_eq!(
        delivered["status"], "ok",
        "an unmuted session takes orchestrator messages again: {delivered}"
    );
    wait_for("the worker to read the orchestrator's line", || {
        door.received(&worker)
            .is_some_and(|read| read.contains("orchestrator-line-two"))
    });

    // 5. A person is never muted.
    let again = server.call(serde_json::json!({
        "op": "mute_session",
        "session": worker,
        "seconds": 600,
    }));
    assert_eq!(again["status"], "ok", "{again}");
    let typed = door.client(&[
        "api",
        "send",
        "--session",
        &worker,
        "--text",
        "typed-by-the-person",
    ]);
    assert!(
        typed.status.success(),
        "a mute is about the orchestrator and never about the person who set it: {}",
        String::from_utf8_lossy(&typed.stderr)
    );
    wait_for("the worker to read the person's line", || {
        door.received(&worker)
            .is_some_and(|read| read.contains("typed-by-the-person"))
    });

    // 6. Idempotent, and it says which case it was.
    let first = server.call(serde_json::json!({
        "op": "unmute_session",
        "session": worker,
    }));
    assert_eq!(first["result"]["was_muted"], true, "{first}");
    let second = server.call(serde_json::json!({
        "op": "unmute_session",
        "session": worker,
    }));
    assert_eq!(
        second["status"], "ok",
        "unmuting a session nobody muted is the state the caller asked for: {second}"
    );
    assert_eq!(
        second["result"]["was_muted"], false,
        "and it must say which case it was: {second}"
    );
}

/// **Line 1717's word *temporarily*, as a fact rather than a promise.**
///
/// A mute that never ended would be a session quietly out of the
/// orchestrator's reach for the life of the door, so the expiry is the half of
/// this line that a mute/unmute pair cannot prove. One second, because that is
/// what makes the wait a second rather than the twelve hours the ceiling
/// allows.
///
/// A zero-second mute is refused in the same test: an expiry that has already
/// happened is not a mute, and *"ok"* for a request that did nothing is the
/// shape this project keeps paying for.
#[test]
fn a_mute_expires_on_its_own() {
    let door = Door::new();
    let server = Serving::start(&door);
    let worker = server.spawn_worker();
    wait_for("the worker's harness to start", || {
        door.argv(&worker).is_some()
    });

    let zero = server.call(serde_json::json!({
        "op": "mute_session",
        "session": worker,
        "seconds": 0,
    }));
    assert_eq!(
        zero["status"], "error",
        "a mute with no duration must be refused rather than answered `ok` and forgotten: {zero}"
    );

    let muted = server.call(serde_json::json!({
        "op": "mute_session",
        "session": worker,
        "seconds": 1,
    }));
    assert_eq!(muted["status"], "ok", "{muted}");
    assert_eq!(
        server.machine_send(&worker, "too-early")["status"],
        "error",
        "the mute is in force immediately"
    );

    // The one wait in this file that is a wait rather than a condition: there
    // is no observable event for "a duration elapsed", and the door offers no
    // way to ask how long is left without also being the thing under test.
    // One second is the whole cost.
    wait_for("the mute to expire", || {
        server.machine_send(&worker, "after-expiry")["status"] == "ok"
    });
    wait_for(
        "the worker to read the message the expiry let through",
        || {
            door.received(&worker)
                .is_some_and(|read| read.contains("after-expiry"))
        },
    );
    assert!(
        door.received(&worker)
            .is_some_and(|read| !read.contains("too-early")),
        "and the message refused while it was muted never arrived late"
    );
}

// ---------------------------------------------------------------------------
// Line 1719 — a person outranks a machine
// ---------------------------------------------------------------------------

/// **Line 1719.** *"Make user input take precedence over automated
/// orchestration when both target the same session."*
///
/// A person types into a worker through the shipped client. An orchestrator's
/// message into the same session, immediately afterwards, is refused and told
/// why — and the worker never reads it, which is the half a refused status
/// code alone would not prove.
///
/// The interrupt at the end is the boundary of the rule: what a person is
/// protected from is being *talked over*, and a stop is not talking.
///
/// The window's **expiry** is proven where it can be proven without waiting
/// for it: `session::api`'s own
/// `a_machine_message_is_delivered_once_the_persons_window_has_passed`, which
/// hands `SessionRuntime::note_user_input` a moment in the past — the same
/// call the binary makes, with the clock as its argument.
#[test]
fn a_persons_keystroke_outranks_a_machine_message_to_the_same_session() {
    let door = Door::new();
    let server = Serving::start(&door);
    let worker = server.spawn_worker();
    wait_for("the worker's harness to start", || {
        door.argv(&worker).is_some()
    });

    // Before anybody has typed, an orchestrator's message is delivered
    // normally. Asserted first so the refusal below is a fact about the
    // person's keystroke rather than about this door refusing everything.
    let before = server.machine_send(&worker, "orchestrator-before");
    assert_eq!(before["status"], "ok", "{before}");
    wait_for("the worker to read the orchestrator's first line", || {
        door.received(&worker)
            .is_some_and(|read| read.contains("orchestrator-before"))
    });

    let typed = door.client(&[
        "api",
        "send",
        "--session",
        &worker,
        "--text",
        "the-person-is-typing",
    ]);
    assert!(
        typed.status.success(),
        "`glasshouse api send` failed: {}",
        String::from_utf8_lossy(&typed.stderr)
    );
    wait_for("the worker to read the person's line", || {
        door.received(&worker)
            .is_some_and(|read| read.contains("the-person-is-typing"))
    });

    let refused = server.machine_send(&worker, "orchestrator-interrupting");
    assert_eq!(
        refused["status"], "error",
        "a machine message into a session a person is using must be refused: {refused}"
    );
    let message = error_message(&refused);
    assert!(
        message.contains("keyboard"),
        "and the refusal must say whose the session is right now: {message}"
    );
    assert!(
        !message.contains("orchestrator-interrupting"),
        "the refusal must not carry the text it refused: {message}"
    );
    assert!(
        door.received(&worker)
            .is_some_and(|read| !read.contains("orchestrator-interrupting")),
        "and — the half a status code cannot prove — the worker never read it: {:?}",
        door.received(&worker)
    );

    // A stop is not talking, so it is not held back.
    let interrupted = server.call(serde_json::json!({
        "op": "interrupt",
        "session": worker,
    }));
    assert_eq!(
        interrupted["status"], "ok",
        "an orchestrator must still be able to stop a worker a person is typing into: \
         {interrupted}"
    );
    wait_for("the worker to handle a real SIGINT", || {
        door.reacted_to_interrupt(&worker)
    });
}

// ---------------------------------------------------------------------------
// Line 1718 — taking a worker over
// ---------------------------------------------------------------------------

/// **Line 1718.** *"Allow the user to take over an orchestrated worker
/// directly."*
///
/// # Why this is a test and not a refusal
///
/// Line 745's three verbs — `api send`, `api interrupt`, `api read` — already
/// put a person *inside* a running worker without an orchestrator between
/// them. What they could not do until this package is get the orchestrator
/// *out*: a person typing into a worker an orchestrator was driving was
/// racing it, and nothing anywhere gave them the session. Lines 1717 and 1719
/// are the two halves that close that, and "take over" is what the two
/// together mean.
///
/// So this drives the whole thing as a person would: an orchestrator is
/// driving a worker, the person takes it, works in it, reads it back, and the
/// orchestrator is refused for as long as the person holds it — then hands it
/// back and the orchestrator resumes.
#[test]
fn a_person_takes_over_an_orchestrated_worker_and_the_orchestrator_is_locked_out() {
    let door = Door::new();
    let server = Serving::start(&door);
    let worker = server.spawn_worker();
    wait_for("the worker's harness to start", || {
        door.argv(&worker).is_some()
    });

    // The orchestrator is driving it.
    assert_eq!(
        server.machine_send(&worker, "orchestrator-task")["status"],
        "ok"
    );
    wait_for("the worker to read the orchestrator's task", || {
        door.received(&worker)
            .is_some_and(|read| read.contains("orchestrator-task"))
    });

    // The person takes it: quiet the orchestrator, then work in it.
    let muted = door.client(&["api", "mute", "--session", &worker, "--for", "600"]);
    assert!(
        muted.status.success(),
        "`glasshouse api mute` failed: {}",
        String::from_utf8_lossy(&muted.stderr)
    );

    let typed = door.client(&[
        "api",
        "send",
        "--session",
        &worker,
        "--text",
        "taken-over-by-the-person",
    ]);
    assert!(
        typed.status.success(),
        "`glasshouse api send` failed: {}",
        String::from_utf8_lossy(&typed.stderr)
    );
    wait_for("the worker to read the person's line", || {
        door.received(&worker)
            .is_some_and(|read| read.contains("taken-over-by-the-person"))
    });

    // They can see what came back, which is what makes it *being in* the
    // worker rather than shouting into it.
    wait_for("the worker's echo to reach the person's read", || {
        let read = door.client(&["api", "read", "--session", &worker]);
        read.status.success()
            && String::from_utf8_lossy(&read.stdout).contains("got:taken-over-by-the-person")
    });

    // And the orchestrator is out, for two independent reasons — either alone
    // would do, and the point of the takeover is that both hold.
    let shut_out = server.machine_send(&worker, "orchestrator-still-trying");
    assert_eq!(
        shut_out["status"], "error",
        "the orchestrator must not be able to talk into a worker a person has taken: {shut_out}"
    );
    // **And it must be the mute that shut it out.** Without this the test
    // passes on a build where `glasshouse api mute` does nothing at all: the
    // person typed a moment ago, so line 1719 would refuse the orchestrator
    // anyway and the assertion above could not tell the two apart. The
    // mutation `1718-the-person-cannot-quiet-the-orchestrator` SURVIVED until
    // this line existed.
    assert!(
        error_message(&shut_out).contains("muted"),
        "the reason must be the mute the person set, not the keystroke that happened to \
         follow it — otherwise this test cannot tell a working `api mute` from one that does \
         nothing: {}",
        error_message(&shut_out)
    );
    assert!(
        door.received(&worker)
            .is_some_and(|read| !read.contains("orchestrator-still-trying")),
        "and nothing of its reached the worker"
    );

    // Handing it back. The mute is lifted explicitly; the keyboard window is
    // the reason the orchestrator's message is still refused for a moment
    // after, which the refusal itself now says.
    let released = door.client(&["api", "unmute", "--session", &worker]);
    assert!(
        released.status.success(),
        "`glasshouse api unmute` failed: {}",
        String::from_utf8_lossy(&released.stderr)
    );
    let still_held = server.machine_send(&worker, "orchestrator-too-soon");
    assert_eq!(
        still_held["status"], "error",
        "the person typed a moment ago, so line 1719 still holds the session: {still_held}"
    );
    assert!(
        error_message(&still_held).contains("keyboard"),
        "and the reason must now be the keyboard rather than the mute — two controls, two \
         sentences, and a person handing a worker back can tell which one is still in force: {}",
        error_message(&still_held)
    );
}
