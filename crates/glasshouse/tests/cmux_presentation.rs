//! Phase 17 (capability map lines 754–763) and Phase 54 (1892–1895): cmux
//! as an *optional* presentation backend, proven through the shipped binary.
//!
//! Every test here drives `glasshouse` itself — `launch`, `sessions`,
//! `sessions focus`, and the control door — against a **fake `cmux`** placed
//! first on `PATH`: a shell script that answers exactly as the documented
//! commands do (`PONG`, `OK workspace:7`, an `identify --json` document) and
//! appends every invocation to a log the test reads back. That is the point
//! of the seam `integrations::cmux` puts around cmux (practice §35): the
//! production callers are exercised end to end, and what cmux was asked is
//! a fact on disk rather than an inference.
//!
//! The fake's `workspace create` also *runs* the `--command` it is given, in
//! the background, exactly as cmux's login shell would — so the process the
//! outer launch puts in the pane really starts, really asks the fake
//! `identify` which workspace it is in, and really records the session. The
//! outer process's wait for that record is therefore a real wait.
//!
//! Absence is the first-class path (line 755): the first test runs every
//! verb with no cmux at all and with a cmux whose socket is dead, and checks
//! that each one says so and carries on.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use clap::Parser;
use glasshouse::session::{NewSession, ProjectSessions, SessionPresentation};
use glasshouse::{Cli, Runtime, bootstrap};

// -------------------------------------------------------------------------
// Fixture: a project, a fake harness, a fake cmux, and the binary
// -------------------------------------------------------------------------

/// The workspace reference the fake `cmux` hands out for every `workspace
/// create`, and reports as the caller's from `identify`.
const FAKE_WORKSPACE: &str = "workspace:7";

/// A fake harness that exits at once — enough for a launch to record a
/// session and finish.
#[cfg(unix)]
const EXITING_HARNESS: &str = "#!/bin/sh\nexit 0\n";
#[cfg(windows)]
const EXITING_HARNESS: &str = "@echo off\r\nexit /b 0\r\n";

