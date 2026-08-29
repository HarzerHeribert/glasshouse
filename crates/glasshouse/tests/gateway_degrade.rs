//! Map line 1735 — "Detect gateway failure separately from harness-process
//! failure" — proved through the real wire, not either mechanism's own unit
//! tests in isolation.
//!
//! `gateway::session::gateway_failure` (new) classifies a real finished
//! exchange; `events::degrade_resource` (already shipped, previously called
//! from nowhere but its own tests) publishes `GatewayUnhealthy` for every
//! session bound to the failing resource, and only those. The seam between
//! them is `gateway::DegradeSink`, invoked from `gateway::mod`'s real
//! `accept_loop` — the same function every gateway-backed request in this
//! binary goes through — via `gateway::start_if_required_with_degrade_sink`.
//!
//! This drives a real `TcpStream` through a real gateway pointed at an
//! address nothing answers, exactly the way `tests/routing_live.rs` proves
//! its own production wiring, rather than calling `gateway_failure` or
//! `degrade_resource` directly: practice §35/§36 is that a caller a test can
//! bypass is not a caller, and `degrade_resource` was defined in production
//! and called from nowhere but tests before this package.
//!
//! # Two levels, and only the second one closes the line
//!
//! The first test enters at `start_if_required_with_degrade_sink` and builds
//! its own sink. That is the **seam**, and it was green for a whole batch
//! while the shipped binary passed `None` at both of its gateway starts —
//! which is precisely §35's failure and precisely why the evidence ledger
//! refused this line. It is kept because it is the only place the "and only
//! the bound session" half can be observed with two sessions in play.
//!
//! The tests under *through the shipped binary* enter at `glasshouse launch`
//! and read the project's durable event log. Reverting either `main.rs` call
//! site to `None` leaves the first test passing and fails those.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glasshouse::events::{EventBus, GatewayFailure, LifecycleEvent, degrade_resource};
use glasshouse::gateway::{DegradeSink, Route, Upstream, UpstreamBackend};
use glasshouse::integrations::IntegrationId;
use glasshouse::profile::{BackendResource, LaunchProfile};
use glasshouse::routing::{Cost, CredentialId};
use glasshouse::secret::{EnvironmentSecretStore, Secret, SecretRef, SecretStore};
use glasshouse::session::{
    SessionId, SessionLifecycle, SessionPresentation, SessionRecord, SessionRole,
};

/// A variable unique to this test file, set and cleared immediately around
/// the one resolve that needs it.
const CREDENTIAL_VAR: &str = "GLASSHOUSE_GATEWAY_DEGRADE_TEST_KEY";

/// A stand-in provider credential, resolved through the real environment
/// store rather than a crate-private test constructor: `secret::Secret` has
/// no public way to mint one outside `crate::secret` by design (see that
/// module's own "not from arbitrary text" doc), and this file is an
/// integration test, outside the crate.
fn test_credential() -> Secret {
    // SAFETY: `CREDENTIAL_VAR` is unique to this test binary's one caller and
    // is removed again immediately below, before the resolved value is even
    // inspected, so no other test can observe it set.
    unsafe {
        std::env::set_var(CREDENTIAL_VAR, "sk-planted-not-a-real-key-aaaabbbbcccc");
    }
    let resolved = EnvironmentSecretStore::new()
        .resolve(&SecretRef::Environment {
            var: CREDENTIAL_VAR.to_owned(),
        })
        .expect("the variable was just set");
    unsafe {
        std::env::remove_var(CREDENTIAL_VAR);
    }
    resolved
}

fn credential_id() -> CredentialId {
    CredentialId::new(
        "fixture",
        SecretRef::Environment {
            var: CREDENTIAL_VAR.to_owned(),
        },
    )
}

/// A gateway-backed upstream pointed at an address nothing answers.
/// `127.0.0.1:1` is this project's own idiom for "unreachable" — used the
/// same way by `gateway::session::tests` — so the very first real exchange
/// produces `ingress::Outcome::Unreachable` (a refused connection) rather
/// than a timeout this test would have to wait out.
fn unreachable_upstream() -> Upstream {
    let backend = UpstreamBackend::new(
        "fixture".to_owned(),
        vec![Route::new(
            "anthropic-messages".to_owned(),
            &["/messages"],
            "http://127.0.0.1:1",
        )],
        test_credential(),
        credential_id(),
        Cost::Metered,
    )
    .expect("a loopback http URL is absolute and this credential is header-safe");
    Upstream::with_failover(vec![backend]).expect("one backend is not none")
}

