//! Phase 15/16 — a session driven through `glasshouse api serve` has a
//! history, and an observer can read it.
//!
//! Capability map line 748, *"record user intervention so the orchestrator
//! can be informed that the worker state may have changed"*, and the larger
//! defect underneath it that `report-gh-worker-access.md` measured: **the
//! door recorded nothing at all.** Not interventions, not `session_started`,
//! not `process_exited`. A worker's whole life was absent from the durable
//! record unless a separate `glasshouse hook` process happened to write a row
//! from outside.
//!
//! # Why a test that reads SQLite is not enough, and this file's shape
//!
//! The obvious fix — attach an event-log sink to the door's runtime, exactly
//! as `shell::run` does — was written, mutated in, and **survived**: rows
//! appeared in `lifecycle_events`, correctly stamped `machine`, and
//! `Request::Events` still returned `[]`. `EventLog::observed_since` filtered
//! them out, and an intervention is not something a harness reports.
//!
//! So the defect had two independent causes and the fix has two halves, and
//! this file keeps a distinct failure for each of them:
//!
//! - [`a_session_driven_through_the_door_has_a_history_an_observer_can_read`]
//!   and [`an_intervention_is_recorded_with_the_origin_that_says_who_made_it`]
//!   drive the shipped binary end to end and read back through the same door,
//!   so **both** halves have to be present for either to pass.
//! - [`an_event_no_harness_reported_is_still_this_projects_history`] seeds one
//!   unobserved row directly and never spawns anything, so it fails when the
//!   **read** half is reverted and passes when only the write half is. That is
//!   the test the SURVIVED mutation did not have.
//!
//! Everything else here guards what the new write path could break: the
//! project boundary, the content boundary, and the rule that bookkeeping
//! never costs the door a request.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use glasshouse::cli::Cli;
use glasshouse::events::{EventBus, EventLog, LifecycleEvent, MessageOrigin};
use glasshouse::memory::{MemoryAuthority, MemoryKind, NewMemory, ProjectMemory};
use glasshouse::session::SessionId;

const TIMEOUT: Duration = Duration::from_secs(30);

/// A project with an installed harness that records every line it reads under
/// a name derived from its own `--settings` argument.
///
/// The same fixture `worker_access.rs` and `worker_wakeup.rs` use, and for
/// the same reason: the session tag comes from the lifecycle-hook
/// installation's own argument, so a door that stopped installing hooks would
/// fail these tests rather than quietly pass them against an unattributable
/// log file.
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

    /// This fixture's own [`glasshouse::Runtime`] for one project, bootstrapped
    /// exactly as the binary bootstraps its own.
    ///
    /// Used for the two things a test needs that the socket cannot give it:
    /// the project's database path, and a way to seed a row the door did not
    /// write.
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

    /// Every event this project's history holds, from the start of the log,
    /// as the orchestrator on the far end of the socket sees it.
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

    /// The same, narrowed to one session.
    fn events_for(&self, session: &str) -> Vec<serde_json::Value> {
        self.events()
            .into_iter()
            .filter(|event| event["session"] == session)
            .collect()
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

fn kinds(events: &[serde_json::Value]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| event["kind"].as_str())
        .collect()
}

