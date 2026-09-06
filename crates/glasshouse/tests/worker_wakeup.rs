//! Phase 15 — the orchestrator wake-up flow, capability map lines 733-739.
//!
//! Everything here drives the shipped binary: `glasshouse api serve` over its
//! real Unix domain socket for the door, and a real `glasshouse hook`
//! *process* for the completion signal. That second part is the point of the
//! file. A turn ending is something a **harness** reports, through a separate
//! short-lived hook process, and the only honest way to prove Glasshouse
//! detects it "from native lifecycle hooks" (line 734) is to let that process
//! run and write the row itself, exactly as Claude Code's `Stop` hook does.
//!
//! Nothing here reads a session's terminal output to decide anything, and
//! nothing waits for a screen to go quiet. `scripts/worker-watch.sh` — this
//! project's own shell-script version of this capability — does both, and has
//! announced a worker finished three times while it was still thinking.
//!
//! # How a session is told apart from its sibling
//!
//! The fixture's harness derives its log file's name from the `--settings`
//! argument Glasshouse passes it, which names the session's own state
//! directory. Two consequences, both deliberate:
//!
//! - a line delivered to the orchestrator can be distinguished from one
//!   delivered to the worker, which is what makes "the orchestrator was
//!   woken" an assertion rather than a hope;
//! - and if the door ever stops installing lifecycle hooks on the sessions it
//!   spawns, every test in this file fails, because the argument the file
//!   name comes from is the hook installation's own.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(30);

/// A project with an installed harness that echoes every line it reads and
/// records it under a name derived from its own `--settings` argument.
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
    ///
    /// Absent until the harness reads its first line, so callers poll it.
    fn received(&self, root: &Path, session: &str) -> Option<String> {
        std::fs::read_to_string(root.join(format!("received-{session}.log"))).ok()
    }

    /// The command line the harness running `session` was started with.
    fn argv(&self, root: &Path, session: &str) -> Option<String> {
        std::fs::read_to_string(root.join(format!("argv-{session}.log"))).ok()
    }

    /// Every command line any harness in this project was started with,
    /// recorded whether or not it could be attributed to a session.
    ///
    /// The per-session name above is derived from the lifecycle-hook
    /// argument, so it is *absent* precisely when hook installation is
    /// broken — which would turn every assertion about that argument into a
    /// timeout, and a timeout proves only that something is wrong. This one
    /// is written unconditionally, so the assertion about the argument runs
    /// and fails on its own terms.
    fn any_argv(&self, root: &Path) -> Option<String> {
        std::fs::read_to_string(root.join("argv-any.log")).ok()
    }

    /// Run a real `glasshouse hook` process, exactly as a harness's own
    /// lifecycle hook does — a separate short-lived invocation that reports
    /// one event and exits.
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
            "`glasshouse hook --session {session} --event {event}` must never fail: \
             a harness treats a non-zero hook exit as a veto"
        );
    }
}

/// A harness that names its own log files after the session it was started
/// for, taken from the `--settings <state>/sessions/<id>/settings.json`
/// argument the lifecycle-hook installation adds.
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
         echo \"$@\" >> \"$PWD/argv-any.log\"\n\
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

    /// Spawn a session and return its identifier.
    fn spawn(&self, role: &str) -> String {
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

/// Every `glasshouse worker-completion` line in `text`, decoded.
fn completions(text: &str) -> Vec<serde_json::Value> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("glasshouse worker-completion "))
        .map(|json| serde_json::from_str(json).expect("a completion payload is one line of JSON"))
        .collect()
}

