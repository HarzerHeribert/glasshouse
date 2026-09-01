//! Capability map lines 619 and 620 — the one missing in-session surface
//! `phase-9k.md` names as shared between them.
//!
//! Both lines were blocked on the same absence: a profile is selected at
//! launch, and nothing let a person reach a *running* session to change its
//! style or hand it a one-turn instruction. `glasshouse sessions restyle` and
//! `glasshouse sessions tell` are that surface. Both deliver through
//! `crate::api::send_message` — the same input path `glasshouse api send` and
//! a person's own typing already use — so what these tests actually exercise
//! is the warning gate in front of it (619) and the framing and refusal
//! rules around it (620), not a new way of reaching a session.
//!
//! `Fixture`/`Server` mirror `tests/api_event_log.rs`'s own scaffold: a real
//! `glasshouse api serve`, a fake harness that logs every line it reads from
//! its terminal under a name derived from the session it belongs to. The
//! warm-session scenarios need that — a warning that never fires because
//! nothing was ever live to check would prove nothing. The cold/InPlace/
//! unsupported-harness scenarios do not: those refusals (or lack of one) are
//! decided from the session's own stored record before any live delivery is
//! attempted, so those tests seed a record directly and never start a
//! daemon or a process.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use glasshouse::cli::Cli;
use glasshouse::session::{NewSession, ProjectSessions, SessionDisposition, SessionLifecycle};

const TIMEOUT: Duration = Duration::from_secs(30);

/// The marker `harness::response::one_turn_override` wraps every delivered
/// instruction in. Distinctive enough that a plain `contains` on it cannot
/// mistake ordinary session chatter for a delivered override.
const MARKER: &str = "[glasshouse] One-turn instruction from your operator";

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
}

impl Fixture {
    fn new() -> Self {
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
                "version = 1\nimplementation_policy = false\n\n[integrations.claude-code]\nenabled = \
                 true\nexecutable = \"{escaped}\"\n"
            ),
        )
        .expect("write user config");

        Self { _tmp: tmp, base }
    }

    fn project_root(&self, name: &str) -> PathBuf {
        let root = self.base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        std::fs::canonicalize(&root).expect("canonicalize project root")
    }

    /// Everything the harness running `session` has read from its terminal.
    fn received(&self, root: &Path, session: &str) -> Option<String> {
        std::fs::read_to_string(root.join(format!("received-{session}.log"))).ok()
    }

    fn argv(&self, root: &Path, session: &str) -> Option<String> {
        std::fs::read_to_string(root.join(format!("argv-{session}.log"))).ok()
    }

    /// This fixture's own [`glasshouse::Runtime`] for one project, for the
    /// scenarios that seed a session record directly instead of starting a
    /// process for it.
    fn runtime(&self, root: &Path) -> glasshouse::Runtime {
        let cli = Cli {
            scope: Some(root.to_path_buf()),
            allow_unsafe_scope: false,
            data_dir: Some(self.base.join("data")),
            config_dir: Some(self.base.join("config")),
            log_level: None,
            log_file: None,
            log_stderr: false,
            command: None,
        };
        glasshouse::bootstrap(&cli, root).expect("bootstrap the fixture runtime")
    }

    /// Run the shipped binary against this fixture's own scope/data/config,
    /// the way a person would from a shell.
    fn cli(&self, root: &Path, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .output()
            .expect("run the glasshouse binary")
    }
}

