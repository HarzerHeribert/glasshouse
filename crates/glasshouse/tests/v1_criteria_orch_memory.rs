//! Phase 55 — six V1-completion criteria (capability map lines 1925, 1926,
//! 1938, 1927, 1928, 1929), proven directly against mechanisms the map
//! already records COMPLETE: Phase 15/16's wake-up and worker-access door,
//! Phase 17/46/54's optional cmux presentation, Phase 20/22/23's durable
//! memory, and Phase 40/45's portable checkpoint.
//!
//! One test per line, driven through the shipped `glasshouse` binary or the
//! nearest deterministic production seam. No production code is changed
//! here — each test's killing mutation is run separately with
//! `scripts/mutate.sh --allow-dirty`, which always restores byte-identical.

use clap::Parser;

use glasshouse::memory::search::SearchScope;
use glasshouse::memory::{MemoryKind, NewMemory, ProjectMemory};
use glasshouse::{Cli, Runtime, bootstrap};

/// Open a `Runtime` against a project this test already put a `glasshouse`
/// subprocess through — the same idiom `tests/cmux_presentation.rs` and
/// `tests/checkpoint_portability.rs` use to inspect what a subprocess wrote,
/// rather than parsing the subprocess's own text output.
/// Called only from `mod cmux_optional` and `mod checkpoint_handoff`, both
/// `#[cfg(unix)]`; gated with them.
#[cfg(unix)]
fn open_runtime(base: &std::path::Path, root: &std::path::Path) -> Runtime {
    let cli = Cli::try_parse_from([
        "glasshouse",
        "--data-dir",
        base.join("data").to_str().unwrap(),
        "--config-dir",
        base.join("config").to_str().unwrap(),
    ])
    .unwrap();
    bootstrap(&cli, root).unwrap()
}

// ===========================================================================
// 1925, 1926 — the orchestrator wake-up flow and the user's direct access to
// an orchestrated worker, both through `glasshouse api serve`'s real door.
// ===========================================================================

#[cfg(unix)]
mod worker_control {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    const TIMEOUT: Duration = Duration::from_secs(30);

    /// A project with an installed harness that echoes every line it reads
    /// and records it under a name derived from its own `--settings`
    /// argument — the same tagging idiom `tests/worker_wakeup.rs` and
    /// `tests/worker_access.rs` use, so a door that stopped installing
    /// lifecycle hooks or routing a session's terminal correctly fails these
    /// tests rather than passing them against an unattributable log.
    pub struct Fixture {
        _tmp: tempfile::TempDir,
        base: PathBuf,
    }

    impl Fixture {
        pub fn new() -> Self {
            let tmp = tempfile::tempdir().expect("tempdir");
            let base = tmp.path().to_path_buf();

            let bin_dir = base.join("bin");
            std::fs::create_dir_all(&bin_dir).expect("create bin dir");
            let harness = install_session_tagging_harness(&bin_dir);

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

        pub fn base(&self) -> &Path {
            &self.base
        }

        pub fn project_root(&self, name: &str) -> PathBuf {
            let root = self.base.join("workspace").join(name);
            std::fs::create_dir_all(root.join(".git")).expect("create project root");
            std::fs::canonicalize(&root).expect("canonicalize project root")
        }

        pub fn received(&self, root: &Path, session: &str) -> Option<String> {
            std::fs::read_to_string(root.join(format!("received-{session}.log"))).ok()
        }

        pub fn argv(&self, root: &Path, session: &str) -> Option<String> {
            std::fs::read_to_string(root.join(format!("argv-{session}.log"))).ok()
        }

        pub fn reacted_to_interrupt(&self, root: &Path, session: &str) -> bool {
            root.join(format!("interrupted-{session}.log")).exists()
        }

        /// Run the shipped `glasshouse` binary as a user would, against this
        /// project — the same `--data-dir`/`--config-dir` [`Server::start`]
        /// uses, so the client resolves the same control socket the server
        /// did.
        pub fn client(&self, root: &Path, args: &[&str]) -> std::process::Output {
            Command::new(env!("CARGO_BIN_EXE_glasshouse"))
                .arg("--scope")
                .arg(root)
                .arg("--data-dir")
                .arg(self.base.join("data"))
                .arg("--config-dir")
                .arg(self.base.join("config"))
                .args(args)
                .stdin(Stdio::null())
                .output()
                .expect("run the glasshouse client")
        }

        /// Run a real `glasshouse hook` process, exactly as a harness's own
        /// lifecycle hook does — a separate short-lived invocation that
        /// reports one event and exits.
        pub fn hook(&self, root: &Path, session: &str, event: &str) {
            let status = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
                .arg("--scope")
                .arg(root)
                .arg("--data-dir")
                .arg(self.base.join("data"))
                .arg("--config-dir")
                .arg(self.base.join("config"))
                .arg("hook")
                .arg("--session")
                .arg(session)
                .arg("--event")
                .arg(event)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("run `glasshouse hook`");
            assert!(
                status.success(),
                "`glasshouse hook --session {session} --event {event}` must never fail"
            );
        }
    }