/// Line 734's producer, and the premise every other test here rests on
/// (§17): a session this door spawns is started with its harness's own
/// lifecycle hooks installed, so it has something to report a turn ending
/// *with*.
///
/// Before this, `api::unix::spawn_session` was the one launch path in the
/// binary that installed nothing — `main.rs`'s `launch_session` always did —
/// so an orchestrator's own worker was the only kind of session that could
/// finish a turn silently.
#[test]
fn a_worker_this_door_spawns_is_started_with_its_lifecycle_hooks_installed() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let worker = server.spawn("worker");
    wait_for("the harness to record its command line", || {
        fixture.any_argv(&root).is_some()
    });

    let argv = fixture.any_argv(&root).expect("the harness argv");
    assert!(
        argv.contains("--settings"),
        "a spawned worker must be pointed at a lifecycle-hook document: {argv}"
    );
    assert!(
        argv.contains(&format!("sessions/{worker}")),
        "the hook document must be the one written for this session: {argv}"
    );

    let document = argv
        .split_whitespace()
        .find(|arg| arg.ends_with(".json"))
        .expect("a settings document on the command line");
    let contents = std::fs::read_to_string(document).expect("the hook document must exist");
    assert!(
        contents.contains("Stop"),
        "the installed document must ask the harness to report `Stop` — the one \
         event that carries a completion claim: {contents}"
    );
    assert!(
        contents.contains(&worker),
        "each hook must report against this session by name: {contents}"
    );
}

/// Line 733's premise (§17), and the refusal that keeps a watch honest.
///
/// A watch is a standing instruction to type into a session. Registering one
/// against a session this process does not hold would succeed, deliver
/// nothing, and tell nobody — which is the exact shape of the failure this
/// project's own `worker-watch.sh` produced when a finished worker was lost
/// because nothing was really watching it.
#[test]
fn a_watch_is_refused_when_it_could_never_be_delivered() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let worker = server.spawn("worker");
    let orchestrator = server.spawn("orchestrator");

    let live = server.call(serde_json::json!({
        "op": "watch_worker", "session": worker, "notify": orchestrator,
    }));
    assert_eq!(
        live["status"], "ok",
        "two live sessions in this project must be watchable: {live}"
    );

    let unknown = server.call(serde_json::json!({
        "op": "watch_worker", "session": "no-such-session", "notify": orchestrator,
    }));
    assert_eq!(unknown["status"], "error", "{unknown}");

    let nowhere = server.call(serde_json::json!({
        "op": "watch_worker", "session": worker, "notify": "no-such-session",
    }));
    assert_eq!(nowhere["status"], "error", "{nowhere}");

    let itself = server.call(serde_json::json!({
        "op": "watch_worker", "session": worker, "notify": worker,
    }));
    assert_eq!(
        itself["status"], "error",
        "a session must not be watched on its own behalf: {itself}"
    );
}

