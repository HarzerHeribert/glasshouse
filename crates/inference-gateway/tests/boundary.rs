//! Five process-boundary invariants for the `inference-gateway` binary,
//! driven the way `tests/bin.rs` drives it: spawn the built binary,
//! configure it by file, speak HTTP to the announced address, stop it by
//! stdin EOF or a signal. See `tests/common/mod.rs` for the shared fixtures
//! (`FakeProvider`, `post`, `gateway`, `wait_for_exit`) this file reuses
//! rather than re-implementing.

use std::io::{BufRead, BufReader, Read};
#[cfg(unix)]
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, Stdio};
#[cfg(unix)]
use std::time::Duration;

mod common;
use common::{FakeProvider, gateway, post, post_raw, post_with_headers, wait_for_exit};

/// A spawned gateway child, killed on every exit path — including a panic
/// unwinding through a failed assertion, which plain [`Child`] does not do
/// on drop. Mirrors the shape `FakeProvider`'s own `Drop` already has in
/// `common::mod`, applied to the process side of the fixture instead of the
/// listener side.
struct ChildGuard(Child);

impl ChildGuard {
    fn spawn(mut command: std::process::Command) -> Self {
        Self(command.spawn().expect("the built binary runs"))
    }
}

impl std::ops::Deref for ChildGuard {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for ChildGuard {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Read the ready line `{"listening":…,"token":…}` a freshly spawned
/// gateway prints, and return `(listening, token)`. Mirrors
/// `bin.rs`'s own inline parse, which this file cannot import without
/// changing that test's behaviour.
fn read_ready_line(stdout: &mut impl BufRead) -> (String, String) {
    let mut ready = String::new();
    stdout.read_line(&mut ready).expect("a ready line arrives");
    let ready: serde_json::Value =
        serde_json::from_str(ready.trim()).expect("the ready line is one JSON object");
    (
        ready["listening"].as_str().expect("a URL").to_owned(),
        ready["token"].as_str().expect("a token").to_owned(),
    )
}

/// Line 511/514's process-level form: `a-live` sorts first and declares a
/// different model than the request names, `z-dead` sorts second, declares
/// the requested model, and sits at a dead port. The request must come back
/// refused without ever reaching `a-live`.
///
/// The account order is deliberate: `a-live` sorts before `z-dead` in the
/// catalogue's `BTreeMap`, so `Upstream`'s default serving index (0) would
/// pick it if model-based selection did not override that default.
/// `gateway/ingress.rs` calls `serving_for_target`, which resolves the
/// account declaring the requested model — `z-dead` — before anything is
/// sent, so the dead account is chosen and the fake `a-live` is never
/// touched. Before `gateway.toml` could declare `models` on an api-key
/// account, `pool_from_catalogue` never called `UpstreamBackend::with_models`
/// for one, so neither account declared anything, `for_model` never matched
/// either, and the default index-0 pick — `a-live` — would have carried the
/// request itself. This is why the mutation dropping that call is caught
/// here: without it, the fake records a request instead of nothing.
#[test]
fn a_request_for_a_dead_model_is_refused_rather_than_served_by_another_model() {
    let a_live = FakeProvider::start();
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let config_path = scratch.path().join("gateway.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
[providers.live]
base_url = "{}"
protocol = "anthropic-messages"
credential_env = ["GATEWAY_BOUNDARY_LIVE_KEY"]

[providers.dead]
base_url = "http://127.0.0.1:1"
protocol = "anthropic-messages"
credential_env = ["GATEWAY_BOUNDARY_DEAD_KEY"]

[accounts.a-live]
kind = "api-key"
provider = "live"
credential = {{ env = "GATEWAY_BOUNDARY_LIVE_KEY" }}
models = ["m-only-a-live-serves"]

[accounts.z-dead]
kind = "api-key"
provider = "dead"
credential = {{ env = "GATEWAY_BOUNDARY_DEAD_KEY" }}
models = ["m-only-z-dead-serves"]
"#,
            a_live.base_url()
        ),
    )
    .expect("the configuration is written");