/// A minimal session record, the same literal shape
/// `events::mod::tests::record` builds — every field is `pub`, and
/// `degrade_resource` only ever reads `id` and `backend_resource`.
fn record(id: &str, backend_resource: Option<&str>) -> SessionRecord {
    SessionRecord {
        id: SessionId::new(id),
        project_id: "project".to_owned(),
        harness: "claude-code".to_owned(),
        native_session_id: None,
        role: SessionRole::Normal,
        lifecycle: SessionLifecycle::Running,
        presentation: SessionPresentation::Embedded,
        created_at: 1,
        last_activity_at: 2,
        launch_profile: None,
        backend_resource: backend_resource.map(str::to_owned),
        model: None,
        pairing_class: None,
        protocol: None,
        response_profile: None,
        response_mechanism: None,
        display_name: None,
        purpose: None,
        source_session_id: None,
    }
}

fn messages_request(token: &str) -> Vec<u8> {
    let body = "{\"model\":\"probe\"}";
    format!(
        "POST /v1/messages HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Authorization: Bearer {token}\r\n\
         Content-Type: application/json\r\n\
         Anthropic-Version: 2023-06-01\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {body}",
        body.len()
    )
    .into_bytes()
}

/// Send `raw` and return everything the gateway wrote back, to the close.
fn send_and_read(address: SocketAddr, raw: &[u8]) -> String {
    let mut client = TcpStream::connect(address).expect("the gateway accepts connections");
    client
        .set_read_timeout(Some(Duration::from_secs(20)))
        .expect("a non-zero read timeout is valid");
    client
        .write_all(raw)
        .expect("the gateway reads the request");
    client.flush().expect("the gateway reads the request");
    let mut out = Vec::new();
    client
        .read_to_end(&mut out)
        // A Windows server that closes with unread bytes still in its receive
        // queue sends RST rather than FIN, so the client's read answers
        // WSAECONNRESET (10054) instead of a clean end of stream — *after*
        // `out` already holds the whole response. Treating that as end of
        // stream is not leniency: the bytes are already here, and the
        // assertions below are what decide whether they are right. Measured on
        // the ARM64 VM: same tree, same target, one run clean and the next
        // reset.
        .or_else(|err| match err.kind() {
            std::io::ErrorKind::ConnectionReset => Ok(out.len()),
            _ => Err(err),
        })
        .expect("the gateway answers and then closes");
    String::from_utf8_lossy(&out).into_owned()
}