/// Lines 733, 734, 735, 736 and 737 together, end to end and through the
/// shipped binary.
///
/// An orchestrator registers interest in a worker; the worker's own harness
/// hook reports that a turn ended; the door notices and types one structured
/// line into the orchestrator's terminal as Glasshouse rather than as the
/// user.
///
/// The completion is produced by a **real `glasshouse hook` process**, which
/// is what makes this line 734 rather than a test that seeds the answer it
/// wants: the row in the log is written by `session::lifecycle::event_for`,
/// the single construction site of `TurnEnded`, from the harness's own word
/// `Stop`.
#[test]
fn a_workers_completion_reported_by_a_lifecycle_hook_wakes_the_orchestrator() {
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

    // The worker works, and then its harness says it stopped.
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

    // Line 737, field by field.
    assert_eq!(completion["worker"], worker, "{completion}");
    assert_eq!(
        completion["harness"], "claude-code",
        "the harness that reported it: {completion}"
    );
    assert_eq!(
        completion["outcome"], "completed",
        "the lifecycle result: {completion}"
    );
    assert!(
        completion["seq"].as_i64().is_some_and(|seq| seq > 0),
        "the log position the claim is anchored to: {completion}"
    );
    assert!(
        completion["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("turn_ended")),
        "a concise summary of what Glasshouse observed: {completion}"
    );

    // The worker itself was never written to. The wake-up flow reads a log
    // and types into the orchestrator; it does not touch the session it is
    // reporting about.
    let worker_received = fixture.received(&root, &worker).unwrap_or_default();
    assert!(
        completions(&worker_received).is_empty(),
        "a completion must never be delivered into the worker that produced it: \
         {worker_received}"
    );
}

/// Capability map line 740 — "Preserve the user's ability to enter and
/// modify a worker session before the orchestrator acts on its result."
///
/// The wake-up path (a `Stop` hook reported, the orchestrator woken) must
/// not itself close, lock or mark the worker read-only: `send_message`
/// still delivers to it afterward, and `session_state` still reports it
/// rather than refusing as `NotLive`.
#[test]
fn send_message_still_delivers_to_a_worker_after_its_wake_up_completion() {
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

    // The worker's turn has ended and the orchestrator has been notified.
    // Sending to the worker now must still work: nothing about the wake-up
    // itself may refuse or lock the session out.
    const AFTER: &str = "still reachable after the completion";
    let sent = server.call(serde_json::json!({
        "op": "send_message", "session": worker, "text": AFTER,
    }));
    assert_eq!(
        sent["status"], "ok",
        "a send to a worker that just reported completion must not be \
         refused as not-live: {sent}"
    );

    let state = server.call(serde_json::json!({ "op": "session_state", "session": worker }));
    assert_eq!(state["status"], "ok", "{state}");
    assert_ne!(
        state["result"]["lifecycle"], "closed",
        "the wake-up path must not have closed the session: {state}"
    );

    wait_for("the worker to receive the post-completion message", || {
        fixture
            .received(&root, &worker)
            .is_some_and(|text| text.contains(AFTER))
    });
}

/// Line 733: registering interest is a statement about what happens next.
///
/// A watch registered after a worker has already finished a turn must not
/// replay that turn. Two reasons, and the second is the one that bites: an
/// orchestrator restarting a watch would be woken for work it already acted
/// on, and a watch registered against a long-lived project would deliver its
/// whole history in one burst.
#[test]
fn registering_interest_replays_nothing_that_already_happened() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let worker = server.spawn("worker");
    let orchestrator = server.spawn("orchestrator");
    wait_for("both harnesses to start", || {
        fixture.argv(&root, &worker).is_some() && fixture.argv(&root, &orchestrator).is_some()
    });

    // A turn ends before anybody is watching.
    fixture.hook(&root, &worker, "Stop");

    let registered = server.call(serde_json::json!({
        "op": "watch_worker", "session": worker, "notify": orchestrator,
    }));
    assert_eq!(registered["status"], "ok", "{registered}");
    let from = registered["result"]["from"]
        .as_i64()
        .expect("a log position");
    assert!(
        from > 0,
        "the watch must start from the log's real position, not from zero: {registered}"
    );

    // A second turn ends, this time with the watch in place. Its arrival is
    // what proves the absence below is an absence and not merely a race:
    // everything up to and including this event has been pumped.
    fixture.hook(&root, &worker, "Stop");
    wait_for("the watched completion to arrive", || {
        fixture
            .received(&root, &orchestrator)
            .is_some_and(|text| !completions(&text).is_empty())
    });

    let text = fixture.received(&root, &orchestrator).unwrap();
    let delivered = completions(&text);
    assert_eq!(
        delivered.len(),
        1,
        "only the completion that happened after registration should arrive: {text}"
    );
    assert!(
        delivered[0]["seq"].as_i64().unwrap() > from,
        "the delivered completion must be newer than the watch's start: {text}"
    );
}

/// Line 739: the cursor is the entire dedup mechanism, and this proves it
/// holds once delivery has actually happened, not merely once.
///
/// A single `Stop` must produce exactly one completion, forever — not just
/// at the instant it arrives. The server's tick runs every 50ms, so this
/// keeps calling the door for ~2s (≥40 ticks) after the first completion
/// lands and re-checks the count, rather than trusting a one-shot read.
#[test]
fn one_completion_wakes_the_orchestrator_exactly_once() {
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

    // Keep the door doing something meaningful for ~40 ticks so this is a
    // real wait, not a sleep, while giving a broken cursor a real chance to
    // re-deliver the same row.
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let listed = server.call(serde_json::json!({ "op": "list_sessions" }));
        assert_eq!(listed["status"], "ok", "{listed}");
        std::thread::sleep(Duration::from_millis(50));
    }

    let text = fixture.received(&root, &orchestrator).unwrap();
    assert_eq!(
        completions(&text).len(),
        1,
        "a single `Stop` must wake the orchestrator exactly once, not just \
         once at first glance: {text}"
    );
}