/// The headline, and the thing that was wholly absent: a session that exists
/// only because the door started it leaves a record an observer can read.
///
/// Both halves of the fix are load-bearing here. Without the sink, the events
/// are published into a bus with no subscriber and never reach the database.
/// Without the unfiltered read, they reach it and `Request::Events` filters
/// them straight back out — which is exactly the state the discovery
/// package's mutation left the tree in, with rows in SQLite and `[]` on the
/// wire.
#[test]
fn a_session_driven_through_the_door_has_a_history_an_observer_can_read() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    // The premise (§17): before the spawn there is nothing, so nothing below
    // can be an artefact of a log that was already full.
    assert!(
        server.events().is_empty(),
        "a project with no sessions must have no history"
    );

    let worker = server.spawn_worker();
    // Causal, not a sleep: the file appears only once the harness has run far
    // enough to read its own argument list.
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &worker).is_some()
    });

    let history = server.events_for(&worker);
    assert!(
        kinds(&history).contains(&"session_started"),
        "a worker spawned through the door must appear in the project's own \
         history — the whole point of line 748's surrounding phase is that an \
         orchestrated worker is not invisible. Saw: {history:?}"
    );

    // And it keeps being a history rather than one row: the exit of the
    // process the door owns is recorded too, which is the other half of what
    // `glasshouse api serve` used to lose entirely.
    let interrupted = server.call(serde_json::json!({ "op": "interrupt", "session": worker }));
    assert_eq!(interrupted["status"], "ok", "{interrupted}");
    wait_for("the worker's exit to be recorded", || {
        kinds(&server.events_for(&worker)).contains(&"process_exited")
    });
}

/// Map line 748 itself: *"record user intervention so the orchestrator can be
/// informed that the worker state may have changed."*
///
/// Two interventions are delivered through the door and both come back on the
/// orchestrator's own read path, each carrying the origin that says who made
/// it. The origin is the part that makes the record answer the question line
/// 748 asks — a `text_delivered` with no origin would say the worker's state
/// changed without saying at whose hand, and `MessageOrigin` exists precisely
/// because a machine-sent line and a typed one are otherwise identical bytes.
///
/// # This is the **default** branch, and that is what it is for
///
/// Both requests below are made the way an orchestrator makes them — the
/// protocol spoken straight into the door, with **no `origin` field at all** —
/// and both must still record `machine`. `Request::SendMessage` and
/// `Request::Interrupt` grew an origin when `glasshouse api send` shipped and
/// a person's keystrokes started arriving here; the field defaults to
/// `protocol::RequestOrigin::Machine` precisely so that every caller written
/// before it existed, this test included, keeps meaning what it meant.
///
/// A request that *does* state its origin is
/// [`the_origin_a_request_states_is_the_origin_recorded`], and the two
/// together are what make this door's log answer line 748's *"user
/// intervention"* rather than merely record that something happened.
#[test]
fn an_intervention_is_recorded_with_the_origin_that_says_who_made_it() {
    const TEXT: &str = "intervention-one";

    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let worker = server.spawn_worker();
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &worker).is_some()
    });

    let sent = server.call(serde_json::json!({
        "op": "send_message", "session": worker, "text": TEXT,
    }));
    assert_eq!(sent["status"], "ok", "{sent}");

    // Waited for *before* the interrupt, not after. An interrupt reaches this
    // harness as a real `0x03` and kills its `sh` read loop outright, so
    // sending both and then looking is a race the interrupt wins. It is also
    // what the assertion needs: the delivery has to have happened for its
    // record to mean anything.
    wait_for("the worker to read the delivered line", || {
        fixture
            .received(&root, &worker)
            .is_some_and(|text| text.contains(TEXT))
    });

    let interrupted = server.call(serde_json::json!({ "op": "interrupt", "session": worker }));
    assert_eq!(interrupted["status"], "ok", "{interrupted}");

    let history = server.events_for(&worker);

    let delivered = history
        .iter()
        .find(|event| event["kind"] == "text_delivered")
        .unwrap_or_else(|| panic!("no `text_delivered` in the worker's history: {history:?}"));
    assert_eq!(
        delivered["origin"], "machine",
        "the record must say who intervened, and a request that named no \
         origin is Glasshouse or an orchestrator through it: {delivered}"
    );
    assert_eq!(
        delivered["bytes"],
        serde_json::json!(TEXT.len() + 1),
        "the length of the line delivered, carriage return included: {delivered}"
    );

    let interrupt = history
        .iter()
        .find(|event| event["kind"] == "interrupt_delivered")
        .unwrap_or_else(|| panic!("no `interrupt_delivered` in the worker's history: {history:?}"));
    assert_eq!(
        interrupt["origin"], "machine",
        "an interrupt is an intervention too, and defaults the same way: \
         {interrupt}"
    );
}

