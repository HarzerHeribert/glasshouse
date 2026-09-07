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
    use std::io::Read;
    use std::net::TcpStream;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use tempfile::TempDir;

    use super::*;

    struct Fake {
        _temp: TempDir,
        executable: PathBuf,
        capture: PathBuf,
    }

    impl Fake {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let executable = temp.path().join("fake-cliproxyapi");
            let capture = temp.path().join("sanitized-config");
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
while True:
    connection, _ = listener.accept()
    connection.close()
"#,
            )
            .unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
            Self {
                _temp: temp,
                executable,
                capture,
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
        assert_no_secret_in_tree(data.path(), broker.internal_api_key());
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

    fn assert_instances_empty(paths: &RuntimePaths, entitlement: &str) {
        let instances = paths
            .subscription_broker_entitlement_dir(entitlement)
            .join("instances");
        let mut entries = fs::read_dir(instances).unwrap();
        assert!(entries.next().is_none());
    }

    fn assert_no_secret_in_tree(root: &Path, secret: &str) {
        fn visit(path: &Path, secret: &[u8]) {
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    visit(&path, secret);
                } else {
                    let mut bytes = Vec::new();
                    fs::File::open(path)
                        .unwrap()
                        .read_to_end(&mut bytes)
                        .unwrap();
                    assert!(!bytes.windows(secret.len()).any(|window| window == secret));
                }
            }
        }
        visit(root, secret.as_bytes());
    }
}
