//! Phase 10 — the unified session model, against the shipped binary.
//!
//! Everything here runs `glasshouse` as a real process, because that is what
//! the capability lines are about. A session record is written by one process
//! and read by another; a rename is something a person types; and *"close the
//! record without deleting the native provider history"* is a claim about
//! what is on the filesystem after the command returns, which no in-process
//! test can make.
//!
//! The store's own unit tests cover what only the store can answer — the
//! seven columns, the labels, migration 8. These cover the production path:
//! that `glasshouse launch` really records the seven facts, that
//! `glasshouse sessions` and its subcommands really show and change them, and
//! that a session belongs to exactly one project no matter which project asks.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;
use glasshouse::pty::{PtyProcess, TerminalCommand};
use glasshouse::session::{ProjectSessions, SessionId};
use glasshouse::{Cli, Runtime};

/// A project with its own data and config roots and a fake installed harness.
struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    /// Where the fake harness writes what stands in for its own session
    /// history — a directory Glasshouse has never heard of, which is the
    /// point.
    harness_history: PathBuf,
}

impl Fixture {
    fn new(profiles: &str) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let bin_dir = base.join("bin");
        let harness_history = base.join("harness-history");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        std::fs::create_dir_all(&harness_history).expect("create harness history dir");
        let harness = install_fake_harness(&bin_dir, &harness_history);

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        let escaped = harness.display().to_string().replace('\\', "\\\\");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
                 {profiles}"
            ),
        )
        .expect("write user config");

        Self {
            _tmp: tmp,
            base,
            root,
            harness_history,
        }
    }

    /// The same fixture pointed at a second project sharing the machine's
    /// data and config roots — which is how two projects really coexist.
    fn sibling_root(&self, name: &str) -> PathBuf {
        let root = self.base.join(name);
        std::fs::create_dir_all(root.join(".git")).expect("create sibling root");
        std::fs::canonicalize(&root).expect("canonicalize sibling root")
    }

    fn glasshouse_in(&self, root: &Path, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .output()
            .expect("the glasshouse binary must be runnable")
    }

    /// Run the binary in this project and return its standard output,
    /// insisting it succeeded.
    fn run(&self, args: &[&str]) -> String {
        let output = self.glasshouse_in(&self.root, args);
        assert!(
            output.status.success(),
            "`glasshouse {}` failed: {}\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout),
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Run the binary expecting a refusal, and return what it said.
    fn refuse(&self, args: &[&str]) -> String {
        let output = self.glasshouse_in(&self.root, args);
        assert!(
            !output.status.success(),
            "`glasshouse {}` was expected to refuse but succeeded:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
        );
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    fn runtime_for(&self, root: &Path) -> Runtime {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            self.base.join("data").to_str().unwrap(),
            "--config-dir",
            self.base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        glasshouse::bootstrap(&cli, root).unwrap()
    }

    /// The one session this project recorded.
    fn only_session(&self) -> SessionId {
        let runtime = self.runtime_for(&self.root);
        let sessions = ProjectSessions::open(&runtime).unwrap();
        let records = sessions.store().list().unwrap();
        assert_eq!(records.len(), 1, "expected exactly one recorded session");
        records[0].id.clone()
    }

    /// Every file under the fake harness's own history directory, by path and
    /// content. What `sessions close` must leave exactly as it found it.
    fn harness_history_snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut out = BTreeMap::new();
        let entries = std::fs::read_dir(&self.harness_history).expect("read harness history");
        for entry in entries {
            let entry = entry.expect("read a harness history entry");
            out.insert(
                entry.path(),
                std::fs::read(entry.path()).expect("read file"),
            );
        }
        out
    }
}

/// A fake installed harness that writes something in its own history
/// directory and exits cleanly.
///
/// It writes rather than merely exiting because line 654 is about what
/// survives: a harness that left nothing behind would make
/// `closing_a_record_leaves_the_harnesss_own_history_on_disk` pass against a
/// build that deleted everything.
#[cfg(unix)]
fn install_fake_harness(bin_dir: &Path, history: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join("fake-claude");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf 'a native transcript\\n' > '{}/transcript.jsonl'\nexit 0\n",
            history.display()
        ),
    )
    .expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_fake_harness(bin_dir: &Path, history: &Path) -> PathBuf {
    let path = bin_dir.join("fake-claude.cmd");
    std::fs::write(
        &path,
        format!(
            "@echo off\r\necho a native transcript> \"{}\\transcript.jsonl\"\r\nexit /b 0\r\n",
            history.display()
        ),
    )
    .expect("write fake harness");
    path
}

