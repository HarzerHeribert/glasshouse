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
//! - **A user surface reaches this door**, which is new, and is what turned
//!   two of those premise-invalid lines valid. This paragraph used to say the
//!   opposite — *"no user surface reaches this door at all"* — and recorded
//!   why it was not testable: `UnixStream::connect` appeared nowhere in
//!   `crates/glasshouse/src/`, and `cli::ApiCommand` had exactly one variant,
//!   `Serve`. `api::client` and `glasshouse api send` / `api interrupt` are
//!   that surface, and the second half of this file tests them the same way
//!   the first half tests the door: by running the shipped binary.
//!
//! - **Line 745 is still open, and this file does not pretend otherwise.**
//!   *"Enter"* a worker means seeing it, and no request on this wire returns
//!   a worker's terminal output — a client built from the existing verbs can
//!   put input in and cannot show what came back. What is missing is only the
//!   verb: `session::api::SessionApi::recent_output` already reads a live
//!   session's scrollback inside the process that owns the pty, project-scoped
//!   through the same seam, with no production caller. See `api::client`'s own
//!   doc comment, which records that beside the wiring rather than only here.
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
        Self::rooted_at(tmp, base)
    }

    /// A fixture whose data directory is short enough that the door's
    /// preferred socket path — `<state dir>/control.sock` — fits inside
    /// `sockaddr_un`.
    ///
    /// `/tmp` explicitly rather than the platform temp directory: on macOS
    /// the latter is a `/var/folders/…` path long enough on its own to push
    /// the preferred path over the limit, so a test that wanted the short
    /// branch and used [`Fixture::new`] would silently measure the long one.
    /// POSIX guarantees `/tmp`, and this file is `#![cfg(unix)]`.
    fn with_a_short_socket_path() -> Self {
        let tmp = tempfile::Builder::new()
            .prefix("gh")
            .tempdir_in("/tmp")
            .expect("tempdir under /tmp");
        let base = tmp.path().to_path_buf();
        Self::rooted_at(tmp, base)
    }

    /// A fixture whose data directory is deep enough to push the preferred
    /// socket path past `sockaddr_un`, forcing the door onto its temp-directory
    /// fallback.
    ///
    /// The nesting is the point and it is not arbitrary: `<state
    /// dir>/control.sock` is the data directory plus `projects/`, a
    /// thirty-two character project id, and the file name — so a hundred
    /// characters of directory guarantees the fallback on every Unix,
    /// whichever temp directory the platform hands out.
    fn with_a_long_socket_path() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().join("d".repeat(100));
        std::fs::create_dir_all(&base).expect("create the deep base");
        Self::rooted_at(tmp, base)
    }

    fn rooted_at(tmp: tempfile::TempDir, base: PathBuf) -> Self {
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

    /// The base directory this fixture's `--data-dir` and `--config-dir` sit
    /// under, as text — for asserting that it never appears in output.
    fn base_path(&self) -> String {
        self.base.display().to_string()
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

    /// Whether the harness running `session` has handled a real `SIGINT`.
    ///
    /// Written by the harness's own `trap`, so it is the worker's account of
    /// what reached it rather than the door's account of what it sent.
    fn reacted_to_interrupt(&self, root: &Path, session: &str) -> bool {
        root.join(format!("interrupted-{session}.log")).exists()
    }

    /// Run the shipped `glasshouse` binary as a user would, against this
    /// project.
    ///
    /// The same `--scope`, `--data-dir` and `--config-dir` [`Server::start`]
    /// uses, so the client resolves the *same* project — and therefore the
    /// same control socket — the server did. Nothing tells it where that
    /// socket is: finding it is part of what these tests check.
    fn client(&self, root: &Path, args: &[&str]) -> std::process::Output {
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
///
/// # The `INT` trap, and why an interrupt needs one
///
/// A door that answers `ok` to `Request::Interrupt` proves nothing about what
/// arrived — this session fixed a pty wedge (`cd27803`) that answered `ok`
/// while delivering nothing. The trap turns the interrupt into something the
/// **worker itself** writes down: `interrupt-$tag.log` can only exist if a
/// real `SIGINT` was raised in this process, and a `SIGINT` can only be raised
/// here because the terminal's line discipline turned a `0x03` byte on its
/// own pty into one. Nothing Glasshouse says about the request can produce
/// that file.
///
/// Trapping also keeps the harness *alive* through the interrupt (an untrapped
/// `sh` dies on `SIGINT`), which is what lets
/// [`an_interrupt_sent_by_the_client_makes_the_worker_react`] go on to prove
/// the session still takes input afterwards. An interrupt that killed the
/// worker would satisfy a weaker test and would not be an interrupt.
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
         trap 'echo interrupted >> \"$PWD/interrupted-$tag.log\"' INT\n\
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

    /// Whether the door bound inside the project's own state directory
    /// rather than falling back to a short name in the temp directory.
    ///
    /// Read from the path the server itself announced, so it reports which
    /// branch the *server* took — the client's agreement with it is what the
    /// test then measures, and a test that guessed the branch would prove
    /// nothing about the branch actually exercised.
    fn bound_in_the_state_dir(&self) -> bool {
        self.socket.file_name() == Some(std::ffi::OsStr::new("control.sock"))
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

// ---------------------------------------------------------------------------
// The client half — capability map lines 746 and 747.
//
// Everything above proves the *door*. Everything below proves the **surface**,
// and it is the surface that those two lines were returned premise-invalid
// for: the transport was real and no user could reach it. Each of these runs
// the shipped `glasshouse` binary as a person would, with no knowledge of
// where the socket is.
// ---------------------------------------------------------------------------

/// A `String` of `stderr`, for asserting on what a person was told.
fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Map line 746 — *"Allow direct user input to an orchestrated worker without
/// requiring the orchestrator as an intermediary."*
///
/// [`a_message_sent_through_the_door_reaches_a_real_worker_process`] proves
/// the transport; this proves a **person** can be the one who uses it. The
/// difference is the whole line, and it is visible in what this test starts:
/// a `glasshouse api send` process, from outside, with no agent running and
/// nothing consulted between the words and the worker's terminal.
///
/// The client is told the project, never the socket. Finding the door is part
/// of what is under test — see
/// [`the_client_finds_the_door_the_server_actually_bound`].
#[test]
fn a_message_sent_by_the_client_reaches_a_real_worker_process() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let worker = server.spawn_worker();
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &worker).is_some()
    });

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
    assert!(
        stdout_of(&sent).contains(&worker),
        "a successful send must name the session it reached: {:?}",
        stdout_of(&sent)
    );

    wait_for("the worker to read the line the client sent", || {
        fixture
            .received(&root, &worker)
            .is_some_and(|text| text.contains("typed-by-a-person"))
    });
}

