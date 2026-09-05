//! Capability map lines 2414–2416 — Phase 60's H slice: a direct file
//! overlap the edit-intent hook detects (lines 2409–2410) is told to this
//! project's one live orchestrator session, and Glasshouse says plainly when
//! there is nobody unambiguous to tell rather than guessing.
//!
//! # Everything here drives the shipped binary, and reads its log
//!
//! Practice §35: a caller every test bypasses is not a caller. Every test
//! spawns `glasshouse edit-intent hook` in its own process, exactly as
//! `tests/edit_intent.rs` does. The decision this file exists to prove —
//! which orchestrator got told, or that none unambiguous did — is never on
//! the `PreToolUse` answer: that channel reaches the two conflicting
//! sessions, not a third one. So every test enables `--log-level debug
//! --log-file <path>` and reads the log back, the same way a person running
//! `--log-file` would.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(unix)]
use std::io::{BufRead, BufReader};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(unix)]
use std::time::{Duration, Instant};

use clap::Parser;

use glasshouse::session::{NewSession, ProjectSessions, SessionId, SessionLifecycle, SessionRole};
use glasshouse::{Cli, Runtime};

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    log: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let log = base.join("glasshouse.log");
        let runtime = bootstrap(&base, &root);
        Self {
            _tmp: tmp,
            base,
            root,
            log,
            runtime,
        }
    }

    /// `glasshouse edit-intent hook`, exactly as Claude Code runs it, with
    /// logging turned on so this test can read what the hook decided about
    /// delivery. Every hook process appends to the same log file.
    fn hook(&self, session: &SessionId, payload: &str) -> serde_json::Value {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command
            .current_dir(&self.root)
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("--log-level")
            .arg("debug")
            .arg("--log-file")
            .arg(&self.log)
            .args(["edit-intent", "hook", "--session", session.as_str()]);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the glasshouse binary must be runnable");
        child
            .stdin
            .as_mut()
            .expect("stdin was piped")
            .write_all(payload.as_bytes())
            .expect("the hook must read its payload rather than closing the pipe");
        let output = child.wait_with_output().expect("the hook must exit");
        assert!(
            output.status.success(),
            "a PreToolUse hook that exits non-zero vetoes the tool call; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap_or_else(|err| {
            panic!(
                "the hook must answer with JSON ({err}): {:?}",
                String::from_utf8_lossy(&output.stdout)
            )
        })
    }

    /// Every line every `hook` call so far has logged, read back the way
    /// `--log-file` is meant to be read.
    fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn session(&self, role: SessionRole) -> SessionId {
        let sessions = ProjectSessions::open(&self.runtime).unwrap();
        let store = sessions.store();
        let record = store
            .create(NewSession::embedded("claude-code").with_role(role))
            .unwrap();
        store
            .set_lifecycle(&record.id, SessionLifecycle::Running)
            .unwrap();
        record.id
    }
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