    /// A harness that names its own log files after the session it was
    /// started for, taken from the `--settings <state>/sessions/<id>/settings.json`
    /// argument the lifecycle-hook installation adds. Traps `SIGINT` so an
    /// interrupt is provably delivered rather than merely acknowledged, and
    /// stays alive through it — the same shape `tests/worker_access.rs`
    /// documents at length, restated here rather than shared, because
    /// integration test binaries do not share code.
    fn install_session_tagging_harness(bin_dir: &Path) -> PathBuf {
        let path = bin_dir.join("session-tagging-harness");
        std::fs::write(
            &path,
            "#!/bin/sh\n\
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
             done\n",
        )
        .expect("write the session-tagging harness");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    pub struct Server {
        child: Child,
        socket: PathBuf,
    }

    impl Server {
        pub fn start(fixture: &Fixture, root: &Path) -> Self {
            let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
                .arg("--scope")
                .arg(root)
                .arg("--data-dir")
                .arg(fixture.base().join("data"))
                .arg("--config-dir")
                .arg(fixture.base().join("config"))
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

        pub fn call(&self, request: serde_json::Value) -> serde_json::Value {
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

        pub fn spawn(&self, role: &str) -> String {
            let response = self.call(serde_json::json!({
                "op": "spawn_session",
                "harness": "claude-code",
                "role": role,
            }));
            assert_eq!(response["status"], "ok", "{response}");
            response["result"]["session"]
                .as_str()
                .expect("a session id")
                .to_owned()
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

    fn completions(text: &str) -> Vec<serde_json::Value> {
        text.lines()
            .filter_map(|line| line.trim().strip_prefix("glasshouse worker-completion "))
            .map(|json| {
                serde_json::from_str(json).expect("a completion payload is one line of JSON")
            })
            .collect()
    }

    fn stderr_of(output: &std::process::Output) -> String {
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    fn stdout_of(output: &std::process::Output) -> String {
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Map line 1925 — *"Consider V1 usable when a worker completion event
    /// can reliably wake or notify the orchestrator."*
    ///
    /// An orchestrator registers interest in a worker; a real `glasshouse
    /// hook` process reports that its turn ended, exactly as a harness's own
    /// `Stop` hook does; the door notices and types one structured
    /// `glasshouse worker-completion` line into the orchestrator's own
    /// terminal, naming the worker, the outcome and a log position — never
    /// into the worker itself.
    ///
    /// Mutation: drop the notification on completion.
    #[test]
    fn a_workers_completion_event_reliably_wakes_the_orchestrator() {
        let fixture = Fixture::new();
        let root = fixture.project_root("alpha");
        let server = Server::start(&fixture, &root);

        let worker = server.spawn("worker");
        let orchestrator = server.spawn("orchestrator");
        wait_for("both harnesses to start", || {
            fixture.argv(&root, &worker).is_some() && fixture.argv(&root, &orchestrator).is_some()
        });

        let registered = server.call(serde_json::json!({
            "op": "watch_worker", "session": worker, "notify": orchestrator,
        }));
        assert_eq!(registered["status"], "ok", "{registered}");

        fixture.hook(&root, &worker, "UserPromptSubmit");
        fixture.hook(&root, &worker, "Stop");

        wait_for("the orchestrator to be woken", || {
            fixture
                .received(&root, &orchestrator)
                .is_some_and(|text| !completions(&text).is_empty())
        });

        let text = fixture.received(&root, &orchestrator).unwrap();
        let delivered = completions(&text);
        assert_eq!(
            delivered.len(),
            1,
            "exactly one completion should have been delivered: {text}"
        );
        let completion = &delivered[0];
        assert_eq!(completion["worker"], worker, "{completion}");
        assert_eq!(completion["outcome"], "completed", "{completion}");
        assert!(
            completion["seq"].as_i64().is_some_and(|seq| seq > 0),
            "the notification must anchor to a real log position: {completion}"
        );

        // The wake-up flow reads a log and types into the orchestrator; it
        // never touches the worker it is reporting about.
        let worker_received = fixture.received(&root, &worker).unwrap_or_default();
        assert!(
            completions(&worker_received).is_empty(),
            "a completion must never be delivered into the worker that produced it: \
             {worker_received}"
        );
    }

    /// Map line 1926 — *"Consider V1 usable when the user can enter and
    /// directly control any orchestrated worker."*
    ///
    /// A person, from their own terminal, sends text to a live worker
    /// (`glasshouse api send`), interrupts it with a real `SIGINT`
    /// (`glasshouse api interrupt`) and reads back what the worker's own
    /// terminal has printed (`glasshouse api read`) — the three verbs that
    /// together make an orchestrated worker something a person can enter and
    /// change, not merely watch.
    ///
    /// Mutation: refuse attach for sessions that have an owning orchestrator
    /// — realised here as the client asking a different, harmless verb
    /// instead of delivering the person's text, which is the shape a wrongly
    /// scoped refusal would take (the door has no separate "owning
    /// orchestrator" concept to gate on; every session it holds is already
    /// equally reachable to a person, and this mutation proves that path is
    /// the one actually carrying the delivery).
    #[test]
    fn a_user_can_enter_and_directly_control_an_orchestrated_worker() {
        let fixture = Fixture::new();
        let root = fixture.project_root("alpha");
        let server = Server::start(&fixture, &root);

        let worker = server.spawn("worker");
        wait_for("the worker's harness to start", || {
            fixture.argv(&root, &worker).is_some()
        });

        // Enter: a person's own text reaches the worker's terminal.
        let sent = fixture.client(
            &root,
            &[
                "api",
                "send",
                "--session",
                &worker,
                "--text",
                "typed-by-a-person",
            ],
        );
        assert!(
            sent.status.success(),
            "`glasshouse api send` failed: {}",
            stderr_of(&sent)
        );
        wait_for("the worker to read the line a person sent", || {
            fixture
                .received(&root, &worker)
                .is_some_and(|text| text.contains("typed-by-a-person"))
        });

        // Control: a real SIGINT reaches the worker's own process, and the
        // session survives it.
        let interrupted = fixture.client(&root, &["api", "interrupt", "--session", &worker]);
        assert!(interrupted.status.success(), "{}", stderr_of(&interrupted));
        wait_for("the worker to handle a real SIGINT", || {
            fixture.reacted_to_interrupt(&root, &worker)
        });
        let after = fixture.client(
            &root,
            &[
                "api",
                "send",
                "--session",
                &worker,
                "--text",
                "still-listening",
            ],
        );
        assert!(
            after.status.success(),
            "the session must survive an interrupt: {}",
            stderr_of(&after)
        );
        wait_for("the interrupted worker to read a later line", || {
            fixture
                .received(&root, &worker)
                .is_some_and(|text| text.contains("still-listening"))
        });

        // See: what the worker's terminal has printed in response comes
        // back to the reader.
        let read = fixture.client(&root, &["api", "read", "--session", &worker]);
        assert!(
            read.status.success(),
            "`glasshouse api read` failed: {}",
            stderr_of(&read)
        );
        let shown = stdout_of(&read);
        assert!(
            shown.contains("got:typed-by-a-person"),
            "a person's own delivered line must be visible in what comes back: {shown:?}"
        );
        assert!(
            shown.contains("got:still-listening"),
            "the exchange after the interrupt must be visible too: {shown:?}"
        );
    }
}

// ===========================================================================
// 1938 — cmux integration can expose or spawn a session externally, and is
// never required for normal operation.
// ===========================================================================

#[cfg(unix)]
mod cmux_optional {
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    /// The workspace reference the fake `cmux` hands out for every
    /// `workspace create` and reports as the caller's own from `identify`.
    const FAKE_WORKSPACE: &str = "workspace:9";

    const EXITING_HARNESS: &str = "#!/bin/sh\nexit 0\n";

    /// A fake `cmux` that answers the documented verbs the wrapper uses and
    /// logs every invocation — the same idiom `tests/cmux_presentation.rs`
    /// uses, restated here because integration test binaries share no code.
    const FAKE_CMUX: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_CMUX_LOG"
case "$1" in
  ping)
    echo PONG
    exit 0
    ;;
  identify)
    printf '{\n  "caller" : {\n    "surface_ref" : "surface:1",\n    "workspace_ref" : "workspace:9"\n  }\n}\n'
    exit 0
    ;;
  workspace)
    case "$2" in
      create)
        shift 2
        cmd=
        while [ $# -gt 0 ]; do
          case "$1" in
            --command) cmd=$2; shift 2 ;;
            *) shift ;;
          esac
        done
        if [ -n "$cmd" ]; then
          (sh -c "$cmd" </dev/null >/dev/null 2>&1 &)
        fi
        echo "OK workspace:9"
        exit 0
        ;;
    esac
    ;;
esac
echo "fake cmux: unsupported invocation: $*" >&2
exit 2
"#;

    struct Fixture {
        _tmp: tempfile::TempDir,
        base: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().expect("tempdir");
            let base = tmp.path().to_path_buf();

            let root = base.join("workspace");
            std::fs::create_dir_all(root.join(".git")).expect("create project root");

            let bin_dir = base.join("bin");
            std::fs::create_dir_all(&bin_dir).expect("create bin dir");
            let harness = install(&bin_dir, "fake-claude-code", EXITING_HARNESS);
            install(&bin_dir, "cmux", FAKE_CMUX);

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

            Fixture { _tmp: tmp, base }
        }

        fn root(&self) -> PathBuf {
            std::fs::canonicalize(self.base.join("workspace")).expect("canonicalize project root")
        }

        fn path_with_fake_cmux(&self) -> std::ffi::OsString {
            let mut paths = vec![self.base.join("bin")];
            paths.extend(
                std::env::var_os("PATH")
                    .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
                    .unwrap_or_default(),
            );
            std::env::join_paths(paths).expect("join PATH")
        }

        fn command(&self, inside_cmux: bool) -> Command {
            let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
            command
                .current_dir(self.root())
                .arg("--data-dir")
                .arg(self.base.join("data"))
                .arg("--config-dir")
                .arg(self.base.join("config"))
                .env("PATH", self.path_with_fake_cmux())
                .env("FAKE_CMUX_LOG", self.base.join("cmux.log"));
            if inside_cmux {
                command
                    .env("CMUX_SOCKET_PATH", self.base.join("cmux.sock"))
                    .env("CMUX_SURFACE_ID", "FAKE-SURFACE")
                    .env("CMUX_WORKSPACE_ID", "FAKE-WORKSPACE");
            } else {
                command
                    .env_remove("CMUX_SOCKET_PATH")
                    .env_remove("CMUX_SURFACE_ID")
                    .env_remove("CMUX_WORKSPACE_ID");
            }
            command
        }

        fn glasshouse(&self, inside_cmux: bool, args: &[&str]) -> Output {
            self.command(inside_cmux)
                .args(args)
                .output()
                .expect("the glasshouse binary must run")
        }
    }

    fn install(bin_dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = bin_dir.join(name);
        std::fs::write(&path, body).expect("write executable");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn stderr(output: &Output) -> String {
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    /// Map line 1938 — *"Consider V1 usable when cmux integration can
    /// expose or spawn a session externally without being required for
    /// normal operation."*
    ///
    /// With no cmux on `PATH` at all, a normal launch runs unchanged and a
    /// launch that asks for a pane degrades to running embedded, saying why.
    /// With a real (fake) `cmux` on `PATH`, a launch asking for a pane opens
    /// one, and the pane is recorded as metadata **on the same session
    /// row** — exactly one more session, presented `external`, naming the
    /// workspace — never as a second identity.
    ///
    /// Mutation: make a missing `cmux` an error on the normal path.
    #[test]
    fn cmux_exposes_a_session_externally_and_is_never_required_for_normal_operation() {
        let fixture = Fixture::new();

        // No cmux at all: normal operation is unaffected, and a launch that
        // asks for a pane degrades to headless rather than failing.
        let plain = fixture.glasshouse(false, &["launch", "claude-code", "--headless"]);
        assert!(
            plain.status.success(),
            "a normal launch must work without cmux: {}",
            stderr(&plain)
        );

        let asked_for_a_pane = fixture.glasshouse(
            false,
            &[
                "launch",
                "claude-code",
                "--headless",
                "--fresh",
                "--presentation",
                "cmux",
            ],
        );
        assert!(
            asked_for_a_pane.status.success(),
            "a launch asking for cmux must still run when cmux is absent: {}",
            stderr(&asked_for_a_pane)
        );
        assert!(
            stderr(&asked_for_a_pane).contains("cmux is not available"),
            "the launch must say why it did not open a pane: {}",
            stderr(&asked_for_a_pane)
        );

        let runtime_without_cmux = super::open_runtime(&fixture.base, &fixture.root());
        let sessions_without_cmux =
            glasshouse::session::ProjectSessions::open(&runtime_without_cmux)
                .unwrap()
                .store()
                .list()
                .unwrap();
        assert_eq!(
            sessions_without_cmux.len(),
            2,
            "one session per launch above"
        );
        assert!(
            sessions_without_cmux
                .iter()
                .all(|record| record.presentation_ref.is_none()),
            "no pane was recorded without cmux: {sessions_without_cmux:?}"
        );

        // A real cmux on PATH: the pane is opened, and it is metadata on the
        // session the pane's own launch recorded — not a second session.
        let opened = fixture.glasshouse(
            true,
            &[
                "launch",
                "claude-code",
                "--headless",
                "--fresh",
                "--presentation",
                "cmux",
            ],
        );
        assert!(opened.status.success(), "{}", stderr(&opened));

        let runtime_with_cmux = super::open_runtime(&fixture.base, &fixture.root());
        let sessions = glasshouse::session::ProjectSessions::open(&runtime_with_cmux)
            .unwrap()
            .store()
            .list()
            .unwrap();
        assert_eq!(
            sessions.len(),
            3,
            "exactly one more session than before — the pane's own launch recorded \
             it once, not as a second identity: {sessions:?}"
        );
        let paned = sessions
            .iter()
            .find(|record| record.presentation_ref.as_deref() == Some(FAKE_WORKSPACE))
            .unwrap_or_else(|| panic!("no session recorded the pane: {sessions:?}"));
        assert_eq!(
            paned.presentation,
            glasshouse::session::SessionPresentation::External,
            "the paned session must be presented `external`: {paned:?}"
        );
    }
}

// ===========================================================================
// 1927 — durable project memory stores each of the six initial memory kinds.
// 1928 — project memory is searched with FTS5, not a substring scan.
// ===========================================================================

struct MemoryFixture {
    _workspace: tempfile::TempDir,
    _data: tempfile::TempDir,
    runtime: Runtime,
}

impl MemoryFixture {
    fn new() -> Self {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".git")).unwrap();
        let data = tempfile::tempdir().unwrap();

        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            data.path().to_str().unwrap(),
            "--config-dir",
            data.path().to_str().unwrap(),
        ])
        .unwrap();
        let runtime = bootstrap(&cli, workspace.path()).unwrap();