/// The other branch of line 748's *"user intervention"*: a request that says
/// whose intervention it is.
///
/// [`an_intervention_is_recorded_with_the_origin_that_says_who_made_it`]
/// covers the default — a caller that names no origin is a machine. This
/// covers the field being read at all, on both write verbs, and it is
/// deliberately driven **on the wire** rather than through the shipped
/// client: `tests/worker_access.rs` proves `glasshouse api send` sets the
/// field, and this proves the door acts on it. Two halves, tested where each
/// one lives.
///
/// # Why the same session, in the same log, in one test
///
/// The two origins are asserted against rows written seconds apart through
/// one door into one worker. A test that could only produce `user_keystroke`
/// rows would pass a door that ignored the field and stamped everything as
/// the person; the machine-originated pair asserted here alongside them is
/// what rules that out — and the ordering (`seq`) is what proves four rows
/// rather than two were written.
///
/// # An origin the protocol does not know is refused, not defaulted
///
/// The last section asserts that `"origin": "orchestrator"` — a plausible
/// word this vocabulary does not contain — fails the request outright. That
/// is the safer of the two available behaviours: a door that silently
/// defaulted an unrecognised origin to `machine` would record a caller's
/// intent as its opposite whenever a future client and an older server
/// disagreed about the vocabulary, and would do it invisibly. Refusing says
/// so while the caller is still there to hear it.
#[test]
fn the_origin_a_request_states_is_the_origin_recorded() {
    const BY_A_PERSON: &str = "typed-by-a-person";
    const BY_THE_AGENT: &str = "sent-by-the-agent";

    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let worker = server.spawn_worker();
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &worker).is_some()
    });

    // Same length, so `bytes` cannot be what distinguishes the rows.
    assert_eq!(BY_A_PERSON.len(), BY_THE_AGENT.len());

    for (text, origin) in [(BY_A_PERSON, "user"), (BY_THE_AGENT, "machine")] {
        let sent = server.call(serde_json::json!({
            "op": "send_message",
            "session": worker,
            "text": text,
            "origin": origin,
        }));
        assert_eq!(sent["status"], "ok", "{sent}");
        // The delivery has to have happened for its record to mean anything.
        wait_for("the worker to read the delivered line", || {
            fixture
                .received(&root, &worker)
                .is_some_and(|read| read.contains(text))
        });
    }

    let delivered: Vec<String> = server
        .events_for(&worker)
        .iter()
        .filter(|event| event["kind"] == "text_delivered")
        .filter_map(|event| event["origin"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        delivered,
        vec!["user_keystroke".to_owned(), "machine".to_owned()],
        "the door must record the origin each request stated, in the order \
         the requests were made — both lines went through the same verb into \
         the same session and differ only in that field: {:?}",
        server.events_for(&worker)
    );

    // A word the vocabulary does not contain is refused, and nothing is
    // written for it. Asserted here rather than at the end, because the
    // interrupt below ends this harness's read loop and the `process_exited`
    // row that follows would land asynchronously between the two counts.
    let before = server.events_for(&worker).len();
    let refused = server.call(serde_json::json!({
        "op": "send_message",
        "session": worker,
        "text": "never-delivered",
        "origin": "orchestrator",
    }));
    assert_eq!(
        refused["status"], "error",
        "an origin this protocol does not know must be refused rather than \
         quietly defaulted to its opposite: {refused}"
    );
    assert_eq!(
        server.events_for(&worker).len(),
        before,
        "a refused request must write no row at all"
    );

    // The interrupt half, last, because a real 0x03 ends this harness's read
    // loop.
    let interrupted = server.call(serde_json::json!({
        "op": "interrupt",
        "session": worker,
        "origin": "user",
    }));
    assert_eq!(interrupted["status"], "ok", "{interrupted}");

    let interrupts: Vec<String> = server
        .events_for(&worker)
        .iter()
        .filter(|event| event["kind"] == "interrupt_delivered")
        .filter_map(|event| event["origin"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        interrupts,
        vec!["user_keystroke".to_owned()],
        "an interrupt carries the same attribution a line of text does: {:?}",
        server.events_for(&worker)
    );
}

/// Glasshouse's own words are never recorded as the person's, even when the
/// person is the one who made the request that carried them.
///
/// `Request::SendMessage` selects project memory for the text it is about to
/// deliver and sends it as its own separate line before the caller's — the
/// door's `deliver_memory`. That line rides in on a request that may now say
/// `"origin": "user"`, and it is **not** the user's: they did not write it,
/// have not seen it, and it was chosen from the project's store by Glasshouse
/// on their behalf. Stamping it with the requester's origin would record
/// Glasshouse's own text as a person's intervention, which is the exact
/// confusion the origin field exists to end.
///
/// # Why this test exists rather than a comment saying so
///
/// The mutation that stamps the briefing as the person **survived** every
/// test in `context_injection.rs`, which is the file that otherwise owns this
/// path: those tests assert what arrives on the harness's terminal, and the
/// origin is not on the terminal at all — it is only in the log. So the
/// ruling was unwatched, and a later change that threaded the request's
/// origin through every write on this path would have looked correct and
/// broken nothing.
///
/// Both rows come from **one** request, so this cannot pass by the two lines
/// being sent differently: the door split them, and the door chose a
/// different origin for each half.
#[test]
fn glasshouses_own_memory_block_is_never_recorded_as_the_persons() {
    const TASK: &str = "kestrel export";

    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");

    // Seeded before the door opens, so the selection has something to find.
    ProjectMemory::open(&fixture.runtime(&root))
        .expect("open this project's memory")
        .store()
        .record(
            NewMemory::new(
                MemoryKind::Constraint,
                "The kestrel export must never write partial files.",
            )
            .with_subject(Some(TASK))
            .with_authority(Some(MemoryAuthority::Constraint)),
        )
        .expect("record a memory for the door to select");

    let server = Server::start(&fixture, &root);
    let worker = server.spawn_worker();
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &worker).is_some()
    });

    // A person's request, stated as one.
    let sent = server.call(serde_json::json!({
        "op": "send_message",
        "session": worker,
        "text": TASK,
        "origin": "user",
    }));
    assert_eq!(sent["status"], "ok", "{sent}");
    wait_for("the worker to read the person's own line", || {
        fixture
            .received(&root, &worker)
            .is_some_and(|read| read.contains(TASK))
    });

    let delivered: Vec<(String, u64)> = server
        .events_for(&worker)
        .iter()
        .filter(|event| event["kind"] == "text_delivered")
        .map(|event| {
            (
                event["origin"].as_str().unwrap_or("<none>").to_owned(),
                event["bytes"].as_u64().expect("a byte count"),
            )
        })
        .collect();

    assert_eq!(
        delivered.len(),
        2,
        "one request must have produced two deliveries — the memory block and \
         the person's own line — or this test is not looking at the split it \
         is about: {:?}",
        server.events_for(&worker)
    );
    assert_eq!(
        delivered[0].0, "machine",
        "the memory block is Glasshouse speaking: it was selected from this \
         project's store, not typed by the person whose request carried it, \
         and recording it as theirs would put words in their mouth: {delivered:?}"
    );
    assert_eq!(
        delivered[1].0, "user_keystroke",
        "the person's own line, from the same request, is theirs: {delivered:?}"
    );

    // The two really are the two halves this test names, not one line
    // recorded twice: the person's is their bytes plus the carriage return,
    // and the block is very much larger.
    assert_eq!(
        delivered[1].1,
        TASK.len() as u64 + 1,
        "the second delivery must be the caller's own bytes: {delivered:?}"
    );
    assert!(
        delivered[0].1 > delivered[1].1,
        "the first delivery must be the labelled memory block, which is \
         larger than the task that selected it: {delivered:?}"
    );
}