/// A fake harness that stays alive reading its terminal, so a session the
/// door spawned is still *live* when the door is asked to send to it.
#[cfg(unix)]
const LIVE_HARNESS: &str = "#!/bin/sh\nexec cat\n";

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    runtime: Runtime,
    /// One line per fake `cmux` invocation: its arguments, space-joined.
    cmux_log: PathBuf,
    /// Where the fake `workspace create` sends the output of the command it
    /// runs in the background — the "pane".
    pane_out: PathBuf,
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
        let harness = install_executable(&bin_dir, "fake-claude-code", harness_body);
        #[cfg(unix)]
        install_executable(&bin_dir, "cmux", FAKE_CMUX);

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

        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            config_dir.to_str().unwrap(),
        ])
        .unwrap();
        let runtime = bootstrap(&cli, &root).unwrap();

        let cmux_log = base.join("cmux.log");
        std::fs::write(&cmux_log, "").expect("create the cmux log");
        let pane_out = base.join("pane.out");
        Fixture {
            _tmp: tmp,
            base,
            runtime,
            cmux_log,
            pane_out,
        }
    }

    fn root(&self) -> &Path {
        self.runtime.project().root()
    }

    /// `glasshouse` with this project's directories, `PATH` led by the
    /// fake `cmux`, and the cmux control environment set or removed.
    fn command(&self, inside_cmux: Cmux) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command
            .current_dir(self.root())
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .env("PATH", self.path_with_fake_cmux())
            .env("FAKE_CMUX_LOG", &self.cmux_log)
            .env("FAKE_CMUX_PANE_OUT", &self.pane_out)
            .env_remove("FAKE_CMUX_DEAD");
        match inside_cmux {
            Cmux::Absent => {
                command
                    .env_remove("CMUX_SOCKET_PATH")
                    .env_remove("CMUX_SURFACE_ID")
                    .env_remove("CMUX_WORKSPACE_ID");
            }
            Cmux::Answering | Cmux::Dead => {
                command
                    .env("CMUX_SOCKET_PATH", self.base.join("cmux.sock"))
                    .env("CMUX_SURFACE_ID", "FAKE-SURFACE")
                    .env("CMUX_WORKSPACE_ID", "FAKE-WORKSPACE");
                if inside_cmux == Cmux::Dead {
                    command.env("FAKE_CMUX_DEAD", "1");
                }
            }
        }
        command
    }

    fn glasshouse(&self, inside_cmux: Cmux, args: &[&str]) -> Output {
        self.command(inside_cmux)
            .args(args)
            .output()
            .expect("the glasshouse binary must run")
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

    fn cmux_calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.cmux_log)
            .expect("read the cmux log")
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn cmux_calls_starting_with(&self, prefix: &str) -> Vec<String> {
        self.cmux_calls()
            .into_iter()
            .filter(|call| call.starts_with(prefix))
            .collect()
    }

    fn sessions(&self) -> String {
        stdout(&self.glasshouse(Cmux::Absent, &["sessions"]))
    }

    fn session_ids(&self) -> Vec<String> {
        let sessions = ProjectSessions::open(&self.runtime).unwrap();
        let mut records = sessions.store().list().unwrap();
        records.sort_by_key(|record| record.created_at);
        records
            .into_iter()
            .map(|record| record.id.as_str().to_owned())
            .collect()
    }

    /// A session recorded directly through the store — the way a launch
    /// inside a pane records one — without starting any process.
    fn record_external_session(&self, reference: &str) -> String {
        let sessions = ProjectSessions::open(&self.runtime).unwrap();
        let record = sessions
            .store()
            .create(
                NewSession::embedded("claude-code")
                    .with_presentation(SessionPresentation::External)
                    .with_presentation_ref(Some(reference.to_owned())),
            )
            .unwrap();
        record.id.as_str().to_owned()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cmux {
    /// No cmux control environment at all.
    Absent,
    /// Inside cmux, and the fake answers `ping`.
    Answering,
    /// Inside cmux by the environment's account, but `ping` fails — the
    /// stale-variable case line 754's ruling names.
    Dead,
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn both(output: &Output) -> String {
    format!("{}{}", stdout(output), stderr(output))
}

#[cfg(unix)]
fn install_executable(bin_dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join(name);
    std::fs::write(&path, body).expect("write executable");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_executable(bin_dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = bin_dir.join(format!("{name}.cmd"));
    std::fs::write(&path, body).expect("write executable");
    path
}

/// The fake `cmux`. It answers the five documented commands the integration
/// may use, refuses anything else loudly, logs every call, and — for
/// `workspace create` — runs the given `--command` in the background the
/// way cmux's login shell would, with the pane's output captured to a file.
#[cfg(unix)]
const FAKE_CMUX: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_CMUX_LOG"
case "$1" in
  ping)
    if [ -n "$FAKE_CMUX_DEAD" ]; then
      echo "Error: dial unix: connect: no such file or directory" >&2
      exit 1
    fi
    echo PONG
    exit 0
    ;;
  identify)
    printf '{\n  "caller" : {\n    "surface_ref" : "surface:9",\n    "workspace_ref" : "workspace:7"\n  },\n  "focused" : {\n    "surface_ref" : "surface:1",\n    "workspace_ref" : "workspace:1"\n  }\n}\n'
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
        if [ -n "$cmd" ] && [ -n "$FAKE_CMUX_PANE_OUT" ]; then
          (sh -c "$cmd" </dev/null >"$FAKE_CMUX_PANE_OUT" 2>&1 &)
        fi
        echo "OK workspace:7"
        exit 0
        ;;
      select)
        echo "OK $3"
        exit 0
        ;;
    esac
    ;;
  send)
    echo "OK surface:9 workspace:7"
    exit 0
    ;;
esac
echo "fake cmux: unsupported invocation: $*" >&2
exit 2
"#;

// -------------------------------------------------------------------------
// 755, 1894 — absence is the first-class path
// -------------------------------------------------------------------------