    let mut child = ChildGuard::spawn({
        let mut command = gateway(&config_path, scratch.path());
        command
            .args(["serve", "--listen", "127.0.0.1:0"])
            .env("GATEWAY_BOUNDARY_LIVE_KEY", "live-account-key") // glasshouse:not-a-secret
            .env("GATEWAY_BOUNDARY_DEAD_KEY", "dead-account-key") // glasshouse:not-a-secret
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    });
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout was piped"));
    let (listening, token) = read_ready_line(&mut stdout);

    let raw = post_with_headers(
        &format!("{listening}/v1/messages"),
        &format!("Bearer {token}"),
        &[("x-glasshouse-model", "m-only-z-dead-serves")],
        r#"{"model":"m-only-z-dead-serves","max_tokens":1,"messages":[{"role":"user","content":"ping"}]}"#,
    );
    assert!(
        !raw.starts_with("HTTP/1.1 2"),
        "a request whose account is unreachable must not read as served: {raw}"
    );
    assert!(
        a_live.requests(0).is_empty(),
        "the account naming a different model must never be tried for this exchange"
    );

    drop(child.stdin.take().expect("stdin was piped"));
    let status = wait_for_exit(&mut child);
    assert!(status.success(), "clean exit on stdin EOF, got {status:?}");
}

/// The signal half of the shutdown contract `bin.rs`'s
/// `serve_announces_one_line_forwards_with_it_and_exits_when_stdin_closes`
/// already covers for stdin EOF: `SIGTERM` reaches `wait_for_shutdown`
/// through `ctrlc`'s handler (Cargo.toml's `ctrlc` carries the
/// `termination` feature specifically so `SIGTERM`, not only `SIGINT`, is
/// caught), the same shutdown path runs, and the process exits `0` with its
/// listening port released.
#[cfg(unix)]
#[test]
fn a_termination_signal_stops_serving_and_the_process_exits() {
    let provider = FakeProvider::start();
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let config_path = scratch.path().join("gateway.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
[providers.fixture]
base_url = "{}"
protocol = "anthropic-messages"
credential_env = ["GATEWAY_BOUNDARY_SIGNAL_KEY"]

[accounts.local]
kind = "api-key"
provider = "fixture"
credential = {{ env = "GATEWAY_BOUNDARY_SIGNAL_KEY" }}
"#,
            provider.base_url()
        ),
    )
    .expect("the configuration is written");

    let mut child = ChildGuard::spawn({
        let mut command = gateway(&config_path, scratch.path());
        command
            .args(["serve", "--listen", "127.0.0.1:0"])
            .env("GATEWAY_BOUNDARY_SIGNAL_KEY", "fixture-key") // glasshouse:not-a-secret
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    });
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout was piped"));
    let (listening, token) = read_ready_line(&mut stdout);

    let (status, _) = post(
        &format!("{listening}/v1/messages"),
        &format!("Bearer {token}"),
        r#"{"model":"fixture-model","max_tokens":1,"messages":[{"role":"user","content":"ping"}]}"#,
    );
    assert!(status.contains("200"), "the request completed: {status}");
    assert_eq!(provider.requests(1).len(), 1);

    let address: SocketAddr = listening
        .strip_prefix("http://")
        .expect("a loopback URL")
        .parse()
        .expect("the announced address parses");

    let pid = child.id() as libc::pid_t;
    let killed = unsafe { libc::kill(pid, libc::SIGTERM) };
    assert_eq!(killed, 0, "SIGTERM could be sent to the child");

    let status = wait_for_exit(&mut child);
    assert!(
        status.success(),
        "a termination signal stops serving cleanly, got {status:?}"
    );

    let refused = TcpStream::connect_timeout(&address, Duration::from_millis(500));
    assert!(
        refused.is_err(),
        "the listening port must be released once the process has exited"
    );
}