/// A profile naming a model, so the pairing question has a real answer rather
/// than the "nothing was assigned" one.
const PROBE_PROFILE: &str = "[profiles.probe]\nharness = \"claude-code\"\nmodel = \"opus\"\n";

/// The environment variable the resume fixture's provider names as its
/// credential — read from this test process's own environment, never set to
/// a real value, and never printed: only its *presence* under a fixed name
/// is what the fake harness records.
#[cfg(unix)]
const RESUME_CREDENTIAL_VAR: &str = "GLASSHOUSE_RESUME_OVERLAY_TEST_KEY";

/// A launch profile backed directly by a configured provider — Claude Code's
/// own environment-variable mechanism, so a resumed session's overlay is
/// observable without a generated configuration file in the way.
#[cfg(unix)]
const RESUME_PROFILE: &str = "[providers.resume-probe]\n\
     template = \"anthropic-compatible\"\n\
     base_url = \"https://resume-probe.example/v1\"\n\
     credential_env = [\"GLASSHOUSE_RESUME_OVERLAY_TEST_KEY\"]\n\n\
     [profiles.probe]\nharness = \"claude-code\"\n\n\
     [profiles.probe.backend]\nkind = \"direct-provider\"\nprovider = \"resume-probe\"\n";

/// A fake `claude-code` that overwrites `dump_path` with every direct-provider
/// environment key it was launched with, on **every** invocation — so a
/// dump read after a *second* invocation can only contain what that second
/// invocation was actually given, never a value left over from the first.
#[cfg(unix)]
fn install_env_dumping_harness(bin_dir: &Path, dump_path: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join("fake-claude");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n\
             : > '{dump}'\n\
             for name in ANTHROPIC_BASE_URL ANTHROPIC_AUTH_TOKEN; do\n\
             eval \"val=\\$$name\"\n\
             if [ -n \"$val\" ]; then echo \"$name=$val\" >> '{dump}'; fi\n\
             done\n\
             exit 0\n",
            dump = dump_path.display()
        ),
    )
    .expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

// Retained but unreachable while the test above is `cfg(unix)`: it is what a
// Windows port would start from, and deleting it would make that port begin
// by rewriting something that already works. `dead_code` rather than removal.
#[cfg(windows)]
#[allow(dead_code)]
fn install_env_dumping_harness(bin_dir: &Path, dump_path: &Path) -> PathBuf {
    let path = bin_dir.join("fake-claude.cmd");
    std::fs::write(
        &path,
        format!(
            "@echo off\r\n\
             break > \"{dump}\"\r\n\
             if defined ANTHROPIC_BASE_URL echo ANTHROPIC_BASE_URL=%ANTHROPIC_BASE_URL%>> \"{dump}\"\r\n\
             if defined ANTHROPIC_AUTH_TOKEN echo ANTHROPIC_AUTH_TOKEN=%ANTHROPIC_AUTH_TOKEN%>> \"{dump}\"\r\n\
             exit /b 0\r\n",
            dump = dump_path.display()
        ),
    )
    .expect("write fake harness");
    path
}

/// A [`Fixture`] whose one harness dumps its direct-provider environment to
/// `dump_path` on every invocation, backed by [`RESUME_PROFILE`].
#[cfg(unix)]
fn resume_fixture(dump_path: &Path) -> Fixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().to_path_buf();
    let root = base.join("workspace");
    std::fs::create_dir_all(root.join(".git")).expect("create project root");
    let root = std::fs::canonicalize(&root).expect("canonicalize project root");

    let bin_dir = base.join("bin");
    let harness_history = base.join("harness-history");
    std::fs::create_dir_all(&bin_dir).expect("create bin dir");
    std::fs::create_dir_all(&harness_history).expect("create harness history dir");
    let harness = install_env_dumping_harness(&bin_dir, dump_path);

    let config_dir = base.join("config");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    let escaped = harness.display().to_string().replace('\\', "\\\\");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n\
             [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
             {RESUME_PROFILE}"
        ),
    )
    .expect("write user config");

    Fixture {
        _tmp: tmp,
        base,
        root,
        harness_history,
    }
}