        Self {
            _workspace: workspace,
            _data: data,
            runtime,
        }
    }

    fn open(&self) -> ProjectMemory {
        ProjectMemory::open(&self.runtime).unwrap()
    }
}

/// Map line 1927 — *"Consider V1 usable when project-specific durable memory
/// can store the six initial memory kinds."*
///
/// Each of Phase 20's six kinds is recorded, the kind returned at write time
/// is checked, and — separately — each is read back by id, so a kind that
/// survived the write but not a fresh read (or vice versa) would be caught.
///
/// Mutation: collapse two kinds into one on write.
#[test]
fn project_memory_stores_each_of_the_six_kinds_and_reads_each_back_with_its_kind_intact() {
    let fixture = MemoryFixture::new();
    let project = fixture.open();
    let store = project.store();

    let kinds = [
        (
            MemoryKind::Decision,
            "Chose SQLite over Postgres for project-local storage.",
        ),
        (
            MemoryKind::Constraint,
            "Every checkpoint must fit inside 8 KiB once trimmed.",
        ),
        (
            MemoryKind::Feature,
            "Glasshouse can hand a checkpoint to a session under a different harness.",
        ),
        (
            MemoryKind::Finding,
            "The Linux pty gate is flaky under full workspace load.",
        ),
        (
            MemoryKind::FailedAttempt,
            "Tried mocking the database in the isolation tests; discarded it.",
        ),
        (
            MemoryKind::Todo,
            "Wire the resume path's degrade sink into a behavioural test.",
        ),
    ];

    let mut ids = Vec::new();
    for (kind, body) in kinds {
        let recorded = store.record(NewMemory::new(kind, body)).unwrap();
        assert_eq!(
            recorded.kind, kind,
            "the kind handed back at write time must be the one asked for"
        );
        ids.push((recorded.id, kind));
    }

    for (id, kind) in ids {
        let read_back = store
            .get(&id)
            .unwrap()
            .unwrap_or_else(|| panic!("memory {id} must still be there"));
        assert_eq!(
            read_back.kind, kind,
            "kind {kind:?} must survive a fresh read back by id"
        );
    }
}