/// The whole line, through the real production caller.
///
/// - **premise first (§17):** before the request, neither session has any
///   `GatewayUnhealthy` recorded.
/// - a real upstream failure produces exactly one `GatewayUnhealthy`, naming
///   the resource and the failure variant, for the session bound to it.
/// - **the session is still running afterwards** — the recorded event's own
///   `implied_state()` is `None`, so nothing here can have moved it, and it
///   is the *only* event recorded: no `ProcessExited`, no exit of any kind.
/// - a session bound to a different resource records nothing at all.
#[test]
fn a_real_gateway_failure_degrades_only_the_bound_session_and_moves_no_lifecycle() {
    let bus = EventBus::new();
    let resource = BackendResource::GlasshouseGateway.slug();
    let on_gateway = record("on-gateway", Some(resource.as_str()));
    let elsewhere = record("elsewhere", Some("some-other-backend"));
    let records = vec![on_gateway.clone(), elsewhere.clone()];

    // Premise first: the world before the failure has nothing recorded for
    // either session, whatever their lifecycle otherwise is.
    assert!(
        bus.history_for(&on_gateway.id).is_empty(),
        "no GatewayUnhealthy should exist before any exchange has run"
    );
    assert!(bus.history_for(&elsewhere.id).is_empty());

    // The sink `main.rs::DegradeRelay` builds in production, reduced to what
    // this test needs: it closes over the bus and the session records this
    // test controls. Building it here is what makes this a *seam* test — see
    // this file's header, and the binary-level tests below.
    let calls: Arc<Mutex<Vec<(String, GatewayFailure)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink: DegradeSink = {
        let bus = bus.clone();
        let records = records.clone();
        let calls = Arc::clone(&calls);
        Arc::new(move |resource: &str, reason: GatewayFailure| {
            calls.lock().unwrap().push((resource.to_owned(), reason));
            degrade_resource(&bus, &records, resource, reason);
        })
    };

    let mut profile = LaunchProfile::native(IntegrationId::ClaudeCode);
    profile.backend = BackendResource::GlasshouseGateway;
    let gateway = glasshouse::gateway::start_if_required_with_degrade_sink(
        &[profile],
        || Ok(unreachable_upstream()),
        None,
        None,
        None,
        Some(sink),
    )
    .expect("loopback is bindable")
    .expect("a gateway-backed profile requires a gateway");

    let response = send_and_read(
        gateway.address(),
        &messages_request(gateway.token().expose()),
    );
    assert!(
        response.starts_with("HTTP/1.1 502"),
        "an unreachable upstream must be reported to the harness as a gateway error: {response}"
    );

    // The connection thread's own bookkeeping (routing, quota, and this
    // sink) runs after `ingress::serve` has already closed the response
    // socket, so `send_and_read` returning is not proof the sink has been
    // called yet — only that the client side of the exchange is over.
    let deadline = Instant::now() + Duration::from_secs(5);
    while calls.lock().unwrap().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(
        *calls.lock().unwrap(),
        vec![(resource.clone(), GatewayFailure::Unreachable)],
        "the gateway must report exactly one failure, naming the resource and the variant that \
         matches `ingress::Outcome::Unreachable`"
    );

    let events = bus.history_for(&on_gateway.id);
    assert_eq!(
        events.len(),
        1,
        "exactly one event — the failure — and nothing else: {events:?}"
    );
    assert_eq!(
        events[0].event(),
        &LifecycleEvent::GatewayUnhealthy {
            resource: resource.clone(),
            reason: GatewayFailure::Unreachable,
        }
    );
    assert_eq!(
        events[0].event().implied_state(),
        None,
        "a gateway failure must not move a live session's state — marking it failed would be a \
         lie about a live, steerable harness process"
    );
    assert!(
        !matches!(events[0].event(), LifecycleEvent::ProcessExited { .. }),
        "a gateway failure must never be recorded as a process exit"
    );

    assert!(
        bus.history_for(&elsewhere.id).is_empty(),
        "a session bound to a different resource must record nothing at all"
    );
}

// --- through the shipped binary --------------------------------------------
//
// Everything above enters at `start_if_required_with_degrade_sink`, which is
// the seam. §35 is that a caller every test bypasses is not a caller, and the
// evidence ledger refused this line for exactly that: the mechanism was built,
// proven at the seam, and never installed by the binary. So this half enters
// where a person does — `glasshouse launch` — and asserts on the *durable*
// event log, which is what `api/unix.rs::gateway_failure_str` renders and what
// survives the process that wrote it.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use glasshouse::cli::Cli;
use glasshouse::events::EventLog;

/// A project with its own data and config roots, a fake harness on a path the
/// configuration names, and a provider pointed at an address nothing answers.
///
/// The provider's `base_url` is `unreachable_upstream`'s idiom again — the
/// gateway resolves its upstream from the configured providers, so pointing
/// *that* at `127.0.0.1:1` is what makes a real request through the real
/// binary produce `ingress::Outcome::Unreachable`.
struct BinaryFixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

/// The variable the fake harness is told to dump its environment into, and
/// the one it watches for permission to exit. Named per file, set only on the
/// spawned child, so nothing else in this test binary can see either.
const ENV_DUMP_VAR: &str = "GLASSHOUSE_GATEWAY_DEGRADE_ENV_DUMP";
const STOP_VAR: &str = "GLASSHOUSE_GATEWAY_DEGRADE_STOP";
const PROBE_KEY_VAR: &str = "GLASSHOUSE_GATEWAY_DEGRADE_PROBE_KEY";

/// How many ticks the waiting harness waits — a tenth of a second each on
/// Unix, about a second each on Windows — before giving up and exiting on its
/// own.
///
/// **The harness has to be able to end without being told to.** A panicking
/// test unwinds through `TempDir`'s `Drop`, which deletes the directory the
/// stop file was going to be written into, so a harness that only ever watched
/// for that file would wait forever — and its `glasshouse launch` parent, and
/// that launch's gateway listener, with it. Four of those accumulated while
/// this test was being written. This bound is what makes that impossible;
/// `Launch`'s own `Drop` below is what makes it rare.
const HARNESS_TICKS: u32 = 900;