/// The read half, on its own, with no door-written row anywhere near it.
///
/// One `text_delivered` is appended to the log with **no observation** — the
/// shape every event this process mints has, and the shape
/// `EventLog::observed_since` dropped. Nothing is spawned and nothing is sent,
/// so the write half cannot be what makes this pass.
///
/// This is the assertion the discovery package's SURVIVED mutation was
/// missing. Restoring `observed_since` in `project_events` fails here and
/// nowhere else in this file that a rows-landed-in-SQLite probe would notice.
#[test]
fn an_event_no_harness_reported_is_still_this_projects_history() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let session = SessionId::new("seeded-session".to_owned());

    {
        let runtime = fixture.runtime(&root);
        let log = EventLog::open(&runtime).expect("open the event log");
        let bus = EventBus::new();
        let recorded = bus.publish(
            &session,
            LifecycleEvent::TextDelivered {
                origin: MessageOrigin::Machine,
                bytes: 7,
            },
        );
        // `None`: nothing observed this, because no harness reports a line
        // being typed into it. That is the whole class of event that used to
        // be unreadable through this door.
        log.append(&recorded, None).expect("append a fixture event");
    }

    let server = Server::start(&fixture, &root);
    let history = server.events_for(session.as_str());

    assert_eq!(
        kinds(&history),
        vec!["text_delivered"],
        "an event with no harness observation is still one of this project's \
         lifecycle events, and the reader on the far end of this socket has \
         no other way to have seen it: {history:?}"
    );
    assert!(
        history[0]["harness"].is_null(),
        "and it comes back saying honestly that no harness reported it: {history:?}"
    );
}