/// Run `glasshouse resume <session>` against `fixture`'s project through a
/// real pseudo-terminal — `session::attach` needs one, exactly as the harness
/// it is attaching to would need one outside a test.
///
/// The output must be drained on its own thread while waiting, not read only
/// after: `glasshouse resume` writes terminal control sequences (entering and
/// leaving the alternate screen) to the pty as it runs, and if nothing reads
/// them the kernel's pty buffer fills and the write blocks — which blocks the
/// child's exit, which blocks this function's `wait()` forever. This is the
/// same reason `tests/pty_smoke.rs`'s own harness never reads output only
/// after waiting.
#[cfg(unix)]
fn resume_through_a_real_terminal(fixture: &Fixture, session: &str) {
    let command = TerminalCommand::new(env!("CARGO_BIN_EXE_glasshouse"), fixture.root.clone())
        .arg("--scope")
        .arg(fixture.root.clone())
        .arg("--data-dir")
        .arg(fixture.base.join("data"))
        .arg("--config-dir")
        .arg(fixture.base.join("config"))
        .arg("resume")
        .arg(session)
        // The fixture's provider credential is read from this test process's
        // own environment; `TerminalCommand` snapshots exactly that
        // environment for the child (see its own doc comment), so nothing
        // further is needed here beyond `std::env::set_var` having already
        // been called by the caller.
        .size(glasshouse::pty::TerminalSize::new(24, 80));
    let (mut process, mut output) = PtyProcess::spawn(command).expect("spawn `glasshouse resume`");
    let drain = std::thread::spawn(move || {
        use std::io::Read as _;
        let mut sink = Vec::new();
        let _ = output.read_to_end(&mut sink);
        sink
    });
    let status = process.wait().expect("wait for `glasshouse resume`");
    let drained = drain.join().expect("the drain thread must not panic");
    assert!(
        status.success(),
        "`glasshouse resume {session}` reported {status}\n--- output ---\n{}\n--- end ---",
        String::from_utf8_lossy(&drained)
    );
}

/// The value of one labelled line of `glasshouse sessions show`.
fn field<'a>(report: &'a str, label: &str) -> &'a str {
    report
        .lines()
        .find(|line| line.starts_with(label) && line[label.len()..].starts_with(' '))
        .unwrap_or_else(|| panic!("no `{label}` line in:\n{report}"))[label.len()..]
        .trim()
}

/// Lines 641 and 645, through `glasshouse launch`.
///
/// The launch path is the only production writer of these columns, so this
/// enters through it. Deleting the `.with_pairing_class(...)` call — or any
/// of its four siblings — in `main.rs::launch_session` fails here, which is
/// the §35 shape: the mutation is on the *call*, and no fixture in this file
/// records a session by hand.
#[test]
fn a_launched_session_records_seven_facts_and_the_binary_shows_them_apart() {
    let fixture = Fixture::new(PROBE_PROFILE);

    fixture.run(&["launch", "claude-code", "--profile", "probe", "--headless"]);
    let id = fixture.only_session();

    let shown = fixture.run(&["sessions", "show", id.as_str()]);

    assert_eq!(field(&shown, "session"), id.as_str());
    assert_eq!(field(&shown, "harness"), "claude-code");
    assert_eq!(field(&shown, "launch profile"), "probe");
    assert_eq!(field(&shown, "backend resource"), "native");
    assert_eq!(field(&shown, "model"), "opus");
    assert_eq!(field(&shown, "pairing class"), "vendor-native");
    assert_eq!(field(&shown, "protocol"), "anthropic-messages");
    assert_eq!(field(&shown, "response mechanism"), "none");
    let profile = field(&shown, "response profile");
    for axis in [
        "verbosity=",
        "audience=",
        "narration=",
        "evidence=",
        "format=",
    ] {
        assert!(
            profile.contains(axis),
            "the response profile line must name all five axes, got `{profile}`"
        );
    }

    // Seven answers, and no two of them the same word. A build that filled
    // the pairing class in from the launch profile, or the model from the
    // backend resource, prints a duplicate here.
    let facts = [
        field(&shown, "harness"),
        field(&shown, "launch profile"),
        field(&shown, "backend resource"),
        field(&shown, "model"),
        field(&shown, "pairing class"),
        field(&shown, "protocol"),
        profile,
    ];
    let mut seen = std::collections::BTreeSet::new();
    for fact in facts {
        assert!(
            seen.insert(fact),
            "`{fact}` is printed for two different facts:\n{shown}"
        );
    }

    // And the native session identifier is its own answer, separate from the
    // Glasshouse one — line 644.
    let native = field(&shown, "native session");
    assert_ne!(
        native, "-",
        "Claude Code is assigned an identifier up front"
    );
    assert_ne!(native, id.as_str());
}