fn write_event(root: &Path, relative: &str) -> String {
    serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Write",
        "tool_input": {
            "file_path": root.join(relative).display().to_string(),
            "content": "done",
        },
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// 2414 — ambiguity is reported, not guessed.
// ---------------------------------------------------------------------------

/// Zero live orchestrator sessions: the conflict is real, and Glasshouse says
/// plainly it could not be delivered rather than staying silent about it.
#[test]
fn zero_live_orchestrators_reports_undeliverable() {
    let fixture = Fixture::new();
    let first = fixture.session(SessionRole::Normal);
    let second = fixture.session(SessionRole::Normal);
    let event = write_event(&fixture.root, "src/main.rs");

    fixture.hook(&first, &event);
    fixture.hook(&second, &event);

    let log = fixture.log_text();
    assert!(
        log.contains("no live orchestrator session is running in this project"),
        "{log}"
    );
}

/// **The mutation-killing test.** More than one live orchestrator session is
/// exactly as ambiguous as none — Glasshouse must say the conflict could not
/// be delivered rather than picking one of them as a guess. A mutation that
/// delivers to the first candidate instead removes this log line entirely,
/// which is what this test reads for.
#[test]
fn two_live_orchestrators_reports_undeliverable_rather_than_guessing() {
    let fixture = Fixture::new();
    let _orchestrator_a = fixture.session(SessionRole::Orchestrator);
    let _orchestrator_b = fixture.session(SessionRole::Orchestrator);
    let first = fixture.session(SessionRole::Normal);
    let second = fixture.session(SessionRole::Normal);
    let event = write_event(&fixture.root, "src/main.rs");

    fixture.hook(&first, &event);
    fixture.hook(&second, &event);

    let log = fixture.log_text();
    assert!(
        log.contains(
            "more than one live orchestrator session is running in this project, and \
             Glasshouse does not guess which one"
        ),
        "{log}"
    );
}

/// Exactly one live orchestrator, and it is neither of the two conflicting
/// sessions: the ambiguity path must not fire, and delivery must be
/// attempted through the real seam rather than skipped.
#[test]
fn one_unambiguous_orchestrator_is_attempted_not_reported_ambiguous() {
    let fixture = Fixture::new();
    let orchestrator = fixture.session(SessionRole::Orchestrator);
    let first = fixture.session(SessionRole::Normal);
    let second = fixture.session(SessionRole::Normal);
    let event = write_event(&fixture.root, "src/main.rs");

    fixture.hook(&first, &event);
    fixture.hook(&second, &event);

    let log = fixture.log_text();
    assert!(
        !log.contains("no live orchestrator session is running"),
        "exactly one candidate exists and must not be reported as none: {log}"
    );
    assert!(
        !log.contains("more than one live orchestrator session"),
        "exactly one candidate exists and must not be reported as ambiguous: {log}"
    );
    // Delivery is attempted through the real production seam
    // (`api::send_machine_message`, which connects to this project's
    // control socket) and is expected to report "not listening" here: this
    // fixture never starts `glasshouse api serve`, so no socket exists for
    // this hook's subprocess to reach — see
    // `a_conflict_notice_reaches_the_orchestrator_through_the_control_api`
    // for the case where one is running and delivery actually lands. The
    // decisive fact this test reads is that the correct, single recipient
    // was resolved and an attempt was logged against it.
    assert!(
        log.contains("this project's control API is not listening")
            && log.contains(orchestrator.as_str()),
        "the one candidate must be named as the attempted recipient: {log}"
    );
}

/// No self-notification: the project's one live orchestrator is itself one
/// of the two conflicting sessions, and is not told about its own claim
/// through this second channel — it already has the fact, from being a
/// party to it.
#[test]
fn the_only_orchestrator_being_a_conflict_party_is_not_notified_again() {
    let fixture = Fixture::new();
    let orchestrator = fixture.session(SessionRole::Orchestrator);
    let other = fixture.session(SessionRole::Normal);
    let event = write_event(&fixture.root, "src/main.rs");

    // The orchestrator holds the first claim; the conflict is between it and
    // `other`.
    fixture.hook(&orchestrator, &event);
    fixture.hook(&other, &event);

    let log = fixture.log_text();
    assert!(
        !log.contains("no live orchestrator session is running")
            && !log.contains("more than one live orchestrator session"),
        "exactly one candidate exists; this is not the ambiguous case: {log}"
    );
    assert!(
        !log.contains("could not deliver a conflict notice to the orchestrator")
            && !log.contains("delivered a conflict notice to the orchestrator"),
        "the sole orchestrator is a party to its own conflict and must not be notified \
         through a second channel: {log}"
    );
}

// ---------------------------------------------------------------------------
// 2415 — granularity: a conflict on one path names only that path.
// ---------------------------------------------------------------------------

/// A conflict on one path is reported against that path, and a session
/// working on an unrelated, unclaimed path produces no delivery attempt that
/// names it. If the signal were task-wide rather than path-wide, the second
/// write's own path would appear in the same delivery attempt as the first.
#[test]
fn a_conflict_on_one_path_names_only_that_path() {
    let fixture = Fixture::new();
    let _orchestrator = fixture.session(SessionRole::Orchestrator);
    let first = fixture.session(SessionRole::Normal);
    let second = fixture.session(SessionRole::Normal);
    let third = fixture.session(SessionRole::Normal);

    fixture.hook(&first, &write_event(&fixture.root, "src/a.rs"));
    fixture.hook(&second, &write_event(&fixture.root, "src/a.rs"));
    // `third` writes an entirely different, unclaimed path: no conflict, no
    // delivery attempt at all for it.
    fixture.hook(&third, &write_event(&fixture.root, "src/b.rs"));

    let log = fixture.log_text();
    assert!(
        log.contains("path=src/a.rs") || log.contains("path=\"src/a.rs\""),
        "the conflicting path must be named: {log}"
    );
    assert!(
        !log.contains("src/b.rs"),
        "an unrelated, unclaimed path must never appear in a delivery attempt: {log}"
    );
}

// ---------------------------------------------------------------------------
// The control-API transport itself (GH-CONFLICT-NOTICE-VIA-API).
// ---------------------------------------------------------------------------

/// With a serving control API that started the orchestrator, the hook's
/// stdout still parses as one JSON document (asserted by every `hook()` call
/// in this file already), and undeliverable is reported by name rather than
/// silently.
///
/// No API server, no socket, no `#[cfg(unix)]` needed: `api::CallError`'s
/// non-Unix fallback (`api/mod.rs`) reports exactly this same "not
/// listening" case, so this test is not gated even though
/// [`a_conflict_notice_reaches_the_orchestrator_through_the_control_api`]
/// below is.
#[test]
fn a_conflict_notice_with_no_api_listening_is_reported_undeliverable_and_the_hook_still_answers() {
    let fixture = Fixture::new();
    let orchestrator = fixture.session(SessionRole::Orchestrator);
    let holder = fixture.session(SessionRole::Normal);
    let editor = fixture.session(SessionRole::Normal);
    let event = write_event(&fixture.root, "src/main.rs");

    fixture.hook(&holder, &event);
    // `Fixture::hook` already asserts that the *entire* stdout parses as one
    // JSON document (`serde_json::from_str` on the whole buffer), which is
    // exactly the "hook's stdout is exactly its PreToolUse response and
    // nothing else" requirement — a stray line from the delivery attempt
    // would fail that parse, not this assertion.
    let response = fixture.hook(&editor, &event);
    assert!(
        response.get("hookSpecificOutput").is_some(),
        "the hook must still answer with its ordinary PreToolUse shape: {response}"
    );

    let log = fixture.log_text();
    assert!(
        log.contains("this project's control API is not listening"),
        "{log}"
    );
    assert!(
        log.contains(orchestrator.as_str()),
        "the resolved orchestrator must be named even though delivery failed: {log}"
    );
}

/// **The mutation-killing test for 2414.** A serving control API that
/// actually started the orchestrator delivers the notice to it — proven by
/// running the shipped binary against a real `glasshouse api serve` and
/// reading what a real harness process received on its stdin, exactly as
/// `context_injection.rs`'s own `Server` fixture proves delivery for that
/// door. A mutation that addresses the notice to `editor` instead of
/// `orchestrator.id` sends it to a session with no harness reading from the
/// door at all, so the orchestrator's `received-*.log` never appears and
/// this test times out waiting for it.
#[cfg(unix)]
#[test]
fn a_conflict_notice_reaches_the_orchestrator_through_the_control_api() {
    let fixture = Fixture::new();
    let bin_dir = fixture.base.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let harness = install_receiving_harness(&bin_dir);
    write_harness_config(&fixture.base, &harness);

    let server = Server::start(&fixture);
    let orchestrator = server.spawn_orchestrator();
    let argv_log = fixture.root.join(format!("argv-{orchestrator}.log"));
    wait_for("the orchestrator's harness to start", || argv_log.exists());

    // The editor/holder side of the conflict is this file's other fixture,
    // exactly as every other test here: only the orchestrator needs to be a
    // real process the door holds live.
    let holder = fixture.session(SessionRole::Normal);
    let editor = fixture.session(SessionRole::Normal);
    let event = write_event(&fixture.root, "src/shared.rs");

    fixture.hook(&holder, &event);
    fixture.hook(&editor, &event);

    let received_log = fixture.root.join(format!("received-{orchestrator}.log"));
    wait_for("the orchestrator to receive the conflict notice", || {
        received_log.exists()
    });
    let received = std::fs::read_to_string(&received_log).unwrap();
    let lines: Vec<&str> = received.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "exactly one delivery must reach the orchestrator: {lines:#?}"
    );

    let notice = lines[0];
    let editor_short: String = editor.as_str().chars().take(12).collect();
    let holder_short: String = holder.as_str().chars().take(12).collect();
    assert!(notice.contains("src/shared.rs"), "{notice}");
    assert!(notice.contains(&editor_short), "{notice}");
    assert!(notice.contains(&holder_short), "{notice}");
}

