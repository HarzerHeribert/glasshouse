#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sha2::{Digest, Sha256};

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
            "[entitlements.personal]\nkind = \"claude\"\nvendor = \"claude\"\nnative_harness = \"claude-code\"\n",
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
        let digest = hex::encode(Sha256::digest(b"personal"));
        self.data
            .join("subscription-broker/accounts/anthropic")
            .join(digest)
            .join("auth")
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
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf 'token-from-child-stdout\\n'\n",
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
    assert!(!String::from_utf8_lossy(&output.stdout).contains("token-from-child"));
    let argv = fs::read_to_string(record).unwrap();
    assert!(argv.starts_with("-config\n"), "{argv}");
    assert!(argv.ends_with("\n-claude-login\n"), "{argv}");

    let auth = fixture.auth_dir();
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
    let other = fixture
        .data
        .join("subscription-broker/accounts/anthropic/other/auth");
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