/// Line 650, typed at the binary.
///
/// The native session identifier is read back out of the record afterwards,
/// not merely reported as unchanged by the command that changed the name.
#[test]
fn renaming_a_session_through_the_binary_leaves_its_native_identifier_alone() {
    let fixture = Fixture::new(PROBE_PROFILE);
    fixture.run(&["launch", "claude-code", "--headless"]);
    let id = fixture.only_session();

    let before = field(
        &fixture.run(&["sessions", "show", id.as_str()]),
        "native session",
    )
    .to_owned();
    assert_ne!(before, "-");

    let said = fixture.run(&["sessions", "rename", id.as_str(), "the auth probe"]);
    assert!(
        said.contains("the auth probe"),
        "the rename must say what the session is now called: {said}"
    );

    let after = fixture.run(&["sessions", "show", id.as_str()]);
    assert_eq!(field(&after, "name"), "the auth probe");
    assert_eq!(
        field(&after, "native session"),
        before,
        "renaming changed the identifier a resume continues from"
    );
    assert_eq!(field(&after, "session"), id.as_str());

    // The listing shows it too, which is what makes the name worth having.
    let listed = fixture.run(&["sessions"]);
    assert!(
        listed.contains("the auth probe"),
        "a named session must be named in the listing:\n{listed}"
    );

    let cleared = fixture.run(&["sessions", "rename", id.as_str(), "--clear"]);
    assert!(cleared.contains("no name"), "{cleared}");
    assert_eq!(
        field(&fixture.run(&["sessions", "show", id.as_str()]), "name"),
        "-"
    );
}

/// Line 651.
#[test]
fn a_session_can_be_tagged_with_a_purpose_and_untagged_again() {
    let fixture = Fixture::new(PROBE_PROFILE);
    fixture.run(&["launch", "claude-code", "--headless"]);
    let id = fixture.only_session();

    fixture.run(&["sessions", "tag", id.as_str(), "auth"]);
    let shown = fixture.run(&["sessions", "show", id.as_str()]);
    assert_eq!(field(&shown, "purpose"), "auth");
    assert_eq!(
        field(&shown, "name"),
        "-",
        "tagging is not naming; they are two columns"
    );
    assert!(fixture.run(&["sessions"]).contains("auth"));

    // A purpose is free text, because the map says "such as".
    fixture.run(&["sessions", "tag", id.as_str(), "release rehearsal"]);
    assert_eq!(
        field(&fixture.run(&["sessions", "show", id.as_str()]), "purpose"),
        "release rehearsal"
    );

    fixture.run(&["sessions", "tag", id.as_str(), "--clear"]);
    assert_eq!(
        field(&fixture.run(&["sessions", "show", id.as_str()]), "purpose"),
        "-"
    );

    // And a purpose that cannot be stored is refused by name rather than
    // truncated into something the user did not type.
    let refused = fixture.refuse(&["sessions", "tag", id.as_str(), &"x".repeat(33)]);
    assert!(refused.contains("32 characters"), "{refused}");
}

/// Line 654, and the half of it that is about the filesystem.
///
/// The assertion is that the harness's own history is *still there*, compared
/// byte for byte against a snapshot taken before the close — not that the
/// command returned without an error.
#[test]
fn closing_a_record_leaves_the_harnesss_own_history_on_disk() {
    let fixture = Fixture::new(PROBE_PROFILE);
    fixture.run(&["launch", "claude-code", "--headless"]);
    let id = fixture.only_session();

    let before = fixture.harness_history_snapshot();
    assert!(
        !before.is_empty(),
        "the fake harness must have written something, or this test proves nothing"
    );

    let said = fixture.run(&["sessions", "close", id.as_str()]);
    assert!(said.contains("did not delete it"), "{said}");

    assert_eq!(
        fixture.harness_history_snapshot(),
        before,
        "closing a Glasshouse session record deleted or changed the harness's own history"
    );

    let shown = fixture.run(&["sessions", "show", id.as_str()]);
    assert_eq!(field(&shown, "lifecycle"), "closed");
    assert_eq!(field(&shown, "state"), "closed");
    assert_ne!(
        field(&shown, "native session"),
        "-",
        "the pointer to that history has to survive too, or the history is \
         kept and unfindable"
    );
    assert!(
        fixture.run(&["sessions"]).contains(&id.as_str()[..12]),
        "a closed record is retired, not removed from the listing"
    );
}