/// Map line 747 — *"Allow the user to interrupt an orchestrated worker
/// directly."*
///
/// **The `ok` is not the evidence.** A door that answered `ok` and delivered
/// nothing is a defect this session has already paid for once (`cd27803`), so
/// what is asserted here is written by the *worker*: its `SIGINT` trap
/// appends a file, which cannot happen unless a real signal was raised in that
/// process — which cannot happen unless a real `0x03` reached its own
/// terminal's line discipline.
///
/// Then the other half, which is what makes it an *interrupt* rather than a
/// kill: the session is still there afterwards and still takes input. A test
/// that stopped at the reaction would pass just as happily against a client
/// that had killed the worker.
#[test]
fn an_interrupt_sent_by_the_client_makes_the_worker_react() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let worker = server.spawn_worker();
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &worker).is_some()
    });

    // Before, so the reaction below is known to be this interrupt's and not
    // something the harness does on the way up.
    assert!(
        !fixture.reacted_to_interrupt(&root, &worker),
        "the worker recorded an interrupt before one was sent"
    );

    let interrupted = fixture.client(&root, &["api", "interrupt", "--session", &worker]);
    assert!(
        interrupted.status.success(),
        "`glasshouse api interrupt` failed: {}",
        stderr_of(&interrupted)
    );

    wait_for("the worker to handle a real SIGINT", || {
        fixture.reacted_to_interrupt(&root, &worker)
    });

    // An interrupt, not a kill: the session survives it and still hears the
    // next thing a person says.
    let after = fixture.client(
        &root,
        &[
            "api",
            "send",
            "--session",
            &worker,
            "--text",
            "still-listening-after-the-interrupt",
        ],
    );
    assert!(
        after.status.success(),
        "the session must still be usable after an interrupt: {}",
        stderr_of(&after)
    );
    wait_for("the interrupted worker to read a later line", || {
        fixture
            .received(&root, &worker)
            .is_some_and(|text| text.contains("still-listening-after-the-interrupt"))
    });
}