/// Line 2711's carrier, at the process boundary: a session bound to a chat
/// account still reaches a `typesafe-systemone` account for the one target
/// only it claims, and the same session's `/v1/messages` traffic keeps
/// going to the chat account. Mirrors
/// `gateway::conformance::a_systemone_request_reaches_the_typesafe_account_while_messages_stay_with_the_bound_one`,
/// through the shipped binary and real sockets instead of the in-crate
/// harness that test uses.
#[test]
fn a_systemone_request_reaches_the_typesafe_account_through_the_binary() {
    let claude_fixture = FakeProvider::start();
    let typesafe_fixture = FakeProvider::answering(
        "HTTP/1.1 200 OK",
        "content-type: application/json\r\n",
        r#"{"model":"jev-latest","answers":{},"usage":{"input_tokens":1,"output_tokens":1}}"#,
    );
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let config_path = scratch.path().join("gateway.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
[providers.claude-max]
base_url = "{}"
protocol = "anthropic-messages"
credential_env = ["GATEWAY_BOUNDARY_CLAUDE_KEY"]

[providers.typesafe]
base_url = "{}"
protocol = "typesafe-systemone"
credential_env = ["GATEWAY_BOUNDARY_TYPESAFE_KEY"]

[accounts.claude-max]
kind = "api-key"
provider = "claude-max"
credential = {{ env = "GATEWAY_BOUNDARY_CLAUDE_KEY" }}

[accounts.typesafe]
kind = "api-key"
provider = "typesafe"
credential = {{ env = "GATEWAY_BOUNDARY_TYPESAFE_KEY" }}
"#,
            claude_fixture.base_url(),
            typesafe_fixture.base_url()
        ),
    )
    .expect("the configuration is written");

    let mut child = ChildGuard::spawn({
        let mut command = gateway(&config_path, scratch.path());
        command
            .args(["serve", "--listen", "127.0.0.1:0"])
            .env("GATEWAY_BOUNDARY_CLAUDE_KEY", "claude-account-key") // glasshouse:not-a-secret
            .env("GATEWAY_BOUNDARY_TYPESAFE_KEY", "typesafe-account-key") // glasshouse:not-a-secret
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    });
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout was piped"));
    let (listening, token) = read_ready_line(&mut stdout);
    let bearer = format!("Bearer {token}");

    let decision_body = r#"{"state":{},"model":"jev-latest","questions":{}}"#;
    let raw = post_with_headers(
        &format!("{listening}/v1/systemone"),
        &bearer,
        &[("x-glasshouse-model", "jev-latest")],
        decision_body,
    );
    assert!(
        raw.starts_with("HTTP/1.1 200 OK"),
        "the decision exchange did not complete: {raw}"
    );

    let forwarded = typesafe_fixture.requests(1);
    assert_eq!(forwarded.len(), 1);
    assert_eq!(forwarded[0].request_line, "POST /v1/systemone HTTP/1.1");
    assert_eq!(forwarded[0].body, decision_body.as_bytes());
    assert!(
        forwarded[0].header("x-glasshouse-model").is_none(),
        "the routing header is hop-by-hop and must never reach the provider: {:?}",
        forwarded[0].headers
    );
    assert!(
        !forwarded[0]
            .header("authorization")
            .unwrap_or_default()
            .contains(&token),
        "the client's own token must never reach typesafe"
    );
    assert!(
        claude_fixture.requests(0).is_empty(),
        "a target only typesafe claims must never open a connection to claude-max"
    );

    let (status, _) = post(
        &format!("{listening}/v1/messages"),
        &bearer,
        r#"{"model":"fixture-model","max_tokens":1,"messages":[{"role":"user","content":"ping"}]}"#,
    );
    assert!(
        status.contains("200"),
        "the messages exchange failed: {status}"
    );
    assert_eq!(
        claude_fixture.requests(1).len(),
        1,
        "the session's own chat traffic still reaches claude-max"
    );
    assert_eq!(
        typesafe_fixture.requests(1).len(),
        1,
        "the messages exchange must not also reach typesafe"
    );

    drop(child.stdin.take().expect("stdin was piped"));
    let status = wait_for_exit(&mut child);
    assert!(status.success(), "clean exit on stdin EOF, got {status:?}");
}

