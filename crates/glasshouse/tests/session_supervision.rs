//! Phase 10A — session supervision, against real processes.
//!
//! Every claim this phase makes is about a process: whether one is running,
//! whether it is the one that was recorded, whether starting another beside it
//! is allowed. None of that can be proved by constructing a value, so nothing
//! here does. Each test either runs the shipped `glasshouse` binary, kills a
//! real process, or drives a real pseudo-terminal.
//!
//! The scenario the file keeps coming back to is the one that caused the
//! phase: on 2026-08-26 three `glasshouse` processes outlived the pane that
//! started them, and nothing in this project could see them.
//! `an_orphaned_session_is_discovered_and_recorded_as_lost` is that, made to
//! happen on purpose.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use clap::Parser;
use glasshouse::session::supervision::{self, ProcessIdentity};
use glasshouse::session::{ProjectSessions, SessionId, SessionLifecycle};
use glasshouse::{Cli, Runtime};

/// How long a test waits for something a separate process has to do.
const PATIENCE: Duration = Duration::from_secs(20);

// -------------------------------------------------------------------------
// Fixture
// -------------------------------------------------------------------------

/// A project with its own data and config roots and a fake installed harness.
///
/// The harness's body is a parameter because this phase is about how a start
/// goes: one that runs for a while, one that finishes at once, and one that
/// fails are three different things and the difference is the whole point.
struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    harness: PathBuf,
}