/// With no cmux — and with a cmux whose environment is set but whose socket
/// is dead — every verb answers *cmux is not available* with the reason and
/// carries on: the launch runs where it always did, the listing is laid out
/// exactly as before panes existed, `focus` refuses by name, and an unknown
/// backend is refused before anything is recorded.
#[test]
fn without_cmux_every_command_runs_embedded_and_says_so() {
    let fixture = Fixture::new(EXITING_HARNESS);

    // Outside cmux altogether.
    let launched = fixture.glasshouse(
        Cmux::Absent,
        &[
            "launch",
            "claude-code",
            "--headless",
            "--presentation",
            "cmux",
        ],
    );
    assert!(launched.status.success(), "{}", both(&launched));
    let said = stderr(&launched);
    assert!(
        said.contains(
            "cmux is not available (not running inside a cmux surface); the session runs headless"
        ),
        "the launch must say why it is not in a pane:\n{said}"
    );
    let ids = fixture.session_ids();
    assert_eq!(ids.len(), 1, "one launch, one session");

    let listing = fixture.sessions();
    assert!(
        listing.contains("  headless  "),
        "the session ran headless, and the listing says so:\n{listing}"
    );
    assert!(
        !listing.contains("workspace:"),
        "no pane was recorded:\n{listing}"
    );
    // Byte-for-byte the layout a listing had before panes existed: the
    // `PRESENTED` column is nine wide when nothing in it needs more.
    assert!(
        listing.contains("ROLE          PRESENTED  LAST ACTIVITY"),
        "a listing with no pane keeps its old column widths:\n{listing}"
    );
    let shown = stdout(&fixture.glasshouse(Cmux::Absent, &["sessions", "show", &ids[0]]));
    assert!(shown.contains("presented          headless\n"), "{shown}");
    assert!(shown.contains("presentation ref   -\n"), "{shown}");

    // `focus` on a session with no pane refuses by name and touches cmux
    // never.
    let focused = fixture.glasshouse(Cmux::Absent, &["sessions", "focus", &ids[0]]);
    assert!(!focused.status.success(), "{}", both(&focused));
    assert!(
        stderr(&focused).contains("has no external pane to focus; it is presented headless"),
        "{}",
        stderr(&focused)
    );

    // An unknown backend is refused before a harness is selected or a
    // session recorded. (`--fresh` from here on: the router would otherwise
    // continue the session above, which is its job and not this test's
    // subject.)
    let refused = fixture.glasshouse(
        Cmux::Absent,
        &[
            "launch",
            "claude-code",
            "--headless",
            "--fresh",
            "--presentation",
            "tmux",
        ],
    );
    assert!(!refused.status.success(), "{}", both(&refused));
    assert!(
        stderr(&refused).contains("unknown presentation backend `tmux`; known backends: cmux"),
        "{}",
        stderr(&refused)
    );
    assert_eq!(
        fixture.session_ids().len(),
        1,
        "a refused launch records nothing"
    );

    // A malformed pane reference is refused the same way.
    let refused = fixture.glasshouse(
        Cmux::Absent,
        &[
            "launch",
            "claude-code",
            "--headless",
            "--fresh",
            "--presentation-ref",
            "workspace:1; rm -rf /",
        ],
    );
    assert!(!refused.status.success(), "{}", both(&refused));
    assert!(
        stderr(&refused).contains("is not a cmux reference"),
        "{}",
        stderr(&refused)
    );
    assert_eq!(fixture.session_ids().len(), 1);

    // Inside cmux by the environment's account, with a cmux that does not
    // answer: a set variable in a dead environment reads as absent (754).
    #[cfg(unix)]
    {
        let launched = fixture.glasshouse(
            Cmux::Dead,
            &[
                "launch",
                "claude-code",
                "--headless",
                "--fresh",
                "--presentation",
                "cmux",
            ],
        );
        assert!(launched.status.success(), "{}", both(&launched));
        let said = stderr(&launched);
        assert!(
            said.contains("cmux is not available (cmux did not answer a ping:")
                && said.contains("the session runs headless"),
            "{said}"
        );
        assert_eq!(
            fixture.cmux_calls(),
            vec!["ping"],
            "detection pings once and, unanswered, asks cmux nothing more"
        );
        assert_eq!(fixture.session_ids().len(), 2, "the launch still ran");
        assert!(
            !fixture.sessions().contains("workspace:"),
            "{}",
            fixture.sessions()
        );

        // The same for `--presentation-ref caller`: with no cmux to ask,
        // the launch says so and records no pane.
        let launched = fixture.glasshouse(
            Cmux::Absent,
            &[
                "launch",
                "claude-code",
                "--headless",
                "--fresh",
                "--presentation-ref",
                "caller",
            ],
        );
        assert!(launched.status.success(), "{}", both(&launched));
        assert!(
            stderr(&launched).contains(
                "cmux is not available (not running inside a cmux surface); the session runs \
                 headless"
            ),
            "{}",
            stderr(&launched)
        );
        assert!(!fixture.sessions().contains("workspace:"));
    }
}