/// The project boundary, against the client rather than against the door.
///
/// [`the_door_refuses_to_deliver_into_another_projects_worker`] proves the far
/// side refuses a foreign session. This proves the near side hands it no way
/// around that, which is a different claim: every scope check the door
/// performs is about the **session named in the request**, so a client that
/// could be aimed at another project's *door* would satisfy all of them and
/// cross the boundary anyway. **Aiming is the attack, so the aim is not a
/// parameter.**
///
/// # Both doors are put in one directory on purpose
///
/// [`Fixture::with_a_short_socket_path`] keeps both projects on the door's
/// preferred branch, so alpha's and beta's sockets are siblings under one
/// `projects/` directory that anything can enumerate. That is the harder case
/// and the one worth asserting: on the temp-directory fallback the two paths
/// are unguessable hashes, which would make this pass for a reason that
/// disappears the moment a project's state directory is short.
///
/// # What the refusal actually says, and why it is not what the packet
/// expected
///
/// The packet asked that a boundary crossing be *distinguishable from "no such
/// session"*. **It is not, and it should not be.** Every project has its own
/// database (`<state dir>/glasshouse.db`), so alpha's session is not a foreign
/// row in beta's store — it is not in beta's store at all, and
/// `ApiError::ForeignProject` is unreachable between two real projects. The
/// answer beta gives is *"no session `x` in this project"*, and saying more
/// would mean confirming to a caller that the session exists **somewhere
/// else**, which is the leak the boundary exists to prevent. The
/// distinguishing information a user needs is in the sentence already: the
/// answer is scoped, and it says so.
#[test]
fn the_client_cannot_reach_another_projects_worker() {
    let fixture = Fixture::with_a_short_socket_path();

    let alpha_root = fixture.project_root("alpha");
    let beta_root = fixture.project_root("beta");
    let alpha = Server::start(&fixture, &alpha_root);
    let beta = Server::start(&fixture, &beta_root);

    // §80 case 3: state the site is on the path before trusting anything
    // measured against it. Both doors must really be siblings, or the
    // enumerable-neighbour case below is not the case being tested.
    assert!(
        alpha.bound_in_the_state_dir() && beta.bound_in_the_state_dir(),
        "both doors must bind inside their own state directories for this test to be \
         about neighbouring sockets: {:?} and {:?}",
        alpha.socket,
        beta.socket
    );

    let victim = alpha.spawn_worker();
    wait_for("alpha's worker to start", || {
        fixture.argv(&alpha_root, &victim).is_some()
    });

    // (1) Beta's client, alpha's session.
    let crossed = fixture.client(
        &beta_root,
        &[
            "api",
            "send",
            "--session",
            &victim,
            "--text",
            "crossing-through-the-client",
        ],
    );
    assert!(
        !crossed.status.success(),
        "a client scoped to beta must not deliver into alpha's worker"
    );
    let refusal = stderr_of(&crossed);
    assert!(
        refusal.contains("no session") && refusal.contains("in this project"),
        "the refusal must scope itself, so a user knows the answer is about this \
         project rather than about the session's existence: {refusal:?}"
    );

    // (2) There is no socket to aim. `serve` takes `--socket`; the client
    // must not, or the door's project scope becomes a suggestion. **This is
    // the assertion that fails if the escape hatch is ever added.**
    let aimed = fixture.client(
        &beta_root,
        &[
            "api",
            "send",
            "--socket",
            &alpha.socket.display().to_string(),
            "--session",
            &victim,
            "--text",
            "aimed-at-alphas-door",
        ],
    );
    assert!(
        !aimed.status.success(),
        "`glasshouse api send --socket` must not exist; it is a path around every \
         project-scope check the door performs"
    );
    let rejected = stderr_of(&aimed);
    assert!(
        rejected.contains("--socket"),
        "the argument must be rejected by name, so the failure is legible: {rejected:?}"
    );

    // (3) Beta's door still works, so none of the above passed because
    // nothing can be delivered at all.
    let bystander = beta.spawn_worker();
    wait_for("beta's worker to start", || {
        fixture.argv(&beta_root, &bystander).is_some()
    });
    let delivered = fixture.client(
        &beta_root,
        &[
            "api",
            "send",
            "--session",
            &bystander,
            "--text",
            "staying-inside-this-project",
        ],
    );
    assert!(delivered.status.success(), "{}", stderr_of(&delivered));
    wait_for("beta's own worker to read its line", || {
        fixture
            .received(&beta_root, &bystander)
            .is_some_and(|text| text.contains("staying-inside-this-project"))
    });

    // Nothing crossed, by alpha's worker's own account rather than by status.
    assert!(
        !fixture.received(&alpha_root, &victim).is_some_and(|text| {
            text.contains("crossing-through-the-client") || text.contains("aimed-at-alphas-door")
        }),
        "alpha's worker received a line addressed to it from outside its project"
    );
}