/// A harness that names its log files after the session it was started for,
/// taken from the `--settings <state>/sessions/<id>/settings.json` argument
/// the lifecycle-hook installation adds — the same fixture harness
/// `tests/api_event_log.rs` uses, for the same reason: the tag comes from the
/// hook installation's own argument, so a build that stopped installing hooks
/// would fail these tests rather than quietly pass against an unattributable
/// log file.
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
         echo READY\n\
         while IFS= read -r line; do\n\
         printf '%s\\n' \"$line\" >> \"$PWD/received-$tag.log\"\n\
         echo \"got:$line\"\n\
         done\n",
    )
    .expect("write the session-tagging harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// A running `glasshouse api serve`, killed on drop.
struct Server {
    child: Child,
    socket: PathBuf,
}

impl Server {
    fn start(fixture: &Fixture, root: &Path) -> Self {
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

fn occurrences(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

// ---------------------------------------------------------------------------
// 619 — the warning, and that refusing it changes nothing.
// ---------------------------------------------------------------------------

/// The headline of line 619, and REQUIRED BEHAVIOR's first two bullets in one
/// test: a warm session on a harness that declares `NewSession` is warned
/// about before anything happens, refusing the confirmation delivers
/// nothing, and `--accept-loss` on the same session delivers exactly once.
///
/// Kills mutation (a) — the warmth condition inverted — from both directions:
/// "warn never" fails the first half (the refusal case expects a warning and
/// gets none), "warn always" fails nothing here directly but is caught by
/// the InPlace/cold tests below, which this test's sibling scenarios cover.
#[test]
fn restyle_warns_before_a_warm_new_session_change_and_delivers_once_confirmed() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let worker = server.spawn_worker();
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &worker).is_some()
    });

    let before = fixture.cli(&root, &["sessions", "show", &worker]);
    assert!(before.status.success(), "{before:?}");
    let before_out = String::from_utf8_lossy(&before.stdout).into_owned();

    // Claude Code declares `StyleChange::NewSession` (harness/claude_code.rs)
    // and this worker is live, so restyling without `--accept-loss` must
    // warn and refuse rather than deliver anything.
    let refused = fixture.cli(
        &root,
        &[
            "sessions",
            "restyle",
            &worker,
            "--profile",
            "concise-technical",
        ],
    );
    assert!(
        !refused.status.success(),
        "a refused restyle must exit non-zero: {refused:?}"
    );
    let refused_err = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(
        refused_err.contains("warm") && refused_err.contains(&worker[..worker.len().min(12)]),
        "the refusal must name the session and its warmth: {refused_err}"
    );
    assert!(
        fixture.received(&root, &worker).is_none(),
        "refusing the confirmation must deliver nothing: {:?}",
        fixture.received(&root, &worker)
    );
    let after_refusal = fixture.cli(&root, &["sessions", "show", &worker]);
    assert_eq!(
        String::from_utf8_lossy(&after_refusal.stdout),
        before_out,
        "a refused restyle must leave the session's own record byte-identical"
    );

    // The same request, `--accept-loss`d, delivers exactly once.
    let accepted = fixture.cli(
        &root,
        &[
            "sessions",
            "restyle",
            &worker,
            "--profile",
            "concise-technical",
            "--accept-loss",
        ],
    );
    assert!(accepted.status.success(), "{accepted:?}");
    wait_for("the delivered restyle instruction", || {
        fixture
            .received(&root, &worker)
            .is_some_and(|text| text.contains(MARKER))
    });
    let received = fixture.received(&root, &worker).unwrap();
    assert_eq!(
        occurrences(&received, MARKER),
        1,
        "exactly one instruction must reach the harness: {received:?}"
    );
}

/// A harness whose declaration says `InPlace` never triggers the warning,
/// however warm the session is — the other half of REQUIRED BEHAVIOR's first
/// bullet and mutation (a)'s "warn always" direction. No process is started:
/// the gate is decided from the record and the compiled-in adapter before
/// any delivery is attempted, so a `--profile` restyle here only ever fails
/// for want of a live session to deliver into, never for warmth.
#[test]
fn restyle_on_an_in_place_harness_never_warns() {
    let fixture = Fixture::new();
    let root = fixture.project_root("beta");
    let runtime = fixture.runtime(&root);

    let sessions = ProjectSessions::open(&runtime).expect("open project sessions");
    let record = sessions
        .store()
        .create(NewSession::embedded("hermes"))
        .expect("seed a hermes-harness session");
    assert_eq!(record.lifecycle, SessionLifecycle::Starting);

    let result = fixture.cli(
        &root,
        &[
            "sessions",
            "restyle",
            record.id.as_str(),
            "--profile",
            "concise-technical",
        ],
    );
    let err = String::from_utf8_lossy(&result.stderr).into_owned();
    assert!(
        !err.contains("warm") && !err.contains("accept-loss"),
        "Hermes declares StyleChange::InPlace; restyling it must never read as a warm-session \
         warning, whatever else it fails for: {err}"
    );
}

/// A cold session never triggers the warning either — REQUIRED BEHAVIOR's
/// "a cold/exited session restyles without ceremony", and the other case
/// mutation (a) has to distinguish from "warn always".
#[test]
fn restyle_on_a_cold_session_never_warns() {
    let fixture = Fixture::new();
    let root = fixture.project_root("gamma");
    let runtime = fixture.runtime(&root);

    let sessions = ProjectSessions::open(&runtime).expect("open project sessions");
    let store = sessions.store();
    let record = store
        .create(NewSession::embedded("claude-code"))
        .expect("seed a claude-code session");
    // `Stopped` with no native session id ever recorded is `disposition() ==
    // Closed` (store.rs's own `disposition` doc: "without one there is
    // nothing to resume to"), which is what "cold" means for this gate.
    let record = store
        .set_lifecycle(&record.id, SessionLifecycle::Stopped)
        .expect("stop it");
    assert_eq!(record.disposition(), SessionDisposition::Closed);

    let result = fixture.cli(
        &root,
        &[
            "sessions",
            "restyle",
            record.id.as_str(),
            "--profile",
            "concise-technical",
        ],
    );
    let err = String::from_utf8_lossy(&result.stderr).into_owned();
    assert!(
        !err.contains("warm") && !err.contains("accept-loss"),
        "a closed session has nothing warm to lose, so restyling it must never read as a \
         warm-session warning: {err}"
    );
}