/// A planted `429` with a fixed body and a `retry-after` header reaches the
/// client exactly as the provider sent it. Mirrors
/// `gateway::conformance::a_provider_error_reaches_the_harness_byte_for_byte_while_the_diagnostic_keeps_only_its_status`'s
/// client-facing half.
///
/// **The diagnostic half does not exist at the process level today**, so it
/// is not asserted as a positive claim here: `src/main.rs`'s `serve` never
/// installs a `tracing` subscriber, `gateway::null_sink` drops every
/// exchange observation a standalone gateway produces, and
/// `gateway/ingress.rs`'s `refuse`/`forward` write only to the client
/// socket. There is no stderr line naming this exchange's status to check
/// against. What *is* checked, and does hold: stderr never carries the
/// planted sentinel — the byte-for-byte guarantee's other direction, that
/// nothing echoes the provider's body into a log, holds even though nothing
/// echoes the status either. See this package's report, `packet_errors`.
#[test]
fn a_provider_error_reaches_the_client_byte_for_byte_through_the_binary() {
    const RATE_LIMIT_SENTINEL: &str = "PLANTED-RATE-LIMIT-SENTINEL-8f2c";
    let rate_limit_body = format!(
        r#"{{"type":"error","error":{{"type":"rate_limit_error","message":"{RATE_LIMIT_SENTINEL}"}}}}"#
    );
    let provider = FakeProvider::answering(
        "HTTP/1.1 429 Too Many Requests",
        "content-type: application/json\r\nretry-after: 17\r\n",
        &rate_limit_body,
    );
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let config_path = scratch.path().join("gateway.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
[providers.fixture]
base_url = "{}"
protocol = "anthropic-messages"
credential_env = ["GATEWAY_BOUNDARY_429_KEY"]

[accounts.local]
kind = "api-key"
provider = "fixture"
credential = {{ env = "GATEWAY_BOUNDARY_429_KEY" }}
"#,
            provider.base_url()
        ),
    )
    .expect("the configuration is written");

    let mut child = ChildGuard::spawn({
        let mut command = gateway(&config_path, scratch.path());
        command
            .args(["serve", "--listen", "127.0.0.1:0"])
            .env("GATEWAY_BOUNDARY_429_KEY", "fixture-key") // glasshouse:not-a-secret
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    });
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout was piped"));
    let (listening, token) = read_ready_line(&mut stdout);

    let raw = post_raw(
        &format!("{listening}/v1/messages"),
        &format!("Bearer {token}"),
        r#"{"model":"fixture-model","max_tokens":1,"messages":[{"role":"user","content":"ping"}]}"#,
    );
    assert!(
        raw.starts_with("HTTP/1.1 429 Too Many Requests\r\n"),
        "the provider's status did not reach the client: {raw}"
    );
    assert!(
        raw.contains("retry-after: 17"),
        "the provider's retry-after did not reach the client: {raw}"
    );
    let (_, body) = raw.split_once("\r\n\r\n").expect("a head/body split");
    assert_eq!(
        body,
        rate_limit_body.as_str(),
        "the provider's error body did not reach the client byte-for-byte"
    );

    drop(child.stdin.take().expect("stdin was piped"));
    let status = wait_for_exit(&mut child);
    assert!(status.success(), "clean exit on stdin EOF, got {status:?}");

    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("stderr was piped")
        .read_to_string(&mut stderr)
        .expect("stderr can be drained after exit");
    assert!(
        !stderr.contains(RATE_LIMIT_SENTINEL),
        "the provider's error body must never reach a diagnostic: {stderr}"
    );
}