/// The content boundary, against the door's own new write path.
///
/// `tests/session_hook.rs` holds this for the hook path — a payload is drained
/// into `io::sink()` unread — and recording the door's own deliveries is the
/// first thing that writes to the log from inside a process that has the text
/// in its hands. `LifecycleEvent::TextDelivered` carries a **length**, and
/// this proves the type is the reason rather than the author's care: the
/// canary is nowhere in the response and nowhere in the database file.
///
/// The database is read as raw bytes rather than queried, so a leak into a
/// column no query here names would still fail this (§17: the viewport is as
/// wide as the file). The positive half is asserted first, so an empty log or
/// an undelivered line cannot make the absence trivially true.
#[test]
fn nothing_a_worker_was_sent_reaches_the_log() {
    const CANARY: &str = "CANARY-9c1d40-secret-payload";

    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let database = fixture.runtime(&root).database_path();
    let server = Server::start(&fixture, &root);

    let worker = server.spawn_worker();
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &worker).is_some()
    });

    let sent = server.call(serde_json::json!({
        "op": "send_message", "session": worker, "text": CANARY,
    }));
    assert_eq!(sent["status"], "ok", "{sent}");
    wait_for("the worker to actually read the canary", || {
        fixture
            .received(&root, &worker)
            .is_some_and(|text| text.contains(CANARY))
    });

    let history = server.events_for(&worker);
    let response = serde_json::to_string(&history).expect("encode the history");
    assert!(
        history
            .iter()
            .any(|event| event["kind"] == "text_delivered"),
        "the delivery must be recorded before its contents' absence means \
         anything: {history:?}"
    );
    assert!(
        !response.contains(CANARY),
        "a worker's own words reached the orchestrator's read path: {response}"
    );

    let raw = std::fs::read(&database).expect("read the project database");
    assert!(
        !raw.windows(CANARY.len()).any(|w| w == CANARY.as_bytes()),
        "a worker's own words reached the project database at {}",
        database.display()
    );
}

