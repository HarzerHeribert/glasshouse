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
// Only the `cfg(unix)` resume-through-a-real-terminal cluster below uses these,
// so on Windows they are unused imports and `-D warnings` refuses them.
#[cfg(unix)]
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

/// Phase 42 — the external control API, against the shipped binary.
///
/// Every capability the socket door claims is proven the same way this
/// file proves everything else: run `glasshouse` for real and observe what
/// actually happened, never by calling the door's handlers in-process. That
/// is not a stylistic choice here — `mod api` is declared from `main.rs`,
/// so nothing outside the binary can reach it any other way (see that
/// module's own doc comment for why).
#[cfg(unix)]
mod control_api {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    const TIMEOUT: Duration = Duration::from_secs(15);

    /// A project with an installed harness that echoes every line it reads,
    /// forever — alive long enough for a send and, separately, for an
    /// interrupt to have something real to land on. Every line it receives
    /// is also appended to `received.log` in the project root, which is how
    /// these tests observe a machine-sent line without the control API
    /// itself exposing a way to read scrollback — box 4 is "send", not
    /// "read back", and this fixture proves the send with a side channel
    /// the API never touches instead of manufacturing a capability nobody
    /// asked for.
    struct ApiFixture {
        _tmp: tempfile::TempDir,
        base: PathBuf,
    }

    impl ApiFixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().expect("tempdir");
            let base = tmp.path().to_path_buf();

            let bin_dir = base.join("bin");
            std::fs::create_dir_all(&bin_dir).expect("create bin dir");
            let harness = install_looping_echo_harness(&bin_dir);

            let config_dir = base.join("config");
            std::fs::create_dir_all(&config_dir).expect("create config dir");
            let escaped = harness.display().to_string().replace('\\', "\\\\");
            std::fs::write(
                config_dir.join("config.toml"),
                format!(
                    "version = 1\n\n[integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n"
                ),
            )
            .expect("write user config");