/// Waits for `done` to become true, polling — copied from
/// `context_injection.rs`'s own helper of the same name and purpose.
#[cfg(unix)]
fn wait_for<F: FnMut() -> bool>(what: &str, mut done: F) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if done() {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A shell harness that tags its log files by the session it was started
/// for, taken from its own `--settings <state>/sessions/<id>/settings.json`
/// argument — copied from `context_injection.rs`'s
/// `install_session_tagging_harness` rather than shared, the way that file
/// copied its own from `worker_access.rs`.
#[cfg(unix)]
fn install_receiving_harness(bin_dir: &Path) -> PathBuf {
    let path = bin_dir.join("receiving-harness");
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
    .expect("write the receiving harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// The user config that points the `claude-code` integration at `harness`,
/// with the implementation policy off — copied from `context_injection.rs`'s
/// `Fixture::new`, for the same reason it turns the policy off there: this
/// test is about the conflict notice and nothing else, and the policy is
/// several more machine-origin deliveries into every spawned session.
#[cfg(unix)]
fn write_harness_config(base: &Path, harness: &Path) {
    let config_dir = base.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let escaped = harness.display().to_string().replace('\\', "\\\\");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\nimplementation_policy = false\n\n[integrations.claude-code]\n\
             enabled = true\nexecutable = \"{escaped}\"\n"
        ),
    )
    .expect("write user config");
}

/// A running `glasshouse api serve`, killed on drop — copied from
/// `context_injection.rs`'s own `Server`, trimmed to the one verb this file
/// needs: spawning a role the door really holds live, so
/// `api::send_machine_message` has a real session to reach rather than this
/// file's other fixture, whose sessions are rows in the database and no
/// process at all.
#[cfg(unix)]
struct Server {
    child: std::process::Child,
    socket: PathBuf,
}

#[cfg(unix)]
impl Server {
    fn start(fixture: &Fixture) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&fixture.root)
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
        let deadline = Instant::now() + Duration::from_secs(30);
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
        let deadline = Instant::now() + Duration::from_secs(30);
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

    fn spawn_orchestrator(&self) -> String {
        let response = self.call(serde_json::json!({
            "op": "spawn_session",
            "harness": "claude-code",
            "role": "orchestrator",
        }));
        assert_eq!(response["status"], "ok", "{response}");
        response["result"]["session"]
            .as_str()
            .expect("a session id")
            .to_owned()
    }
}

#[cfg(unix)]
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
