use super::*;

#[test]
fn absent_executable_refuses_with_names_and_no_managed_path() {
    let temp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths::new(temp.path().join("private-data"), temp.path().join("config"));
    let error = discover_executable(&paths, None).unwrap_err().to_string();
    assert!(error.contains("CLIProxyAPI"));
    assert!(error.contains("managed tools directory"));
    assert!(!error.contains(&temp.path().to_string_lossy().to_string()));

    let missing = temp.path().join("very-private").join("custom-proxy");
    let error = discover_executable(&paths, Some(missing.into_os_string()))
        .unwrap_err()
        .to_string();
    assert!(error.contains(ENV_CLIPROXYAPI_BIN));
    assert!(error.contains("custom-proxy"));
    assert!(!error.contains("very-private"));
}

#[cfg(unix)]
mod process {
    use std::ffi::OsString;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::Duration;

    use tempfile::TempDir;

    use super::*;

    struct Fake {
        _temp: TempDir,
        executable: PathBuf,
        capture: PathBuf,
        request_capture: PathBuf,
        request_count: PathBuf,
    }

    impl Fake {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let executable = temp.path().join("fake-cliproxyapi");
            let capture = temp.path().join("sanitized-config");
            let request_capture = temp.path().join("request");
            let request_count = temp.path().join("request-count");
            fs::write(
                &executable,
                r#"#!/usr/bin/env python3
import os, socket, sys, time

config_path = sys.argv[sys.argv.index("-config") + 1]
with open(config_path, "r", encoding="utf-8") as handle:
    lines = handle.read().splitlines()

port = int(next(line.split(":", 1)[1].strip() for line in lines if line.startswith("port:")))
safe = []
for line in lines:
    if line.startswith("api-keys:"):
        safe.append("api-keys: [configured-and-redacted]")
    else:
        safe.append(line)
with open(os.environ["FAKE_CAPTURE"], "w", encoding="utf-8") as handle:
    handle.write("\n".join(safe))

mode = os.environ.get("FAKE_MODE", "ready")
if mode == "exit":
    sys.exit(23)
if mode == "timeout":
    time.sleep(60)
    sys.exit(0)

listener = socket.socket()
listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind(("127.0.0.1", port))
listener.listen()
if mode == "rogue":
    pid = os.fork()
    if pid > 0:
        time.sleep(0.12)
        sys.exit(24)
    listener.settimeout(0.1)
    rogue_deadline = time.time() + 0.45
while True:
    try:
        connection, _ = listener.accept()
    except socket.timeout:
        if mode == "rogue" and time.time() >= rogue_deadline:
            sys.exit(0)
        continue
    request = b""
    while b"\r\n\r\n" not in request:
        chunk = connection.recv(4096)
        if not chunk:
            break
        request += chunk
    head, _, body = request.partition(b"\r\n\r\n")
    content_length = 0
    for line in head.split(b"\r\n"):
        if line.lower().startswith(b"content-length:"):
            content_length = int(line.split(b":", 1)[1].strip())
    while len(body) < content_length:
        chunk = connection.recv(4096)
        if not chunk:
            break
        body += chunk
    request = head + b"\r\n\r\n" + body
    if b"GET /v1/models" in request and b"Authorization: Bearer " in request:
        payload = b'{"data":[]}' if mode == "empty" else b'{"data":[{"id":"ready"}]}'
        connection.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: " + str(len(payload)).encode() + b"\r\nConnection: close\r\n\r\n" + payload)
    elif b"POST /v1/messages" in request:
        with open(os.environ["FAKE_REQUEST"], "wb") as handle:
            handle.write(request)
        count_path = os.environ["FAKE_REQUEST_COUNT"]
        count = int(open(count_path).read()) if os.path.exists(count_path) else 0
        with open(count_path, "w") as handle:
            handle.write(str(count + 1))
        if b'"stream":true' in body:
            payload = b'event: message_stop\ndata: {"type":"message_stop"}\n\n'
            connection.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: " + str(len(payload)).encode() + b"\r\nConnection: close\r\n\r\n" + payload)
        else:
            payload = b'{"content":[{"type":"text","text":"exact"}]}'
            connection.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: " + str(len(payload)).encode() + b"\r\nConnection: close\r\n\r\n" + payload)
    else:
        connection.sendall(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    connection.close()
"#,
            )
            .unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
            Self {
                _temp: temp,
                executable,
                capture,
                request_capture,
                request_count,
            }
        }