// -------------------------------------------------------------------------
// 754, 757, 760, 761 — an external spawn, end to end
// -------------------------------------------------------------------------

/// `glasshouse launch --presentation cmux`, inside a cmux that answers:
/// detection pings, one `workspace create` is issued with the project root
/// as `--cwd` and this same launch as `--command`, the process inside the
/// pane asks cmux which workspace it is in and records the session as
/// `external` with that workspace, and the outer process — having started
/// nothing else — reports the session id once the record appears.
#[cfg(unix)]
#[test]
fn an_external_spawn_records_the_pane_as_presentation_metadata() {
    let fixture = Fixture::new(EXITING_HARNESS);

    // A launch that would be refused is refused *here*, before any pane
    // opens.
    let refused = fixture.glasshouse(
        Cmux::Answering,
        &["launch", "no-such-harness", "--presentation", "cmux"],
    );
    assert!(!refused.status.success(), "{}", both(&refused));
    assert!(
        fixture
            .cmux_calls_starting_with("workspace create")
            .is_empty(),
        "a refused launch opens no pane: {:?}",
        fixture.cmux_calls()
    );

    let opened = fixture.glasshouse(
        Cmux::Answering,
        &[
            "launch",
            "claude-code",
            "--headless",
            "--presentation",
            "cmux",
            "--",
            "--resume",
        ],
    );
    assert!(opened.status.success(), "{}", both(&opened));
    let ids = fixture.session_ids();
    assert_eq!(
        ids.len(),
        1,
        "the process inside the pane recorded the session, and nothing else did"
    );
    assert!(
        stdout(&opened).contains(&format!(
            "glasshouse: session {} is running in cmux {FAKE_WORKSPACE}",
            ids[0]
        )),
        "the outer process reports the id the pane recorded:\n{}",
        both(&opened)
    );

    // What cmux was asked, in order: the outer detection's ping, one
    // `workspace create`, and then — from inside the pane — the inner
    // detection's ping and the `identify` that resolved the workspace.
    let calls = fixture.cmux_calls();
    let creates = fixture.cmux_calls_starting_with("workspace create");
    assert_eq!(creates.len(), 1, "exactly one workspace: {calls:?}");
    let create = &creates[0];
    assert_eq!(calls[0], "ping", "{calls:?}");
    assert_eq!(&calls[1], create, "{calls:?}");
    assert_eq!(
        &calls[2..],
        ["ping", "identify --json"],
        "the pane's process detects cmux and asks which workspace it is in: {calls:?}"
    );

    // The pane opens in the project root, runs this same launch, and tells
    // the process inside where to record itself.
    let root = fixture.root().display().to_string();
    assert!(
        create.contains(&format!("--cwd {root}")),
        "the pane's directory is the project root and nothing else:\n{create}"
    );
    for expected in [
        "launch claude-code",
        "--headless",
        "--presentation-ref caller",
        "-- --resume",
        &format!("--scope {root}"),
        "--data-dir",
        "--config-dir",
        "--focus true",
    ] {
        assert!(
            create.contains(expected),
            "the pane command must carry `{expected}`:\n{create}"
        );
    }
    assert!(
        !create.contains("--presentation cmux"),
        "the pane's launch must not ask for a pane of its own:\n{create}"
    );
    assert!(
        !create.contains("CMUX_SOCKET") && !create.contains("CAPABILITY"),
        "no cmux variable or capability token travels in the command:\n{create}"
    );

    // The record: `external`, with the workspace cmux itself named.
    let listing = fixture.sessions();
    assert!(
        listing.contains(&format!("external {FAKE_WORKSPACE}")),
        "`glasshouse sessions` shows the pane:\n{listing}"
    );
    let shown = stdout(&fixture.glasshouse(Cmux::Absent, &["sessions", "show", &ids[0]]));
    assert!(shown.contains("presented          external\n"), "{shown}");
    assert!(
        shown.contains(&format!("presentation ref   {FAKE_WORKSPACE}\n")),
        "{shown}"
    );

    // And a reference given by hand is recorded as given, asking cmux
    // nothing.
    let calls_before = fixture.cmux_calls().len();
    let hosted = fixture.glasshouse(
        Cmux::Absent,
        &[
            "launch",
            "claude-code",
            "--headless",
            "--fresh",
            "--presentation-ref",
            "workspace:12",
        ],
    );
    assert!(hosted.status.success(), "{}", both(&hosted));
    assert_eq!(
        fixture.cmux_calls().len(),
        calls_before,
        "a literal ref asks cmux nothing"
    );
    let listing = fixture.sessions();
    assert!(listing.contains("external workspace:12"), "{listing}");

    // A pane's launch that the router resolves to *continuing* a recorded
    // session moves that session into the pane rather than recording
    // nothing: the record it continues now names the pane.
    let continued = fixture.glasshouse(
        Cmux::Absent,
        &[
            "launch",
            "claude-code",
            "--headless",
            "--presentation-ref",
            "workspace:13",
        ],
    );
    assert!(continued.status.success(), "{}", both(&continued));
    let said = stderr(&continued);
    let Some(continued_id) = said
        .lines()
        .find_map(|line| line.strip_prefix("glasshouse: continuing session "))
        .and_then(|rest| rest.split(' ').next())
    else {
        panic!("the router was expected to continue a recorded session here:\n{said}");
    };
    let shown = stdout(&fixture.glasshouse(Cmux::Absent, &["sessions", "show", continued_id]));
    assert!(shown.contains("presented          external\n"), "{shown}");
    assert!(
        shown.contains("presentation ref   workspace:13\n"),
        "the continued session is recorded where it is now shown:\n{shown}"
    );
}