/// The canonical-mode refusal, surfaced to the person who typed the line.
///
/// `SessionRuntime` refuses a line that would overflow the terminal's
/// `MAX_CANON` buffer, because writing it would discard the line *and every
/// byte written afterwards* — the session goes deaf and the sender is told
/// `ok`. That refusal is only worth anything if it reaches the user, so this
/// asserts three things in order: the client fails, it says why in the
/// terminal's own terms, and the session still works.
///
/// **The length is deliberately not derived from the limit** (§80 case 6). A
/// test that built its line from `MAX_CANONICAL_LINE_BYTES` would rescale with
/// any mutation of it and survive. 8192 is a fixed number chosen to sit above
/// every platform's real `MAX_CANON` — 1024 on the BSD family including macOS,
/// 4096 on Linux — so it is over the limit for a reason that has nothing to do
/// with what the constant says.
#[test]
fn a_line_over_the_canonical_limit_is_refused_to_the_user_and_the_session_survives() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let worker = server.spawn_worker();
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &worker).is_some()
    });

    let too_long = "x".repeat(8192);
    let refused = fixture.client(
        &root,
        &["api", "send", "--session", &worker, "--text", &too_long],
    );
    assert!(
        !refused.status.success(),
        "a line the terminal cannot take must not be reported as delivered: {}",
        stdout_of(&refused)
    );
    let told = stderr_of(&refused);
    assert!(
        told.contains("canonical mode") && told.contains("refused"),
        "the user must be told the terminal refused the line, in the terminal's own \
         terms: {told:?}"
    );
    assert!(
        !stdout_of(&refused).contains("delivered"),
        "a refused send must not also print a delivery: {:?}",
        stdout_of(&refused)
    );

    // The refusal exists to keep the session usable. Prove it did.
    let after = fixture.client(
        &root,
        &[
            "api",
            "send",
            "--session",
            &worker,
            "--text",
            "short-line-after-the-refusal",
        ],
    );
    assert!(
        after.status.success(),
        "the session must still take input after a refused line: {}",
        stderr_of(&after)
    );
    wait_for("the worker to read a line sent after the refusal", || {
        fixture
            .received(&root, &worker)
            .is_some_and(|text| text.contains("short-line-after-the-refusal"))
    });
    assert!(
        !fixture
            .received(&root, &worker)
            .is_some_and(|text| text.contains(&too_long)),
        "the refused line must not have been written to the terminal at all"
    );
}