/// Lines 647 and 653: a stopped session with something to resume to is listed
/// as `resumable`, apart from a live one, and closing moves it out of that
/// group without deleting it.
#[test]
fn a_stopped_but_resumable_session_stays_visible_and_separate() {
    let fixture = Fixture::new(PROBE_PROFILE);
    fixture.run(&["launch", "claude-code", "--headless"]);
    let id = fixture.only_session();

    let shown = fixture.run(&["sessions", "show", id.as_str()]);
    assert_eq!(
        field(&shown, "lifecycle"),
        "stopped",
        "the harness exited cleanly"
    );
    assert_eq!(
        field(&shown, "state"),
        "resumable",
        "and it recorded an identifier, so there is something to resume to"
    );

    fixture.run(&["sessions", "close", id.as_str()]);
    assert_eq!(
        field(&fixture.run(&["sessions", "show", id.as_str()]), "state"),
        "closed"
    );

    // Which is also what makes `closed` reachable at all: nothing else in the
    // shipped binary writes that state, so line 647's seventh state exists
    // because line 654's command does.
}

/// Line 652. One session, two projects asking, and only one of them is told
/// anything.
#[test]
fn a_session_belongs_to_exactly_one_project_whichever_project_asks() {
    let fixture = Fixture::new(PROBE_PROFILE);
    fixture.run(&["launch", "claude-code", "--headless"]);
    let id = fixture.only_session();

    let other = fixture.sibling_root("elsewhere");
    let listed = fixture.glasshouse_in(&other, &["sessions"]);
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(
        listed.contains("No sessions recorded"),
        "the other project must not see this project's session:\n{listed}"
    );

    // Not merely filtered out of a shared list: naming the session outright
    // from the other project finds nothing either, and every command that
    // takes one says so.
    for command in [
        vec!["sessions", "show", id.as_str()],
        vec!["sessions", "rename", id.as_str(), "stolen"],
        vec!["sessions", "tag", id.as_str(), "stolen"],
        vec!["sessions", "close", id.as_str()],
        vec!["resume", id.as_str()],
    ] {
        let output = fixture.glasshouse_in(&other, &command);
        assert!(
            !output.status.success(),
            "`{}` from another project must be refused:\n{}",
            command.join(" "),
            String::from_utf8_lossy(&output.stdout)
        );
    }

    // And the record is untouched afterwards — a refusal that had already
    // written something would be worse than no refusal.
    let shown = fixture.run(&["sessions", "show", id.as_str()]);
    assert_eq!(field(&shown, "name"), "-");
    assert_eq!(field(&shown, "purpose"), "-");
    assert_eq!(field(&shown, "lifecycle"), "stopped");
}

/// Line 649. Two of the three presentations are things `glasshouse launch`
/// can produce, and the record says which without being asked twice.
#[test]
fn a_sessions_presentation_is_recorded_as_the_binary_started_it() {
    let fixture = Fixture::new(PROBE_PROFILE);
    fixture.run(&["launch", "claude-code", "--headless"]);
    assert_eq!(
        field(
            &fixture.run(&["sessions", "show", fixture.only_session().as_str()]),
            "presented"
        ),
        "headless"
    );
}