// -------------------------------------------------------------------------
// 758, 759 — focus and send go through the integration; the door first
// -------------------------------------------------------------------------

#[cfg(unix)]
mod door {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::process::{Child, Stdio};
    use std::time::{Duration, Instant};

    use super::{Cmux, Fixture};

    const TIMEOUT: Duration = Duration::from_secs(20);

    pub struct Server {
        child: Child,
        socket: PathBuf,
    }

    impl Server {
        pub fn start(fixture: &Fixture, inside_cmux: Cmux) -> Self {
            let mut child = fixture
                .command(inside_cmux)
                .arg("--scope")
                .arg(fixture.root())
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
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// `sessions focus` issues exactly one `workspace select <ref>` for a
/// session with a pane, and refuses when cmux is not there to select it.
/// Through the door, `send_message` prefers Glasshouse's own delivery —
/// a live session is reached without cmux being asked anything — and falls
/// back to `cmux send` only for a session no runtime here holds, saying
/// which happened either way. A door spawn asked for cmux opens the pane,
/// reports the workspace and the session the pane recorded, and delivers
/// the task through cmux; the same spawn without cmux runs headless here
/// and says so.
#[cfg(unix)]
#[test]
fn focus_and_send_go_through_the_integration_and_the_door_is_preferred() {
    use door::Server;

    let fixture = Fixture::new(LIVE_HARNESS);
    let external = fixture.record_external_session(FAKE_WORKSPACE);

    // --- focus (759) ------------------------------------------------------
    let focused = fixture.glasshouse(Cmux::Answering, &["sessions", "focus", &external]);
    assert!(focused.status.success(), "{}", both(&focused));
    assert!(
        stdout(&focused).contains(&format!(
            "glasshouse: focused cmux {FAKE_WORKSPACE} for session {external}"
        )),
        "{}",
        both(&focused)
    );
    assert_eq!(
        fixture.cmux_calls_starting_with("workspace select"),
        vec![format!("workspace select {FAKE_WORKSPACE}")],
        "exactly one select, for exactly the recorded workspace: {:?}",
        fixture.cmux_calls()
    );

    let refused = fixture.glasshouse(Cmux::Absent, &["sessions", "focus", &external]);
    assert!(!refused.status.success(), "{}", both(&refused));
    assert!(
        stderr(&refused).contains(&format!(
            "is presented in cmux {FAKE_WORKSPACE}, but cmux is not available from here"
        )),
        "{}",
        stderr(&refused)
    );
    assert_eq!(
        fixture.cmux_calls_starting_with("workspace select").len(),
        1,
        "a refusal selects nothing"
    );

    // A stored reference that is not a cmux reference is refused by name
    // before cmux is asked — the validation on the way out.
    let bogus = fixture.record_external_session("workspace:1 --window window:2");
    let refused = fixture.glasshouse(Cmux::Answering, &["sessions", "focus", &bogus]);
    assert!(!refused.status.success(), "{}", both(&refused));
    assert!(
        stderr(&refused).contains("is not a cmux reference"),
        "{}",
        stderr(&refused)
    );
    assert_eq!(
        fixture.cmux_calls_starting_with("workspace select").len(),
        1
    );

    // --- send (758), through the door ------------------------------------
    let server = Server::start(&fixture, Cmux::Answering);

    // A session no runtime here holds, with a pane: the door cannot deliver,
    // so cmux does, and the answer says so.
    let sent = server.call(serde_json::json!({
        "op": "send_message",
        "session": external,
        "text": "hello there",
    }));
    assert_eq!(
        sent,
        serde_json::json!({
            "status": "ok",
            "result": { "via": "cmux", "presentation_ref": FAKE_WORKSPACE },
        }),
        "{sent}"
    );
    assert_eq!(
        fixture.cmux_calls_starting_with("send"),
        vec![format!(
            "send --workspace {FAKE_WORKSPACE} -- hello there\\r"
        )],
        "{:?}",
        fixture.cmux_calls()
    );

    // A session this door spawned and holds live: delivered by the door,
    // and cmux is asked nothing.
    let spawned = server.call(serde_json::json!({
        "op": "spawn_session",
        "harness": "claude-code",
    }));
    let live = spawned["result"]["session"]
        .as_str()
        .unwrap_or_else(|| panic!("a headless spawn answers with a session: {spawned}"))
        .to_owned();
    assert!(
        spawned["result"].get("presentation_note").is_none(),
        "a spawn that asked for no backend carries no note: {spawned}"
    );
    let sends_before = fixture.cmux_calls_starting_with("send").len();
    let sent = server.call(serde_json::json!({
        "op": "send_message",
        "session": live,
        "text": "through the door",
    }));
    assert_eq!(
        sent,
        serde_json::json!({ "status": "ok", "result": { "via": "door" } }),
        "{sent}"
    );
    assert_eq!(
        fixture.cmux_calls_starting_with("send").len(),
        sends_before,
        "the door delivered; cmux was not asked: {:?}",
        fixture.cmux_calls()
    );

    // A session with no pane and no runtime is the refusal it always was.
    let orphan = {
        let sessions = ProjectSessions::open(&fixture.runtime).unwrap();
        let record = sessions
            .store()
            .create(NewSession::embedded("claude-code"))
            .unwrap();
        record.id.as_str().to_owned()
    };
    let refused = server.call(serde_json::json!({
        "op": "send_message",
        "session": orphan,
        "text": "nobody home",
    }));
    let message = refused["message"]
        .as_str()
        .unwrap_or_else(|| panic!("an error: {refused}"));
    assert!(
        message.contains("is not live in this Glasshouse"),
        "{message}"
    );
    assert!(
        !message.contains("cmux"),
        "no pane, no cmux in the answer: {message}"
    );

    // --- spawn into cmux (757, 761), through the door ---------------------
    let creates_before = fixture.cmux_calls_starting_with("workspace create").len();
    let spawned = server.call(serde_json::json!({
        "op": "spawn_session",
        "harness": "claude-code",
        "presentation": "cmux",
        "task": "do the thing",
        "args": ["--resume"],
    }));
    let result = &spawned["result"];
    assert_eq!(result["presentation"], "external", "{spawned}");
    assert_eq!(result["presentation_ref"], FAKE_WORKSPACE, "{spawned}");
    assert_eq!(result["task_delivery"], "cmux", "{spawned}");
    let in_pane = result["session"]
        .as_str()
        .unwrap_or_else(|| panic!("the pane's launch recorded a session: {spawned}"))
        .to_owned();
    assert!(
        fixture.session_ids().contains(&in_pane),
        "the id answered is one the store holds"
    );
    let creates = fixture.cmux_calls_starting_with("workspace create");
    assert_eq!(creates.len(), creates_before + 1, "{creates:?}");
    let create = creates.last().unwrap();
    assert!(
        create.contains("--focus false"),
        "an orchestrator's view is not stolen:\n{create}"
    );
    assert!(create.contains("launch claude-code"), "{create}");
    assert!(create.contains("--presentation-ref caller"), "{create}");
    assert!(create.contains("-- --resume"), "{create}");
    assert_eq!(
        fixture.cmux_calls_starting_with("send").last().unwrap(),
        &format!("send --workspace {FAKE_WORKSPACE} -- do the thing\\r"),
        "{:?}",
        fixture.cmux_calls()
    );
    let listing = fixture.sessions();
    assert!(
        listing.contains(&format!("external {FAKE_WORKSPACE}")),
        "{listing}"
    );

    let refused = server.call(serde_json::json!({
        "op": "spawn_session",
        "harness": "claude-code",
        "presentation": "tmux",
    }));
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|m| m.contains("unknown presentation backend `tmux`")),
        "{refused}"
    );

