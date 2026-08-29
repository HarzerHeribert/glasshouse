//! Phase 15/16 — the user's direct access to an orchestrated worker.
//!
//! Capability map lines 740, 745, 746, 747 and 748. This file exists to make
//! that package's findings *executable* rather than asserted: four of the five
//! lines were returned premise-invalid, and a refusal nobody can run is a
//! refusal nobody can falsify.
//!
//! Everything here drives the shipped binary — `glasshouse api serve` over its
//! real Unix domain socket, and a real `glasshouse hook` process for the one
//! event class the door cannot mint itself. The socket is where an
//! orchestrated worker actually lives: `api::unix::serve` owns the pty, and
//! every other Glasshouse process (a TUI, a `glasshouse sessions` invocation)
//! is a different process that cannot see it.
//!
//! # What is proven here, and what is deliberately not
//!
//! - **The transport a person's input would travel on exists, and is
//!   cross-process.** [`a_message_sent_through_the_door_reaches_a_real_worker_process`]
//!   proves it against a real harness that writes down what it read.
//! - **What travels it is recorded where an orchestrator can read it.**
//!   [`an_intervention_through_the_door_reaches_the_orchestrators_event_read_path`]
//!   is map line 748, and it used to assert the opposite — see below.
//! - **No user surface reaches this door at all.** That one is not testable
//!   from here and is not attempted: `UnixStream::connect` appears nowhere in
//!   `crates/glasshouse/src/`, and `cli::ApiCommand` has exactly one variant,
//!   `Serve`. Both are facts about the source, and the honest place for them
//!   is the evidence ledger, not an assertion dressed up as a test.
//!
//! # This file pinned a defect, and the pin worked
//!
//! `an_intervention_through_the_door_never_reaches_the_orchestrators_event_read_path`
//! asserted the wrong behaviour on purpose, so that the gap had a name, a
//! viewport (§17) and a failure the moment somebody fixed it. Somebody did:
//! `glasshouse api serve` now attaches an event-log sink and `Request::Events`
//! reads the project's whole history rather than only what a harness reported.
//! The test failed with the message it was written to print, and it was
//! **inverted rather than deleted** — same setup, same viewport, opposite
//! assertion. `tests/api_event_log.rs` holds the rest of that closure,
//! including the read half on its own.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(30);

/// A project with an installed harness that records every line it reads under
/// a name derived from its own `--settings` argument.
///
/// Deliberately the same shape as `worker_wakeup.rs`'s fixture: the session
/// tag comes from the lifecycle-hook installation's own argument, so a door
/// that stopped installing hooks would fail these tests rather than quietly
/// pass them against an unattributable log file.
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

    /// Everything the harness running `session` has read from its terminal.
    fn received(&self, root: &Path, session: &str) -> Option<String> {
        std::fs::read_to_string(root.join(format!("received-{session}.log"))).ok()
    }

    /// The command line the harness running `session` was started with.
    ///
    /// Present only once the harness is actually running, which is what makes
    /// it a causal ready signal rather than a sleep.
    fn argv(&self, root: &Path, session: &str) -> Option<String> {
        std::fs::read_to_string(root.join(format!("argv-{session}.log"))).ok()
    }

    /// Run a real `glasshouse hook` process, exactly as a harness's own
    /// lifecycle hook does.
    fn hook(&self, root: &Path, session: &str, event: &str) {
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

/// A harness that names its log files after the session it was started for,
/// taken from the `--settings <state>/sessions/<id>/settings.json` argument
/// the lifecycle-hook installation adds.
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
///
/// `Child::drop` does not kill, and this project has accumulated runaway
/// harness sessions four times; the explicit kill is what stops a failed
/// assertion from leaving a pty behind.
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

    /// Every event the orchestrator's own read path can see, from the start
    /// of the log.
    fn events(&self) -> Vec<serde_json::Value> {
        let response = self.call(serde_json::json!({
            "op": "events",
            "after": 0,
            "limit": 1000,
        }));
        assert_eq!(response["status"], "ok", "{response}");
        response["result"]["events"]
            .as_array()
            .expect("an events array")
            .clone()
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

/// Map line 746's transport, and the premise the rest of this file rests on.
///
/// *"Allow direct user input to an orchestrated worker without requiring the
/// orchestrator as an intermediary."* The **carrying** half of that is real
/// and this proves it end to end: a caller that is not the orchestrator
/// writes one line of JSON to the door, and a separate, already-running
/// harness process writes the text down as having arrived on its terminal.
///
/// What it does not prove — and what line 746 turns on — is that a *user* has
/// any way to be that caller, or that the delivery is distinguishable from
/// the orchestrator's own. `Request::SendMessage` is documented as speaking
/// "as Glasshouse rather than as the user" and routes through
/// `SessionApi::send_text`, which is hard-wired to `MessageOrigin::Machine`.
#[test]
fn a_message_sent_through_the_door_reaches_a_real_worker_process() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let worker = server.spawn_worker();
    // Causal, not a sleep: the file appears only once the harness has run far
    // enough to read its own argument list.
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &worker).is_some()
    });

    let response = server.call(serde_json::json!({
        "op": "send_message",
        "session": worker,
        "text": "intervention-one",
    }));
    assert_eq!(response["status"], "ok", "{response}");

    wait_for("the worker to read the delivered line", || {
        fixture
            .received(&root, &worker)
            .is_some_and(|text| text.contains("intervention-one"))
    });
}