/// The project boundary, on the write side.
///
/// Two doors, two projects, one worker each. Each project's history holds its
/// own worker and only its own — asserted in both directions, so a door that
/// wrote into a shared log would fail even though each project's own rows
/// were present.
#[test]
fn each_door_records_into_its_own_projects_log_and_no_other() {
    let fixture = Fixture::new();

    let alpha_root = fixture.project_root("alpha");
    let beta_root = fixture.project_root("beta");
    let alpha = Server::start(&fixture, &alpha_root);
    let beta = Server::start(&fixture, &beta_root);

    let alpha_worker = alpha.spawn_worker();
    let beta_worker = beta.spawn_worker();
    wait_for("both workers' harnesses to start", || {
        fixture.argv(&alpha_root, &alpha_worker).is_some()
            && fixture.argv(&beta_root, &beta_worker).is_some()
    });

    let alpha_history = alpha.events();
    let beta_history = beta.events();
    assert!(
        !alpha_history.is_empty() && !beta_history.is_empty(),
        "both doors must have recorded something before the separation means \
         anything: alpha {alpha_history:?}, beta {beta_history:?}"
    );

    for (name, history, mine, theirs) in [
        ("alpha", &alpha_history, &alpha_worker, &beta_worker),
        ("beta", &beta_history, &beta_worker, &alpha_worker),
    ] {
        assert!(
            history
                .iter()
                .all(|event| event["session"] == mine.as_str()),
            "{name}'s history holds a session that is not its own: {history:?}"
        );
        assert!(
            !history
                .iter()
                .any(|event| event["session"] == theirs.as_str()),
            "{name}'s history holds the other project's worker: {history:?}"
        );
    }
}

/// Bookkeeping never costs the door a request.
///
/// The project's database is made unopenable *after* the door is running —
/// `database::configure` refuses a read-only file by name — so the recorder's
/// first attempt to open the log fails. The door must go on spawning and
/// delivering: a project whose database cannot be opened loses event history
/// and keeps its sessions, which is the direction `shell::attach_event_log`
/// already trades in and the direction this inherits.
///
/// It is deliberately the *recorder's* open that is broken and not the
/// process's first one. A door whose database was unopenable at startup never
/// starts at all — `serve` opens `ProjectSessions` before it binds — so that
/// state is not constructible through the shipped binary, and claiming it as
/// tested would be a fiction.
#[test]
fn a_log_that_cannot_be_opened_does_not_stop_the_door() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let database = fixture.runtime(&root).database_path();
    let server = Server::start(&fixture, &root);
    // One answered request first. The announcement this fixture waits for now
    // comes after the door has opened its database, but asserting that from
    // the outside is what makes this test independent of the ordering rather
    // than dependent on it: nothing below is about a door that is still
    // starting.
    assert_eq!(
        server.call(serde_json::json!({ "op": "list_sessions" }))["status"],
        "ok",
        "the door must be serving before its database is taken away"
    );

    let mut perms = std::fs::metadata(&database)
        .expect("stat the project database")
        .permissions();
    perms.set_mode(0o444);
    std::fs::set_permissions(&database, perms).expect("make the database read-only");

    // A process that can write it anyway — root, or a filesystem ignoring the
    // mode — would make everything below vacuous, so say so rather than pass.
    if std::fs::OpenOptions::new()
        .write(true)
        .open(&database)
        .is_ok()
    {
        eprintln!(
            "skipping: this process can write {} despite mode 0444, so the \
             failure this test needs cannot be constructed here",
            database.display()
        );
        return;
    }

    let worker = server.spawn_worker();
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &worker).is_some()
    });

    let sent = server.call(serde_json::json!({
        "op": "send_message", "session": worker, "text": "still-serving",
    }));
    assert_eq!(
        sent["status"], "ok",
        "an unrecordable event must not fail the request that produced it: {sent}"
    );
    wait_for("the worker to read the line anyway", || {
        fixture
            .received(&root, &worker)
            .is_some_and(|text| text.contains("still-serving"))
    });

    // And the door still answers every other question it is asked.
    let listed = server.call(serde_json::json!({ "op": "list_sessions" }));
    assert_eq!(listed["status"], "ok", "{listed}");
}
