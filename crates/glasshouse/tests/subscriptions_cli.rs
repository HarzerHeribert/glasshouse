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
        // The account is the gateway's since the 2026-09-11 ruling; the
        // native route is Glasshouse's policy about it.
        fs::write(
            config.join("config.toml"),
            "[entitlements.personal]\nnative_harness = \"claude-code\"\n",
        )
        .unwrap();
        fs::write(
            config.join("gateway.toml"),
            "[accounts.personal]\nkind = \"claude\"\nvendor = \"claude\"\n\
             subscription_broker = \"cliproxyapi\"\n",
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
        self.run_with_gateway(args, broker, None)
    }

    /// `gateway` names the `inference-gateway` a forward should run — always
    /// a fake here, because a forward with none set would find whichever one
    /// is installed on the machine running the test.
    fn run_with_gateway(
        &self,
        args: &[&str],
        broker: Option<&Path>,
        gateway: Option<&Path>,
    ) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        if let Some(gateway) = gateway {
            command.env("INFERENCE_GATEWAY_BIN", gateway);
        }
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

/// **`subscriptions login` is a forward, and `status` is still a read.**
///
/// The login half asserts the shape the gateway is handed and that the child
/// is pointed at the same store this process resolved. The status half plants
/// an auth directory by hand — the layout is the gateway's and Glasshouse
/// only looks at it — and proves the read reports presence without ever
/// reading a token's contents.
#[test]
fn login_forwards_to_the_gateway_and_status_reads_presence_without_token_contents() {
    let fixture = Fixture::new();
    let record = fixture._temp.path().join("forwarded");
    let fake_gateway = fixture._temp.path().join("inference-gateway");
    fs::write(
        &fake_gateway,
        format!(
            "#!/bin/sh\n{{ printf 'argv:%s\\n' \"$*\"; printf 'config:%s\\n' \
             \"$INFERENCE_GATEWAY_CONFIG\"; }} > '{}'\nexit 0\n",
            record.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_gateway, fs::Permissions::from_mode(0o700)).unwrap();

    let output = fixture.run_with_gateway(
        &[
            "subscriptions",
            "login",
            "anthropic",
            "--entitlement",
            "personal",
        ],
        None,
        Some(&fake_gateway),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let forwarded = fs::read_to_string(&record).unwrap();
    assert!(
        forwarded.contains("argv:subscriptions login anthropic --entitlement personal"),
        "{forwarded}"
    );
    assert!(
        forwarded.contains(&format!(
            "config:{}",
            fixture.config.join("gateway.toml").display()
        )),
        "the child reads the store this process resolved: {forwarded}"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("is the gateway's; forwarding to"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The read half: an auth directory laid out the way the broker lays it
    // out, and a status that reports presence and nothing inside it.
    let auth = fixture.auth_dir();
    fs::create_dir_all(&auth).unwrap();
    fs::write(auth.join("account.json"), b"{\"account\":\"fixture\"}").unwrap();
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

/// **`logout` is the gateway's too.** Removing an auth directory is writing
/// broker state, and Glasshouse neither owns nor manages it since the
/// 2026-09-11 ruling — so what this asserts is the forward, and that
/// Glasshouse touched nothing on its way there.
#[test]
fn logout_forwards_to_the_gateway_and_removes_nothing_itself() {
    let fixture = Fixture::new();
    let selected = fixture.auth_dir();
    fs::create_dir_all(&selected).unwrap();
    fs::write(selected.join("opaque.json"), b"selected-secret").unwrap();

    let record = fixture._temp.path().join("forwarded");
    let fake_gateway = fixture._temp.path().join("inference-gateway");
    fs::write(
        &fake_gateway,
        format!(
            "#!/bin/sh\nprintf 'argv:%s\\n' \"$*\" > '{}'\nexit 0\n",
            record.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_gateway, fs::Permissions::from_mode(0o700)).unwrap();

    let output = fixture.run_with_gateway(
        &[
            "subscriptions",
            "logout",
            "anthropic",
            "--entitlement",
            "personal",
        ],
        None,
        Some(&fake_gateway),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fs::read_to_string(&record)
            .unwrap()
            .contains("argv:subscriptions logout anthropic --entitlement personal"),
        "the gateway is the one asked to disconnect"
    );
    assert_eq!(
        fs::read(selected.join("opaque.json")).unwrap(),
        b"selected-secret",
        "Glasshouse must not remove broker state itself"
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