impl BinaryFixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = std::fs::canonicalize(tmp.path()).expect("canonicalize the fixture base");
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        std::fs::create_dir_all(base.join("config")).expect("create config dir");
        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");

        let harness = install_waiting_harness(&bin_dir);
        // TOML needs a Windows path's backslashes escaped, the same way
        // `launch_overlay.rs`'s fixture does it.
        let escaped = harness.display().to_string().replace('\\', "\\\\");
        std::fs::write(
            base.join("config").join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
                 [providers.probe]\ntemplate = \"anthropic-compatible\"\n\
                 base_url = \"http://127.0.0.1:1\"\n\
                 credential_env = [\"{PROBE_KEY_VAR}\"]\n\n\
                 [profiles.gateway-probe]\nharness = \"claude-code\"\n\n\
                 [profiles.gateway-probe.backend]\nkind = \"glasshouse-gateway\"\n"
            ),
        )
        .expect("write user config");

        Self {
            _tmp: tmp,
            base,
            root,
        }
    }

    fn env_dump(&self) -> PathBuf {
        self.base.join("harness-env.txt")
    }

    fn stop_file(&self) -> PathBuf {
        self.base.join("stop")
    }

    /// `glasshouse launch … --headless`, spawned rather than waited on: the
    /// gateway only exists while the session does, so the request below has to
    /// be made against a live process.
    ///
    /// `--headless` rather than an attached session because this test is about
    /// the event log, not the terminal: `run_headless` takes no terminal, and
    /// the gateway start it goes through is the same one — it happens before
    /// the headless branch is even reached.
    fn spawn_launch(&self) -> Launch {
        let child = Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(["launch", "claude-code", "--profile", "gateway-probe"])
            .arg("--headless")
            .env(ENV_DUMP_VAR, self.env_dump())
            .env(STOP_VAR, self.stop_file())
            // A planted credential, so `gateway_upstream` finds one and the
            // gateway actually starts. It never leaves this process tree.
            .env(PROBE_KEY_VAR, "sk-planted-not-a-real-key-aaaabbbbcccc")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("the glasshouse binary must be runnable");
        Launch { child }
    }

    /// This project's durable event log, opened through the same bootstrap a
    /// second Glasshouse command would use.
    fn logged_events(&self) -> Vec<glasshouse::events::LoggedEvent> {
        let cli = Cli {
            scope: Some(self.root.clone()),
            allow_unsafe_scope: false,
            data_dir: Some(self.base.join("data")),
            config_dir: Some(self.base.join("config")),
            log_level: None,
            log_file: None,
            log_stderr: false,
            command: None,
        };
        let runtime =
            glasshouse::bootstrap(&cli, &self.root).expect("bootstrap the fixture runtime");
        EventLog::open(&runtime)
            .expect("open the project event log")
            .all()
            .expect("read the project event log")
    }
}

/// A spawned `glasshouse launch`, killed when the test ends however it ends.
///
/// A bare `Child` is not enough: `Child::drop` **does not** kill the process,
/// so a failing assertion leaves a launch — and its gateway listener — behind
/// on the machine running the tests. The harness's own tick bound is the
/// backstop; this is the ordinary case.
struct Launch {
    child: Child,
}

impl std::ops::Deref for Launch {
    type Target = Child;

    fn deref(&self) -> &Child {
        &self.child
    }
}

impl std::ops::DerefMut for Launch {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.child
    }
}

