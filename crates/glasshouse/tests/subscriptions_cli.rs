#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    data: PathBuf,
    config: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        let data = temp.path().join("data");
        let config = temp.path().join("config");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("config.toml"),
            "[entitlements.personal]\nkind = \"claude\"\nvendor = \"claude\"\nnative_harness = \"claude-code\"\nsubscription_broker = \"cliproxyapi\"\n",
        )
        .unwrap();
        Self {
            _temp: temp,
            root,
            data,
            config,
        }
    }

    fn run(&self, args: &[&str], broker: Option<&Path>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command.args([
            "--scope",
            self.root.to_str().unwrap(),
            "--data-dir",
            self.data.to_str().unwrap(),
            "--config-dir",
            self.config.to_str().unwrap(),
        ]);
        command.args(args);
        if let Some(broker) = broker {
            command.env("GLASSHOUSE_CLIPROXYAPI_BIN", broker);
        }
        command.output().unwrap()
    }

    fn auth_dir(&self) -> PathBuf {
        glasshouse::RuntimePaths::new(&self.data, &self.config)
            .subscription_broker_auth_dir("personal")
    }
}

#[test]
fn login_dispatch_and_status_expose_no_child_output_or_token_contents() {
    let fixture = Fixture::new();
    let record = fixture._temp.path().join("argv");
    let fake = fixture._temp.path().join("cliproxyapi");
    fs::write(
        &fake,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nauth_dir=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[\"auth-dir\"])' \"$2\")\nmkdir -p \"$auth_dir\"\nprintf '{{\"account\":\"fixture\"}}' > \"$auth_dir/account.json\"\n",
            record.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();

    let output = fixture.run(
        &[
            "subscriptions",
            "login",
            "anthropic",
            "--entitlement",
            "personal",
        ],
        Some(&fake),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "anthropic\tpersonal\tpresent\n"
    );
    let argv = fs::read_to_string(record).unwrap();
    assert!(argv.starts_with("-config\n"), "{argv}");
    assert!(argv.ends_with("\n-claude-login\n"), "{argv}");

    let auth = fixture.auth_dir();
    assert!(auth.join("account.json").is_file());
    fs::write(auth.join("opaque.json"), b"token-contents-must-not-be-read").unwrap();
    let status = fixture.run(&["subscriptions", "status"], None);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let report = String::from_utf8_lossy(&status.stdout);
    assert_eq!(
        report,
        "provider\tentitlement\tstatus\nanthropic\tpersonal\tpresent\n"
    );
    assert!(!report.contains("token-contents"));
}

#[test]
fn logout_removes_only_the_selected_accounts_auth_directory() {
    let fixture = Fixture::new();
    let selected = fixture.auth_dir();
    fs::create_dir_all(&selected).unwrap();
    fs::write(selected.join("opaque.json"), b"selected-secret").unwrap();
    let other = glasshouse::RuntimePaths::new(&fixture.data, &fixture.config)
        .subscription_broker_auth_dir("other");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("opaque.json"), b"other-secret").unwrap();

    let output = fixture.run(
        &[
            "subscriptions",
            "logout",
            "anthropic",
            "--entitlement",
            "personal",
        ],
        None,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "anthropic\tpersonal\tabsent\n"
    );
    assert!(selected.is_dir());
    assert_eq!(fs::read_dir(selected).unwrap().count(), 0);
    assert_eq!(
        fs::read(other.join("opaque.json")).unwrap(),
        b"other-secret"
    );
}

#[test]
fn entitlement_refresh_reads_the_connected_brokers_live_model_catalogue() {
    let fixture = Fixture::new();
    let auth = fixture.auth_dir();
    fs::create_dir_all(&auth).unwrap();
    fs::write(auth.join("account.json"), b"opaque-auth-material").unwrap();

    let fake = fixture._temp.path().join("catalogue-cliproxyapi");
    fs::write(
        &fake,
        r#"#!/usr/bin/python3
import socket, sys

config_path = sys.argv[sys.argv.index("-config") + 1]
with open(config_path, "r", encoding="utf-8") as handle:
    lines = handle.read().splitlines()
port = int(next(line.split(":", 1)[1].strip() for line in lines if line.startswith("port:")))
listener = socket.socket()
listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind(("127.0.0.1", port))
listener.listen()
payload = b'{"data":[{"id":"claude-live-b"},{"id":"claude-live-a"}]}'
while True:
    connection, _ = listener.accept()
    request = b""
    while b"\r\n\r\n" not in request:
        chunk = connection.recv(4096)
        if not chunk:
            break
        request += chunk
    if b"GET /v1/models" in request and b"authorization: bearer " in request.lower():
        connection.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: " + str(len(payload)).encode() + b"\r\nConnection: close\r\n\r\n" + payload)
    else:
        connection.sendall(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    connection.close()
"#,
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();

    let output = fixture.run(&["entitlements", "--json", "--refresh"], Some(&fake));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["accounts"][0]["provider"], "claude");
    assert_eq!(body["accounts"][0]["scope"], "account-declared");
    assert_eq!(
        body["accounts"][0]["models"],
        serde_json::json!(["claude-live-a", "claude-live-b"])
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("opaque-auth-material"));
    assert!(!text.to_ascii_lowercase().contains("authorization"));
}

#[test]
fn entitlement_refresh_refuses_a_symlinked_broker_auth_directory() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let outside = fixture._temp.path().join("outside-auth");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("opaque.json"), b"outside-secret").unwrap();
    let auth = fixture.auth_dir();
    fs::create_dir_all(auth.parent().unwrap()).unwrap();
    symlink(&outside, &auth).unwrap();

    let output = fixture.run(&["entitlements", "--json", "--refresh"], None);
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("not a private directory"), "{said}");
    assert!(!said.contains("outside-secret"));
    assert_eq!(
        fs::read(outside.join("opaque.json")).unwrap(),
        b"outside-secret"
    );
}
