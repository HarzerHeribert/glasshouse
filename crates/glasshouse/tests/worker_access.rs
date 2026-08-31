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
//! - **A person can now see what came back, which is the rest of line 745.**
//!   This paragraph used to say the opposite — that no request on this wire
//!   returned a worker's terminal output, so a client built from the existing
//!   verbs could type into a worker blind. `glasshouse api read` is the
//!   missing verb, answered by `Request::RecentOutput` through
//!   `session::api::SessionApi::recent_output`, which had lived in this
//!   repository with no production caller outside its own tests. The third
//!   section of this file tests it the same way the other two test theirs: by
//!   running the shipped binary. What is still *not* here is a transparent
//!   full-terminal attach — see `session::attach`'s own doc comment.
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
use std::os::unix::process::CommandExt;
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
        Self::rooted_at(tmp, base, install_session_tagging_harness)
    }

    /// A fixture whose `claude-code` is [`install_quiet_harness`] instead —
    /// a harness that prints **nothing** until it is asked to.
    ///
    /// The two reading tests that need it need it for opposite reasons and
    /// neither can use the tagging harness: that one prints `READY` the
    /// instant it starts, so "a live worker that has printed nothing yet"
    /// has no way to exist under it, and it echoes one line per line sent,
    /// so filling a scrollback past the door's ceiling through it would mean
    /// hundreds of round trips.
    ///
    /// Registered under the same `claude-code` id rather than as a second
    /// integration, so everything else about the fixture — [`Server::spawn_worker`],
    /// the `--settings` session tag, [`Fixture::argv`] — works unchanged and
    /// no other test in this file is affected by its existence.
    fn with_a_quiet_harness() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        Self::rooted_at(tmp, base, install_quiet_harness)
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
        Self::rooted_at(tmp, base, install_session_tagging_harness)
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
        Self::rooted_at(tmp, base, install_session_tagging_harness)
    }

    fn rooted_at(tmp: tempfile::TempDir, base: PathBuf, install: fn(&Path) -> PathBuf) -> Self {
        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let harness = install(&bin_dir);

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        let escaped = harness.display().to_string().replace('\\', "\\\\");
        // `implementation_policy = false`: this file is about an orchestrator's
        // access to a worker's terminal, and Glasshouse's implementation
        // policy (`src/policy`) is several machine-origin deliveries into
        // every session it briefs — which would shift every delivery count and
        // every scrollback these tests read, without saying anything about
        // worker access. The policy's own delivery, including its default of
        // on, is proven in `tests/implementation_policy.rs`.
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

/// The script [`install_session_tagging_harness`] writes, as a constant so
/// [`the_tagging_harness_survives_an_interrupt_under_every_posix_shell_here`]
/// runs the very same bytes rather than a copy that could drift away from it.
const SESSION_TAGGING_HARNESS: &str = "#!/bin/sh\n\
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
     done\n";

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
/// Trapping also keeps the harness *alive* through the interrupt, which is
/// what lets [`an_interrupt_sent_by_the_client_makes_the_worker_react`] go on
/// to prove the session still takes input afterwards. An interrupt that killed
/// the worker would satisfy a weaker test and would not be an interrupt.
///
/// # Why the read loop is written the long way
///
/// A trap alone is *not* enough, and believing it was is what turned this
/// fixture into a kill test on Linux while it stayed an interrupt test on
/// macOS. The shells disagree about what an interrupted `read` builtin
/// returns, and `while IFS= read -r line` ends on any non-zero:
///
/// | shell | after a trapped `SIGINT` during `read` |
/// |---|---|
/// | `bash` — macOS `/bin/sh` | restarts the read; returns `0` with the real line |
/// | `dash` — Debian/Ubuntu `/bin/sh` | returns `1` with an empty line |
/// | `ksh` | returns `258` with an empty line |
///
/// Measured on both platforms rather than reasoned: on macOS 26 against
/// `/bin/sh`, `/bin/dash`, `/bin/ksh`, `/bin/bash` and `/bin/zsh`, and inside
/// the gate's own `rust:1.98.0` container — where `/bin/sh` is
/// `/usr/bin/dash` — against `/bin/sh` and `/bin/bash`. Under the old
/// one-line loop the trap fired everywhere and the *loop* then ended
/// everywhere except `bash`, so the harness exited and the next
/// `glasshouse api send` was answered *"session `…` has already exited"*.
/// That is what `test (ubuntu)` reported on `655bbc0`.
///
/// So the loop distinguishes the two reasons `read` can fail. The trap sets a
/// flag, a failed read with the flag set is an interrupt and is resumed, and
/// a failed read without it is the real end of input and breaks. The flag is
/// cleared on the way through, so a later end-of-input is still an
/// end-of-input rather than an endless retry.
///
/// [`the_tagging_harness_survives_an_interrupt_under_every_posix_shell_here`]
/// runs these exact bytes under every shell the machine has, so the next
/// person to shorten this loop finds out on macOS instead of in the gate.
fn install_session_tagging_harness(bin_dir: &Path) -> PathBuf {
    let path = bin_dir.join("session-tagging-harness");
    std::fs::write(&path, SESSION_TAGGING_HARNESS).expect("write the session-tagging harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// How many lines [`install_quiet_harness`]'s burst prints, and why the number
/// is what it is.
///
/// The burst exists to put **more than the door's own ceiling** into a
/// session's scrollback, so that a read asking for more than the ceiling has
/// something to be cut off. Two thousand lines of roughly seventy-six bytes
/// is about 150 KB: comfortably over `unix::MAX_RECENT_OUTPUT_BYTES` (64 KiB,
/// restated as [`MAX_RECENT_OUTPUT_BYTES`] below) and comfortably under
/// `session::runtime::DEFAULT_SCROLLBACK_BYTES` (256 KiB), so the scrollback
/// holds all of it and the only thing doing any cutting is the door.
const BURST_LINES: usize = 2000;

/// The last thing [`install_quiet_harness`]'s burst prints.
///
/// A causal signal that the whole burst has been written and drained into the
/// scrollback, rather than a sleep: it is the final line, so a tail of the
/// scrollback containing it is a scrollback containing everything before it.
const BURST_SENTINEL: &str = "burst-complete";

/// A harness that says **nothing at all** until it is asked to, and then says
/// a great deal.
///
/// Same session tagging as [`install_session_tagging_harness`] — the
/// `--settings` argument the lifecycle-hook installation adds — so
/// [`Fixture::argv`] is still the causal "the harness is running" signal, and
/// it is still written to a *file* rather than printed, which is what keeps
/// the session's terminal genuinely empty.
///
/// # Why silence has to be a fixture and cannot be a moment
///
/// [`install_session_tagging_harness`] prints `READY` as its first act, so
/// under it "a live session that has printed nothing yet" only exists in the
/// milliseconds before the harness runs — a state a test could only reach by
/// racing, which would pass or fail on machine speed rather than on
/// behaviour. This harness makes that state *stable*: it is live, it is
/// reading its terminal, and it has printed nothing, for as long as nobody
/// speaks to it.
///
/// # The burst
///
/// A line beginning `burst` makes it print [`BURST_LINES`] lines and then
/// [`BURST_SENTINEL`]. The trigger is a **prefix** match on purpose: a line
/// arriving through a canonical-mode terminal may or may not still carry a
/// carriage return depending on the line discipline's translation, and a
/// harness that had to parse the line exactly would be testing that rather
/// than the door.
///
/// There is no `INT` trap here, unlike the tagging harness, and that is also
/// used: an untrapped `sh` dies on `SIGINT`, which is how
/// [`a_live_worker_with_nothing_to_say_and_a_session_with_no_process_are_different_answers`]
/// gets a session whose process is gone without killing the door that owns
/// the store.
fn install_quiet_harness(bin_dir: &Path) -> PathBuf {
    let path = bin_dir.join("quiet-harness");
    let filler = "0123456789012345678901234567890123456789012345678901234567890123";
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n\
             tag=unknown\n\
             prev=\"\"\n\
             for a in \"$@\"; do\n\
             if [ \"$prev\" = \"--settings\" ]; then tag=$(basename \"$(dirname \"$a\")\"); fi\n\
             prev=\"$a\"\n\
             done\n\
             echo \"$@\" > \"$PWD/argv-$tag.log\"\n\
             while IFS= read -r line; do\n\
             printf '%s\\n' \"$line\" >> \"$PWD/received-$tag.log\"\n\
             case \"$line\" in\n\
             burst*)\n\
             i=0\n\
             while [ \"$i\" -lt {BURST_LINES} ]; do\n\
             echo \"burst $i {filler}\"\n\
             i=$((i + 1))\n\
             done\n\
             echo \"{BURST_SENTINEL}\"\n\
             ;;\n\
             esac\n\
             done\n"
        ),
    )
    .expect("write the quiet harness");
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
        self.spawn_as("worker")
    }

    /// Spawn a session under an arbitrary role.
    ///
    /// The fourth section of this file needs an *orchestrator* session as
    /// well as a worker, because `Request::WatchWorker` refuses a session
    /// watching itself and the completion has to be delivered somewhere.
    /// The role is a label on the record; both sessions are the same kind of
    /// live pseudo-terminal, which is itself part of what that section
    /// measures.
    fn spawn_as(&self, role: &str) -> String {
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

    /// The highest log position this door has issued so far.
    ///
    /// Used as a causal fence rather than a clock: a row with a greater
    /// `seq` was written after everything counted here, whatever the wall
    /// clock says.
    fn max_seq(&self) -> i64 {
        self.events()
            .iter()
            .filter_map(|event| event["seq"].as_i64())
            .max()
            .unwrap_or(0)
    }

    /// Whether the log holds any `text_delivered` row at all.
    ///
    /// A causal "the completion handoff has happened" signal that does **not**
    /// depend on *which* session received it. §80 case 5: a fixture must be
    /// able to observe a failure independently of the thing being mutated, or
    /// a mutation that redirects the delivery kills the test through the
    /// fixture's own timeout and the test's real assertions never run. That
    /// happened once here, with `hand-the-result-back-to-the-worker`.
    ///
    /// Sound because the handoff is the **first** write into any session in
    /// the two tests that use this: nothing is sent to anyone before it.
    fn a_delivery_was_recorded(&self) -> bool {
        self.events()
            .iter()
            .any(|event| event["kind"] == "text_delivered")
    }

    /// Every `text_delivered` row the log holds for one session, in order.
    fn deliveries_to(&self, session: &str) -> Vec<serde_json::Value> {
        self.events()
            .into_iter()
            .filter(|event| event["session"] == session && event["kind"] == "text_delivered")
            .collect()
    }

    /// Every `interrupt_delivered` row the log holds for one session, in
    /// order.
    ///
    /// Separate from [`Server::deliveries_to`] rather than a parameter of it,
    /// because the two answer different questions and the tests that use this
    /// one are asserting that an interrupt carries the same attribution a
    /// line of text does — an interrupt is an intervention too.
    fn interrupts_to(&self, session: &str) -> Vec<serde_json::Value> {
        self.events()
            .into_iter()
            .filter(|event| event["session"] == session && event["kind"] == "interrupt_delivered")
            .collect()
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
/// any way to be that caller. This request names no origin at all, so it is
/// recorded as the machine it is; whether a person's own delivery is
/// distinguishable from an orchestrator's is
/// [`a_persons_intervention_and_the_orchestrators_own_are_different_rows`],
/// and it is a fact about `Request::SendMessage`'s `origin` field rather
/// than about this transport.
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

/// The shells [`the_tagging_harness_survives_an_interrupt_under_every_posix_shell_here`]
/// runs the harness under, when the machine has them.
///
/// Every entry is a shell that is, or can be, some platform's `/bin/sh`:
/// `bash` is macOS's, `dash` is Debian's and Ubuntu's — and therefore the
/// gate's `rust:1.98.0` container's, where `/bin/sh` is `/usr/bin/dash`.
/// `zsh` is here because macOS ships it and it costs a second.
///
/// **`/bin/ksh` is deliberately absent, and it is the shell that would
/// otherwise look like the best argument for including it** — it fails the
/// old loop exactly as `dash` does. Measured on macOS 26: parked in `read` on
/// a *pipe*, `ksh` does not run an `INT` trap at all, so the wait on the trap
/// file below — the thing that proves a real signal arrived rather than a
/// timing coincidence — never completes and the test fails for a reason that
/// is about `ksh`'s signal timing rather than about the harness. On a *pty*
/// `ksh` traps normally, and with the loop as it now stands it survives the
/// interrupt on both. It is not any platform's `/bin/sh`, so nothing here
/// turns on it.
const CANDIDATE_SHELLS: [&str; 4] = ["/bin/sh", "/bin/dash", "/bin/bash", "/bin/zsh"];

/// **The reproduction (§59), and it is a fixture that is under test.**
///
/// [`an_interrupt_sent_by_the_client_makes_the_worker_react`] failed on
/// `test (ubuntu)` for `655bbc0` — *"the session must still be usable after an
/// interrupt: glasshouse: session `d6a29df1…` has already exited"* — while
/// passing on macOS, and Glasshouse was not the thing that differed. The
/// interrupt path is one `write` of `0x03` onto the session's own terminal and
/// it behaved identically on both platforms: the gate got **past** the wait on
/// the worker's `SIGINT` trap file, so a real signal had been raised in the
/// worker's own process on Linux too. What differed was
/// [`install_session_tagging_harness`]'s read loop — see its doc comment for
/// the measured table.
///
/// The state under test is therefore *"a POSIX shell that is not `bash`"*,
/// not *"Linux"*, and §59 says to reproduce the state rather than the event.
/// This runs the harness's own bytes under **every** shell this machine has,
/// which is §25's repair applied to a shell rather than to a socket: assert
/// the property, and exercise every variant that can produce it, so the
/// platform that has `dash` as `/bin/sh` is not the only one that finds out.
///
/// Under the loop this replaces, it fails on macOS at `/bin/dash` and
/// `/bin/ksh`, and on Linux at `/bin/sh`.
///
/// The signal is raised with `kill -INT` rather than through a pty, on
/// purpose: the line discipline's part is already proven by the test above and
/// by `pty_smoke::interrupt_is_delivered_as_a_terminal_interrupt`, and what is
/// in question here is only what the shell does once the signal has arrived.
/// Sending it directly is also what makes this cheap enough to run per shell.
/// See [`CANDIDATE_SHELLS`] for the one shell that choice excludes.
#[test]
fn the_tagging_harness_survives_an_interrupt_under_every_posix_shell_here() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create bin dir");
    let harness = install_session_tagging_harness(&bin_dir);

    let mut covered = Vec::new();
    for shell in CANDIDATE_SHELLS {
        if !Path::new(shell).is_file() {
            continue;
        }
        covered.push(shell);
        an_interrupt_leaves_this_shell_reading(shell, &harness, tmp.path());
    }

    // Never silently vacuous: a machine with none of these would otherwise
    // report a pass having run nothing at all.
    assert!(
        !covered.is_empty(),
        "none of the candidate shells exist here, so this test proved nothing"
    );
}

/// One shell, one interrupt: the trap runs, the process lives, and the next
/// line still arrives.
///
/// Its own working directory, because the harness names its log files after a
/// tag it takes from `--settings` and there is no such argument here — every
/// shell would otherwise write to the same `*-unknown.log`.
fn an_interrupt_leaves_this_shell_reading(shell: &str, harness: &Path, base: &Path) {
    let work = base.join(shell.replace('/', "-"));
    std::fs::create_dir_all(&work).expect("create the shell's working directory");
    let printed = work.join("stdout.log");
    let stdout = std::fs::File::create(&printed).expect("create the stdout log");

    // `SIGINT` back to its default disposition in the child, and this line is
    // the whole of what GH-INTERRUPT-TEST-FLAKE was chasing.
    //
    // **A signal that is ignored on entry to a non-interactive shell cannot be
    // trapped by it.** POSIX requires that, and `bash`, `dash` and every other
    // conforming shell obey it. `SIG_IGN` survives `execve`, and Rust's
    // `Command` resets the child's signal *mask* but not its *dispositions*,
    // so the shell under test starts with whatever this test binary inherited
    // from whoever launched `cargo`. Where that is `SIG_IGN`, the harness's
    // `trap ... INT` is a no-op and the `kill -INT` below is discarded by the
    // kernel — after which this function waits 30s for a trap that was never
    // installed, on a shell that is perfectly healthy.
    //
    // That is not a race and not load, and it is not even slightly marginal.
    // Measured 2026-08-30 on macOS 26 by spawning this test binary with
    // `SIGINT` set explicitly to each disposition and changing nothing else:
    //
    // | disposition in the child | without this line | with it |
    // |---|---|---|
    // | `SIG_DFL` | 20/20 pass, slowest 0.25s | 20/20 pass, slowest 0.49s |
    // | `SIG_IGN` | **0/20 pass**, every one 30.09s | 20/20 pass, slowest 0.30s |
    //
    // and every failure in that `0/20` cell reads `exited=None` with
    // `printed so far: "READY\n"` — the gate's message byte for byte. A shell
    // without job control hands a background child exactly that disposition,
    // so `sh -c 'cargo test ... & wait'` reproduces the gate's failure in 30
    // seconds and is how this was found.
    //
    // `/bin/sh`, `/bin/dash` and `/bin/bash` all fail that way; `/bin/zsh`
    // installs the trap regardless, so reordering [`CANDIDATE_SHELLS`] moves
    // the failure to `/bin/dash` rather than leaving it at the front. It
    // follows the shell, not the position — `/bin/sh` was only ever named
    // because it is the first of the three that cannot.
    //
    // It is also why the Ubuntu leg is green while the macOS one is not, and
    // the reason is not the operating system: `scripts/ci-local.sh` runs the
    // Linux jobs under `docker run`, whose process the daemon spawns rather
    // than the CLI, so it starts from default dispositions no matter how the
    // gate was launched. The macOS jobs run natively and inherit. Only the
    // native leg inherits anything, which is the whole of the asymmetry.
    //
    // So this is not a workaround for a flake. It is the precondition the
    // experiment always needed and never stated — what is under test here is
    // what a shell does *once a `SIGINT` has arrived* — and it makes the test
    // independent of how anyone launched `cargo`. It changes no timing on any
    // platform and is a no-op wherever the disposition was already default,
    // which is every foreground run including today's green Ubuntu leg.
    //
    // Only this test needs it: under the reproducing launcher the other
    // eighteen in this file pass,
    // [`an_interrupt_sent_by_the_client_makes_the_worker_react`] included, so
    // the product's own interrupt path is not exposed to the inherited
    // disposition the way this raw `Command::spawn` is.
    let mut command = Command::new(shell);
    command
        .arg(harness)
        .current_dir(&work)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::null());
    // SAFETY: `pre_exec` requires the closure to be async-signal-safe, because
    // it runs between `fork` and `execve` in a child that may hold locks the
    // parent's other threads own. This one is a single `signal(2)` call: no
    // allocation, no locks, no library state.
    unsafe {
        command.pre_exec(|| {
            if libc::signal(libc::SIGINT, libc::SIG_DFL) == libc::SIG_ERR {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .unwrap_or_else(|err| panic!("could not start `{shell}`: {err}"));
    let said = || std::fs::read_to_string(&printed).unwrap_or_default();

    // The trap has to be installed before the signal arrives or this would
    // prove the opposite of what it claims; the harness prints `READY` on the
    // line after it traps.
    wait_for(&format!("`{shell}` to trap and print READY"), || {
        said().contains("READY")
    });

    let signalled = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("run kill");
    // Worth knowing what this does and does not prove: `kill(2)` succeeds on a
    // signal the target ignores, so a green line here says the pid existed and
    // says nothing about delivery. The `pre_exec` above is what makes delivery
    // follow from it.
    assert!(signalled.success(), "`kill -INT` failed for `{shell}`");

    // Not `wait_for`: a bare "timed out waiting for `{shell}` to run its INT
    // trap" is exactly the message this test produced under macOS gate load
    // on 2026-08-30, and it does not say whether the trap is late or the
    // signal never arrived at all — the two defects GH-INTERRUPT-TEST-FLAKE
    // was opened to tell apart. It kept the deadline and reported instead
    // whether the shell had already exited and what it had printed, and that
    // is what closed the investigation: `exited=None` with `"READY\n"` ruled
    // out both a kill and a shell that had not reached its read loop, which
    // left "the signal was never delivered" as the only reading and led to the
    // ignored disposition the spawn above now resets. Kept because the same
    // three-way answer is what any future occurrence will need.
    let trap_deadline = Instant::now() + TIMEOUT;
    loop {
        if work.join("interrupted-unknown.log").is_file() {
            break;
        }
        if Instant::now() >= trap_deadline {
            let exited = child.try_wait().expect("try_wait");
            panic!(
                "timed out waiting for `{shell}` to run its INT trap after \
                 {TIMEOUT:?}: exited={exited:?} (Some(_) here means the \
                 signal killed the shell rather than being trapped; None \
                 means it is still running and the trap is merely late), \
                 printed so far: {:?}",
                said()
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    // Deliberately not asserted: a write to a pipe whose reader has just gone
    // may still succeed, so its result says nothing either way. What the line
    // *comes back* as is the evidence.
    let _ = child
        .stdin
        .as_mut()
        .expect("the harness's stdin")
        .write_all(b"still-listening-after-the-interrupt\n");

    let deadline = Instant::now() + TIMEOUT;
    let mut echoed = said().contains("got:still-listening-after-the-interrupt");
    while !echoed && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        echoed = said().contains("got:still-listening-after-the-interrupt");
    }
    let exited = child.try_wait().expect("try_wait");
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        echoed,
        "`{shell}` must still be reading its terminal after a trapped SIGINT, \
         and this is the difference the Linux gate found: `bash` restarts an \
         interrupted `read` builtin, `dash` and `ksh` return failure from it \
         and end a `while read` loop, which ends the harness. \
         exited={exited:?}, printed so far: {:?}",
        said()
    );
    assert!(
        exited.is_none(),
        "`{shell}` must not have ended: an interrupt that kills the worker is \
         not an interrupt ({exited:?})"
    );
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

// ---------------------------------------------------------------------------
// The reading half — capability map line 745.
//
// `send` and `interrupt` are a person writing into a running worker. This is
// the half that lets them see it, and until it existed line 745 was recorded
// as blocked on a decision about which process owns a worker's pseudo-terminal.
// It was not: `SessionApi::recent_output` reads a live session's scrollback
// inside the process that already owns the pty, project-scoped through the
// same seam the other two verbs resolve through, and had no production caller
// at all. These five tests are that caller's proof, and every one of them runs
// the shipped binary.
// ---------------------------------------------------------------------------

/// `unix::MAX_RECENT_OUTPUT_BYTES`, restated here because `api` is declared
/// from `main.rs` and no integration test can import a constant out of the
/// binary — the same reason `routing_api.rs` restates
/// `MAX_ROUTE_ALTERNATIVES`.
///
/// Asserted as an exact number rather than as "not too much" on purpose: a
/// bound a test only checks loosely is a bound a mutation can raise without
/// anything noticing, which is exactly what
/// [`an_absurd_byte_bound_still_comes_back_bounded`] exists to catch.
const MAX_RECENT_OUTPUT_BYTES: usize = 64 * 1024;

/// Acceptance test 1 — what a real harness actually printed comes back to a
/// person, through the shipped client, across a process boundary.
///
/// The output asserted on is not Glasshouse's: `READY` and `got:…` are the
/// fixture harness's own words, written to its own terminal by a process the
/// reading client never touches. Between them sits everything line 745 is
/// about — a pty owned by `glasshouse api serve`, a socket, and a separate
/// `glasshouse api read` process started from a terminal with no agent
/// running.
///
/// The second half is the stream discipline the verb promises: the worker's
/// bytes go to standard output and **nothing else does**, so a read can be
/// piped into a file or a pager without Glasshouse's own voice in it.
#[test]
fn output_a_real_harness_printed_comes_back_through_the_client() {
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
            "say-this-back-to-me",
        ],
    );
    assert!(sent.status.success(), "{}", stderr_of(&sent));
    // The worker's own account that it read the line, so what the read below
    // looks for is known to have been printed by then.
    wait_for("the worker to read the line", || {
        fixture
            .received(&root, &worker)
            .is_some_and(|text| text.contains("say-this-back-to-me"))
    });

    let read = fixture.client(&root, &["api", "read", "--session", &worker]);
    assert!(
        read.status.success(),
        "`glasshouse api read` failed: {}",
        stderr_of(&read)
    );

    let shown = stdout_of(&read);
    assert!(
        shown.contains("got:say-this-back-to-me"),
        "the worker's answer to the delivered line must come back to the reader: {shown:?}"
    );
    assert!(
        shown.contains("READY"),
        "the harness's own startup line is output too, and a read that only \
         returned the most recent exchange would be a different verb: {shown:?}"
    );
    assert!(
        stderr_of(&read).is_empty(),
        "a successful read must say nothing of Glasshouse's own, so standard output \
         is exactly the worker's terminal: {:?}",
        stderr_of(&read)
    );
}

/// Acceptance test 2, and ruling 3 — **the one most likely to catch a wrong
/// build.**
///
/// `SessionApi::recent_output` refuses a session with no live process rather
/// than answering `""`, because in its own words *"returning an empty string
/// would be a lie the caller has no way to detect."* That distinction has to
/// survive a wire, a client and a shell, and this asserts it end to end on
/// **one session id** — so the only thing that differs between the two halves
/// is whether a process is running it.
///
/// # How each half is reached, and why neither is a race
///
/// *Live and silent* is a fixture rather than a moment:
/// [`install_quiet_harness`] prints nothing until spoken to, so the state is
/// stable for as long as the test wants it.
///
/// *No live process* is the case `api/mod.rs`'s own doc comment describes —
/// a session the store knows and this door's runtime does not — and it is
/// produced the way a user would produce it: the door that owned the pty is
/// gone, and a new one is opened on the same project. The second door reads
/// the same store, resolves the same session, and holds no process for it.
#[test]
fn a_live_worker_with_nothing_to_say_and_a_session_with_no_process_are_different_answers() {
    let fixture = Fixture::with_a_quiet_harness();
    let root = fixture.project_root("alpha");
    let first = Server::start(&fixture, &root);

    let worker = first.spawn_worker();
    wait_for("the quiet worker's harness to start", || {
        fixture.argv(&root, &worker).is_some()
    });

    // (1) Live, and has printed nothing. Not an error: there is a process,
    // it is reading its terminal, and it has nothing to show yet.
    let quiet = fixture.client(&root, &["api", "read", "--session", &worker]);
    assert!(
        quiet.status.success(),
        "a live worker that has printed nothing is not a failure to read it: {}",
        stderr_of(&quiet)
    );
    assert!(
        stdout_of(&quiet).is_empty(),
        "there was nothing to show, so nothing may be shown: {:?}",
        stdout_of(&quiet)
    );
    let said = stderr_of(&quiet);
    assert!(
        said.contains("printed nothing yet") && said.contains(&worker),
        "silence must be reported as silence, about this session by name, rather than \
         left as an empty screen the user has to interpret: {said:?}"
    );

    // (2) The same session, with nothing running it. Killing the door kills
    // the process that owned the pty; the store outlives both.
    drop(first);
    let _second = Server::start(&fixture, &root);

    let dead = fixture.client(&root, &["api", "read", "--session", &worker]);
    assert!(
        !dead.status.success(),
        "a session no process is running has no output to give, and saying so is the \
         whole point of the verb: {:?}",
        stdout_of(&dead)
    );
    assert!(
        stdout_of(&dead).is_empty(),
        "a refused read must not also print output: {:?}",
        stdout_of(&dead)
    );
    let refusal = stderr_of(&dead);
    assert!(
        refusal.contains("not live") && refusal.contains(&worker),
        "the refusal must say that nothing is running this session, naming it: {refusal:?}"
    );

    // The claim itself, stated once rather than left implicit in the two
    // halves above: these are different answers, and a build that collapsed
    // them would have satisfied neither.
    assert_ne!(
        quiet.status.success(),
        dead.status.success(),
        "`live but quiet` and `no process` must not be the same answer"
    );
    assert!(
        !refusal.contains("printed nothing"),
        "a session with no process must never be described as one that printed \
         nothing: {refusal:?}"
    );
}

/// Acceptance test 3, and the security invariant: **no worker's scrollback
/// crosses a project boundary.**
///
/// A worker's terminal is the most sensitive thing this door returns — it is
/// whatever the harness printed, which is whatever the agent read or echoed —
/// so the refusal is asserted twice over: by status, and by looking for a
/// marker that exists **only** in alpha's scrollback anywhere in what beta's
/// client printed.
///
/// # What the refusal says, and why it is not what the packet expected
///
/// The packet asked that a boundary crossing be *distinguishable from "no
/// such session"*. It is not, for the reason
/// [`the_client_cannot_reach_another_projects_worker`] records for the write
/// verbs and which is unchanged here: every project has its own database, so
/// alpha's session is not a foreign row in beta's store, it is not in beta's
/// store at all, and `ApiError::ForeignProject` is unreachable between two
/// real projects. Saying more would confirm to the caller that the session
/// exists **somewhere else**, which is a leak in its own right and a worse
/// one for a read verb than for a write one. The scoped sentence is the
/// answer, and it says which project it is about.
///
/// A crafted identifier is asked for too, and gets the same sentence — which
/// is the property worth having: an id a caller invented and an id belonging
/// to a project it cannot see are indistinguishable from outside.
#[test]
fn another_projects_worker_cannot_be_read_and_a_crafted_id_says_the_same_thing() {
    let fixture = Fixture::with_a_short_socket_path();

    let alpha_root = fixture.project_root("alpha");
    let beta_root = fixture.project_root("beta");
    let alpha = Server::start(&fixture, &alpha_root);
    let beta = Server::start(&fixture, &beta_root);

    // §80 case 3: both doors must really be neighbours in one enumerable
    // directory, or this is not the case being tested.
    assert!(
        alpha.bound_in_the_state_dir() && beta.bound_in_the_state_dir(),
        "both doors must bind inside their own state directories: {:?} and {:?}",
        alpha.socket,
        beta.socket
    );

    let victim = alpha.spawn_worker();
    wait_for("alpha's worker to start", || {
        fixture.argv(&alpha_root, &victim).is_some()
    });

    // Something specific to look for. The harness echoes what it reads, so
    // after this the marker is in alpha's scrollback and nowhere else.
    let secret = "alphas-private-scrollback-marker";
    let planted = fixture.client(
        &alpha_root,
        &["api", "send", "--session", &victim, "--text", secret],
    );
    assert!(planted.status.success(), "{}", stderr_of(&planted));
    wait_for("alpha's worker to print the marker", || {
        fixture
            .received(&alpha_root, &victim)
            .is_some_and(|text| text.contains(secret))
    });

    // The viewport (§17): alpha's own client can see the marker, so its
    // absence from beta's answer below means beta was refused rather than
    // that the marker was never there.
    let mine = fixture.client(&alpha_root, &["api", "read", "--session", &victim]);
    assert!(mine.status.success(), "{}", stderr_of(&mine));
    assert!(
        stdout_of(&mine).contains(secret),
        "the marker must be readable inside its own project, or the assertions below \
         prove nothing: {:?}",
        stdout_of(&mine)
    );

    // (1) Beta's client, alpha's session.
    let crossed = fixture.client(&beta_root, &["api", "read", "--session", &victim]);
    assert!(
        !crossed.status.success(),
        "a client scoped to beta must not read alpha's worker: {:?}",
        stdout_of(&crossed)
    );
    let refusal = stderr_of(&crossed);
    assert!(
        refusal.contains("no session") && refusal.contains("in this project"),
        "the refusal must scope itself, so the answer is about this project rather \
         than about the session's existence anywhere: {refusal:?}"
    );

    // (2) A crafted identifier, answered the same way — an invented id and
    // another project's id must not be tellable apart from out here.
    let crafted = fixture.client(
        &beta_root,
        &[
            "api",
            "read",
            "--session",
            "0123456789abcdef0123456789abcdef",
        ],
    );
    assert!(!crafted.status.success(), "there is no such session");
    let invented = stderr_of(&crafted);
    assert!(
        invented.contains("no session") && invented.contains("in this project"),
        "a crafted id must get the scoped sentence too: {invented:?}"
    );

    // (3) The security claim itself, by content rather than by status: not
    // one byte of alpha's terminal appears in anything beta's client printed.
    for (what, output) in [
        ("another project's session", &crossed),
        ("a crafted id", &crafted),
    ] {
        let all = format!("{}{}", stdout_of(output), stderr_of(output));
        assert!(
            !all.contains(secret),
            "reading `{what}` through beta's door leaked alpha's scrollback: {all:?}"
        );
        assert!(
            !all.contains(&fixture.base_path()) && !all.contains(".sock"),
            "a refused read named a filesystem path: {all:?}"
        );
    }

    // (4) Beta's door reads its own worker, so none of the above passed
    // because reading is broken.
    let bystander = beta.spawn_worker();
    wait_for("beta's worker to start", || {
        fixture.argv(&beta_root, &bystander).is_some()
    });
    let ours = fixture.client(&beta_root, &["api", "read", "--session", &bystander]);
    assert!(ours.status.success(), "{}", stderr_of(&ours));
    assert!(
        stdout_of(&ours).contains("READY"),
        "beta must be able to read its own worker: {:?}",
        stdout_of(&ours)
    );
}

/// Acceptance test 4 — a caller may lower the ceiling and cannot raise it.
///
/// A session's scrollback is filled by whatever the harness printed and
/// bounded by the runtime, not by anything either end of this socket chose,
/// so a caller asking for everything must not get everything. The absurd
/// bound here is a hundred million bytes, which is more than the scrollback
/// can ever hold — the answer must still be [`MAX_RECENT_OUTPUT_BYTES`].
///
/// # The excess is proven before its absence is asserted (§17, §80 case 3)
///
/// A bound is only observable when there is more output than the bound. So
/// the worker is made to print about 150 KB first, and the test asserts it
/// really did — by asking for exactly the ceiling and getting exactly the
/// ceiling, which can only happen if the scrollback holds at least that much.
/// Without that half, a build with no ceiling at all would pass this test on
/// any project whose worker happened to be quiet.
///
/// **Neither number is derived from the constant under test** (§80 case 6).
/// The request is a fixed absurd literal and the assertion is a fixed literal
/// restating the door's own constant, so raising the door's ceiling moves the
/// answer and not the assertion.
#[test]
fn an_absurd_byte_bound_still_comes_back_bounded() {
    let fixture = Fixture::with_a_quiet_harness();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let worker = server.spawn_worker();
    wait_for("the quiet worker's harness to start", || {
        fixture.argv(&root, &worker).is_some()
    });

    let asked = server.call(serde_json::json!({
        "op": "send_message",
        "session": worker,
        "text": "burst",
    }));
    assert_eq!(asked["status"], "ok", "{asked}");

    // Causal: the sentinel is the burst's last line, so a tail holding it is
    // a scrollback holding everything before it.
    wait_for("the worker to finish printing its burst", || {
        let response = server.call(serde_json::json!({
            "op": "recent_output",
            "session": worker,
            "max_bytes": 256,
        }));
        response["result"]["output"]
            .as_str()
            .is_some_and(|tail| tail.contains(BURST_SENTINEL))
    });

    // The excess, proven. Asking for exactly the ceiling can only return
    // exactly the ceiling if there is at least that much to return.
    let full = fixture.client(
        &root,
        &["api", "read", "--session", &worker, "--max-bytes", "65536"],
    );
    assert!(full.status.success(), "{}", stderr_of(&full));
    assert_eq!(
        full.stdout.len(),
        MAX_RECENT_OUTPUT_BYTES,
        "the burst must have put more than the ceiling into the scrollback, or the \
         bound below has nothing to bind"
    );

    // The bound itself.
    let absurd = fixture.client(
        &root,
        &[
            "api",
            "read",
            "--session",
            &worker,
            "--max-bytes",
            "100000000",
        ],
    );
    assert!(absurd.status.success(), "{}", stderr_of(&absurd));
    assert!(
        absurd.stdout.len() <= MAX_RECENT_OUTPUT_BYTES,
        "a caller asking for a hundred million bytes received {} of them; the door's \
         ceiling is not being applied",
        absurd.stdout.len()
    );

    // And the other direction, which is the half a ceiling alone would not
    // give: a caller that asks for less gets less.
    let modest = fixture.client(
        &root,
        &["api", "read", "--session", &worker, "--max-bytes", "512"],
    );
    assert!(modest.status.success(), "{}", stderr_of(&modest));
    assert!(
        modest.stdout.len() <= 512,
        "a caller may lower the ceiling: {}",
        modest.stdout.len()
    );
}

/// Acceptance test 5, and ruling 4 — **reading changes nothing.**
///
/// The same negative proof `routing_api.rs::a_recommendation_executes_nothing_and_records_nothing`
/// uses for line 1681, for the same reason: a verb that quietly did something
/// on the way to answering would satisfy every other test in this section.
/// Three observations taken across one read — the session list, the whole
/// event log, and the worker's own record of what reached its terminal — plus
/// a second read, because a read that consumed what it returned would be a
/// different verb wearing this one's name.
#[test]
fn reading_a_worker_changes_nothing_about_it() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let worker = server.spawn_worker();
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &worker).is_some()
    });

    // Something in the log before the observation, so "unchanged" is a claim
    // about a log that has content rather than about an empty one (§17).
    fixture.hook(&root, &worker, "UserPromptSubmit");
    let sent = fixture.client(
        &root,
        &[
            "api",
            "send",
            "--session",
            &worker,
            "--text",
            "before-the-read",
        ],
    );
    assert!(sent.status.success(), "{}", stderr_of(&sent));
    wait_for("the worker to read the line sent before the read", || {
        fixture
            .received(&root, &worker)
            .is_some_and(|text| text.contains("before-the-read"))
    });

    let sessions_before = server.call(serde_json::json!({ "op": "list_sessions" }));
    let events_before = server.events();
    let received_before = fixture.received(&root, &worker);
    assert_eq!(sessions_before["status"], "ok", "{sessions_before}");
    assert!(
        events_before
            .iter()
            .any(|event| event["session"] == worker.as_str()),
        "the event log must already hold something for this worker, or its being \
         unchanged proves nothing: {events_before:?}"
    );

    let first = fixture.client(&root, &["api", "read", "--session", &worker]);
    assert!(first.status.success(), "{}", stderr_of(&first));
    assert!(
        stdout_of(&first).contains("got:before-the-read"),
        "the read must have actually returned this worker's output, or the \
         assertions below are about a call that did nothing at all: {:?}",
        stdout_of(&first)
    );

    let sessions_after = server.call(serde_json::json!({ "op": "list_sessions" }));
    let events_after = server.events();
    let received_after = fixture.received(&root, &worker);

    assert_eq!(
        sessions_before, sessions_after,
        "reading a worker must not create, close or touch a session"
    );
    assert_eq!(
        events_before, events_after,
        "reading a worker must not put anything in this project's event log: looking \
         at a terminal is not an intervention, and an orchestrator woken by one would \
         be woken by nothing"
    );
    assert_eq!(
        received_before, received_after,
        "nothing may be written to the worker's terminal by a read of it"
    );
    assert!(
        !fixture.reacted_to_interrupt(&root, &worker),
        "a read must not signal the worker"
    );

    // A read is not a drain: the same bytes are still there for the next
    // reader, and for the person who runs the command twice.
    let second = fixture.client(&root, &["api", "read", "--session", &worker]);
    assert!(second.status.success(), "{}", stderr_of(&second));
    assert_eq!(
        stdout_of(&first),
        stdout_of(&second),
        "reading a worker twice must return the same output; a read that consumed \
         the scrollback would be a different verb"
    );
}

/// What `ok` on a read actually means, pinned — because it is narrower than
/// "a process is alive", and the difference is not visible from the verb.
///
/// [`a_live_worker_with_nothing_to_say_and_a_session_with_no_process_are_different_answers`]
/// gets its refusal by taking the **door** away: a session this process holds
/// no pseudo-terminal for is `not live`, which is the same rule `send` and
/// `interrupt` follow and what `api/mod.rs`'s doc comment describes. This is
/// the other case, and it answers differently: the door is still running, the
/// worker's *process* is gone, and the read succeeds and returns what the
/// worker printed before it died.
///
/// **That is deliberate rather than an oversight, and worth having.** A
/// crashed worker's last words are the most useful thing anyone could ask
/// this verb for, and `SessionRuntime` already keeps them on purpose — a
/// restart reuses *the same scrollback*, because what the harness said before
/// it died is the session's. A read that refused here would be answering
/// "there is nothing to show" about a terminal that has plenty to show.
///
/// It is pinned rather than left implicit because the packet's phrasing for
/// this verb was *"no live process"*, and that is not the line the refusal
/// actually falls on. If anyone later makes `recent_output` consult the
/// process's exit status, this test fails and they have to decide about a
/// crashed worker's output on purpose.
#[test]
fn a_worker_whose_process_died_under_a_live_door_still_reads_back_what_it_printed() {
    let fixture = Fixture::with_a_quiet_harness();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let worker = server.spawn_worker();
    wait_for("the quiet worker's harness to start", || {
        fixture.argv(&root, &worker).is_some()
    });

    // Something to still be there afterwards.
    let asked = server.call(serde_json::json!({
        "op": "send_message",
        "session": worker,
        "text": "burst",
    }));
    assert_eq!(asked["status"], "ok", "{asked}");
    wait_for("the worker to print something before it dies", || {
        let response = server.call(serde_json::json!({
            "op": "recent_output",
            "session": worker,
            "max_bytes": 256,
        }));
        response["result"]["output"]
            .as_str()
            .is_some_and(|tail| tail.contains(BURST_SENTINEL))
    });

    // The quiet harness traps nothing, so a real `SIGINT` on its own terminal
    // ends it — which is exactly how a worker dies under a door that keeps
    // running. Marked as deliberate by `interrupt`, so supervision does not
    // restart it and the process really is gone.
    let killed = fixture.client(&root, &["api", "interrupt", "--session", &worker]);
    assert!(killed.status.success(), "{}", stderr_of(&killed));
    // Causal, and the worker's own death rather than a sleep: `poll_exits`
    // publishes `process_exited` when it reaps the process, and that reaches
    // the orchestrator's read path through this door's own event log.
    wait_for("the worker's process to be reaped", || {
        server
            .events()
            .iter()
            .any(|event| event["session"] == worker.as_str() && event["kind"] == "process_exited")
    });

    let read = fixture.client(&root, &["api", "read", "--session", &worker]);
    assert!(
        read.status.success(),
        "a worker that died under a live door is still readable: {}",
        stderr_of(&read)
    );
    assert!(
        stdout_of(&read).contains(BURST_SENTINEL),
        "what the worker printed before it died must survive it: {:?}",
        stdout_of(&read).len()
    );
}

// ---------------------------------------------------------------------------
// Map line 740 — and why these three tests are a refusal rather than a closure.
// ---------------------------------------------------------------------------

/// Every `glasshouse worker-completion` line one session's harness has read.
///
/// The same shape `worker_wakeup.rs` uses, restated here rather than shared:
/// these tests need the *payload* to pull a log position out of it.
fn completions(text: &str) -> Vec<serde_json::Value> {
    text.lines()
        .filter_map(|line| line.split_once("glasshouse worker-completion "))
        .filter_map(|(_, payload)| serde_json::from_str(payload.trim()).ok())
        .collect()
}

/// Line 740: *"Preserve the user's ability to enter and modify a worker
/// session before the orchestrator acts on its result."*
///
/// **This is an ordering claim, and one of its two events still does not
/// exist in this build.** The three tests below are written to say so
/// executably, in the shape this file already uses twice
/// ([`an_intervention_through_the_door_reaches_the_orchestrators_event_read_path`]
/// asserted the wrong behaviour on purpose until somebody fixed it, and was
/// then inverted rather than deleted; the first test of this section has now
/// been inverted the same way). Each pins today's behaviour with a failure
/// message naming what the fix would be, so the refusal is falsifiable by
/// anyone who disagrees with it.
///
/// # The user's half is now recorded *as the user's* — this half is fixed
///
/// It was not when this section was written. `SessionApi::send_text` and
/// `SessionApi::interrupt` hard-wired
/// [`glasshouse::events::MessageOrigin::Machine`], and they are the only
/// write path this door has, so a person running `glasshouse api send` and
/// an orchestrator issuing `Request::SendMessage` produced log rows equal
/// field for field. Both now take the origin from their caller, the request
/// carries it (`protocol::RequestOrigin`, defaulting to machine), and
/// `api::client` states the person it is —
/// [`a_persons_intervention_and_the_orchestrators_own_are_different_rows`]
/// is the inverted test, and it is what a reader should look at first.
///
/// **It is attribution, not identity.** There is still no principal to
/// record: `unix::authorize` (`src/api/unix.rs:309`) admits a peer whose uid
/// equals this process's, so the person and the agent acting for them remain
/// the **same principal** by construction, and a caller that stated an
/// origin it was not would be believed. What changed is that the honest
/// callers — the shipped client, and Glasshouse's own deliveries — stopped
/// being indistinguishable from each other.
///
/// The *"enter"* half is still not recorded at all — reading a worker writes
/// nothing, which [`reading_a_worker_changes_nothing_about_it`] already
/// asserts as a feature.
///
/// # The orchestrator's half is not represented
///
/// [`nothing_in_the_log_records_the_orchestrator_acting_on_a_result`] shows
/// the only moment this build writes down is the **converse** one: Glasshouse
/// handing a result *to* an orchestrator (`pump_watches`, delivering through
/// `SessionApi::send_text` at `src/api/unix.rs:2041`). What the orchestrator
/// then does — re-read, decide, integrate — leaves no row, and an
/// orchestrator that polls `Request::Events` instead of registering a watch
/// leaves not even the handoff.
///
/// # What does hold, and it is worth having
///
/// [`a_person_can_still_enter_and_change_a_worker_after_its_result_reached_the_orchestrator`]
/// proves the *ability* is never foreclosed: the handoff does not close,
/// quiesce or reap the worker, so a person can read it, type into it and
/// interrupt it strictly **after** the orchestrator has been handed its
/// result — which subsumes "before" without needing to win a race. That is
/// an absence of teardown rather than a preserved guarantee, and it is the
/// honest residue of this line.
///
/// # So the line stays refused, for exactly one reason instead of two
///
/// Both events had to exist for an ordering to be provable. One of them now
/// does. The other — the orchestrator *acting on* a result — still leaves no
/// row, and no test here can be written for a moment nothing records.
#[test]
fn a_persons_intervention_and_the_orchestrators_own_are_different_rows() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let worker = server.spawn_worker();
    wait_for("the worker's harness to start", || {
        fixture.argv(&root, &worker).is_some()
    });

    // Deliberately the same length, so `bytes` cannot be mistaken for the
    // thing that tells the two apart.
    const BY_A_PERSON: &str = "from-the-person";
    const BY_THE_AGENT: &str = "by-orchestrator";
    assert_eq!(
        BY_A_PERSON.len(),
        BY_THE_AGENT.len(),
        "the two lines must be the same length or `bytes` would distinguish \
         them for a reason that has nothing to do with who sent them"
    );

    // The orchestrator first, speaking the protocol straight into the door.
    //
    // The orchestrator goes first since capability map line 1719: a machine
    // line into a session a person has just used is refused for
    // `session::runtime::USER_INPUT_PRECEDENCE`, which is a rule
    // `tests/user_control.rs` proves and this test has no business asserting
    // the opposite of. Nothing here depends on the order — both rows are
    // still written, through one door, into one worker, and the question is
    // which origin each carries.
    let machine = server.call(serde_json::json!({
        "op": "send_message",
        "session": worker,
        "text": BY_THE_AGENT,
    }));
    assert_eq!(machine["status"], "ok", "{machine}");
    wait_for("the worker to read the orchestrator's line", || {
        fixture
            .received(&root, &worker)
            .is_some_and(|text| text.contains(BY_THE_AGENT))
    });

    // A person, in a process of their own, running the shipped client.
    let typed = fixture.client(
        &root,
        &["api", "send", "--session", &worker, "--text", BY_A_PERSON],
    );
    assert!(
        typed.status.success(),
        "`glasshouse api send` failed: {}",
        stderr_of(&typed)
    );
    wait_for("the worker to read the person's line", || {
        fixture
            .received(&root, &worker)
            .is_some_and(|text| text.contains(BY_A_PERSON))
    });

    // The viewport (§17): both deliveries demonstrably happened, so the rows
    // being alike is a fact about the rows and not about an empty log.
    let rows = server.deliveries_to(&worker);
    assert_eq!(
        rows.len(),
        2,
        "both lines reached the worker, so the orchestrator's read path must \
         hold exactly two deliveries for it — otherwise this test is \
         comparing something other than the two writes it made: {rows:?}"
    );

    assert_eq!(
        rows[0]["origin"], "machine",
        "the first delivery was the orchestrator speaking the protocol \
         straight into the door with no origin field at all, so it must \
         still be `machine` — the default is what keeps every caller written \
         before the field existed meaning what it meant: {}",
        rows[0]
    );
    assert_eq!(
        rows[1]["origin"], "user_keystroke",
        "the second delivery was made by `glasshouse api send`, a process a \
         person started from their own terminal, and the log must say so — \
         this is the second of the two events line 740's ordering is over, \
         and until the door carried an origin it was unwritable: {}",
        rows[1]
    );

    let stripped: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let mut row = row.clone();
            let fields = row.as_object_mut().expect("an event object");
            // `seq` orders them and `at` is a wall clock. Neither says *who*,
            // which is the only question line 740 is asking.
            fields.remove("seq");
            fields.remove("at");
            row
        })
        .collect();

    assert_ne!(
        stripped[0], stripped[1],
        "a person's intervention and an orchestrator's own message must not \
         be the same row. Session, kind and byte count are identical here by \
         construction — the two lines are the same length on purpose — so \
         the origin is the only field that can tell them apart, and a reader \
         of this log needs it to say that *the user* did something. Rows: \
         {rows:?}"
    );

    // Both halves of the inequality above, so it cannot pass for a reason
    // that is not the origin: everything except the origin still matches.
    // Without this, a mutation that changed `bytes` would keep `assert_ne!`
    // green while destroying the property it is asserting.
    for field in ["session", "kind", "bytes"] {
        assert_eq!(
            stripped[0][field], stripped[1][field],
            "`{field}` must be identical across the two rows, or the \
             inequality above would be satisfied by something other than who \
             sent them: {rows:?}"
        );
    }
}

