//! Phase 14 (orchestrator role) — the boxes this package's audit found
//! genuinely missing behind Phase 42's door, plus the fail-closed proof
//! design decision 4 asked for: every operation, not just the two Phase 42
//! already covered.
//!
//! `mod api` is declared from `main.rs`, so — exactly as `session_model.rs`'s
//! own `control_api` module explains — nothing outside the binary can reach
//! the control door any other way. This drives `glasshouse api serve` for
//! real over its Unix domain socket, the same harness shape `session_model.rs`
//! and `capacity_api.rs` already use.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(15);

/// A project with an installed harness that echoes every line it reads,
/// forever, and logs every line to `received.log` — the same fixture shape
/// `session_model.rs`'s `control_api::ApiFixture` uses and for the same
/// reason: a side channel the control API itself never touches is how these
/// tests observe a machine-sent line without inventing a "read scrollback"
/// capability nobody asked for.
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

    fn project_root(&self, name: &str) -> PathBuf {
        let root = self.base.join("workspace").join(name);
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        std::fs::canonicalize(&root).expect("canonicalize project root")
    }

    fn received_log(&self, root: &Path) -> PathBuf {
        root.join("received.log")
    }
}

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

/// Box 1 (tag a session with the orchestrator role) and part of box 5 (spawn
/// a *worker*): a session spawned through the socket with no role stated is
/// a worker by default, and an explicit role is honored and visible in the
/// listing — the same `ROLE` word `glasshouse sessions` prints, reached
/// through the production `session_summary` this door already used for
/// everything else.
#[test]
fn spawning_tags_a_worker_by_default_and_an_explicit_role_is_honored() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let default_spawn =
        server.call(serde_json::json!({"op": "spawn_session", "harness": "claude-code"}));
    assert_eq!(default_spawn["status"], "ok", "{default_spawn}");
    let default_id = default_spawn["result"]["session"].as_str().unwrap();

    let orchestrator_spawn = server.call(serde_json::json!({
        "op": "spawn_session",
        "harness": "claude-code",
        "role": "orchestrator",
    }));
    assert_eq!(orchestrator_spawn["status"], "ok", "{orchestrator_spawn}");
    let orchestrator_id = orchestrator_spawn["result"]["session"].as_str().unwrap();

    let listed = server.call(serde_json::json!({"op": "list_sessions"}));
    let entries = listed["result"].as_array().expect("a session list");

    let default_role = entries
        .iter()
        .find(|entry| entry["session"] == default_id)
        .unwrap_or_else(|| panic!("the default-role spawn must be listed: {listed}"))["role"]
        .as_str()
        .unwrap();
    assert_eq!(
        default_role, "worker",
        "a session spawned through this door with no stated role is a worker by default"
    );

    let orchestrator_role = entries
        .iter()
        .find(|entry| entry["session"] == orchestrator_id)
        .unwrap_or_else(|| panic!("the orchestrator-role spawn must be listed: {listed}"))["role"]
        .as_str()
        .unwrap();
    assert_eq!(orchestrator_role, "orchestrator");

    let unknown_role = server.call(serde_json::json!({
        "op": "spawn_session",
        "harness": "claude-code",
        "role": "not-a-real-role",
    }));
    assert_eq!(
        unknown_role["status"], "error",
        "an unknown role must be refused rather than silently stored: {unknown_role}"
    );
}

/// Box 6: a natural-language task assigned at spawn reaches the harness as
/// its first message — distinct from box 7's follow-up `send_message` to a
/// session that already exists, and proved the same way box 4 was in Phase
/// 42: a side channel the control API itself never touches.
#[test]
fn a_task_given_at_spawn_reaches_the_harness_as_its_first_message() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let spawned = server.call(serde_json::json!({
        "op": "spawn_session",
        "harness": "claude-code",
        "task": "audit phase 14 and report back",
    }));
    assert_eq!(spawned["status"], "ok", "{spawned}");

    let received_log = fixture.received_log(&root);
    wait_for("the harness to record the assigned task", || {
        std::fs::read_to_string(&received_log)
            .map(|text| text.contains("audit phase 14 and report back"))
            .unwrap_or(false)
    });
}

/// Box 10: a checkpoint taken through the socket is retrieved through the
/// socket — the read half `Request::TakeCheckpoint` never had. Named,
/// unambiguous-prefix and `"latest"`/absent resolution are the same rule
/// `glasshouse checkpoint show` uses on the CLI side (proven separately in
/// `session_model.rs`).
#[test]
fn a_checkpoint_taken_through_the_socket_is_retrieved_through_the_socket() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let spawned = server.call(serde_json::json!({"op": "spawn_session", "harness": "claude-code"}));
    let session = spawned["result"]["session"].as_str().unwrap().to_owned();

    let taken = server.call(serde_json::json!({
        "op": "take_checkpoint",
        "session": session,
        "objective": "prove retrieval reaches the same store as the write",
        "state": "took a checkpoint, now reading it back through the socket",
    }));
    assert_eq!(taken["status"], "ok", "{taken}");
    let checkpoint_id = taken["result"]["checkpoint"].as_str().unwrap().to_owned();

    let by_id = server.call(serde_json::json!({
        "op": "get_checkpoint",
        "checkpoint": checkpoint_id,
        "document": true,
    }));
    assert_eq!(by_id["status"], "ok", "{by_id}");
    assert!(
        by_id["result"]["document"]
            .as_str()
            .unwrap()
            .contains("prove retrieval reaches the same store as the write"),
        "{by_id}"
    );
    assert_eq!(by_id["result"]["session"].as_str().unwrap(), session);

    let latest = server.call(serde_json::json!({ "op": "get_checkpoint" }));
    assert_eq!(latest["status"], "ok", "{latest}");
    assert_eq!(
        latest["result"]["checkpoint"].as_str().unwrap(),
        checkpoint_id
    );

    let missing = server.call(serde_json::json!({
        "op": "get_checkpoint",
        "checkpoint": "deadbeef",
    }));
    assert_eq!(
        missing["status"], "error",
        "a checkpoint id nothing ever wrote must be refused, not fabricated: {missing}"
    );
}