    // The same spawn with no cmux: headless here, and the answer says why.
    drop(server);
    let server = Server::start(&fixture, Cmux::Absent);
    let spawned = server.call(serde_json::json!({
        "op": "spawn_session",
        "harness": "claude-code",
        "presentation": "cmux",
    }));
    let result = &spawned["result"];
    assert!(result["session"].is_string(), "{spawned}");
    assert_eq!(result["presentation"], "headless", "{spawned}");
    assert!(
        result["presentation_note"]
            .as_str()
            .is_some_and(|note| note.contains("cmux is not available")),
        "{spawned}"
    );
}

// -------------------------------------------------------------------------
// 762, 763, 1892–1895 — recorded constraints, as tripwires
// -------------------------------------------------------------------------

/// The production code of every file under `session/` and `shell/` — with
/// comments stripped, so a doc sentence may still *mention* the word — is
/// scanned for `cmux`, case-insensitively.
fn production_code_naming_cmux(dir: &Path) -> Vec<PathBuf> {
    let mut offenders = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("read source dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().is_some_and(|ext| ext == "rs") {
                let source = std::fs::read_to_string(&path).expect("read source");
                if production_without_comments(&source)
                    .to_lowercase()
                    .contains("cmux")
                {
                    offenders.push(path);
                }
            }
        }
    }
    offenders.sort();
    offenders
}