/// Map line 748, and the inversion of the test that used to pin its gap.
///
/// *"Record user intervention so the orchestrator can be informed that the
/// worker state may have changed."* An intervention is delivered here,
/// demonstrably arrives, and is then visible through the read path an
/// orchestrator in another process has.
///
/// # What had to change, in two independent places
///
/// 1. `api::unix::serve` built its runtime with `SessionRuntime::new()`,
///    whose `EventBus` has no sink attached — unlike `shell::run`, which
///    calls `attach_event_log` first and passes the bus in. So
///    `LifecycleEvent::TextDelivered` and `InterruptDelivered` were published
///    and discarded in the one process that owns every orchestrated worker.
///    `api::unix::EventRecorder` is that sink, opened lazily on its own
///    writer thread.
/// 2. Even with a sink, `EventLog::observed_since` filters to
///    `observed_harness IS NOT NULL`, and an intervention is not something a
///    harness reports. `Request::Events` now reads `EventLog::since`, because
///    that filter de-duplicates for a reader that shares this process's event
///    bus and there is no such reader on the far end of a socket.
///
/// **Fixing only one of the two changes nothing**, which is why the write
/// half's mutation survived before this test existed in this form.
///
/// # The viewport this is rendered into (§17)
///
/// Kept from the pinning version, because it still earns its place: a real
/// `glasshouse hook` process writes one observed row for **this same
/// session** first, so the read path is demonstrably carrying events for this
/// worker independently of the two changes under test. Without it a door that
/// returned everything for the wrong reason would look the same.
#[test]
fn an_intervention_through_the_door_reaches_the_orchestrators_event_read_path() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let worker = server.spawn_worker();
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &worker).is_some()
    });

    // The viewport proof: one row this read path is known to carry, for this
    // exact session, written by a real separate hook process.
    fixture.hook(&root, &worker, "UserPromptSubmit");

    let sent = server.call(serde_json::json!({
        "op": "send_message",
        "session": worker,
        "text": "intervention-two",
    }));
    assert_eq!(sent["status"], "ok", "{sent}");

    // Waited for *before* the interrupt, not after. An interrupt reaches this
    // harness as a real `0x03` on its terminal and kills the `sh` read loop
    // outright, so sending both and then looking is a race the interrupt wins:
    // the line is delivered to a pty nobody is left to read from. Ordering the
    // two is also what the assertion below actually needs — the delivery has
    // to have happened for its absence from the log to mean anything.
    wait_for("the worker to read the delivered line", || {
        fixture
            .received(&root, &worker)
            .is_some_and(|text| text.contains("intervention-two"))
    });

    let interrupted = server.call(serde_json::json!({
        "op": "interrupt",
        "session": worker,
    }));
    assert_eq!(interrupted["status"], "ok", "{interrupted}");

    let events = server.events();
    let mine: Vec<&serde_json::Value> = events
        .iter()
        .filter(|event| event["session"] == worker.as_str())
        .collect();

    assert!(
        mine.iter().any(|event| event["kind"] == "turn_started"),
        "the viewport must be working before its emptiness means anything: \
         no event at all came back for session `{worker}`, so this test would \
         pass against a read path that was simply broken. Events seen: {events:?}"
    );

    let recorded: Vec<&str> = mine
        .iter()
        .filter_map(|event| event["kind"].as_str())
        .filter(|kind| *kind == "text_delivered" || *kind == "interrupt_delivered")
        .collect();

    assert!(
        recorded.contains(&"text_delivered"),
        "the line delivered above reached the worker and must be in the \
         history the orchestrator reads: {mine:?}"
    );
    assert!(
        recorded.contains(&"interrupt_delivered"),
        "so must the interrupt: {mine:?}"
    );
}

/// The isolation invariant, through the shipped door rather than under it.
///
/// `SessionApi` refuses another project's session in unit tests; this asserts
/// the same thing where it is load-bearing — a second `glasshouse api serve`
/// on a second project, reached over its own socket, cannot be talked into
/// delivering a line into the first project's worker. The refusal is checked
/// behaviourally as well as by status, because a door that answered `error`
/// and delivered anyway would satisfy the weaker assertion.
#[test]
fn the_door_refuses_to_deliver_into_another_projects_worker() {
    let fixture = Fixture::new();

    let alpha_root = fixture.project_root("alpha");
    let beta_root = fixture.project_root("beta");
    let alpha = Server::start(&fixture, &alpha_root);
    let beta = Server::start(&fixture, &beta_root);

    let victim = alpha.spawn_worker();
    wait_for("alpha's worker to start", || {
        fixture.argv(&alpha_root, &victim).is_some()
    });

    let response = beta.call(serde_json::json!({
        "op": "send_message",
        "session": victim,
        "text": "crossing-a-project-boundary",
    }));
    assert_eq!(
        response["status"], "error",
        "a door opened for one project must refuse another project's session: {response}"
    );

    // Prove the refusal was a refusal. Beta's own worker gives the delivery
    // path something to have succeeded at, so this is not asserting against a
    // door that simply cannot deliver anything at all.
    let bystander = beta.spawn_worker();
    wait_for("beta's worker to start", || {
        fixture.argv(&beta_root, &bystander).is_some()
    });
    let delivered = beta.call(serde_json::json!({
        "op": "send_message",
        "session": bystander,
        "text": "staying-inside-this-project",
    }));
    assert_eq!(delivered["status"], "ok", "{delivered}");
    wait_for("beta's own worker to read its line", || {
        fixture
            .received(&beta_root, &bystander)
            .is_some_and(|text| text.contains("staying-inside-this-project"))
    });

    assert!(
        !fixture
            .received(&alpha_root, &victim)
            .is_some_and(|text| text.contains("crossing-a-project-boundary")),
        "alpha's worker received a line delivered through beta's door"
    );
}