impl Drop for Launch {
    fn drop(&mut self) {
        // Already reaped by a test that waited on it: nothing to do, and
        // `kill` on a reaped child is an error rather than a no-op.
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A harness that records the environment it was launched with and then waits
/// to be told to exit.
///
/// It has to outlive the request below: the gateway's listener is a guard held
/// by `launch_session`, so a harness that exits immediately takes the gateway
/// with it before anything can be sent through it.
///
/// The dump is written to a scratch name and renamed, so a reader that sees
/// the file sees all of it.
#[cfg(unix)]
fn install_waiting_harness(bin_dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join("fake-claude");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n\
             env > \"${ENV_DUMP_VAR}.partial\"\n\
             mv \"${ENV_DUMP_VAR}.partial\" \"${ENV_DUMP_VAR}\"\n\
             ticks=0\n\
             while [ ! -f \"${STOP_VAR}\" ] && [ \"$ticks\" -lt {HARNESS_TICKS} ]; do\n\
             ticks=$((ticks + 1)); sleep 0.1\n\
             done\n\
             exit 0\n"
        ),
    )
    .expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_waiting_harness(bin_dir: &Path) -> PathBuf {
    let path = bin_dir.join("fake-claude.cmd");
    // `ping` rather than `timeout`, which refuses to run when stdin is
    // redirected — which it is, inside a pseudo-terminal.
    std::fs::write(
        &path,
        format!(
            "@echo off\r\n\
             set > \"%{ENV_DUMP_VAR}%.partial\"\r\n\
             move /y \"%{ENV_DUMP_VAR}%.partial\" \"%{ENV_DUMP_VAR}%\" >nul\r\n\
             set /a ticks=0\r\n\
             :wait\r\n\
             if exist \"%{STOP_VAR}%\" exit /b 0\r\n\
             if %ticks% GEQ {HARNESS_TICKS} exit /b 0\r\n\
             set /a ticks+=1\r\n\
             ping -n 2 127.0.0.1 >nul\r\n\
             goto wait\r\n"
        ),
    )
    .expect("write fake harness");
    path
}

/// A harness that exits on its own, without ever touching the gateway.
#[cfg(unix)]
fn install_dying_harness(bin_dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join("fake-claude");
    std::fs::write(&path, "#!/bin/sh\nexit 3\n").expect("write fake harness");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(windows)]
fn install_dying_harness(bin_dir: &Path) -> PathBuf {
    let path = bin_dir.join("fake-claude.cmd");
    std::fs::write(&path, "@echo off\r\nexit /b 3\r\n").expect("write fake harness");
    path
}

/// Wait for `path` to exist, or fail saying what the binary printed.
fn wait_for_file(path: &Path, child: &mut Launch, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !path.exists() {
        if let Some(status) = child.try_wait().expect("poll the launch") {
            panic!("the binary exited ({status}) before {what}");
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One `NAME=value` line's value from a dumped environment.
fn dumped(dump: &str, name: &str) -> String {
    dump.lines()
        .find_map(|line| line.strip_prefix(&format!("{name}=")))
        .unwrap_or_else(|| panic!("the harness's environment had no {name}:\n{dump}"))
        .trim()
        .to_owned()
}

/// **Map line 1735, through the command a person types.**
///
/// A real `glasshouse launch` of a gateway-backed profile, a real request
/// through the gateway that launch started, a real upstream that refuses the
/// connection — and the failure is in this project's durable event log,
/// against the session the binary launched, named as a gateway failure.
///
/// Three claims, and each of them is the line rather than a proxy for it:
///
/// - the failure is **recorded by the shipped binary**, not by a sink a test
///   built. Reverting either `main.rs` call site to `None` makes this fail
///   while every seam test above still passes;
/// - it is a `GatewayUnhealthy`, **not** a `ProcessExited`. That separation is
///   the whole content of the line;
/// - and it is recorded **before** the harness exits, with the session's
///   `ProcessExited` arriving afterwards — so the gateway failing did not end
///   the session, which is the half `implied_state() == None` guarantees in
///   the seam test and this one observes in the order of the log.
#[test]
fn the_shipped_binary_records_a_gateway_failure_against_the_session_it_launched() {
    let fixture = BinaryFixture::new();
    let mut child = fixture.spawn_launch();

    wait_for_file(
        &fixture.env_dump(),
        &mut child,
        "the harness to record its environment",
    );
    let dump = std::fs::read_to_string(fixture.env_dump()).expect("read the harness environment");

    // The gateway's own address and token, as the harness was given them —
    // read out of the child's environment rather than guessed, so this is the
    // door the harness itself would knock on.
    let base_url = dumped(&dump, "ANTHROPIC_BASE_URL");
    let token = dumped(&dump, "ANTHROPIC_AUTH_TOKEN");
    let address: SocketAddr = base_url
        .strip_prefix("http://")
        .expect("the gateway is plain loopback HTTP")
        .parse()
        .expect("the gateway's base URL is host:port");

    let response = send_and_read(address, &messages_request(&token));
    assert!(
        response.starts_with("HTTP/1.1 502"),
        "an unreachable upstream must be reported to the harness as a gateway error: {response}"
    );

    // Recorded while the harness is still running: the log is polled with the
    // child alive and the stop file not yet written. A `GatewayUnhealthy` that
    // only appeared after the session ended would not be this line.
    //
    // The wait **runs out** rather than asserting, so that a build which never
    // records anything fails on the named assertion below and prints what it
    // did record — practice §80's fifth case: a mutation killed by a fixture's
    // own timeout has not shown the test's assertions work.
    let deadline = Instant::now() + Duration::from_secs(20);
    let recorded_a_failure = |logged: &[glasshouse::events::LoggedEvent]| {
        logged
            .iter()
            .any(|entry| matches!(entry.event, LifecycleEvent::GatewayUnhealthy { .. }))
    };
    let mut logged = fixture.logged_events();
    while !recorded_a_failure(&logged) && Instant::now() < deadline {
        assert!(
            child.try_wait().expect("poll the launch").is_none(),
            "the binary exited while this test was still waiting for the gateway failure, so \
             nothing here can tell a slow write from an absent one"
        );
        std::thread::sleep(Duration::from_millis(50));
        logged = fixture.logged_events();
    }

    let failures: Vec<_> = logged
        .iter()
        .filter(|entry| matches!(entry.event, LifecycleEvent::GatewayUnhealthy { .. }))
        .collect();
    assert_eq!(
        failures.len(),
        1,
        "the shipped binary must record exactly one gateway failure for a launch whose upstream \
         refused the connection; it recorded {:?}",
        logged
            .iter()
            .map(|entry| entry.event.kind())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        failures[0].event,
        LifecycleEvent::GatewayUnhealthy {
            resource: BackendResource::GlasshouseGateway.slug(),
            reason: GatewayFailure::Unreachable,
        },
        "the recorded failure must name the resource the session resolved to and the variant \
         `ingress::Outcome::Unreachable` produces"
    );

    // The session it was recorded against is the one this launch started, and
    // that session is not over: nothing has exited yet.
    let started = logged
        .iter()
        .find(|entry| entry.event == LifecycleEvent::SessionStarted)
        .expect("the launch records a SessionStarted");
    assert_eq!(
        failures[0].session, started.session,
        "the failure must be recorded against the session the binary launched"
    );
    assert!(
        !logged
            .iter()
            .any(|entry| matches!(entry.event, LifecycleEvent::ProcessExited { .. })),
        "a gateway failure must not be recorded as, or accompanied by, a process exit while the \
         harness is still running: {logged:?}"
    );

    // Now let the harness go, and check the other side of the separation: the
    // process exit is its own event, after the failure, and it does not
    // retroactively become one.
    std::fs::write(fixture.stop_file(), "go").expect("write the stop file");
    let status = child.wait().expect("wait for the launch");
    assert!(status.success(), "the launch exited {status}");

    let after = fixture.logged_events();
    let exits: Vec<_> = after
        .iter()
        .filter(|entry| matches!(entry.event, LifecycleEvent::ProcessExited { .. }))
        .collect();
    assert_eq!(exits.len(), 1, "one process exit, once the harness ends");
    assert!(
        exits[0].seq > failures[0].seq,
        "the gateway failure must have been recorded before the harness exited, not with it"
    );
    assert_eq!(
        after
            .iter()
            .filter(|entry| matches!(entry.event, LifecycleEvent::GatewayUnhealthy { .. }))
            .count(),
        1,
        "the harness exiting must not add a second gateway failure"
    );
}

/// The other direction, which is half of what "separately" means.
///
/// A harness process that dies on its own — no request, no gateway exchange —
/// records a `ProcessExited` carrying its code and **no** gateway failure. A
/// build that reported one would be the collapse `events::GatewayFailure`'s
/// own doc refuses: *"a harness process that dies is one session's problem,
/// and a backend that stops answering is every session pointed at it."*
#[test]
fn a_harness_that_dies_records_a_process_exit_and_no_gateway_failure() {
    let fixture = BinaryFixture::new();
    // Same gateway-backed profile, same unreachable provider — only the
    // harness differs, so the gateway is started and simply never used.
    install_dying_harness(&fixture.base.join("bin"));

    let mut child = fixture.spawn_launch();
    let status = child.wait().expect("wait for the launch");
    assert!(
        !status.success(),
        "a harness exiting 3 must be reported as a failure by the launch"
    );

    let logged = fixture.logged_events();
    let exits: Vec<_> = logged
        .iter()
        .filter_map(|entry| match &entry.event {
            LifecycleEvent::ProcessExited { exit } => Some(exit),
            _ => None,
        })
        .collect();
    assert_eq!(exits.len(), 1, "one process exit: {logged:?}");
    assert_eq!(
        exits[0].code(),
        3,
        "the harness's own exit code must survive: {logged:?}"
    );
    assert!(
        !logged
            .iter()
            .any(|entry| matches!(entry.event, LifecycleEvent::GatewayUnhealthy { .. })),
        "a harness process dying must never be recorded as a gateway failure: {logged:?}"
    );
}