/// Everything before the first `#[cfg(test)]`, minus comment lines.
fn production_without_comments(source: &str) -> String {
    source
        .split("#[cfg(test)]")
        .next()
        .expect("split always yields at least one part")
        .lines()
        .map(|line| match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every double-quoted string literal in `source`.
fn string_literals(source: &str) -> Vec<String> {
    let mut literals = Vec::new();
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '"' {
            continue;
        }
        let mut literal = String::new();
        let mut escaped = false;
        for ch in chars.by_ref() {
            if escaped {
                literal.push(ch);
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                break;
            } else {
                literal.push(ch);
            }
        }
        literals.push(literal);
    }
    literals
}

/// Every command word `cmux --help` lists on this machine's cmux, including
/// the `workspace` subcommands and the tmux-compatibility verbs. A literal
/// in the wrapper that equals one of these is an invocation, and must be one
/// of the five the module declares.
const CMUX_VERBS: &[&str] = &[
    "welcome",
    "docs",
    "settings",
    "config",
    "shortcuts",
    "disable-browser",
    "enable-browser",
    "browser-status",
    "agent-hibernation",
    "restore",
    "restore-session",
    "open",
    "diff",
    "feedback",
    "feed",
    "themes",
    "claude-teams",
    "codex-teams",
    "omo",
    "omx",
    "omc",
    "hooks",
    "ping",
    "iroh-diag",
    "version",
    "capabilities",
    "events",
    "auth",
    "login",
    "logout",
    "vm",
    "cloud",
    "remotes",
    "remote",
    "ai-accounts",
    "rpc",
    "simulator",
    "ios",
    "identify",
    "list-windows",
    "current-window",
    "new-window",
    "focus-window",
    "close-window",
    "move-workspace-to-window",
    "reorder-workspace",
    "reorder-workspaces",
    "workspace-action",
    "workspace",
    "todo",
    "move-tab-to-new-workspace",
    "list-workspaces",
    "new-workspace",
    "ssh",
    "mosh",
    "mosh-tmux",
    "ssh-tmux",
    "ssh-session-list",
    "ssh-session-attach",
    "ssh-session-cleanup",
    "remote-daemon-status",
    "new-split",
    "list-panes",
    "list-pane-surfaces",
    "tree",
    "top",
    "memory",
    "focus-pane",
    "new-pane",
    "new-surface",
    "close-surface",
    "move-surface",
    "split-off",
    "reorder-surface",
    "tab-action",
    "surface",
    "rename-tab",
    "drag-surface-to-split",
    "refresh-surfaces",
    "reload-config",
    "surface-health",
    "debug-terminals",
    "trigger-flash",
    "list-panels",
    "focus-panel",
    "close-workspace",
    "select-workspace",
    "rename-workspace",
    "rename-window",
    "current-workspace",
    "read-screen",
    "send",
    "send-key",
    "send-panel",
    "send-key-panel",
    "notify",
    "list-notifications",
    "dismiss-notification",
    "mark-notification-read",
    "open-notification",
    "jump-to-unread",
    "clear-notifications",
    "right-sidebar",
    "sidebar",
    "set-status",
    "clear-status",
    "list-status",
    "set-progress",
    "clear-progress",
    "log",
    "clear-log",
    "list-log",
    "sidebar-state",
    "set-app-focus",
    "simulate-app-active",
    "simulate-sidebar-drag",
    "capture-pane",
    "resize-pane",
    "pipe-pane",
    "wait-for",
    "swap-pane",
    "break-pane",
    "join-pane",
    "next-window",
    "previous-window",
    "last-window",
    "last-pane",
    "find-window",
    "clear-history",
    "set-hook",
    "popup",
    "bind-key",
    "unbind-key",
    "copy-mode",
    "set-buffer",
    "list-buffers",
    "paste-buffer",
    "respawn-pane",
    "display-message",
    "markdown",
    "browser",
    // `cmux workspace <subcommand>`
    "list",
    "create",
    "env",
    "close",
    "rename",
    "select",
    "status",
    "reconnect",
    "disconnect",
    "loading",
    "group",
];

/// Lines 762 and 763 as a constraint that fails the build when broken: the
/// session abstraction and the shell never name cmux in production code, so
/// neither can come to depend on it (1894), and cmux stays a presentation
/// backend rather than anything the core reaches for. Line 1893 the same
/// way: the wrapper's production code names no cmux verb outside the five
/// its `Subcommand` declares, and starts a process in exactly one place —
/// so the surface Glasshouse depends on is the one the module documents,
/// and widening it is a visible edit here and there (1892, 1895).
#[test]
fn the_session_and_shell_layers_never_name_cmux_and_the_wrapper_uses_only_documented_commands() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    for layer in ["session", "shell"] {
        let offenders = production_code_naming_cmux(&src.join(layer));
        assert!(
            offenders.is_empty(),
            "`src/{layer}/**` must never name cmux in production code (line 762); found it \
             in {offenders:?}"
        );
    }

    let wrapper = std::fs::read_to_string(src.join("integrations").join("cmux.rs"))
        .expect("read integrations/cmux.rs");
    let production = production_without_comments(&wrapper);
    let allowed: std::collections::BTreeSet<&str> = glasshouse::integrations::cmux::Subcommand::ALL
        .iter()
        .flat_map(|subcommand| subcommand.words().iter().copied())
        .collect();
    assert_eq!(
        allowed.iter().copied().collect::<Vec<_>>(),
        [
            "--json",
            "create",
            "identify",
            "ping",
            "select",
            "send",
            "workspace"
        ],
        "the declared surface is these five commands and nothing else"
    );
    let undeclared: Vec<String> = string_literals(&production)
        .into_iter()
        .filter(|literal| CMUX_VERBS.contains(&literal.as_str()))
        .filter(|literal| !allowed.contains(literal.as_str()))
        .collect();
    assert!(
        undeclared.is_empty(),
        "integrations/cmux.rs names cmux verbs its `Subcommand` does not declare (line 1893): \
         {undeclared:?}"
    );
    assert_eq!(
        production.matches("Command::new(").count(),
        1,
        "every cmux invocation goes through the one `run` in `CmuxCli`"
    );
}