/// Phase 9A line 368's resume half, against the shipped binary: a resumed
/// session must actually run under the launch profile the record names, not
/// merely display it.
///
/// The fake harness overwrites one fixed dump file with its direct-provider
/// environment on **every** invocation. That is what makes the second read
/// meaningful: if `resume_session` applied no overlay, the dump after
/// `glasshouse resume` would be whatever that plain resume gave the harness
/// — nothing at all — never a leftover from the original launch, because the
/// file was truncated in between.
/// **Unix only, and the reason is measured rather than assumed.**
///
/// On the Windows ARM64 VM this test does not fail — it **hangs**, with no
/// output for 24 minutes against roughly one second on macOS and Linux. A gate
/// that hangs reports nothing where a failed assertion reports a defect, so it
/// is gated rather than left to stall every Windows run.
///
/// What is established: it hangs on Windows, and it passes on macOS and Linux.
/// What is *not* established is the precise mechanism, and this comment does not
/// pretend otherwise. The strong suspicion is that driving `glasshouse resume`
/// through a pseudo-terminal on Windows nests two ConPTY consoles: the test owns
/// one, and the resumed Glasshouse is itself the terminal for its harness child.
/// Two facts measured on that VM the same day make that plausible — ConPTY does
/// not start a child until something answers its `ESC[6n`, and a reply written to
/// a Windows session that is not actively reading its input is not echoed back
/// (see `tests/pty_smoke.rs`'s DSR pair, gated for the same family of reason).
///
/// **What this costs, stated plainly:** the resume-overlay path has end-to-end
/// proof on Unix and none on Windows. Establishing it there needs a harness that
/// answers the inner console's startup query, which is real work and is owed.
#[cfg(unix)]
#[test]
fn resuming_a_session_reapplies_its_launch_profiles_overlay() {
    let dump_dir = tempfile::tempdir().expect("tempdir for the env dump");
    let dump_path = dump_dir.path().join("env-dump.txt");
    let fixture = resume_fixture(&dump_path);

    // Safety: this test does not run other tests' code concurrently with
    // this variable set, and nothing else in this process reads it — it
    // exists only to give the fixture's provider a credential to resolve.
    unsafe {
        std::env::set_var(RESUME_CREDENTIAL_VAR, "sk-resume-test-credential");
    }

    fixture.run(&["launch", "claude-code", "--profile", "probe", "--headless"]);
    let id = fixture.only_session();

    let after_launch = std::fs::read_to_string(&dump_path).expect("read the dump after launch");
    assert!(
        after_launch.contains("ANTHROPIC_BASE_URL=https://resume-probe.example/v1"),
        "the original launch must have carried the direct-provider overlay: {after_launch:?}"
    );
    assert!(
        after_launch.contains("ANTHROPIC_AUTH_TOKEN=sk-resume-test-credential"),
        "{after_launch:?}"
    );

    // Truncate, so the next read can only be explained by the *resume*
    // rewriting it — never by the launch above.
    std::fs::write(&dump_path, "").expect("truncate the dump before resuming");

    resume_through_a_real_terminal(&fixture, id.as_str());

    let after_resume = std::fs::read_to_string(&dump_path).expect("read the dump after resume");
    assert!(
        after_resume.contains("ANTHROPIC_BASE_URL=https://resume-probe.example/v1"),
        "a resumed session must carry the same overlay a fresh launch under this profile \
         would — the harness saw: {after_resume:?}"
    );
    assert!(
        after_resume.contains("ANTHROPIC_AUTH_TOKEN=sk-resume-test-credential"),
        "{after_resume:?}"
    );

    unsafe {
        std::env::remove_var(RESUME_CREDENTIAL_VAR);
    }
}

/// Phase 9A line 353's sixth axis, against the shipped binary: a launch
/// profile naming a response preset must actually change the session's
/// response profile, with no `--response-profile` on the command line.
#[test]
fn a_launch_profiles_named_response_preset_is_the_sessions_response_profile() {
    let fixture =
        Fixture::new("[profiles.probe]\nharness = \"claude-code\"\nresponse_preset = \"brief\"\n");
    fixture.run(&["launch", "claude-code", "--profile", "probe", "--headless"]);

    let shown = fixture.run(&["sessions", "show", fixture.only_session().as_str()]);
    let profile = field(&shown, "response profile");
    assert!(
        profile.contains("verbosity=terse")
            && profile.contains("audience=executive")
            && profile.contains("narration=silent")
            && profile.contains("evidence=minimal")
            && profile.contains("format=bullets"),
        "the profile's `brief` preset must be the session's response profile: {profile}"
    );
}

/// The other half of line 353: an explicit `--response-profile` on the
/// command line is a stronger, one-time statement than a profile's standing
/// default, so it wins rather than being silently overridden by it.
#[test]
fn an_explicit_response_profile_flag_overrides_the_launch_profiles_preset() {
    let fixture =
        Fixture::new("[profiles.probe]\nharness = \"claude-code\"\nresponse_preset = \"brief\"\n");
    fixture.run(&[
        "launch",
        "claude-code",
        "--profile",
        "probe",
        "--response-profile",
        "audit",
        "--headless",
    ]);

    let shown = fixture.run(&["sessions", "show", fixture.only_session().as_str()]);
    let profile = field(&shown, "response profile");
    assert!(
        profile.contains("narration=detailed") && profile.contains("evidence=audit"),
        "an explicit `--response-profile` must win over the profile's own preset: {profile}"
    );
}