// ---------------------------------------------------------------------------
// 620 — the one-turn instruction, framed, delivered once, and refused by
// name for a harness with no verified mechanism.
// ---------------------------------------------------------------------------

/// The instruction reaches the harness exactly once, and a second, ordinary
/// turn carries no residue of it — REQUIRED BEHAVIOR's third bullet, and the
/// test mutation (b) — the instruction delivered twice — is written against.
#[test]
fn tell_delivers_once_and_a_second_turn_shows_no_residue() {
    let fixture = Fixture::new();
    let root = fixture.project_root("delta");
    let server = Server::start(&fixture, &root);

    let worker = server.spawn_worker();
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &worker).is_some()
    });

    let told = fixture.cli(&root, &["sessions", "tell", &worker, "check the tests"]);
    assert!(told.status.success(), "{told:?}");
    wait_for("the delivered instruction", || {
        fixture
            .received(&root, &worker)
            .is_some_and(|text| text.contains(MARKER))
    });

    // An ordinary second turn, plain text — not through `tell`, so nothing
    // re-frames it. `tell` delivers with `origin: "user"` (`api::send_message`
    // always does — the same attribution `glasshouse api send` uses, because
    // a person ran the command), which gives it precedence over a *machine*
    // message for the next several seconds (`USER_INPUT_PRECEDENCE`, line
    // 1719) — correct behaviour this test should not fight, so the second
    // turn is stated as the same origin rather than waiting out the window.
    const SECOND_TURN: &str = "turn-two-plain-text";
    let sent = server.call(serde_json::json!({
        "op": "send_message", "session": worker, "text": SECOND_TURN, "origin": "user",
    }));
    assert_eq!(sent["status"], "ok", "{sent}");
    wait_for("the second turn to be read", || {
        fixture
            .received(&root, &worker)
            .is_some_and(|text| text.contains(SECOND_TURN))
    });

    let received = fixture.received(&root, &worker).unwrap();
    assert_eq!(
        occurrences(&received, MARKER),
        1,
        "the one-turn instruction must reach the harness exactly once, never repeated on a \
         later turn: {received:?}"
    );
    assert_eq!(
        occurrences(&received, "check the tests"),
        1,
        "the instruction's own text must appear exactly once: {received:?}"
    );
}

/// Codex declares no communication-style mechanism
/// (`harness/codex.rs::COMMUNICATION_STYLE` is `Declared::Unverified`), so
/// `tell` must refuse it by name rather than typing an unframed instruction
/// in — REQUIRED BEHAVIOR's fourth bullet. No process is started: the
/// refusal is decided from the compiled-in adapter's own declaration before
/// any delivery is attempted.
#[test]
fn tell_refuses_a_harness_with_no_verified_communication_style() {
    let fixture = Fixture::new();
    let root = fixture.project_root("epsilon");
    let runtime = fixture.runtime(&root);

    let sessions = ProjectSessions::open(&runtime).expect("open project sessions");
    let record = sessions
        .store()
        .create(NewSession::embedded("codex"))
        .expect("seed a codex-harness session");

    let result = fixture.cli(&root, &["sessions", "tell", record.id.as_str(), "hello"]);
    assert!(
        !result.status.success(),
        "an unsupported harness must be refused, not silently skipped: {result:?}"
    );
    let err = String::from_utf8_lossy(&result.stderr).into_owned();
    assert!(
        err.contains("Codex") && err.contains("communication-style"),
        "the refusal must name the harness and the missing declaration: {err}"
    );
}

/// A payload that could smuggle more than one line is refused outright, the
/// same conservatism `integrations::cmux`'s payload rule uses — never
/// escaped, because there is no correct way to escape it once it has reached
/// this side of the seam.
#[test]
fn tell_refuses_a_control_byte_rather_than_escaping_it() {
    let fixture = Fixture::new();
    let root = fixture.project_root("zeta");

    let result = fixture.cli(
        &root,
        &[
            "sessions",
            "tell",
            "0000000000000000000000000000000",
            "hello\rrm -rf ~",
        ],
    );
    assert!(!result.status.success(), "{result:?}");
    let err = String::from_utf8_lossy(&result.stderr).into_owned();
    assert!(
        err.contains("control byte"),
        "a payload carrying a raw `\\r` must be refused before any session lookup happens: {err}"
    );
}