impl Fixture {
    fn new(harness_body: &str) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let harness = install_fake_harness(&bin_dir, harness_body);

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        let escaped = harness.display().to_string().replace('\\', "\\\\");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n"
            ),
        )
        .expect("write user config");

        Self {
            _tmp: tmp,
            base,
            root,
            harness,
        }
    }

    /// The installed harness's real path. Several tests below resolve this
    /// executable directly (bypassing `glasshouse launch`'s own resolution)
    /// to build a `HarnessLaunch` by hand — they must ask for the path
    /// `install_fake_harness` actually returned rather than reconstructing
    /// `bin_dir.join("fake-claude")`, because a shared fixture no longer
    /// lives there.
    fn harness_path(&self) -> &Path {
        &self.harness
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args);
        command
    }

    fn output(&self, args: &[&str]) -> std::process::Output {
        self.command(args)
            .output()
            .expect("the glasshouse binary must be runnable")
    }

    /// Run the binary and insist it succeeded.
    fn run(&self, args: &[&str]) -> String {
        let output = self.output(args);
        assert!(
            output.status.success(),
            "`glasshouse {}` failed: {}\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout),
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Run the binary expecting a refusal, and return everything it said.
    fn refuse(&self, args: &[&str]) -> String {
        let output = self.output(args);
        assert!(
            !output.status.success(),
            "`glasshouse {}` was expected to refuse but succeeded:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
        );
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    /// Everything `glasshouse` said, standard error included, whether or not
    /// it succeeded. Supervision surfaces on standard error, so a test about
    /// what the user is told has to read both.
    fn everything(&self, args: &[&str]) -> String {
        let output = self.output(args);
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    fn runtime(&self) -> Runtime {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            self.base.join("data").to_str().unwrap(),
            "--config-dir",
            self.base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        glasshouse::bootstrap(&cli, &self.root).unwrap()
    }

    fn database(&self) -> PathBuf {
        self.runtime().database_path()
    }

    /// Read the session table directly.
    ///
    /// Deliberately *not* through `ProjectSessions::open`, because opening
    /// that runs supervision — which is the thing under test. A test that
    /// looked through it would be reading the answer it had just caused.
    fn raw(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.database()).expect("open the project database")
    }

    fn only_session(&self) -> SessionId {
        let ids = self.session_ids();
        assert_eq!(ids.len(), 1, "expected exactly one recorded session");
        ids.into_iter().next().unwrap()
    }

    fn session_ids(&self) -> Vec<SessionId> {
        let conn = self.raw();
        let mut statement = conn
            .prepare("SELECT id FROM sessions ORDER BY created_at ASC, id ASC")
            .unwrap();
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap();
        rows.map(|id| SessionId::new(id.unwrap())).collect()
    }

    fn row(&self, id: &SessionId) -> Row {
        self.raw()
            .query_row(
                "SELECT lifecycle, process_id, process_started_at, process_host, \
                 supervision, supervision_reason FROM sessions WHERE id = ?1",
                [id.as_str()],
                |row| {
                    Ok(Row {
                        lifecycle: row.get(0)?,
                        process_id: row.get(1)?,
                        process_started_at: row.get(2)?,
                        process_host: row.get(3)?,
                        supervision: row.get(4)?,
                        supervision_reason: row.get(5)?,
                    })
                },
            )
            .expect("the session must be in the table")
    }

    /// Wait until a session's row satisfies something, or fail saying what it
    /// looked like instead. Polling the file rather than sleeping a guessed
    /// interval, so the test is bounded rather than lucky.
    fn wait_for(&self, id: &SessionId, what: &str, ready: impl Fn(&Row) -> bool) -> Row {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let row = self.row(id);
            if ready(&row) {
                return row;
            }
            assert!(
                Instant::now() < deadline,
                "waited {PATIENCE:?} for {what}; the row reads {row:?}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Wait until this project has recorded a session at all.
    fn wait_for_a_session(&self) -> SessionId {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(id) = self.session_ids().into_iter().next() {
                return id;
            }
            assert!(
                Instant::now() < deadline,
                "waited {PATIENCE:?} for a session to be recorded"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Overwrite one supervision column, standing in for something a test
    /// cannot make happen to order.
    ///
    /// Used for exactly one thing: making a *live* process id carry a start
    /// time that is not its own, which is what a reused process id looks like.
    /// Waiting for a real pid to be recycled would take a machine hours and
    /// would still be a coincidence rather than a test.
    fn rewrite(&self, id: &SessionId, column: &str, value: i64) {
        let updated = self
            .raw()
            .execute(
                &format!("UPDATE sessions SET {column} = ?2 WHERE id = ?1"),
                rusqlite::params![id.as_str(), value],
            )
            .expect("rewrite a supervision column");
        assert_eq!(updated, 1, "exactly one row must have been rewritten");
    }
}

impl Fixture {
    /// Run one statement against the project database.
    ///
    /// Used to put a record into a state a test cannot otherwise reach in the
    /// time it has — a start that has been starting for ten minutes is a real
    /// condition and an unreasonable thing to wait for.
    fn execute(&self, sql: &str) {
        let changed = self
            .raw()
            .execute(sql, [])
            .expect("rewrite the session row");
        assert_eq!(changed, 1, "exactly one row must have been rewritten");
    }

    /// Deliver one harness event through a real `glasshouse hook` process.
    ///
    /// A separate process on purpose: that is what a harness spawns, and it is
    /// the process whose own `ProjectSessions::open` supervises the session
    /// the event is about.
    fn hook(&self, session: &SessionId, event: &str) -> std::process::ExitStatus {
        let mut child = self
            .command(&["hook", "--session", session.as_str(), "--event", event])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("the glasshouse binary must be runnable");
        std::io::Write::write_all(
            child.stdin.as_mut().expect("stdin was piped"),
            br#"{"session_id":"native-1","hook_event_name":"Stop","cwd":"/somewhere"}"#,
        )
        .expect("the handler must read its payload rather than closing the pipe");
        child
            .wait_with_output()
            .expect("the hook must finish")
            .status
    }

    /// Seconds since the Unix epoch, as the store's own clock reads it.
    fn now(&self) -> i64 {
        i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
        .unwrap()
    }
}

#[derive(Debug)]
struct Row {
    lifecycle: String,
    process_id: Option<i64>,
    process_started_at: Option<i64>,
    process_host: Option<String>,
    supervision: Option<String>,
    supervision_reason: Option<String>,
}

impl Row {
    fn identity(&self) -> Option<ProcessIdentity> {
        Some(ProcessIdentity {
            pid: u32::try_from(self.process_id?).ok()?,
            started_at_ms: self.process_started_at?,
            host: self.process_host.clone()?,
        })
    }
}

/// Write each distinct fixture executable once per test binary instead of
/// once per test, so macOS Gatekeeper (`syspolicyd`/XProtect) validates it
/// once per `session_supervision` run instead of once per test — see the
/// project memory `gatekeeper-scans-make-pty-fixtures-flaky` and
/// GH-FIXTURE-REUSE. Most of this file's `A_HARNESS_THAT_*` bodies are fixed
/// strings reused across several tests, so most calls collapse onto a
/// handful of files.
///
/// Race-free the way `provider/cache.rs::write_json_atomically` is: one
/// process-wide mutex serialises the check-and-write, and the write itself
/// lands in a same-directory temporary name before an atomic rename.
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

/// A fake installed harness whose body the test chooses.
///
/// A body that names its own script's directory (`dirname "$0"` on Unix,
/// `%~dp0` on Windows) — `A_HARNESS_THAT_WORKS_ONCE_THEN_CRASH_LOOPS` and
/// `A_HARNESS_THAT_FINISHES_THEN_STAYS_UP` — keeps a companion marker file
/// beside itself to tell a session's first launch from its later ones apart.
/// Sharing that script across tests would carry the first test's marker into
/// every later test that asks for the same body, silently starting them
/// already in the "already ran once" state. Those two bodies keep writing a
/// private copy into this test's own `bin_dir`, exactly as before; every
/// other body — which carries no state beside itself — is shared.
#[cfg(unix)]
fn install_fake_harness(bin_dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let script = format!("#!/bin/sh\n{body}\n");
    if body.contains("dirname \"$0\"") {
        let path = bin_dir.join("fake-claude");
        std::fs::write(&path, script).expect("write fake harness");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        return path;
    }
    shared_fixture("fake-claude", &script)
}

#[cfg(windows)]
fn install_fake_harness(bin_dir: &Path, body: &str) -> PathBuf {
    let script = format!("@echo off\r\n{body}\r\n");
    if body.contains("%~dp0") {
        let path = bin_dir.join("fake-claude.cmd");
        std::fs::write(&path, script).expect("write fake harness");
        return path;
    }
    shared_fixture("fake-claude.cmd", &script)
}

#[cfg(test)]
mod shared_fixture_proof {
    use super::install_fake_harness;

    /// **The once-per-binary proof, through the real caller.** Two
    /// independent per-test tempdirs asking for the same stateless body —
    /// the ordinary shape most of this file's tests are in — collapse to one
    /// file rather than each writing its own.
    #[cfg(unix)]
    #[test]
    fn two_tempdirs_requesting_the_same_stateless_body_get_one_shared_file() {
        let tmp_a = tempfile::tempdir().expect("tempdir a");
        let tmp_b = tempfile::tempdir().expect("tempdir b");
        let a = install_fake_harness(tmp_a.path(), "exit 0");
        let meta_before = std::fs::metadata(&a).expect("fixture exists after first install");

        let b = install_fake_harness(tmp_b.path(), "exit 0");
        assert_eq!(
            a, b,
            "two different tempdirs requesting the same body must share one file"
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
            "a second install of the same body must not rewrite the file"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                meta_before.ino(),
                meta_after.ino(),
                "a second install of the same body must return the same inode, \
                 not a second copy"
            );
        }
    }

    /// **Bytes unchanged.** The shared fixture is byte-for-byte the same
    /// script every per-test write used to produce.
    #[cfg(unix)]
    #[test]
    fn the_shared_fixture_has_the_original_unshared_bytes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = install_fake_harness(tmp.path(), "exit 0");
        let content = std::fs::read_to_string(&path).expect("read shared fixture");
        assert_eq!(
            content, "#!/bin/sh\nexit 0\n",
            "the shared fixture's bytes must match the original per-test literal exactly"
        );
    }

    /// **The excluded case stays private.** A body that carries a
    /// `dirname "$0"` companion marker must still get its own file in the
    /// caller's own `bin_dir` — never the shared per-binary directory —
    /// exactly as before this conversion, so two tests using that body never
    /// see each other's marker state.
    #[cfg(unix)]
    #[test]
    fn a_self_referential_body_is_never_shared() {
        let tmp_a = tempfile::tempdir().expect("tempdir a");
        let tmp_b = tempfile::tempdir().expect("tempdir b");
        let body = "marker=\"$(dirname \"$0\")/restart-marker\"\nexit 0";
        let a = install_fake_harness(tmp_a.path(), body);
        let b = install_fake_harness(tmp_b.path(), body);
        assert_ne!(
            a, b,
            "a self-referential body must get its own file per test, never a shared one"
        );
        assert!(a.starts_with(tmp_a.path()));
        assert!(b.starts_with(tmp_b.path()));
    }
}

/// A harness that stays up, so the session it belongs to is really running
/// while the test looks at it.
#[cfg(unix)]
const A_HARNESS_THAT_STAYS_UP: &str = "sleep 60";
#[cfg(windows)]
const A_HARNESS_THAT_STAYS_UP: &str = "ping -n 61 127.0.0.1 > nul";

/// A harness that reads what it is sent and says nothing back.
///
/// It must *read*: a harness that ignores its terminal lets the line
/// discipline's buffer fill, and what is lost then is lost to the kernel
/// rather than to anything Glasshouse did.
///
/// # Why it does not echo, which cost a Linux gate run to learn
///
/// It was `cat`. A harness that writes what it read puts **two independent
/// producers** into the pseudo-terminal's output — the line discipline's echo,
/// emitted as it consumes the input queue, and the harness's own write — and
/// on Linux those interleave: a 56-byte echo is emitted in two pieces with the
/// harness's write-back landing between them. The scrollback then contains a
/// message split in half by another message's text although Glasshouse wrote
/// each of them in one indivisible call. macOS does not do it, so the test
/// passed there and failed the first time it was ever run on Linux.
///
/// With nothing writing back, the only producer is the echo, and the echo of
/// serialized writes is serialized. The scrollback is then exactly the record
/// of what Glasshouse wrote and in what order — which is what line 13 is
/// about, and a stricter record than the round trip was.
#[cfg(unix)]
const A_HARNESS_THAT_READS_WHAT_IT_IS_SENT: &str = "while IFS= read -r line; do :; done";

/// A harness that does its work and exits cleanly, at once.
#[cfg(unix)]
const A_HARNESS_THAT_FINISHES: &str = "exit 0";
#[cfg(windows)]
const A_HARNESS_THAT_FINISHES: &str = "exit /b 0";

/// A harness that comes up, stays up long enough to be called healthy, and
/// then dies badly — every time it is run.
///
/// The shape line 10 exists for: a session that was genuinely working and then
/// exited unexpectedly. `sleep` puts it comfortably past
/// `SessionRuntime::HEALTHY_AFTER` without making the test wait for anything
/// it does not have to.
#[cfg(unix)]
const A_HARNESS_THAT_WORKS_THEN_DIES: &str = "echo UP\nsleep 3\nexit 3";

/// The same, but only the first time. Every later run dies quickly.
///
/// The crash loop line 10's bound is for, and the reason the marker is beside
/// the script rather than in the environment: a restart re-runs the recorded
/// launch exactly, so anything that distinguishes the runs has to survive
/// outside it.
///
/// # Why the loop sleeps rather than exiting instantly
///
/// A harness that dies in microseconds is already dead by the next poll, so a
/// build whose health rule ignored `HEALTHY_AFTER` entirely would still reach
/// the bound and the test could not tell the two apart — the mutation and the
/// test would share the assumption that a crash loop is never *seen* alive
/// (§41). Four hundred milliseconds is comfortably long enough to be observed
/// running and comfortably short of `HEALTHY_AFTER`, so a build that granted
/// health on sight would clear the count every time and never stop.
#[cfg(unix)]
const A_HARNESS_THAT_WORKS_ONCE_THEN_CRASH_LOOPS: &str = concat!(
    "marker=\"$(dirname \"$0\")/restart-marker\"\n",
    "if [ -f \"$marker\" ]; then sleep 0.4; exit 3; fi\n",
    ": > \"$marker\"\n",
    "echo UP\n",
    "sleep 3\n",
    "exit 3",
);

/// A harness that says one thing and then dies badly.
///
/// The same three lines `tests/events_lifecycle.rs` has used since Phase 45
/// closed *"preserve terminal output and event history after a worker
/// crashes"*, and the shape that made this phase's readiness bound
/// platform-divergent — see
/// `a_harness_that_printed_and_died_is_a_failed_session_not_a_refused_start`.
#[cfg(unix)]
const A_HARNESS_THAT_PRINTS_AND_DIES: &str = "echo STARTED\nkill -9 $$";
#[cfg(windows)]
const A_HARNESS_THAT_PRINTS_AND_DIES: &str = "echo STARTED\r\nexit /b 3";

/// A `glasshouse launch --headless` running in the background, and a way to
/// end it that does not depend on the test finishing normally.
struct Background(Option<Child>);

impl Background {
    fn launch(fixture: &Fixture) -> Self {
        let child = fixture
            .command(&["launch", "claude-code", "--headless"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("start a background glasshouse");
        Self(Some(child))
    }

    /// End it the way the incident ended those three: abruptly, with no
    /// chance to record anything.
    fn kill(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        self.kill();
    }
}

// -------------------------------------------------------------------------
// Line 1 — a durable process identity, including the start time.
// -------------------------------------------------------------------------

/// `glasshouse launch` records the process the session was started in, and
/// records a start time beside the process id.
///
/// The start time is the whole line: *"so that a reused process identifier
/// cannot match a stale record."* Two launches are compared because that is
/// the observable consequence — two sessions started in two processes carry
/// two identities, and they are not interchangeable even if the operating
/// system hands out the same number twice.
///
/// Nothing here writes a row by hand. The only production writer of these
/// columns is `SessionStore::create`, and it is entered through the shipped
/// binary.
#[test]
fn a_launched_session_records_the_process_it_was_started_in() {
    let fixture = Fixture::new(A_HARNESS_THAT_FINISHES);

    fixture.run(&["launch", "claude-code", "--headless"]);
    // `--fresh` because Phase 37 line 1593 gave a bare second launch a real
    // choice: a warm session this project already has now outranks starting
    // over, so an unadorned second launch continues the first rather than
    // recording a second identity. This test's subject is unchanged — two
    // sessions started in two `glasshouse` processes carry two identities —
    // and `--fresh` is how a caller asks for exactly that. See
    // `tests/route_command.rs` for the routing behaviour itself.
    fixture.run(&["launch", "claude-code", "--headless", "--fresh"]);

    let ids = fixture.session_ids();
    assert_eq!(ids.len(), 2, "two launches, two sessions");

    let host = supervision::host_name().expect("this platform names its host");
    let mut identities = Vec::new();
    for id in &ids {
        let row = fixture.row(id);
        let identity = row
            .identity()
            .unwrap_or_else(|| panic!("no process identity was recorded: {row:?}"));

        assert_eq!(identity.host, host, "the recording machine must be named");
        assert!(
            identity.pid > 0 && identity.pid != std::process::id(),
            "the recorded process is the `glasshouse` that started the session, \
             not this test: {identity}"
        );
        assert!(
            identity.started_at_ms > 1_600_000_000_000,
            "a start time of {}ms is not a wall clock, so the platform's own unit \
             was recorded instead of a comparable one",
            identity.started_at_ms
        );
        identities.push(identity);
    }

    assert_ne!(
        identities[0], identities[1],
        "two sessions started in two processes must carry two identities; if they \
         did not, a stale record could match a later process"
    );
}

// -------------------------------------------------------------------------
// Lines 2 and 3 — discovery on start, and verification before belief.
// -------------------------------------------------------------------------

/// The 2026-08-26 incident, made to happen on purpose and then found.
///
/// A `glasshouse` holding a running session is killed outright, so it never
/// records the exit — exactly the state those three processes' records were
/// in. The next `glasshouse` to open this project must **discover** the record
/// (line 2), **verify** the recorded process against the machine (line 3), and
/// conclude that it is gone rather than that the session is healthy.
///
/// Before this phase, the record simply stayed `running` forever and no
/// command in the binary would have said otherwise.
#[test]
fn an_orphaned_session_is_discovered_and_recorded_as_lost() {
    let fixture = Fixture::new(A_HARNESS_THAT_STAYS_UP);
    let mut background = Background::launch(&fixture);

    let id = fixture.wait_for_a_session();
    let running = fixture.wait_for(&id, "the session to be running", |row| {
        row.lifecycle == "running" && row.process_id.is_some()
    });
    let identity = running.identity().expect("the launch recorded an identity");

    // While it is genuinely alive, the recorded identity verifies. This is the
    // control: without it, a test asserting "gone" afterwards would pass
    // against a build that could never verify anything.
    let host = supervision::host_name().unwrap();
    assert_eq!(
        supervision::verify(&identity, &host),
        supervision::Verdict::Verified,
        "a live session's recorded process must verify while it is running"
    );

    background.kill();

    // Any command that opens this project's sessions is a "start" for line 2.
    fixture.run(&["sessions"]);

    let after = fixture.row(&id);
    assert_eq!(
        after.supervision.as_deref(),
        Some("lost"),
        "the orphan must be found: {after:?}"
    );
    assert_eq!(
        after.lifecycle, "stopped",
        "a session whose process is provably gone is not still running: {after:?}"
    );
    let reason = after.supervision_reason.expect("a stated reason");
    assert!(
        reason.contains(&identity.pid.to_string()),
        "the reason must name the process it looked for: {reason}"
    );
}

// -------------------------------------------------------------------------
// Lines 4 and 5 — adopt what verifies; refuse a second beside it.
// -------------------------------------------------------------------------

/// A second `glasshouse` finds a session whose process it can verify, adopts
/// it rather than starting another, and refuses when asked to start one
/// anyway.
///
/// Both halves are the same fact seen twice: line 4 is what happens without
/// being asked, line 5 is what happens when somebody asks. The refusal names
/// the process, because *"it is already running"* with no way to see what is
/// running is what left three processes unaccounted for in the first place.
#[test]
fn a_verified_live_session_is_adopted_and_a_second_is_refused() {
    let fixture = Fixture::new(A_HARNESS_THAT_STAYS_UP);
    let mut background = Background::launch(&fixture);

    let id = fixture.wait_for_a_session();
    let running = fixture.wait_for(&id, "the session to be running", |row| {
        row.lifecycle == "running" && row.process_id.is_some()
    });
    let identity = running.identity().expect("an identity was recorded");

    // A second, separate `glasshouse` process opens the same project.
    fixture.run(&["sessions"]);

    let after = fixture.row(&id);
    assert_eq!(
        after.supervision.as_deref(),
        Some("adopted"),
        "a verified live session is adopted, not replaced: {after:?}"
    );
    assert_eq!(
        after.lifecycle, "running",
        "adoption changes who is watching, never what the session is doing"
    );

    // And asking for a second session on that record is refused, by name.
    let refusal = fixture.refuse(&["resume", id.as_str()]);
    assert!(
        refusal.contains("refusing to start a second session beside it"),
        "the refusal must be the duplicate refusal's own sentence, not merely \
         some other complaint about a running session: {refusal}"
    );
    assert!(
        refusal.contains(&identity.pid.to_string()),
        "and it must say which process is already running: {refusal}"
    );

    background.kill();
}

/// The other half of line 5, inside one Glasshouse.
///
/// A runtime that already holds a live session under an identifier must refuse
/// a second, and the duplicate here is not hypothetical: two `LiveSession`s
/// under one identifier would give `get`, `focus` and `poll_exits` whichever
/// the vector reached first, and one of the two real processes would then be
/// steerable by nobody. Verification is free at this range — the runtime holds
/// the process — so the refusal is unconditional rather than a check.
#[cfg(unix)]
#[test]
fn a_runtime_refuses_to_start_a_second_session_under_one_identifier() {
    use glasshouse::launch::HarnessLaunch;
    use glasshouse::session::SessionPresentation;
    use glasshouse::session::SessionRuntime;

    let fixture = Fixture::new(A_HARNESS_THAT_STAYS_UP);
    let runtime = fixture.runtime();
    let executable = glasshouse::platform::exec::resolve_explicit(fixture.harness_path())
        .expect("the fake harness resolves");
    let launch = HarnessLaunch::new(executable, runtime.project());

    let mut live = SessionRuntime::new();
    let id = SessionId::new("aaaaaaaabbbbbbbbccccccccdddddddd");
    live.start(id.clone(), SessionPresentation::Headless, &launch)
        .expect("the first session starts");

    let refused = live
        .start(id.clone(), SessionPresentation::Headless, &launch)
        .expect_err("a second session under the same identifier must be refused");
    let said = format!("{refused:#}");
    assert!(
        said.contains("second session"),
        "the refusal must say what it is refusing: {said}"
    );
    assert_eq!(live.len(), 1, "and no second session may have been kept");

    live.close(&id).expect("close the session");
}

// -------------------------------------------------------------------------
// Lines 6, 7 and 8 — quarantine, refuse a replacement, and say so.
// -------------------------------------------------------------------------

/// A recorded session whose process id is alive but is **not** the process
/// that was recorded.
///
/// This is the condition the phase's second architectural requirement exists
/// for: alive and no longer owned, which is neither stopped nor healthy. It
/// must be quarantined (line 6), it must block a replacement while it still
/// holds the session's resources (line 7), and the user must be told what is
/// known and what is held (line 8).
///
/// The reuse is arranged by rewriting the recorded *start time* while the
/// process id stays alive. Waiting for a real process id to be recycled would
/// take hours and would still be a coincidence rather than a test; what is
/// under test is the comparison, and the comparison is given exactly the input
/// a recycled id produces.
#[test]
fn a_process_that_is_alive_and_unaccounted_for_is_quarantined_never_replaced() {
    let fixture = Fixture::new(A_HARNESS_THAT_STAYS_UP);
    let mut background = Background::launch(&fixture);

    let id = fixture.wait_for_a_session();
    let running = fixture.wait_for(&id, "the session to be running", |row| {
        row.lifecycle == "running" && row.process_id.is_some()
    });
    let real_start = running.process_started_at.expect("a recorded start time");
    let pid = running.process_id.expect("a recorded process id");

    // The process id stays alive and stays recorded; only the start time now
    // disagrees with it.
    fixture.rewrite(&id, "process_started_at", real_start - 60_000);

    let said = fixture.everything(&["sessions"]);

    let after = fixture.row(&id);
    assert_eq!(
        after.supervision.as_deref(),
        Some("quarantined"),
        "alive and unaccounted for is its own conclusion: {after:?}"
    );
    assert_eq!(
        after.lifecycle, "running",
        "a quarantined session must not be quietly reported as stopped — that is \
         the distinction this phase is about: {after:?}"
    );

    // Line 8: what is known, and what is still held.
    assert!(said.contains("quarantined"), "{said}");
    assert!(
        said.contains(&pid.to_string()),
        "the user must be told which process: {said}"
    );
    assert!(
        said.contains("session directory"),
        "the user must be told what it still holds: {said}"
    );
    assert!(
        said.contains("end it"),
        "the user must be told Glasshouse will not end it for them: {said}"
    );

    // Line 7: no replacement while it still holds them.
    //
    // The assertion is on the *refusal's own sentence*, not on the word
    // "quarantined" appearing somewhere in the output: opening the project
    // also surfaces the quarantine on standard error, so a laxer check here
    // would pass against a build whose refusal had been deleted outright. It
    // did — that mutation survived until this assertion was tightened.
    let refusal = fixture.refuse(&["resume", id.as_str()]);
    assert!(
        refusal.contains("refusing to start a replacement"),
        "the refusal must be about the replacement, and must come from the \
         refusal rather than from the report beside it: {refusal}"
    );
    assert!(
        refusal.contains("still holds"),
        "the refusal must name what the replacement would have collided with: \
         {refusal}"
    );

    background.kill();
}

// -------------------------------------------------------------------------
// Lines 10 and 11 — a bounded restart, and the one thing that resets it.
// -------------------------------------------------------------------------

/// Start one session on a real harness and drive its runtime like the binary
/// does, until `done` or the deadline.
///
/// `poll_exits` is the production entry point for both of these lines — it is
/// what `main.rs::run_headless` and the shell's draw loop call — so the tests
/// go in through it rather than through anything they could have set up
/// themselves.
#[cfg(unix)]
fn drive_until(
    live: &mut glasshouse::session::SessionRuntime,
    what: &str,
    limit: Duration,
    mut done: impl FnMut(&glasshouse::session::SessionRuntime) -> bool,
) {
    let deadline = Instant::now() + limit;
    loop {
        live.poll_exits();
        if done(live) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "waited {limit:?} for {what}; sessions: {live:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Build a runtime with one session on the fixture's harness.
#[cfg(unix)]
fn one_session(
    fixture: &Fixture,
    runtime: &Runtime,
) -> (glasshouse::session::SessionRuntime, SessionId) {
    use glasshouse::launch::HarnessLaunch;
    use glasshouse::session::{SessionPresentation, SessionRuntime};

    let executable = glasshouse::platform::exec::resolve_explicit(fixture.harness_path())
        .expect("the fake harness resolves");
    let launch = HarnessLaunch::new(executable, runtime.project());
    let mut live = SessionRuntime::new();
    let id = SessionId::new("aaaaaaaabbbbbbbbccccccccdddddddd");
    live.start(id.clone(), SessionPresentation::Headless, &launch)
        .expect("the session starts");
    (live, id)
}

/// Line 10: *"restart a session that exits unexpectedly up to a bounded number
/// of consecutive attempts, and stop with a stated reason when that bound is
/// reached."*
///
/// The harness comes up, is verified healthy, and dies — which is what *exits
/// unexpectedly* means, and is the only shape that is restarted at all. Every
/// later run of it dies at once, so no restart ever becomes healthy, so the
/// count never clears and the bound is really reached rather than approached.
///
/// # A rate, not a pass
///
/// §60: a restart bound proven once is proven for one trial, and this one is
/// timing-shaped — it depends on a process outliving a two-second window under
/// whatever load the machine is under. So the trial is run three times inside
/// the test and every trial must reach the same bound with the same stated
/// reason. The report carries the rate from running the whole file repeatedly.
#[cfg(unix)]
#[test]
fn a_session_that_keeps_crashing_is_restarted_a_bounded_number_of_times() {
    const TRIALS: usize = 3;
    for trial in 0..TRIALS {
        let fixture = Fixture::new(A_HARNESS_THAT_WORKS_ONCE_THEN_CRASH_LOOPS);
        let runtime = fixture.runtime();
        let (mut live, id) = one_session(&fixture, &runtime);

        drive_until(
            &mut live,
            "the restart bound to be reached",
            PATIENCE,
            |live| {
                live.get(&id)
                    .is_some_and(|session| session.restart_halted().is_some())
            },
        );

        let session = live.get(&id).expect("the session is still held");
        assert_eq!(
            session.restarts(),
            glasshouse::session::MAX_CONSECUTIVE_RESTARTS,
            "trial {trial}: the bound must be reached, not passed or stopped short of"
        );
        let reason = session.restart_halted().expect("a stated reason");
        assert!(
            reason.contains(&glasshouse::session::MAX_CONSECUTIVE_RESTARTS.to_string()),
            "trial {trial}: the reason must say how many times: {reason}"
        );
        // And the user is told where they would look: the session's own
        // terminal, not only a log.
        assert!(
            session.scrollback().contains(reason),
            "trial {trial}: the stated reason must reach the session's terminal"
        );

        live.close(&id).expect("close the session");
    }
}

/// Line 10's first exclusion, and the one that keeps this line away from a
/// capability that is already closed.
///
/// A harness that has never once come up did not *exit unexpectedly*. It is a
/// start that did not work, and restarting it three more times would turn a
/// mistyped executable into four processes and four sets of output. Phase 45's
/// crashed worker — one line of output and then a signal — is exactly that
/// shape, and it must be kept with its output rather than tried again.
#[cfg(unix)]
#[test]
fn a_harness_that_never_came_up_is_not_restarted() {
    let fixture = Fixture::new(A_HARNESS_THAT_PRINTS_AND_DIES);
    let runtime = fixture.runtime();
    let (mut live, id) = one_session(&fixture, &runtime);

    drive_until(&mut live, "the harness to die", PATIENCE, |live| {
        live.get(&id)
            .is_some_and(|session| session.exit().is_some())
    });

    // Long enough that a restart, if one were coming, would have happened:
    // the bound is three attempts and each of them is a spawn.
    let settle = Instant::now() + Duration::from_millis(500);
    while Instant::now() < settle {
        live.poll_exits();
        std::thread::sleep(Duration::from_millis(20));
    }

    let session = live.get(&id).expect("the session is still held");
    assert_eq!(
        session.restarts(),
        0,
        "a harness that never came up must not be restarted"
    );
    assert!(session.exit().is_some(), "and it stays exited");
    assert!(
        session.scrollback().contains("STARTED"),
        "with what it said still in its terminal: {:?}",
        session.scrollback()
    );

    live.close(&id).expect("close the session");
}

/// Line 11: *"reset the consecutive-restart count only when a restarted
/// session has been verified healthy, never when it has merely been started."*
///
/// This harness comes up, stays up past the health window, and dies — over and
/// over. Every restart becomes healthy, so the count is cleared every time and
/// the bound is never reached. A build that reset on *started* would look the
/// same here; a build that never reset would stop after three, and that is the
/// difference this test is measuring.
///
/// Two full crash-and-restart cycles, because one proves only that the first
/// restart happened.
#[cfg(unix)]
#[test]
fn a_restarted_session_that_becomes_healthy_again_clears_the_bound() {
    let fixture = Fixture::new(A_HARNESS_THAT_WORKS_THEN_DIES);
    let runtime = fixture.runtime();
    let (mut live, id) = one_session(&fixture, &runtime);

    // The count is watched rather than sampled: the reset is a *transition*
    // from one back to zero, and a build that reset on "started" would leave
    // the count at zero throughout, so a test that only read it at the end
    // could not tell the two apart.
    let mut previous = 0;
    let mut restarts_seen = 0;
    let mut resets_seen = 0;
    drive_until(
        &mut live,
        "two crash-and-restart cycles on a harness that keeps coming back up",
        PATIENCE,
        |live| {
            let session = live.get(&id).expect("the session is still held");
            assert!(
                session.restart_halted().is_none(),
                "a session that keeps coming back up must never reach the bound: {:?}",
                session.restart_halted()
            );
            let now = session.restarts();
            // Never more than the one restart that has not yet proved itself:
            // a build that did not reset would climb to the bound instead.
            assert!(
                now <= 1,
                "the count must be cleared by health, not accumulate: {now}"
            );
            if now > previous {
                restarts_seen += 1;
            }
            if now < previous {
                assert!(
                    session.verified_healthy(),
                    "the count may only be cleared by a session that is verified \
                     healthy, never by one that was merely started"
                );
                resets_seen += 1;
            }
            previous = now;
            resets_seen >= 2
        },
    );

    assert_eq!(
        restarts_seen, resets_seen,
        "every restart that came back up must have cleared the count exactly once"
    );

    live.close(&id).expect("close the session");
}

// -------------------------------------------------------------------------
// Line 9 — a bounded readiness, and a failure with a stated reason.
// -------------------------------------------------------------------------

/// A start that never became ready is a **failure with a stated reason**, and
/// not a session.
///
/// The record is put into the state a killed start really leaves behind — it
/// says `starting`, nothing else ever happened to it, and the process that was
/// starting it is gone — and then aged past the bound, because ten minutes is
/// a real condition and an unreasonable thing for a test to wait out.
///
/// This is where line 9 is answered, and the reason it is answered here rather
/// than inside `SessionRuntime::start`. A bound enforced from inside a start
/// has to decide, in the first few milliseconds, whether a process that has
/// just died never came up or ran and crashed — and nothing the operating
/// system offers separates those two, on either platform. The record does: a
/// start that never became ready is one whose record never left `starting` and
/// whose process is gone. That is durable, it means the same thing on macOS,
/// Linux and Windows, and it survives the `glasshouse` that gave up. See
/// `SessionRuntime::start`'s own comment for the measurement that settled it.
#[test]
fn a_start_that_never_became_ready_is_recorded_as_a_failure_with_a_reason() {
    let fixture = Fixture::new(A_HARNESS_THAT_FINISHES);
    fixture.run(&["launch", "claude-code", "--headless"]);
    let id = fixture.only_session();

    // Ten minutes ago, still `starting`, and nothing was ever identified: the
    // shape a `glasshouse` killed between recording the session and starting
    // its process leaves behind.
    let long_ago = fixture.now() - 600;
    fixture.execute(&format!(
        "UPDATE sessions SET lifecycle = 'starting', created_at = {long_ago}, \
         process_id = NULL, process_started_at = NULL, process_host = NULL, \
         supervision = NULL, supervision_reason = NULL WHERE id = '{}'",
        id.as_str()
    ));

    // First, the control, and the first architectural requirement in miniature:
    // a session that has only just begun starting, with nothing recorded about
    // its process, must be left completely alone. Supervision concludes nothing
    // from an absent identity — it does not guess that the start failed, and it
    // does not guess that it is fine.
    let just_now = fixture.now();
    fixture.execute(&format!(
        "UPDATE sessions SET created_at = {just_now} WHERE id = '{}'",
        id.as_str()
    ));
    fixture.run(&["sessions"]);
    let untouched = fixture.row(&id);
    assert_eq!(
        untouched.lifecycle, "starting",
        "a start with no recorded identity must not be concluded about at all: \
         {untouched:?}"
    );
    assert_eq!(untouched.supervision, None, "{untouched:?}");

    // Now age it past the bound.
    fixture.execute(&format!(
        "UPDATE sessions SET created_at = {long_ago} WHERE id = '{}'",
        id.as_str()
    ));

    let said = fixture.everything(&["sessions"]);

    let row = fixture.row(&id);
    assert_eq!(
        row.lifecycle, "failed",
        "a start that never became ready is a failure, not a session that is \
         still starting: {row:?}"
    );
    let reason = row
        .supervision_reason
        .as_deref()
        .expect("a failure must carry a stated reason");
    assert!(
        reason.contains("never became ready"),
        "the reason must say what went wrong: {reason}"
    );
    assert!(
        said.contains("never started"),
        "and the user must be told: {said}"
    );
}

/// The same line, for the start that got as far as having a process and then
/// lost it.
///
/// A record still reading `starting` whose recorded process is gone did not
/// become a session — it is a failure, and it is a *different* conclusion from
/// the one drawn about a session that was running and has since stopped. A
/// build that collapsed the two would report every failed start as a session
/// that ran.
#[test]
fn a_start_whose_process_died_before_it_ran_is_a_failure_not_a_stopped_session() {
    let fixture = Fixture::new(A_HARNESS_THAT_FINISHES);
    fixture.run(&["launch", "claude-code", "--headless"]);
    let id = fixture.only_session();
    let identity = fixture
        .row(&id)
        .identity()
        .expect("the launch recorded an identity");

    // That `glasshouse` has exited, so its process id is gone; the record is
    // wound back to the moment before it ever reported running.
    fixture.execute(&format!(
        "UPDATE sessions SET lifecycle = 'starting', supervision = 'owned', \
         supervision_reason = NULL WHERE id = '{}'",
        id.as_str()
    ));

    fixture.run(&["sessions"]);

    let row = fixture.row(&id);
    assert_eq!(
        row.lifecycle, "failed",
        "a start whose process is gone is a failed start, never a stopped \
         session: {row:?}"
    );
    let reason = row.supervision_reason.as_deref().expect("a stated reason");
    assert!(
        reason.contains(&identity.pid.to_string()),
        "the reason must name the process that was looked for: {reason}"
    );
    assert!(
        reason.contains("never became ready") || reason.contains("never left"),
        "and it must say the start never came up, not merely that it ended: \
         {reason}"
    );
}

/// The platform decision this phase turns on, at the record.
///
/// A harness that prints something and then dies is **a session that failed**,
/// not a start that was refused. Glasshouse cannot tell, in the milliseconds
/// after `spawn` returns, whether a process that has just died never came up
/// or ran and crashed — and it must not throw away what the process printed in
/// order to pretend it can, because that output is the only thing that says
/// *why*.
///
/// # Why this test exists and what it replaced
///
/// An earlier version of line 9 enforced the bound from inside
/// `SessionRuntime::start` and refused the start when the process died within
/// the settle window. It gave **opposite answers on two operating systems** —
/// one tree, one gate run, `tests/events_lifecycle.rs`: macOS 5 passed, Linux
/// 3 passed and 2 failed. Not a race that a longer window would win: Linux's
/// `/proc/<pid>/stat` still describes a process that has died and not been
/// reaped, and macOS's `proc_pidinfo` does not, so each platform reached a
/// different conclusion every single time. And it turned
/// `docs/product/capability-map.md`'s already-closed *"preserve terminal
/// output and event history after a worker crashes"* red on one of them.
///
/// So the assertion here is deliberately the one a refusal cannot satisfy:
/// the session reaches `failed`, promptly, in the process that started it —
/// not `starting` waiting for a later `glasshouse` to conclude something about
/// it, and not absent.
#[test]
fn a_harness_that_printed_and_died_is_a_failed_session_not_a_refused_start() {
    let fixture = Fixture::new(A_HARNESS_THAT_PRINTS_AND_DIES);

    // Not `run`: whether the *command* succeeds is a separate question from
    // whether the session was recorded, and this test is about what survived.
    let said = fixture.everything(&["launch", "claude-code", "--headless"]);

    // The assertion a refusal cannot satisfy, and the reason the record alone
    // is not enough: a refused start is written down as a failed session too,
    // so a test that read only `lifecycle` would pass against a build that
    // refuses. What changes is **what the user is told**. A session that ran
    // reports the harness's own ending; a refused start reports Glasshouse's
    // guess that it never came up, and the harness's own words — which for a
    // bad flag or an unreadable configuration are the whole diagnosis — are
    // never read by anything.
    assert!(
        said.contains("the harness "),
        "the user must be told how the harness itself ended: {said}"
    );

    let id = fixture.only_session();
    let row = fixture.wait_for(&id, "the dead harness's session to be recorded", |row| {
        row.lifecycle != "starting"
    });
    assert_eq!(
        row.lifecycle, "failed",
        "a harness that died is a session that failed: {row:?}"
    );
}

/// The other half of line 9, and the reason it is not simply "refuse anything
/// that exits quickly".
///
/// A harness that does its work and exits cleanly inside the readiness window
/// **ran**. Its session is recorded as having stopped, exactly as before this
/// phase. A bound that fabricated a failure here would break every short
/// session, which is a far more common thing than a broken start.
#[test]
fn a_harness_that_finishes_at_once_still_ran() {
    let fixture = Fixture::new(A_HARNESS_THAT_FINISHES);

    fixture.run(&["launch", "claude-code", "--headless"]);

    let row = fixture.row(&fixture.only_session());
    assert_eq!(
        row.lifecycle, "stopped",
        "a harness that finished is a session that ran: {row:?}"
    );
    assert!(
        row.process_id.is_some(),
        "and it is still a session whose process was identified: {row:?}"
    );
}

// -------------------------------------------------------------------------
// Line 12 — one ordered path for lifecycle changes.
// -------------------------------------------------------------------------

/// Two writers changing one session's state cannot leave it in a state
/// neither of them asked for.
///
/// The interleaving is **forced**, not waited for:
///
/// 1. a hook writer reads `running` and decides `idle`;
/// 2. the exit writer observes the process end and writes `stopped`;
/// 3. the hook writer writes the `idle` it decided in step 1.
///
/// The result must not be `idle` — a live state for a session whose process is
/// gone, which neither writer asked for.
///
/// # Why this is staged rather than raced
///
/// Two earlier versions raced it: six real `glasshouse hook` processes, then
/// four threads on separate connections with a millisecond between each read
/// and its write. **Neither ever reproduced it**, and both passed against a
/// build with the ordering deliberately removed — which is the worst thing a
/// concurrency test can do. Twenty-five rounds of a race you cannot make
/// happen is twenty-five rounds of proving nothing.
///
/// Staging it costs the "rate" §60 asks for and buys certainty instead: this
/// interleaving happens on every run, and a build that allows it fails on
/// every run. The two connections are real and know nothing about each other,
/// so what is being tested is still SQLite's ordering and not a fixture's.
#[test]
fn a_stopped_session_is_not_revived_by_a_writer_that_read_before_the_stop() {
    let fixture = Fixture::new(A_HARNESS_THAT_FINISHES);
    fixture.run(&["launch", "claude-code", "--headless"]);
    let id = fixture.only_session();

    let hook_runtime = fixture.runtime();
    let exit_runtime = fixture.runtime();
    let hook = ProjectSessions::open(&hook_runtime).unwrap();
    let exit = ProjectSessions::open(&exit_runtime).unwrap();

    fixture.execute(&format!(
        "UPDATE sessions SET lifecycle = 'running' WHERE id = '{}'",
        id.as_str()
    ));

    // 1. The hook reads, and decides from what it read — exactly what
    //    `glasshouse hook` does before it writes anything.
    let seen = hook.store().get(&id).unwrap().expect("the session");
    assert_eq!(seen.lifecycle, SessionLifecycle::Running);
    let decided = SessionLifecycle::Idle;
    assert!(
        glasshouse::session::may_apply(seen.lifecycle, decided),
        "the hook's own check passes on what it read, which is the point"
    );

    // 2. The process ends, and the exit is recorded — entirely between the
    //    hook's read and its write.
    exit.store()
        .set_lifecycle(&id, SessionLifecycle::Stopped)
        .unwrap();

    // 3. The hook writes what it decided in step 1.
    hook.store().set_lifecycle(&id, decided).unwrap();

    let row = fixture.row(&id);
    assert_eq!(
        row.lifecycle, "stopped",
        "a writer that read before the stop revived the session afterwards; the \
         record now claims a live state for a process that has ended, which is \
         a state neither writer asked for"
    );
}

/// The ordered path stays usable when several `glasshouse` processes really do
/// write at once.
///
/// The other half of "one ordered path", and the half a staged test cannot
/// show: ordering that turns contention into errors is not ordering, it is a
/// different failure. Every write here must *succeed* — a deferred transaction
/// would read first and then have to upgrade its lock, which SQLite refuses
/// outright once another connection has committed, and `busy_timeout` cannot
/// help because there is nothing left to wait for.
///
/// A rate rather than a single trial: several connections, many writes each.
#[test]
fn many_writers_on_one_session_all_succeed() {
    const WRITERS: usize = 6;
    const EACH: usize = 40;

    let fixture = Fixture::new(A_HARNESS_THAT_FINISHES);
    fixture.run(&["launch", "claude-code", "--headless"]);
    let id = fixture.only_session();

    // Point the record at a process that really is alive — this test — so that
    // supervision verifies it and leaves it alone. Without this, the first
    // writer to open the project would find the launch's `glasshouse` gone,
    // conclude the session had stopped, and every later write would be
    // correctly refused: a true answer to a different question than the one
    // this test asks.
    let alive = ProcessIdentity::of_this_process().expect("this platform names processes");
    fixture.execute(&format!(
        "UPDATE sessions SET lifecycle = 'running', process_id = {}, \
         process_started_at = {}, process_host = '{}', supervision = 'owned' \
         WHERE id = '{}'",
        alive.pid,
        alive.started_at_ms,
        alive.host.replace('\'', "''"),
        id.as_str()
    ));

    let mut writers = Vec::new();
    for writer in 0..WRITERS {
        let runtime = fixture.runtime();
        let id = id.clone();
        writers.push(std::thread::spawn(move || {
            let sessions = ProjectSessions::open(&runtime).unwrap();
            let store = sessions.store();
            let mut refused = Vec::new();
            for turn in 0..EACH {
                let next = if turn % 2 == 0 {
                    SessionLifecycle::Idle
                } else {
                    SessionLifecycle::Running
                };
                if let Err(err) = store.set_lifecycle(&id, next) {
                    refused.push(format!("writer {writer}, turn {turn}: {err}"));
                }
            }
            refused
        }));
    }

    let refused: Vec<String> = writers
        .into_iter()
        .flat_map(|writer| writer.join().expect("a writing thread"))
        .collect();

    assert!(
        refused.is_empty(),
        "{} of {} concurrent lifecycle writes failed rather than being ordered:\n{}",
        refused.len(),
        WRITERS * EACH,
        refused.join("\n")
    );

    // And the session is still in one of the states somebody asked for.
    let row = fixture.row(&id);
    assert!(
        row.lifecycle == "idle" || row.lifecycle == "running",
        "the session ended in `{}`, which nobody asked for",
        row.lifecycle
    );
}

// -------------------------------------------------------------------------
// Line 13 — never two inputs at once.
// -------------------------------------------------------------------------

/// Many senders, one session, and not one message torn in half.
///
/// A real pseudo-terminal, a real process at the other end, and four threads
/// writing through the runtime the way the shipped binary owns it — behind one
/// `Mutex`, which is what `main.rs` does. The terminal echoes what it is given
/// and the harness reads it and says nothing, so the session's own scrollback
/// is exactly the record of what Glasshouse wrote and in what order — see
/// `A_HARNESS_THAT_READS_WHAT_IT_IS_SENT` for why a harness that wrote back
/// made that record unreadable on Linux and not on macOS.
///
/// # The property is "whole", not "all"
///
/// A terminal's input queue is bounded, and a burst that outruns the process
/// reading it loses characters *in the kernel*. That is not an ordering defect
/// and Glasshouse cannot prevent it, so a test that demanded every message
/// arrive would be measuring the tty's buffer. What it demands instead is that
/// **every message that arrived, arrived in one piece**: each marker is
/// followed by exactly the payload its sender wrote, never by a fragment of
/// somebody else's. Two interleaved writes cannot produce that, however the
/// buffer behaves.
///
/// The count is checked as well, so the property cannot be satisfied by an
/// empty scrollback.
#[cfg(unix)]
#[test]
fn no_two_inputs_are_ever_delivered_to_one_session_at_once() {
    use std::sync::{Arc, Mutex};

    use glasshouse::launch::HarnessLaunch;
    use glasshouse::session::SessionPresentation;
    use glasshouse::session::SessionRuntime;

    const SENDERS: usize = 4;
    const EACH: usize = 25;
    const PAYLOAD: usize = 48;

    let fixture = Fixture::new(A_HARNESS_THAT_READS_WHAT_IT_IS_SENT);
    let runtime = fixture.runtime();

    let executable = glasshouse::platform::exec::resolve_explicit(fixture.harness_path())
        .expect("the fake harness resolves");
    let launch = HarnessLaunch::new(executable, runtime.project());

    // Room for every message several times over, so nothing this test looks
    // for can have been dropped for want of scrollback rather than for want of
    // ordering.
    let live = Arc::new(Mutex::new(SessionRuntime::with_scrollback_bytes(1 << 20)));
    let id = SessionId::new("aaaaaaaabbbbbbbbccccccccdddddddd");
    live.lock()
        .unwrap()
        .start(id.clone(), SessionPresentation::Headless, &launch)
        .expect("the fake harness starts and stays up");

    // Wait until the harness is actually reading before any sender starts.
    //
    // A terminal's input queue is bounded and fills while a process is still
    // being `exec`ed; what overflows it is discarded by the kernel, and the
    // first thing this test learned is that it was measuring that rather than
    // anything Glasshouse does. The probe is echoed by the terminal once its
    // slave end exists and the shell is reading, so seeing it back is the
    // session saying it is ready to be written to.
    {
        let probe = "<ready>";
        live.lock()
            .unwrap()
            .send_text(&id, &format!("{probe}\r"))
            .expect("the session is up");
        let deadline = Instant::now() + PATIENCE;
        loop {
            let text = live.lock().unwrap().get(&id).unwrap().scrollback();
            if text.contains(probe) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the harness never started reading; it said:\n{text}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    let mut senders = Vec::new();
    for sender in 0..SENDERS {
        let live = Arc::clone(&live);
        let id = id.clone();
        senders.push(std::thread::spawn(move || {
            for message in 0..EACH {
                // Long enough that a torn write would be visible, and unique
                // per sender and message so a tear cannot be mistaken for a
                // duplicate.
                let line = format!("<{}>{}<{}>", sender, "x".repeat(PAYLOAD), message);
                // A carriage return, because this is a line: without one the
                // terminal's line discipline holds it and the test would be
                // measuring the kernel's buffer rather than Glasshouse's
                // ordering.
                live.lock()
                    .unwrap()
                    .send_text(&id, &format!("{line}\r"))
                    .expect("the session is up");
                // A breath, so the harness can drain what it is being sent.
                // Without it the four senders outrun the terminal's input
                // queue and the kernel discards characters, which is a fact
                // about the tty and nothing to do with what is under test.
                std::thread::sleep(Duration::from_millis(2));
            }
        }));
    }
    for sender in senders {
        sender.join().expect("a sending thread");
    }

    let expected = SENDERS * EACH;
    let deadline = Instant::now() + PATIENCE;
    let (text, whole) = loop {
        let text = live.lock().unwrap().get(&id).unwrap().scrollback();
        let whole = (0..SENDERS)
            .flat_map(|sender| {
                (0..EACH)
                    .map(move |message| format!("<{sender}>{}<{message}>", "x".repeat(PAYLOAD)))
            })
            .filter(|line| text.contains(line.as_str()))
            .count();
        if whole == expected || Instant::now() >= deadline {
            break (text, whole);
        }
        std::thread::sleep(Duration::from_millis(25));
    };

    // The invariant: every payload in the scrollback is a whole payload. A
    // torn delivery leaves a short run of `x`, because the other sender's
    // marker lands in the middle of this one's.
    let mut runs = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != 'x' {
            continue;
        }
        let mut length = 1;
        while chars.peek() == Some(&'x') {
            chars.next();
            length += 1;
        }
        runs += 1;
        assert_eq!(
            length, PAYLOAD,
            "a run of {length} payload characters is not one message; two \
             deliveries were interleaved.\n\nscrollback:\n{text}"
        );
    }

    assert!(
        runs > 0,
        "nothing arrived at all, so this proved nothing:\n{text}"
    );
    assert!(
        whole * 100 >= expected * 80,
        "only {whole} of {expected} messages arrived whole, which is too few for \
         the check above to have been exercised:\n{text}"
    );

    live.lock().unwrap().close(&id).expect("close the session");
}

// -------------------------------------------------------------------------
// The resume path's own process identity.
// -------------------------------------------------------------------------
//
// Lines 1 and 3 again, at the one boundary that was skipping them.
// `SessionStore::create` records the process a session was started in, and
// every test above proves it does. A **resume** is the other way a session
// becomes live, it happens in a different operating-system process, and it
// wrote no identity at all — so the record went on naming the `glasshouse`
// that first created the session, which had exited. Supervision then verified
// that dead process id on the very next command, correctly concluded the
// session was gone, and took the record back to `stopped`.
//
// That is not a supervision defect and nothing below weakens supervision to
// make it pass: `a_resumed_session_whose_process_is_gone_is_still_lost` is the
// same verdict against the same code, reached against the resumed identity
// instead of the stale one.

/// A harness that finishes at once the first time and stays up afterwards.
///
/// One `Fixture` installs one harness body, and this scenario needs both: a
/// first run that ends so the session becomes `Resumable`, and a second run
/// that is still alive when the test looks at what the resume recorded. The
/// marker lives beside the script for
/// `A_HARNESS_THAT_WORKS_ONCE_THEN_CRASH_LOOPS`'s reason — a resume re-runs
/// the recorded launch, so anything telling the runs apart has to survive
/// outside it.
#[cfg(unix)]
const A_HARNESS_THAT_FINISHES_THEN_STAYS_UP: &str = concat!(
    "marker=\"$(dirname \"$0\")/resume-marker\"\n",
    "if [ -f \"$marker\" ]; then sleep 60; exit 0; fi\n",
    ": > \"$marker\"\n",
    "exit 0",
);
#[cfg(windows)]
const A_HARNESS_THAT_FINISHES_THEN_STAYS_UP: &str = concat!(
    "set \"marker=%~dp0resume-marker\"\r\n",
    "if exist \"%marker%\" goto stay\r\n",
    "type nul > \"%marker%\"\r\n",
    "exit /b 0\r\n",
    ":stay\r\n",
    "ping -n 61 127.0.0.1 > nul\r\n",
    "exit /b 0",
);

/// A resumed session records the process it is *now* running in, and the next
/// `glasshouse` command therefore leaves it alone.
///
/// The reproduction from `report-resume-probe.md`, with the harness taken out
/// of it: the harness only ever mattered for showing that the resumed
/// session's own hook was one of the commands that killed it. Any second
/// command opens `ProjectSessions`, and every one of them supervises.
///
/// Run twice (§60) because a revert that alternated would pass a single pass
/// and be exactly as broken.
#[test]
fn a_resumed_session_records_the_process_it_is_running_in() {
    let fixture = Fixture::new(A_HARNESS_THAT_FINISHES_THEN_STAYS_UP);
    let host = supervision::host_name().expect("this platform names its host");

    // A session that ran, ended, and has a native conversation to resume to.
    fixture.run(&["launch", "claude-code", "--headless"]);
    let id = fixture.only_session();
    let stopped = fixture.row(&id);
    assert_eq!(
        stopped.lifecycle, "stopped",
        "the first launch must have ended: {stopped:?}"
    );
    let created_in = stopped
        .identity()
        .expect("the first launch recorded the process that created the session");

    // The resume: a bare second launch takes the router's continuation branch.
    let mut background = Background::launch(&fixture);
    let resumed = fixture.wait_for(&id, "the resume to be recorded", |row| {
        row.lifecycle == "running"
    });
    assert_eq!(
        fixture.session_ids().len(),
        1,
        "the second launch must have continued the first session rather than \
         started another; this test is about a resume"
    );

    let running_in = resumed
        .identity()
        .expect("a resumed session must record a process identity");
    assert_ne!(
        running_in, created_in,
        "the resume is a different operating-system process, and the record \
         still names the one that created the session: {resumed:?}"
    );
    assert_eq!(
        supervision::verify(&running_in, &host),
        supervision::Verdict::Verified,
        "the identity a resume records must be the process that is actually \
         running, not a plausible one: {resumed:?}"
    );

    // Any command that opens this project's sessions supervises. Before the
    // identity was re-recorded, this pass verified the *creating* process,
    // found it gone, and wrote `stopped` back over the resume.
    for pass in 1..=2 {
        fixture.run(&["sessions"]);
        let after = fixture.row(&id);
        assert_eq!(
            after.lifecycle, "running",
            "pass {pass}: a supervising command must not take a live resumed \
             session back to stopped: {after:?}"
        );
        assert_ne!(
            after.supervision.as_deref(),
            Some("lost"),
            "pass {pass}: nothing is lost — the process is running: {after:?}"
        );
    }

    background.kill();
}

/// And the invariant that stops the fix being worse than the defect: a
/// resumed session whose process is genuinely gone is still reported `lost`.
///
/// The same conclusion `an_orphaned_session_is_discovered_and_recorded_as_lost`
/// proves for a created session, reached against an identity the **resume**
/// wrote. If re-recording the identity had been done by loosening what
/// supervision is willing to conclude, this is the test that would fail.
#[test]
fn a_resumed_session_whose_process_is_gone_is_still_lost() {
    let fixture = Fixture::new(A_HARNESS_THAT_FINISHES_THEN_STAYS_UP);

    fixture.run(&["launch", "claude-code", "--headless"]);
    let id = fixture.only_session();

    let mut background = Background::launch(&fixture);
    let resumed = fixture.wait_for(&id, "the resume to be recorded", |row| {
        row.lifecycle == "running"
    });
    let running_in = resumed
        .identity()
        .expect("a resumed session must record a process identity");

    // Abruptly, with no chance to record the exit — the 2026-08-26 shape,
    // applied to a resume.
    background.kill();

    fixture.run(&["sessions"]);

    let after = fixture.row(&id);
    assert_eq!(
        after.supervision.as_deref(),
        Some("lost"),
        "a resumed session whose process is gone must still be found: {after:?}"
    );
    assert_eq!(
        after.lifecycle, "stopped",
        "and it is not still running: {after:?}"
    );
    let reason = after
        .supervision_reason
        .expect("a stated reason, as every other conclusion carries");
    assert!(
        reason.contains(&running_in.pid.to_string()),
        "the reason must name the process the resume recorded, not the one \
         that created the session: {reason}"
    );
}

/// A resume refused under the write lock records nothing — not a lifecycle,
/// and not an identity.
///
/// `SessionStore::open_for_resume` reads outside a transaction, so a record
/// can be closed or failed between its answer and the write. `begin_resume`
/// re-asks under the lock; this proves the identity write is inside that same
/// decision rather than beside it, because an identity written for a resume
/// that was refused would be a live-looking process on a finished session.
#[test]
fn a_refused_resume_records_neither_a_lifecycle_nor_an_identity() {
    let fixture = Fixture::new(A_HARNESS_THAT_FINISHES);
    fixture.run(&["launch", "claude-code", "--headless"]);
    let id = fixture.only_session();
    let before = fixture.row(&id);
    assert_eq!(before.lifecycle, "stopped");

    for (state, said) in [("failed", "failed"), ("closed", "closed")] {
        fixture.execute(&format!(
            "UPDATE sessions SET lifecycle = 'stopped' WHERE id = '{}'",
            id.as_str()
        ));

        let runtime = fixture.runtime();
        let sessions = ProjectSessions::open(&runtime).unwrap();
        let store = sessions.store();
        let resumable = store
            .open_for_resume(&id)
            .expect("a stopped session with a native identifier is resumable");

        // The window: another process finishes the session after the check
        // and before the write.
        fixture.execute(&format!(
            "UPDATE sessions SET lifecycle = '{state}' WHERE id = '{}'",
            id.as_str()
        ));

        let error = store
            .begin_resume(&resumable)
            .expect_err("a finished session must refuse resumption");
        let message = error.to_string();
        assert!(
            message.contains(said),
            "the refusal must say the session is {said}: {message}"
        );

        let after = fixture.row(&id);
        assert_eq!(
            after.lifecycle, state,
            "a refused resume must leave the lifecycle alone: {after:?}"
        );
        assert_eq!(
            after.process_id, before.process_id,
            "a refused resume must record no identity: {after:?}"
        );
        assert_eq!(
            after.process_started_at, before.process_started_at,
            "a refused resume must record no identity: {after:?}"
        );
        assert_eq!(
            after.process_host, before.process_host,
            "a refused resume must record no identity: {after:?}"
        );
    }
}

/// A session quarantined between the check and the write is not resumed, and
/// its quarantine survives.
///
/// The narrow window `begin_resume` re-asks the disposition for, asked about
/// the *other* refusal. It matters more since the resume began recording an
/// identity: a resume that went ahead here would overwrite `quarantined` with
/// `owned` and erase the record of a process Glasshouse cannot account for —
/// so the refusal that stops it would never fire again, on this session or on
/// a replacement claiming its conversation. Phase 10A's rule is that a
/// quarantined session is never reused, never replaced and never reported as
/// stopped; this is the resume boundary keeping it.
#[test]
fn a_session_quarantined_after_the_check_is_not_resumed_over() {
    let fixture = Fixture::new(A_HARNESS_THAT_FINISHES);
    fixture.run(&["launch", "claude-code", "--headless"]);
    let id = fixture.only_session();

    let runtime = fixture.runtime();
    let sessions = ProjectSessions::open(&runtime).unwrap();
    let store = sessions.store();
    let resumable = store
        .open_for_resume(&id)
        .expect("a stopped session with a native identifier is resumable");

    // Another `glasshouse` finds the recorded process alive and unaccounted
    // for, after this caller was told the session could be resumed.
    fixture.execute(&format!(
        "UPDATE sessions SET supervision = 'quarantined', supervision_reason = \
         'a process Glasshouse cannot account for is still running' WHERE id = '{}'",
        id.as_str()
    ));

    let error = store
        .begin_resume(&resumable)
        .expect_err("a quarantined session must not be resumed");
    let message = error.to_string();
    assert!(
        message.contains("quarantined"),
        "the refusal must say why: {message}"
    );

    let after = fixture.row(&id);
    assert_eq!(
        after.supervision.as_deref(),
        Some("quarantined"),
        "the quarantine must survive a refused resume; overwriting it would \
         retire the refusal that produced it: {after:?}"
    );
    assert_eq!(
        after.lifecycle, "stopped",
        "and the record must not have been made live: {after:?}"
    );
}

/// **The contract.** A resumed session's harness is believed again.
///
/// `tests/session_hook.rs::a_resumed_session_believes_its_harness_again`
/// asserts this already and cannot fail on it: its fixture creates the record
/// with `SessionStore::create` **inside the test process**, so the identity on
/// the row is the test binary's own and verifies for the whole run. A real
/// resume happens in a process the creating `glasshouse` has already left, and
/// that is the case this reproduces — the session below was created by a
/// `glasshouse` that has exited, and resumed by one that has not.
///
/// The hook is a **separate process**, which is the whole of the defect: it
/// opens `ProjectSessions`, supervision runs at that open, and against a
/// stale identity it wrote `stopped` roughly a millisecond before the hook's
/// own transition was refused for arriving at a finished session. `Stop` is
/// the event for `a_resumed_session_believes_its_harness_again`'s reason — it
/// means `Idle`, a state the record can only be holding if the report was
/// applied.
#[test]
fn a_resumed_sessions_hook_is_believed_rather_than_refused_by_its_own_arrival() {
    let fixture = Fixture::new(A_HARNESS_THAT_FINISHES_THEN_STAYS_UP);

    fixture.run(&["launch", "claude-code", "--headless"]);
    let id = fixture.only_session();

    let mut background = Background::launch(&fixture);
    fixture.wait_for(&id, "the resume to be recorded", |row| {
        row.lifecycle == "running"
    });

    let status = fixture.hook(&id, "Stop");
    assert!(
        status.success(),
        "the hook handler must not fail the harness's turn"
    );

    let after = fixture.row(&id);
    assert_eq!(
        after.lifecycle, "idle",
        "the harness reported a turn ending in a session Glasshouse itself \
         resumed, and the report was discarded — by supervision running inside \
         this very hook process: {after:?}"
    );

    background.kill();
}