/// A standalone gateway serves with no ledger, no session store and no
/// evidence file: the data directory it was given, once it has served real
/// exchanges, contains nothing whose name says `ledger` or which is a
/// `.sqlite`/`.db` file. Whatever the directory *does* hold is listed in the
/// assertion, so a new file this test has not been taught about fails loud
/// rather than being silently allowed.
///
/// **It fired on 2026-09-17, exactly as designed, and the expectation that
/// changed is named here rather than loosened.** Until then the directory held
/// *nothing*: `serve` installed `null_sink()`, so the input, output and cached
/// token counts `gateway::usage` reads out of every response were computed and
/// dropped, and `routing-cost --json` — the one command a client asks — had
/// nothing to print. A standalone gateway now keeps those counts in
/// `gateway-turn-costs/<provider>.json`, the same small per-provider JSON cache
/// the quota and context-limit caches already are, written by the **binary**
/// through the sink the library reports to. What has not changed, and is what
/// this test is really for, is below: no database, no session store, and the
/// gateway library itself still keeps nothing and still cannot reach a store.
#[test]
fn standalone_serving_writes_no_ledger() {
    let provider = FakeProvider::start();
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let data_dir = scratch.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("the data directory is created");
    let config_path = scratch.path().join("gateway.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
[providers.fixture]
base_url = "{}"
protocol = "anthropic-messages"
credential_env = ["GATEWAY_BOUNDARY_LEDGER_KEY"]

[accounts.local]
kind = "api-key"
provider = "fixture"
credential = {{ env = "GATEWAY_BOUNDARY_LEDGER_KEY" }}
"#,
            provider.base_url()
        ),
    )
    .expect("the configuration is written");

    let mut child = ChildGuard::spawn({
        let mut command = gateway(&config_path, &data_dir);
        command
            .args(["serve", "--listen", "127.0.0.1:0"])
            .env("GATEWAY_BOUNDARY_LEDGER_KEY", "fixture-key") // glasshouse:not-a-secret
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    });
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout was piped"));
    let (listening, token) = read_ready_line(&mut stdout);

    for _ in 0..3 {
        let (status, _) = post(
            &format!("{listening}/v1/messages"),
            &format!("Bearer {token}"),
            r#"{"model":"fixture-model","max_tokens":1,"messages":[{"role":"user","content":"ping"}]}"#,
        );
        assert!(status.contains("200"), "an exchange failed: {status}");
    }
    assert_eq!(provider.requests(3).len(), 3);

    drop(child.stdin.take().expect("stdin was piped"));
    let status = wait_for_exit(&mut child);
    assert!(status.success(), "clean exit on stdin EOF, got {status:?}");

    let mut entries = Vec::new();
    let mut pending = vec![data_dir.clone()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("the data dir can be listed") {
            let entry = entry.expect("a directory entry reads");
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                entries.push(path);
            }
        }
    }

    // Exactly one kind of file is expected here, and it is named rather than
    // pattern-allowed: the turn-cost cache this gateway keeps so a client can
    // learn what its own turns cost. Anything else — a health cache, a session
    // file, a store nobody meant to add — still fails loud with its name.
    let unexpected: Vec<_> = entries
        .iter()
        .filter(|path| {
            !path
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == "gateway-turn-costs")
        })
        .collect();
    assert!(
        unexpected.is_empty(),
        "only the turn-cost cache is expected after standalone serving; found: {unexpected:?}"
    );
    // The rows are the point: a client asking `routing-cost` must find what
    // the turns it just paid for cost.
    assert_eq!(
        entries.len(),
        1,
        "one provider served, so one provider file: {entries:?}"
    );
    let kept = std::fs::read_to_string(&entries[0]).expect("the turn-cost cache reads");
    // Two rows for three exchanges, and the missing one is the **first**.
    // `gateway::mod`'s accept loop snapshots `routing.assignment()` *before*
    // forwarding — deliberately, so a bind landing mid-flight is not
    // attributed to the exchange in flight — and the first request of a
    // session is the one that binds it, so its snapshot is `None` and
    // `record_routing_observation` returns without producing an observation.
    // Recorded here rather than worked around: it is the producer's
    // behaviour, it predates this cache, and a client reading the *newest*
    // row is unaffected by the oldest one being absent.
    assert_eq!(
        kept.matches("\"observed_at_unix\"").count(),
        2,
        "every exchange after the one that bound the assignment: {kept}"
    );
    assert!(
        kept.contains("\"input_tokens\": 1"),
        "the provider's own counts reached the cache: {kept}"
    );
    let forbidden: Vec<_> = entries
        .iter()
        .filter(|path| {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            name.contains("ledger") || name.ends_with(".sqlite") || name.ends_with(".db")
        })
        .collect();
    assert!(
        forbidden.is_empty(),
        "standalone serving must write no ledger, found: {forbidden:?} among {entries:?}"
    );
}