/// Line 740's second missing event, and the one the packet asked to be named
/// with a `file:line` if it could not be found.
///
/// It could not. The nearest thing this build records is its **converse** —
/// `pump_watches` handing a completion *to* an orchestrator through
/// `SessionApi::send_text` (`src/api/unix.rs:2041`), which lands as one
/// `text_delivered` row on the *notify* session. That is Glasshouse speaking,
/// not the orchestrator acting.
///
/// What an orchestrator does with a result it has been handed is, by design,
/// invisible here: the notification is a statement about log position `seq`
/// and carries no live state (`Completion`'s own doc comment,
/// `src/api/unix.rs:1766`), so consuming it *means* re-reading through
/// `Request::SessionState`, `Request::RecentOutput` and `Request::Events` —
/// and all three are reads, which this door deliberately does not record.
///
/// # The viewport (§17)
///
/// The fence is taken from a log already known to be carrying rows for both
/// sessions — the handoff row is asserted first, by content — so "nothing was
/// added" is measured against a read path demonstrably working, not against
/// one that was broken all along.
#[test]
fn nothing_in_the_log_records_the_orchestrator_acting_on_a_result() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let worker = server.spawn_as("worker");
    let orchestrator = server.spawn_as("orchestrator");
    wait_for("both harnesses to start", || {
        fixture.argv(&root, &worker).is_some() && fixture.argv(&root, &orchestrator).is_some()
    });

    let registered = server.call(serde_json::json!({
        "op": "watch_worker",
        "session": worker,
        "notify": orchestrator,
    }));
    assert_eq!(registered["status"], "ok", "{registered}");

    fixture.hook(&root, &worker, "UserPromptSubmit");
    fixture.hook(&root, &worker, "Stop");

    wait_for("the worker's result to be handed over", || {
        server.a_delivery_was_recorded()
    });

    // The one row the handoff writes, and whose session it is on.
    let handoff = server.deliveries_to(&orchestrator);
    assert_eq!(
        handoff.len(),
        1,
        "the handoff is one machine-originated line typed into the \
         orchestrator's own session: {handoff:?}"
    );
    assert_eq!(
        handoff[0]["origin"], "machine",
        "the recorded moment is Glasshouse speaking to the orchestrator, not \
         the orchestrator acting: {}",
        handoff[0]
    );

    // And it really was a completion, read by the orchestrator's own harness.
    // Asserted *after* the two above so a mutation that redirects the
    // delivery fails on their terms rather than in this wait.
    wait_for("the orchestrator to read the completion", || {
        fixture
            .received(&root, &orchestrator)
            .is_some_and(|text| !completions(&text).is_empty())
    });

    // Causal fence: any row written from here on has a greater `seq`. No
    // clock is consulted.
    let fence = server.max_seq();
    assert!(
        fence > 0,
        "the fence must come from a log that has rows in it, or the emptiness \
         below would be free"
    );

    // Now the orchestrator does the only thing the notification permits it to
    // do with a result: ask again. This *is* acting on it.
    for request in [
        serde_json::json!({ "op": "session_state", "session": worker }),
        serde_json::json!({ "op": "recent_output", "session": worker }),
        serde_json::json!({ "op": "events", "after": 0, "limit": 1000 }),
    ] {
        let response = server.call(request.clone());
        assert_eq!(response["status"], "ok", "{request} -> {response}");
    }

    let after: Vec<serde_json::Value> = server
        .events()
        .into_iter()
        .filter(|event| event["seq"].as_i64().is_some_and(|seq| seq > fence))
        .collect();
    assert!(
        after.is_empty(),
        "IF THIS FAILS, INVERT THIS TEST RATHER THAN DELETING IT. It pins the \
         second half of why capability map line 740 is refused: an \
         orchestrator consuming a worker's result leaves no trace, so there is \
         no moment to order a user's intervention *before*. A row appearing \
         here means one now exists and line 740 has become answerable: {after:?}"
    );

    // Both directions (§17): an emptiness assertion over a fence that could
    // never advance would pass for free. One write that *is* recorded, read
    // back through the identical comparison, is what makes the emptiness
    // above a fact about reads rather than about the fence.
    let control = server.call(serde_json::json!({
        "op": "send_message",
        "session": worker,
        "text": "a-row-that-must-be-seen",
    }));
    assert_eq!(control["status"], "ok", "{control}");
    let seen: Vec<serde_json::Value> = server
        .events()
        .into_iter()
        .filter(|event| event["seq"].as_i64().is_some_and(|seq| seq > fence))
        .collect();
    assert!(
        !seen.is_empty(),
        "the fence must be able to see a new row at all, or the emptiness \
         asserted above is free rather than earned"
    );
}