        fn env(&self, mode: &str) -> Vec<(OsString, OsString)> {
            vec![
                (
                    OsString::from("PATH"),
                    std::env::var_os("PATH").unwrap_or_default(),
                ),
                (
                    OsString::from("FAKE_CAPTURE"),
                    self.capture.clone().into_os_string(),
                ),
                (OsString::from("FAKE_MODE"), OsString::from(mode)),
                (
                    OsString::from("FAKE_REQUEST"),
                    self.request_capture.clone().into_os_string(),
                ),
                (
                    OsString::from("FAKE_REQUEST_COUNT"),
                    self.request_count.clone().into_os_string(),
                ),
            ]
        }
    }

    fn start(
        data: &TempDir,
        fake: &Fake,
        entitlement: &str,
        mode: &str,
        timeout: Duration,
    ) -> Result<RunningSubscriptionBroker> {
        let paths = RuntimePaths::new(data.path().join("data"), data.path().join("config"));
        RunningSubscriptionBroker::start_with(
            &paths,
            entitlement,
            &fake.executable,
            timeout,
            &fake.env(mode),
        )
    }

    #[test]
    fn ready_sidecar_uses_the_private_single_account_config() {
        let data = tempfile::tempdir().unwrap();
        let fake = Fake::new();
        let broker = start(
            &data,
            &fake,
            "google-pro-a",
            "ready",
            Duration::from_secs(3),
        )
        .expect("fake sidecar becomes ready");

        let config = fs::read_to_string(&fake.capture).unwrap();
        for nested in [
            "tls:\n  enable: false",
            "remote-management:\n  allow-remote: false\n  secret-key: \"\"",
            "pprof:\n  enable: false\n  addr: \"127.0.0.1:0\"",
            "plugins:\n  enabled: false",
            "quota-exceeded:\n  switch-project: false",
            "routing:\n  strategy: \"fill-first\"",
            "streaming:\n  keepalive-seconds: 0\n  bootstrap-retries: 0",
        ] {
            assert!(config.contains(nested), "malformed nested YAML: {nested:?}");
        }
        for required in [
            "host: \"127.0.0.1\"",
            "secret-key: \"\"",
            "disable-control-panel: true",
            "disable-auto-update-panel: true",
            "api-keys: [configured-and-redacted]",
            "enabled: false",
            "commercial-mode: true",
            "request-log: false",
            "logging-to-file: false",
            "usage-statistics-enabled: false",
            "request-retry: 0",
            "max-retry-credentials: 1",
            "max-retry-interval: 0",
            "disable-claude-cloak-mode: true",
            "switch-project: false",
            "switch-preview-model: false",
            "antigravity-credits: false",
            "bootstrap-retries: 0",
        ] {
            assert!(
                config.contains(required),
                "missing config invariant {required:?}"
            );
        }
        assert!(broker.base_url().starts_with("http://127.0.0.1:"));
        assert_eq!(
            broker.credential_id().reference(),
            &SecretRef::OsCredential {
                service: CREDENTIAL_SERVICE.to_owned(),
                account: "google-pro-a".to_owned(),
            }
        );

        let auth_mode = fs::metadata(broker.auth_dir())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(auth_mode, 0o700);
        let at_rest = fs::read_to_string(&broker.config_path).unwrap();
        assert!(at_rest.contains(broker.internal_api_key()));
        assert_eq!(
            fs::metadata(&broker.config_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let rendered = format!("{broker:?}");
        assert!(rendered.contains(REDACTED));
        assert!(!rendered.contains(broker.internal_api_key()));
    }

    #[test]
    fn readiness_timeout_and_early_exit_kill_and_clean_the_instance() {
        let data = tempfile::tempdir().unwrap();
        let fake = Fake::new();
        let paths = RuntimePaths::new(data.path().join("data"), data.path().join("config"));

        let timeout = RunningSubscriptionBroker::start_with(
            &paths,
            "timeout-account",
            &fake.executable,
            Duration::from_millis(120),
            &fake.env("timeout"),
        )
        .unwrap_err()
        .to_string();
        assert!(timeout.contains("bounded startup timeout"));
        assert_instances_empty(&paths, "timeout-account");

        let empty = RunningSubscriptionBroker::start_with(
            &paths,
            "empty-account",
            &fake.executable,
            Duration::from_millis(350),
            &fake.env("empty"),
        )
        .unwrap_err()
        .to_string();
        assert!(empty.contains("bounded startup timeout"), "{empty}");
        assert_instances_empty(&paths, "empty-account");

        let exited = RunningSubscriptionBroker::start_with(
            &paths,
            "exit-account",
            &fake.executable,
            Duration::from_secs(2),
            &fake.env("exit"),
        )
        .unwrap_err()
        .to_string();
        assert!(exited.contains("exited before readiness"));
        assert!(exited.contains("23"));
        assert_instances_empty(&paths, "exit-account");

        let rogue = RunningSubscriptionBroker::start_with(
            &paths,
            "rogue-account",
            &fake.executable,
            Duration::from_secs(2),
            &fake.env("rogue"),
        )
        .unwrap_err()
        .to_string();
        assert!(rogue.contains("exited before readiness"), "{rogue}");
        std::thread::sleep(Duration::from_millis(300));
        assert_instances_empty(&paths, "rogue-account");
    }

    #[test]
    fn drop_stops_the_listener_and_reaps_the_sidecar() {
        let data = tempfile::tempdir().unwrap();
        let fake = Fake::new();
        let broker = start(
            &data,
            &fake,
            "cleanup-account",
            "ready",
            Duration::from_secs(3),
        )
        .unwrap();
        let address = broker.base_url().trim_start_matches("http://").to_owned();
        assert!(TcpStream::connect(&address).is_ok());
        drop(broker);
        assert!(TcpStream::connect(&address).is_err());

        let paths = RuntimePaths::new(data.path().join("data"), data.path().join("config"));
        assert_instances_empty(&paths, "cleanup-account");
    }

    #[test]
    fn two_entitlements_have_distinct_auth_roots_and_processes() {
        let data = tempfile::tempdir().unwrap();
        let first_fake = Fake::new();
        let second_fake = Fake::new();
        let first = start(
            &data,
            &first_fake,
            "google-a",
            "ready",
            Duration::from_secs(3),
        )
        .unwrap();
        let second = start(
            &data,
            &second_fake,
            "google-b",
            "ready",
            Duration::from_secs(3),
        )
        .unwrap();

        assert_ne!(first.auth_dir(), second.auth_dir());
        assert_ne!(first.base_url(), second.base_url());
        assert_ne!(first.internal_api_key(), second.internal_api_key());
        assert!(first.auth_dir().is_dir());
        assert!(second.auth_dir().is_dir());
    }

    #[test]
    fn outer_gateway_preserves_anthropic_tool_bytes_and_exact_streaming_responses() {
        let data = tempfile::tempdir().unwrap();
        let fake = Fake::new();
        let broker = start(
            &data,
            &fake,
            "exact-account",
            "ready",
            Duration::from_secs(3),
        )
        .unwrap();
        let inner_key = broker.internal_api_key().to_owned();
        let base = broker.base_url().to_owned();
        let backend = crate::gateway::upstream::UpstreamBackend::from_subscription_broker(
            vec![crate::gateway::Route::new(
                "anthropic-messages".to_owned(),
                &["/messages"],
                &base,
            )],
            broker,
        )
        .unwrap();
        let upstream = crate::gateway::Upstream::with_failover(vec![backend]).unwrap();
        let gateway = crate::gateway::Gateway::start(upstream).unwrap();
        let body = r#"{"model":"claude-sonnet-4-5","system":"system-order","messages":[{"role":"user","content":[{"type":"text","text":"first"}]},{"role":"assistant","content":[{"type":"tool_use","id":"tool_1","name":"write","input":{"code":"fn main() { println!(\"ok\"); }"}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool_1","content":"done"}]}],"stream":false}"#;
        let mut stream = TcpStream::connect(gateway.address()).unwrap();
        let request = format!(
            "POST /v1/messages HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            gateway.token().expose(),
            body.len(),
            body
        );
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert_eq!(
            response.split_once("\r\n\r\n").unwrap().1,
            r#"{"content":[{"type":"text","text":"exact"}]}"#
        );
        let received = fs::read_to_string(&fake.request_capture).unwrap();
        assert!(
            received
                .to_ascii_lowercase()
                .contains(&format!("authorization: bearer {inner_key}"))
        );
        assert!(!received.contains(gateway.token().expose()));
        assert!(received.contains(body));
        let streaming_body = r#"{"model":"claude-sonnet-4-5","system":"system-order","messages":[{"role":"user","content":"stream"}],"stream":true}"#;
        let mut stream = TcpStream::connect(gateway.address()).unwrap();
        let request = format!(
            "POST /v1/messages HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            gateway.token().expose(),
            streaming_body.len(),
            streaming_body
        );
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert_eq!(
            response.split_once("\r\n\r\n").unwrap().1,
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
        );
        assert_eq!(fs::read_to_string(&fake.request_count).unwrap(), "2");
        let address = base.trim_start_matches("http://").to_owned();
        drop(gateway);
        assert!(TcpStream::connect(address).is_err());
    }

    fn assert_instances_empty(paths: &RuntimePaths, entitlement: &str) {
        let instances = paths
            .subscription_broker_entitlement_dir(entitlement)
            .join("instances");
        let mut entries = fs::read_dir(instances).unwrap();
        assert!(entries.next().is_none());
    }
}