/// Line 733's idempotence, the other half of `watch_worker`'s doc comment:
/// a second registration over the same `(worker, notify)` pair replaces the
/// first watch rather than adding a second one, so it must neither replay
/// what already arrived nor double what arrives next.
#[test]
fn re_registering_the_same_watch_does_not_deliver_a_completion_twice() {
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
    wait_for("the first completion to arrive", || {
        fixture
            .received(&root, &orchestrator)
            .is_some_and(|text| !completions(&text).is_empty())
    });

    let reregistered = server.call(serde_json::json!({
        "op": "watch_worker", "session": worker, "notify": orchestrator,
    }));
    assert_eq!(reregistered["status"], "ok", "{reregistered}");

    fixture.hook(&root, &worker, "UserPromptSubmit");
    fixture.hook(&root, &worker, "Stop");
    wait_for("the second completion to arrive", || {
        fixture
            .received(&root, &orchestrator)
            .is_some_and(|text| completions(&text).len() >= 2)
    });

    // A re-registration that replayed the first completion, or a duplicate
    // watch that doubles the second, would over-deliver quickly; give it a
    // moment before settling on the final count.
    std::thread::sleep(Duration::from_millis(300));

    let text = fixture.received(&root, &orchestrator).unwrap();
    assert_eq!(
        completions(&text).len(),
        2,
        "re-registering the same watch must neither replay the first \
         completion nor create a second watch that doubles the next one: \
         {text}"
    );
}

/// Line 738: the notification names a session the orchestrator can actually
/// ask about, using nothing but the identifier the payload itself carries.
#[test]
fn the_orchestrator_can_inspect_the_worker_the_notification_names() {
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
    assert_eq!(delivered.len(), 1, "{text}");
    // Taken out of the payload, deliberately not the `worker` id variable
    // already in scope: the point is that the identifier the notification
    // carries is directly usable.
    let named = delivered[0]["worker"]
        .as_str()
        .expect("a worker field in the completion payload")
        .to_owned();

    let state = server.call(serde_json::json!({ "op": "session_state", "session": named }));
    assert_eq!(state["status"], "ok", "{state}");
    assert!(
        !state["result"]["lifecycle"].is_null(),
        "the response to `session_state` must carry a lifecycle: {state}"
    );

    let listed = server.call(serde_json::json!({ "op": "list_sessions" }));
    assert_eq!(listed["status"], "ok", "{listed}");
    assert!(
        listed["result"].to_string().contains(&named),
        "the worker named by the notification must appear in \
         `list_sessions`: {listed}"
    );
}