            Self { _tmp: tmp, base }
        }

        /// A project root under this fixture's shared data/config roots,
        /// created fresh. Two calls make two projects sharing one machine,
        /// exactly as two real projects would.
        fn project_root(&self, name: &str) -> PathBuf {
            let root = self.base.join("workspace").join(name);
            std::fs::create_dir_all(root.join(".git")).expect("create project root");
            std::fs::canonicalize(&root).expect("canonicalize project root")
        }

        fn received_log(&self, root: &Path) -> PathBuf {
            root.join("received.log")
        }
    }

    #[cfg(unix)]
    fn install_looping_echo_harness(bin_dir: &Path) -> PathBuf {
        let path = bin_dir.join("looping-echo-harness");
        std::fs::write(
            &path,
            "#!/bin/sh\n\
             echo READY\n\
             echo $$ > \"$PWD/pid\"\n\
             touch \"$PWD/ready\"\n\
             while IFS= read -r line; do\n\
             echo \"$line\" >> \"$PWD/received.log\"\n\
             echo \"got:$line\"\n\
             done\n",
        )
        .expect("write looping echo harness");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    /// A running `glasshouse api serve`, killed on drop so a failing
    /// assertion never leaks a process holding a real pty open.
    struct Server {
        child: Child,
        socket: PathBuf,
    }

    impl Server {
        fn start(fixture: &ApiFixture, root: &Path) -> Self {
            let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
                .arg("--scope")
                .arg(root)
                .arg("--data-dir")
                .arg(fixture.base.join("data"))
                .arg("--config-dir")
                .arg(fixture.base.join("config"))
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

        /// Send one request, and return its parsed response.
        ///
        /// A fresh connection per call, exactly as the protocol document
        /// (`src/api/protocol.rs`) says: one request, one response, then the
        /// connection closes.
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

        /// Send raw bytes instead of a well-formed request, to prove a
        /// malformed line gets a clean refusal rather than killing the
        /// connection or the server.
        fn call_raw(&self, bytes: &[u8]) -> String {
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
            stream.write_all(bytes).expect("write raw request");
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).expect("read response");
            line
        }
    }

    impl Drop for Server {
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

    /// Boxes 2 ("list"), 3 ("spawn"), 4 ("send"), and 6 ("lifecycle state"),
    /// end to end through the socket.
    #[test]
    fn spawning_listing_messaging_and_reading_state_go_through_the_socket() {
        let fixture = ApiFixture::new();
        let root = fixture.project_root("alpha");
        let server = Server::start(&fixture, &root);

        let spawned =
            server.call(serde_json::json!({"op": "spawn_session", "harness": "claude-code"}));
        let session = spawned["result"]["session"]
            .as_str()
            .unwrap_or_else(|| panic!("expected a spawned session id: {spawned}"))
            .to_owned();

        let listed = server.call(serde_json::json!({"op": "list_sessions"}));
        let ids: Vec<String> = listed["result"]
            .as_array()
            .expect("a session list")
            .iter()
            .map(|entry| entry["session"].as_str().unwrap().to_owned())
            .collect();
        assert!(
            ids.contains(&session),
            "the spawned session must appear in the listing: {listed}"
        );

        server.call(serde_json::json!({
            "op": "send_message",
            "session": session,
            "text": "hello-from-the-api",
        }));

        let received_log = fixture.received_log(&root);
        wait_for("the harness to record the sent line", || {
            std::fs::read_to_string(&received_log)
                .map(|text| text.contains("hello-from-the-api"))
                .unwrap_or(false)
        });

        let state = server.call(serde_json::json!({"op": "session_state", "session": session}));
        let lifecycle = state["result"]["lifecycle"].as_str().unwrap();
        assert!(
            matches!(lifecycle, "running" | "idle" | "starting"),
            "a session with a live process must not report a terminal lifecycle: {state}"
        );
    }

    /// Box 5: interrupt reaches a real, still-running process, not just an
    /// event-log entry.
    #[test]
    fn interrupting_through_the_socket_kills_a_real_process() {
        let fixture = ApiFixture::new();
        let root = fixture.project_root("alpha");
        let server = Server::start(&fixture, &root);

        let spawned =
            server.call(serde_json::json!({"op": "spawn_session", "harness": "claude-code"}));
        let session = spawned["result"]["session"].as_str().unwrap().to_owned();

        // Wait for the harness's own readiness marker before interrupting,
        // the same discipline `session/api.rs`'s own interrupt test uses: an
        // interrupt delivered before the child is really running would
        // "succeed" against nothing. The control API exposes lifecycle as
        // Glasshouse recorded it, not raw process liveness — this fixture's
        // harness never calls `glasshouse hook`, so the store's lifecycle
        // never leaves `starting`; the marker file is this test's own,
        // out-of-band proof that the real child is past its first line.
        let ready_marker = root.join("ready");
        wait_for("the harness to be ready", || ready_marker.exists());
        let pid: i32 = std::fs::read_to_string(root.join("pid"))
            .expect("read the harness's own pid")
            .trim()
            .parse()
            .expect("a pid is an integer");

        server.call(serde_json::json!({"op": "interrupt", "session": session}));

        // The real proof is the operating system's, not Glasshouse's own
        // bookkeeping: `kill -0` only succeeds while a process with this
        // pid still exists. A dead-at-handshake child, or an interrupt that
        // only wrote an event-log entry, could never make this pass.
        wait_for("the interrupted process to actually exit", || {
            let probe = Command::new("kill")
                .arg("-0")
                .arg(pid.to_string())
                .output()
                .expect("run kill -0");
            !probe.status.success()
        });
    }

    /// Box 13: a session in one project must never appear in another
    /// project's listing, even though both share this machine's data and
    /// config roots and both doors are open at once.
    #[test]
    fn a_session_never_crosses_into_another_projects_listing() {
        let fixture = ApiFixture::new();
        let alpha_root = fixture.project_root("alpha");
        let beta_root = fixture.project_root("beta");

        let alpha = Server::start(&fixture, &alpha_root);
        let beta = Server::start(&fixture, &beta_root);

        let spawned =
            alpha.call(serde_json::json!({"op": "spawn_session", "harness": "claude-code"}));
        let session = spawned["result"]["session"].as_str().unwrap().to_owned();

        let alpha_listing = alpha.call(serde_json::json!({"op": "list_sessions"}));
        let alpha_ids: Vec<String> = alpha_listing["result"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["session"].as_str().unwrap().to_owned())
            .collect();
        assert!(alpha_ids.contains(&session));

        let beta_listing = beta.call(serde_json::json!({"op": "list_sessions"}));
        let beta_ids: Vec<String> = beta_listing["result"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["session"].as_str().unwrap().to_owned())
            .collect();
        assert!(
            !beta_ids.contains(&session),
            "a project's socket must never list another project's session: {beta_listing}"
        );

        // And the other project's door refuses to act on it by name, the
        // same `ForeignProject` refusal `SessionApi` gives everywhere else.
        let foreign_state =
            beta.call(serde_json::json!({"op": "session_state", "session": session}));
        assert_eq!(foreign_state["status"], "error");
    }

    /// Box 12's filesystem half: the socket is owner-only the moment it
    /// exists, not eventually.
    #[test]
    fn the_control_socket_is_owner_only() {
        let fixture = ApiFixture::new();
        let root = fixture.project_root("alpha");
        let server = Server::start(&fixture, &root);

        let mode = std::fs::metadata(&server.socket)
            .expect("stat the control socket")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "the control socket must be readable and writable only by its owner"
        );
    }

    /// A malformed request must get a clean, typed refusal — and the server
    /// must still be serving afterwards, proving one bad connection cannot
    /// wedge or kill the accept loop.
    #[test]
    fn a_malformed_request_is_refused_and_the_server_keeps_serving() {
        let fixture = ApiFixture::new();
        let root = fixture.project_root("alpha");
        let server = Server::start(&fixture, &root);

        let raw = server.call_raw(b"not json at all\n");
        let parsed: serde_json::Value = serde_json::from_str(raw.trim_end())
            .unwrap_or_else(|_| panic!("even the refusal must be well-formed JSON: {raw:?}"));
        assert_eq!(parsed["status"], "error");

        // The server must still answer a well-formed request on a new
        // connection.
        let listed = server.call(serde_json::json!({"op": "list_sessions"}));
        assert_eq!(listed["status"], "ok");
    }

    /// Boxes 10 ("query memory") and 11 ("request a checkpoint"): both reach
    /// the same durable store `glasshouse memory search` and `glasshouse
    /// checkpoint show` read, proven by writing through the socket and
    /// reading back through the CLI.
    #[test]
    fn memory_query_and_checkpoint_reach_the_same_store_the_cli_reads() {
        let fixture = ApiFixture::new();
        let root = fixture.project_root("alpha");
        let server = Server::start(&fixture, &root);

        let spawned =
            server.call(serde_json::json!({"op": "spawn_session", "harness": "claude-code"}));
        let session = spawned["result"]["session"].as_str().unwrap().to_owned();

        let empty_search = server.call(serde_json::json!({
            "op": "query_memory",
            "query": "nothing-should-match-this",
        }));
        assert_eq!(empty_search["status"], "ok");
        assert!(
            empty_search["result"]["report"]
                .as_str()
                .unwrap()
                .contains("No current memories match"),
            "{empty_search}"
        );

        let checkpointed = server.call(serde_json::json!({
            "op": "take_checkpoint",
            "session": session,
            "objective": "prove the checkpoint box end to end",
            "state": "spawned a session through the socket and checkpointed it",
        }));
        assert_eq!(checkpointed["status"], "ok", "{checkpointed}");
        let checkpoint_id = checkpointed["result"]["checkpoint"]
            .as_str()
            .unwrap()
            .to_owned();

        drop(server);

        let shown = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&root)
            .arg("--data-dir")
            .arg(fixture.base.join("data"))
            .arg("--config-dir")
            .arg(fixture.base.join("config"))
            .arg("checkpoint")
            .arg("show")
            .arg(&checkpoint_id)
            .arg("--document")
            .output()
            .expect("run `glasshouse checkpoint show`");
        assert!(
            shown.status.success(),
            "{}",
            String::from_utf8_lossy(&shown.stderr)
        );
        let document = String::from_utf8_lossy(&shown.stdout);
        assert!(
            document.contains("prove the checkpoint box end to end"),
            "the checkpoint taken through the socket must be the one the CLI reads back: {document}"
        );
    }
}