/// What line 740 *does* have, stated exactly: the ability is never foreclosed.
///
/// The handoff does not close, quiesce or reap the worker — `pump_watches`
/// writes into the notify session and touches nothing else — so a person can
/// enter and modify the worker strictly **after** its result reached the
/// orchestrator. That is the strongest honest form of the line's ordering:
/// the user never has to win a race, because the window does not shut.
///
/// **It is an absence of teardown, not a preserved guarantee**, and that is
/// the difference the refusal turns on. The mutation that kills this test —
/// `pump_watches` closing the worker once its completion is delivered —
/// proves the absence is watched; it does not produce a mechanism that
/// *keeps* the window open, because there is none to produce.
///
/// Ordering is by log position, never by wall clock: the person's delivery is
/// asserted to sit at a greater `seq` than the handoff, and it could not be
/// otherwise, because the handoff had already been read by the orchestrator's
/// own harness before the client process was started.
#[test]
fn a_person_can_still_enter_and_change_a_worker_after_its_result_reached_the_orchestrator() {
    let fixture = Fixture::new();
    let root = fixture.project_root("alpha");
    let server = Server::start(&fixture, &root);

    let worker = server.spawn_as("worker");
    let orchestrator = server.spawn_as("orchestrator");
    wait_for("both harnesses to start", || {
        fixture.argv(&root, &worker).is_some() && fixture.argv(&root, &orchestrator).is_some()
    });

    let registered = server.call(serde_json::json!({
        "op": "watch_worker",
        "session": worker,
        "notify": orchestrator,
    }));
    assert_eq!(registered["status"], "ok", "{registered}");

    fixture.hook(&root, &worker, "UserPromptSubmit");
    fixture.hook(&root, &worker, "Stop");

    wait_for("the worker's result to be handed over", || {
        server.a_delivery_was_recorded()
    });
    let handoff = server.deliveries_to(&orchestrator);
    assert_eq!(
        handoff.len(),
        1,
        "the result must have been handed to the *orchestrator* before this \
         test can say anything about what happens after that: {handoff:?}"
    );
    // No-regression, and the reason this test is the one that carries it:
    // the handoff and the person's intervention below land in the *same log*
    // from the same door in the same run, so "the person is distinguishable"
    // and "Glasshouse's own delivery is unchanged" are one assertion pair
    // rather than two tests that could drift apart. `pump_watches` is woken
    // by a worker's completion, not by a request, so there is no origin
    // field anywhere near it and there must never be one.
    assert_eq!(
        handoff[0]["origin"], "machine",
        "Glasshouse handing a result to an orchestrator is machine-originated \
         and stays so: nothing about a person reaching this door may change \
         what `pump_watches` records: {}",
        handoff[0]
    );
    wait_for("the orchestrator to read the completion", || {
        fixture
            .received(&root, &orchestrator)
            .is_some_and(|text| !completions(&text).is_empty())
    });
    let handed_over = handoff[0]["seq"]
        .as_i64()
        .expect("the handoff's log position");

    // ENTER, after the handoff: the person reads the worker's own terminal.
    const AFTER_THE_HANDOFF: &str = "after-the-handoff";
    let read = fixture.client(&root, &["api", "read", "--session", &worker]);
    assert!(
        read.status.success(),
        "a worker whose result has been handed over is still readable: {}",
        stderr_of(&read)
    );
    assert!(
        stdout_of(&read).contains("READY"),
        "the read must show the worker's own terminal, not an empty answer: \
         {:?}",
        stdout_of(&read)
    );

    // MODIFY, after the handoff: the person types into it, and the worker's
    // own account is what proves the line arrived.
    let sent = fixture.client(
        &root,
        &[
            "api",
            "send",
            "--session",
            &worker,
            "--text",
            AFTER_THE_HANDOFF,
        ],
    );
    assert!(
        sent.status.success(),
        "a worker whose result has been handed over still takes input: {}",
        stderr_of(&sent)
    );
    wait_for("the worker to read the person's line", || {
        fixture
            .received(&root, &worker)
            .is_some_and(|text| text.contains(AFTER_THE_HANDOFF))
    });

    // And the change is visible to the person through the same door, so
    // "enter and modify" is one loop rather than two disconnected verbs.
    let again = fixture.client(&root, &["api", "read", "--session", &worker]);
    assert!(again.status.success(), "{}", stderr_of(&again));
    assert!(
        stdout_of(&again).contains(&format!("got:{AFTER_THE_HANDOFF}")),
        "the worker's echo of the person's line must come back through the \
         read verb: {:?}",
        stdout_of(&again)
    );

    // The ordering, from log positions rather than a clock.
    let deliveries = server.deliveries_to(&worker);
    assert_eq!(
        deliveries.len(),
        1,
        "the only line ever sent to this worker is the person's, after the \
         handoff: {deliveries:?}"
    );
    assert_eq!(
        deliveries[0]["origin"], "user_keystroke",
        "the line was typed by a person running the shipped client, and the \
         handoff above is `machine` in this same log — the ordering line 740 \
         asks about is only writable because these two rows differ: {}",
        deliveries[0]
    );
    let intervened = deliveries[0]["seq"]
        .as_i64()
        .expect("the intervention's log position");
    assert!(
        intervened > handed_over,
        "the person's intervention must sit after the handoff in the same \
         ordered log ({intervened} > {handed_over}), which is what makes this \
         the *after* case rather than a race the person happened to win"
    );

    // The worker's own trap file is the evidence, and nothing the door says
    // can produce it. Interrupting last is no longer forced — the harness
    // survives its own interrupt on every shell now, see
    // `install_session_tagging_harness` — but the order is kept, because an
    // interrupt is the last thing a person does to a worker whose result has
    // already been handed over.
    let interrupted = fixture.client(&root, &["api", "interrupt", "--session", &worker]);
    assert!(
        interrupted.status.success(),
        "a worker whose result has been handed over can still be interrupted: \
         {}",
        stderr_of(&interrupted)
    );
    wait_for("the worker to handle a real SIGINT", || {
        fixture.reacted_to_interrupt(&root, &worker)
    });

    // And the interrupt is attributed too. Asserted after the worker's own
    // trap file proves a real `0x03` arrived, so this is about the record of
    // an interrupt that demonstrably happened.
    let interrupts = server.interrupts_to(&worker);
    assert_eq!(
        interrupts.len(),
        1,
        "the only interrupt this worker ever received is the person's: \
         {interrupts:?}"
    );
    assert_eq!(
        interrupts[0]["origin"], "user_keystroke",
        "a person's `Ctrl-C` through `glasshouse api interrupt` is an \
         intervention like their line was, and an orchestrator deciding to \
         stop a worker is a different event even though the byte on the wire \
         is identical: {}",
        interrupts[0]
    );
}