/// Line 737's boundary, proven rather than asserted from the doc comment:
/// nothing a worker said or did — not its own words, not the harness's own
/// event spelling — can reach the line typed into the orchestrator.
///
/// The negative half alone would be vacuous if the summary happened to be
/// empty, so this also asserts the positive half: the summary is built
/// entirely from Glasshouse's own fixed vocabulary.
#[test]
fn a_completion_never_carries_a_workers_own_words() {
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

    const CANARY: &str = "CANARY-e7f3a1-secret-payload";
    let sent = server.call(serde_json::json!({
        "op": "send_message", "session": worker, "text": CANARY,
    }));
    assert_eq!(sent["status"], "ok", "{sent}");
    wait_for("the worker's harness to actually read the canary", || {
        fixture
            .received(&root, &worker)
            .is_some_and(|text| text.contains(CANARY))
    });

    fixture.hook(&root, &worker, "UserPromptSubmit");
    fixture.hook(&root, &worker, "Stop");
    wait_for("the orchestrator to be woken", || {
        fixture
            .received(&root, &orchestrator)
            .is_some_and(|text| !completions(&text).is_empty())
    });

    let text = fixture.received(&root, &orchestrator).unwrap();
    // The raw line, before any JSON decoding: an absence assertion is only
    // as strong as what it renders into, and decoding first would let a
    // leak hide in a field nothing here inspects.
    let line = text
        .lines()
        .find(|line| line.trim().starts_with("glasshouse worker-completion "))
        .expect("a delivered completion line")
        .to_owned();

    assert!(!line.contains(CANARY), "{line}");
    assert!(
        !line.contains("Stop"),
        "the harness's own spelling of the event must not leak: {line}"
    );
    assert!(
        !line.contains("UserPromptSubmit"),
        "the harness's own spelling of the event must not leak: {line}"
    );

    let delivered = completions(&text);
    assert_eq!(delivered.len(), 1, "{text}");
    let summary = delivered[0]["summary"]
        .as_str()
        .expect("a summary string")
        .to_owned();
    assert!(summary.contains("turn_ended"), "{summary}");

    const KINDS: &[&str] = &[
        "session_started",
        "session_resumed",
        "turn_started",
        "turn_ended",
        "waiting_for_user",
        "text_delivered",
        "interrupt_delivered",
        "process_exited",
        "output_ended",
        "gateway_unhealthy",
        "gateway_backend_changed",
    ];

    // Strip an optional ` in <n>s` tail, then check every arrow-separated
    // token against the fixed vocabulary, allowing a leading `… (N earlier)`
    // elision prefix in place of a token.
    let mut body = summary.as_str();
    if let Some(idx) = body.rfind(" in ") {
        let tail = &body[idx + " in ".len()..];
        assert!(
            tail.strip_suffix('s')
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())),
            "the timing tail must read `in <n>s`: {summary}"
        );
        body = &body[..idx];
    }

    for (index, token) in body.split(" → ").enumerate() {
        if index == 0 && token.starts_with('…') {
            assert!(
                token.starts_with("… (") && token.ends_with(" earlier)"),
                "the elision prefix must read `… (N earlier)`: {summary}"
            );
            continue;
        }
        assert!(
            KINDS.contains(&token),
            "unrecognized token `{token}` in summary: {summary}"
        );
    }
}

/// Lines 733 and 737: a watch is interest in **one** worker, and the
/// notification names the worker that actually finished.
///
/// Written because a mutation demanded it. Deleting the
/// `row.session != watch.worker` guard — so a watch delivers every
/// completion in the project — survived the other eight tests in this file
/// untouched, because every one of them runs a single worker. An
/// orchestrator running five would have been told the wrong one finished,
/// which is worse than not being told at all: it is a wake-up that sends the
/// orchestrator to inspect a session that is still working.
///
/// The unwatched worker's turn ends **first**, so a leak would arrive at a
/// lower log position than the watched one and be the first completion read
/// — the absence below is therefore an ordering assertion, not a race.
#[test]
fn a_watch_delivers_only_the_completions_of_the_worker_it_names() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let watched = server.spawn("worker");
    let unwatched = server.spawn("worker");
    let orchestrator = server.spawn("orchestrator");
    wait_for("all three harnesses to start", || {
        fixture.argv(&root, &watched).is_some()
            && fixture.argv(&root, &unwatched).is_some()
            && fixture.argv(&root, &orchestrator).is_some()
    });

    let registered = server.call(serde_json::json!({
        "op": "watch_worker", "session": watched, "notify": orchestrator,
    }));
    assert_eq!(registered["status"], "ok", "{registered}");

    // The worker nobody registered interest in finishes first.
    fixture.hook(&root, &unwatched, "UserPromptSubmit");
    fixture.hook(&root, &unwatched, "Stop");
    // Then the watched one.
    fixture.hook(&root, &watched, "UserPromptSubmit");
    fixture.hook(&root, &watched, "Stop");

    wait_for("the orchestrator to be woken", || {
        fixture
            .received(&root, &orchestrator)
            .is_some_and(|text| !completions(&text).is_empty())
    });
    // Both rows are now behind the cursor; give a leaking pump the chance to
    // deliver the second one before settling on the count.
    std::thread::sleep(Duration::from_millis(300));

    let text = fixture.received(&root, &orchestrator).unwrap();
    let delivered = completions(&text);
    assert_eq!(
        delivered.len(),
        1,
        "only the watched worker's completion may be delivered: {text}"
    );
    assert_eq!(
        delivered[0]["worker"], watched,
        "the completion must name the worker the watch was registered for, \
         not whichever session happened to finish: {text}"
    );
}