/// Map line 1928 — *"Consider V1 usable when project memory can be searched
/// with FTS5."*
///
/// A two-word query is answered only by AND-across-indexed-columns
/// semantics: one term sits in the memory's `subject`, the other in its
/// `body`, so no literal substring of either column contains the query —
/// only an FTS5 `MATCH`, which searches every indexed column and ANDs the
/// terms, can join them.
///
/// Mutation: route the search through the non-FTS path.
#[test]
fn project_memory_is_searched_with_fts5_across_indexed_columns_not_a_substring_scan() {
    let fixture = MemoryFixture::new();
    let project = fixture.open();
    let store = project.store();

    let target = store
        .record(
            NewMemory::new(
                MemoryKind::Constraint,
                "The gateway backs off for thirty seconds after too many requests.",
            )
            .with_subject(Some("throttle limits")),
        )
        .unwrap();

    // A decoy carrying only one of the two terms, so the two-word result
    // below is a real intersection rather than the store happening to hold
    // one memory (§80).
    store
        .record(NewMemory::new(
            MemoryKind::Finding,
            "The nightly export backs up the whole database to cold storage.",
        ))
        .unwrap();

    // Sanity: the literal two-word query never appears as a contiguous
    // substring anywhere the record was written, so a plain LIKE scan of
    // either column could not have found it by accident.
    let subject = target.subject.clone().unwrap_or_default();
    let haystack = format!("{subject} {}", target.body).to_lowercase();
    assert!(
        !haystack.contains("throttle backs") && !haystack.contains("backs throttle"),
        "the query must not be a literal substring of the stored text: {haystack:?}"
    );

    let results = store
        .search("throttle backs", SearchScope::Current, 10)
        .unwrap();
    assert_eq!(
        results.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
        vec![target.id.clone()],
        "an FTS5 MATCH joins a term in the subject with a term in the body; \
         a substring scan of either column alone could not: {results:#?}"
    );

    // Each term alone finds a broader set, so the query above genuinely ANDs
    // rather than the corpus only ever having one memory in it.
    assert!(
        !store
            .search("backs", SearchScope::Current, 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        !store
            .search("throttle", SearchScope::Current, 10)
            .unwrap()
            .is_empty()
    );
}

// ===========================================================================
// 1929 — a small portable checkpoint hands work from one harness to another.
// ===========================================================================

#[cfg(unix)]
mod checkpoint_handoff {
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    fn install(bin_dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = bin_dir.join(name);
        std::fs::write(&path, body).expect("write executable");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn stdout(output: &Output) -> String {
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn stderr(output: &Output) -> String {
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        base: PathBuf,
    }

    impl Fixture {
        /// A project configured with two harnesses under two different
        /// integration ids, each an executable that dumps its own argv to a
        /// file named after the integration and exits — enough to prove
        /// what a launch handed the process, without needing the harness to
        /// stay alive.
        fn new() -> Self {
            let tmp = tempfile::tempdir().expect("tempdir");
            let base = tmp.path().to_path_buf();

            let root = base.join("workspace");
            std::fs::create_dir_all(root.join(".git")).expect("create project root");

            let bin_dir = base.join("bin");
            std::fs::create_dir_all(&bin_dir).expect("create bin dir");
            let dump_argv = "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$ARGV_LOG\"\nexit 0\n";
            let source_harness = install(&bin_dir, "fake-claude-code", dump_argv);
            let target_harness = install(&bin_dir, "fake-codex", dump_argv);

            let config_dir = base.join("config");
            std::fs::create_dir_all(&config_dir).expect("create config dir");
            let escape = |p: &Path| p.display().to_string().replace('\\', "\\\\");
            std::fs::write(
                config_dir.join("config.toml"),
                format!(
                    "version = 1\n\n\
                     [integrations.claude-code]\nenabled = true\nexecutable = \"{}\"\n\n\
                     [integrations.codex]\nenabled = true\nexecutable = \"{}\"\n",
                    escape(&source_harness),
                    escape(&target_harness),
                ),
            )
            .expect("write user config");

            Fixture { _tmp: tmp, base }
        }

        fn root(&self) -> PathBuf {
            std::fs::canonicalize(self.base.join("workspace")).expect("canonicalize project root")
        }

        fn glasshouse(&self, argv_log: &Path, args: &[&str]) -> Output {
            Command::new(env!("CARGO_BIN_EXE_glasshouse"))
                .current_dir(self.root())
                .arg("--data-dir")
                .arg(self.base.join("data"))
                .arg("--config-dir")
                .arg(self.base.join("config"))
                .env("ARGV_LOG", argv_log)
                .args(args)
                .output()
                .expect("the glasshouse binary must run")
        }
    }

    /// Map line 1929 — *"Consider V1 usable when a small portable checkpoint
    /// can hand work from one harness to another."*
    ///
    /// A session is started under `claude-code`, checkpointed with an
    /// objective and a state; a fresh launch under a **different** harness,
    /// `codex`, is asked to start `--from-checkpoint`, and the fresh
    /// session's own process — a different executable entirely — receives
    /// the checkpoint's handoff text on its own command line, carrying the
    /// objective and state forward. The source session is left untouched.
    ///
    /// Mutation: bind the checkpoint to the writing harness.
    #[test]
    fn a_checkpoint_written_under_one_harness_hands_work_to_a_session_of_a_different_harness() {
        let fixture = Fixture::new();
        let no_log = fixture.base.join("unused.log");

        let launched = fixture.glasshouse(&no_log, &["launch", "claude-code", "--headless"]);
        assert!(launched.status.success(), "{}", stderr(&launched));

        let runtime = super::open_runtime(&fixture.base, &fixture.root());
        let sessions = glasshouse::session::ProjectSessions::open(&runtime).unwrap();
        let before = sessions.store().list().unwrap();
        assert_eq!(before.len(), 1, "one session from the first launch");
        let source = before[0].clone();
        assert_eq!(source.harness, "claude-code");

        let saved = fixture.glasshouse(
            &no_log,
            &[
                "checkpoint",
                "save",
                "--session",
                source.id.as_str(),
                "--objective",
                "prove line 1929's cross-harness handoff",
                "--state",
                "the source session is checkpointed and about to hand off",
            ],
        );
        assert!(
            saved.status.success(),
            "`glasshouse checkpoint save` failed: {}",
            stderr(&saved)
        );
        assert!(
            stdout(&saved).contains("checkpoint "),
            "a successful save must name the checkpoint it wrote: {}",
            stdout(&saved)
        );

        let target_log = fixture.base.join("target-argv.log");
        let handed_off = fixture.glasshouse(
            &target_log,
            &[
                "launch",
                "codex",
                "--headless",
                "--from-checkpoint",
                "latest",
            ],
        );
        assert!(
            handed_off.status.success(),
            "`glasshouse launch codex --from-checkpoint` failed: {}",
            stderr(&handed_off)
        );

        let argv = std::fs::read_to_string(&target_log)
            .expect("the target harness must have recorded its own command line");
        assert!(
            argv.contains("prove line 1929's cross-harness handoff"),
            "the fresh session's own process must receive the checkpoint's objective: {argv:?}"
        );
        assert!(
            argv.contains("the source session is checkpointed and about to hand off"),
            "the fresh session's own process must receive the checkpoint's state: {argv:?}"
        );

        let after = sessions.store().list().unwrap();
        assert_eq!(
            after.len(),
            2,
            "the handoff must add one fresh session, not replace the source: {after:?}"
        );
        let fresh = after
            .iter()
            .find(|record| record.id != source.id)
            .expect("a second, fresh session");
        assert_eq!(
            fresh.harness, "codex",
            "the fresh session must run under the harness the launch asked for, \
             not the one the checkpoint was written under: {fresh:?}"
        );

        // The source session is left exactly as it was.
        let source_after = sessions.store().get(&source.id).unwrap().unwrap();
        assert_eq!(
            source_after, source,
            "the source session's own record must be untouched by the handoff"
        );
    }
}