/// Design decision 4: project scoping is a fail-closed test covering every
/// orchestrator operation, not just the two Phase 42's own suite already
/// proved through the socket (list, state). This drives all five —
/// list, state, message, interrupt, retrieve — against a session and a
/// checkpoint that both genuinely belong to a different project's own
/// physical database file.
#[test]
fn every_orchestrator_operation_refuses_a_session_from_another_project() {
    let fixture = Fixture::new();
    let alpha_root = fixture.project_root("alpha");
    let beta_root = fixture.project_root("beta");

    let alpha = Server::start(&fixture, &alpha_root);

    let spawned = alpha.call(serde_json::json!({"op": "spawn_session", "harness": "claude-code"}));
    let session = spawned["result"]["session"].as_str().unwrap().to_owned();

    let ready_marker = alpha_root.join("ready");
    wait_for("the harness to be ready", || ready_marker.exists());

    let taken = alpha.call(serde_json::json!({
        "op": "take_checkpoint",
        "session": session,
        "objective": "a checkpoint that belongs to alpha, not beta",
        "state": "alpha's own state",
    }));
    assert_eq!(taken["status"], "ok", "{taken}");
    let checkpoint_id = taken["result"]["checkpoint"].as_str().unwrap().to_owned();

    // alpha's own door still works, and confirms the fixtures above are real
    // before beta is asked to refuse them.
    let alpha_state = alpha.call(serde_json::json!({"op": "session_state", "session": session}));
    assert_eq!(alpha_state["status"], "ok", "{alpha_state}");

    drop(alpha);
    let beta = Server::start(&fixture, &beta_root);

    // 1. list — the foreign session must never appear.
    let beta_listing = beta.call(serde_json::json!({"op": "list_sessions"}));
    let beta_ids: Vec<String> = beta_listing["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["session"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        !beta_ids.contains(&session),
        "list must never surface another project's session: {beta_listing}"
    );

    // 2. state
    let state = beta.call(serde_json::json!({"op": "session_state", "session": session}));
    assert_eq!(
        state["status"], "error",
        "state must refuse a foreign session: {state}"
    );

    // 3. message
    let messaged = beta.call(serde_json::json!({
        "op": "send_message",
        "session": session,
        "text": "this must never be delivered",
    }));
    assert_eq!(
        messaged["status"], "error",
        "send_message must refuse a foreign session: {messaged}"
    );

    // 4. interrupt
    let interrupted = beta.call(serde_json::json!({"op": "interrupt", "session": session}));
    assert_eq!(
        interrupted["status"], "error",
        "interrupt must refuse a foreign session: {interrupted}"
    );

    // 5. retrieve — a checkpoint id that is real, but in a different
    // project's own database file, must resolve to nothing rather than
    // beta's own most-recent checkpoint or an error that looks like one.
    let retrieved = beta.call(serde_json::json!({
        "op": "get_checkpoint",
        "checkpoint": checkpoint_id,
    }));
    assert_eq!(
        retrieved["status"], "error",
        "retrieve must refuse a checkpoint belonging to another project's database: {retrieved}"
    );
}

/// Phase 16, box 1: a worker created by an orchestrator appears immediately
/// in the *normal* Glasshouse session list — not just the orchestrator's own
/// `list_sessions` view (Phase 14, box 4, already proven above), but the
/// ordinary `glasshouse sessions` surface a person runs. Both read
/// `SessionStore::list` against the same project database file, so nothing
/// here should require the API server to be involved in the CLI's own read
/// at all — proven by running the CLI as a wholly separate process against
/// the same `--data-dir`/`--config-dir`/`--scope`.
#[test]
fn a_worker_spawned_through_the_socket_appears_in_the_ordinary_sessions_listing() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let spawned = server.call(serde_json::json!({
        "op": "spawn_session",
        "harness": "claude-code",
        "role": "worker",
    }));
    assert_eq!(spawned["status"], "ok", "{spawned}");
    let session = spawned["result"]["session"].as_str().unwrap().to_owned();
    let short: String = session.chars().take(12).collect();

    let listing = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
        .arg("--scope")
        .arg(&root)
        .arg("--data-dir")
        .arg(fixture.base.join("data"))
        .arg("--config-dir")
        .arg(fixture.base.join("config"))
        .arg("sessions")
        .output()
        .expect("run `glasshouse sessions`");
    assert!(
        listing.status.success(),
        "{}",
        String::from_utf8_lossy(&listing.stderr)
    );
    let stdout = String::from_utf8_lossy(&listing.stdout);
    assert!(
        stdout.contains(&short),
        "a session spawned through the socket must appear in the ordinary `glasshouse \
         sessions` listing read by a wholly separate process: {stdout}"
    );
    assert!(
        stdout.contains("worker"),
        "its role must be visible in that same listing: {stdout}"
    );
}