/// Every failure prints something a person can act on, and none of them
/// prints a path.
///
/// Two failure modes that have nowhere else to be asserted — a door that is
/// not running, and a session that does not exist — plus the standing
/// constraint on all of them. Commit `8b489b7` fixed a leak of the database's
/// absolute path through this same door; a client is a second surface with the
/// same hazard, and it resolves a socket path it could easily name.
///
/// The absence assertion is rendered into a full, untruncated capture of
/// `stderr` and is paired with a positive one on the same string (§17), so it
/// cannot pass by the output being empty or clipped.
#[test]
fn the_client_says_what_went_wrong_without_naming_a_path() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");

    // No server at all. This is the first thing a user will hit, and the
    // action they need is the same whichever path the socket would have had.
    let closed = fixture.client(
        &root,
        &[
            "api",
            "send",
            "--session",
            "0123456789abcdef0123456789abcdef",
            "--text",
            "nobody-is-listening",
        ],
    );
    assert!(!closed.status.success(), "there is no door to answer");
    let told = stderr_of(&closed);
    assert!(
        told.contains("not listening") && told.contains("glasshouse api serve"),
        "a missing door must name the command that opens one: {told:?}"
    );

    // Now with a door, and a session that is not in it.
    let server = Server::start(&fixture, &root);
    let unknown = fixture.client(
        &root,
        &[
            "api",
            "interrupt",
            "--session",
            "0123456789abcdef0123456789abcdef",
        ],
    );
    assert!(!unknown.status.success(), "there is no such session");
    let missing = stderr_of(&unknown);
    assert!(
        missing.contains("no session") && missing.contains("0123456789abcdef"),
        "an unknown session must be named back to the user: {missing:?}"
    );

    // Neither of them, nor a successful call, may name where anything lives.
    let worker = server.spawn_worker();
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &worker).is_some()
    });
    let ok = fixture.client(
        &root,
        &["api", "send", "--session", &worker, "--text", "fine"],
    );
    assert!(ok.status.success(), "{}", stderr_of(&ok));

    let base = fixture.base_path();
    let root_text = root.display().to_string();
    for (what, output) in [("no door", &closed), ("no session", &unknown), ("ok", &ok)] {
        let all = format!("{}{}", stdout_of(output), stderr_of(output));
        assert!(
            !all.contains(&base) && !all.contains(&root_text) && !all.contains(".sock"),
            "the `{what}` case named a filesystem path: {all:?}"
        );
    }
}

/// The client finds the door **on both branches** of the socket-path
/// computation.
///
/// `unix::socket_path_for` is private to a module this package may not edit,
/// so `client::socket_path_for` is a copy of it. A copy is worth exactly what
/// proves it, and the proof has to cover both branches, because the branch a
/// test takes by accident depends on how long the platform's temp directory
/// is — macOS hands out a `/var/folders/…` path that forces the fallback and
/// Linux hands out `/tmp/…` that does not, so a single fixture would leave one
/// branch unmeasured on every machine and a *different* one on each.
///
/// Each half asserts which branch the **server** took, from the path the
/// server itself announced, before driving a real send through the client that
/// was told only the project. If the two computations ever disagree, the
/// client reports a door that is not listening and this fails.
#[test]
fn the_client_finds_the_door_the_server_actually_bound() {
    for (what, fixture) in [
        ("short", Fixture::with_a_short_socket_path()),
        ("long", Fixture::with_a_long_socket_path()),
    ] {
        let root = fixture.project_root("alpha");
        let server = Server::start(&fixture, &root);

        let in_state_dir = server.bound_in_the_state_dir();
        match what {
            "short" => assert!(
                in_state_dir,
                "a short base must exercise the preferred branch, or this half measures \
                 the same thing as the other; the server bound {:?}",
                server.socket
            ),
            _ => assert!(
                !in_state_dir,
                "a deep base must exercise the temp-directory fallback; the server bound \
                 {:?}",
                server.socket
            ),
        }

        let worker = server.spawn_worker();
        wait_for("the worker's harness to start", || {
            fixture.argv(&root, &worker).is_some()
        });

        let sent = fixture.client(
            &root,
            &[
                "api",
                "send",
                "--session",
                &worker,
                "--text",
                "found-the-door",
            ],
        );
        assert!(
            sent.status.success(),
            "the client did not find the {what} socket the server bound at {:?}: {}",
            server.socket,
            stderr_of(&sent)
        );
        wait_for("the worker to read the line the client sent", || {
            fixture
                .received(&root, &worker)
                .is_some_and(|text| text.contains("found-the-door"))
        });
    }
}
