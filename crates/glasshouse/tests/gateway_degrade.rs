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

    // The sink is the one piece a production caller (main.rs, forbidden to
    // this package) would build: it closes over the bus and the session
    // records this test controls, and is the only place `degrade_resource`
    // is actually called from.
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
