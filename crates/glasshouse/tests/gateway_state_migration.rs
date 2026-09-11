//! The 2026-09-11 ruling through the shipped binary: account and broker
//! state moves to the gateway, and the commands that used to write it
//! forward to the gateway instead.
//!
//! Two halves, and both run the real `glasshouse` executable — practice §35.
//! The first drives `migrate-gateway-state` over a legacy tree and reads
//! back every file it touched. The second runs `subscriptions` and
//! `credentials` against a **fake** `inference-gateway` on
//! `INFERENCE_GATEWAY_BIN`, which records its own argv, environment and
//! stdin: that is what proves the forward's shape without depending on which
//! subcommands the real gateway binary has landed yet.
//!
//! No real credential appears anywhere here. The one value piped to
//! `credentials store` is an obviously fabricated string, and the test
//! asserts that Glasshouse's own output never contains it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

#[cfg(unix)]
use std::io::Write as _;

/// A project, a Glasshouse data/config pair, and the gateway locations that
/// derive from them.
///
/// `--data-dir`/`--config-dir` relocate the gateway with Glasshouse
/// (`RuntimePaths::resolve`), so this fixture never reads the developer's
/// own `gateway.toml` — which is the property that makes running this test
/// on a machine with real accounts safe.
struct Fixture {
    base: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new(base: &Path) -> Self {
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        std::fs::create_dir_all(base.join("config")).unwrap();
        std::fs::create_dir_all(base.join("data")).unwrap();
        Fixture {
            base: base.to_path_buf(),
            root,
        }
    }

    fn data(&self) -> PathBuf {
        self.base.join("data")
    }

    fn config_file(&self) -> PathBuf {
        self.base.join("config").join("config.toml")
    }

    fn gateway_file(&self) -> PathBuf {
        self.base.join("config").join("gateway.toml")
    }

    fn gateway_data(&self) -> PathBuf {
        self.data().join("gateway")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.data())
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("spawn glasshouse")
    }
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A `config.toml` in the shape this build refuses to load and the
/// migration exists to read: two `[entitlements.<name>]` tables holding the
/// five account keys, a comment the user wrote, and policy that must
/// survive.
const LEGACY_CONFIG: &str = "\
version = 1

# The note I wrote about this account.
[entitlements.claude-a]
kind = \"claude\"
vendor = \"claude\"
subscription_broker = \"cliproxyapi\"
native_harness = \"claude-code\"
deny_tiers = [\"frontier\"]

[entitlements.spare]
provider = \"openrouter\"
";

/// **The whole migration, over a legacy tree, through the binary.**
///
/// One test rather than six because the properties are one act's
/// consequences and checking them apart would let a run that did half of
/// them pass: the plan, the gateway file, the rewrite, the backup, the five
/// directories, and the second run that finds nothing left.
#[test]
fn migrate_gateway_state_moves_the_accounts_and_directories_and_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    write(&fixture.config_file(), LEGACY_CONFIG);

    // The five directories the gateway owns, each with a file in it so a
    // move can be told from a create.
    for (dir, file) in [
        ("subscription-brokers", "entitlement-6161/auth/account.json"),
        ("tools", "cliproxyapi/current"),
        ("providers", "openrouter.json"),
        ("gateway-quota", "openrouter.json"),
        ("gateway-health", "openrouter.json"),
    ] {
        write(&fixture.data().join(dir).join(file), "{}\n");
    }

    // ---- the plan changes nothing -------------------------------------
    let dry = fixture.run(&["migrate-gateway-state", "--dry-run"]);
    assert!(dry.status.success(), "{dry:?}");
    let plan = String::from_utf8_lossy(&dry.stdout).into_owned();
    assert!(plan.contains("would account `claude-a`"), "{plan}");
    assert!(plan.contains("would account `spare`"), "{plan}");
    assert!(plan.contains("would back up"), "{plan}");
    assert!(plan.contains("would move"), "{plan}");
    assert!(plan.contains("model-catalogues"), "{plan}");
    assert!(plan.contains("dry run: nothing was changed"), "{plan}");
    assert!(
        !fixture.gateway_file().exists(),
        "a dry run must not write the gateway's file"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.config_file()).unwrap(),
        LEGACY_CONFIG,
        "a dry run must not rewrite the configuration"
    );
    assert!(fixture.data().join("providers").is_dir());

    // ---- the migration -------------------------------------------------
    let run = fixture.run(&["migrate-gateway-state"]);
    assert!(run.status.success(), "{run:?}");

    // The gateway's own file holds the five account keys, and nothing else.
    let gateway = std::fs::read_to_string(fixture.gateway_file()).unwrap();
    assert!(gateway.contains("[accounts.claude-a]"), "{gateway}");
    assert!(gateway.contains("kind = \"claude\""), "{gateway}");
    assert!(gateway.contains("vendor = \"claude\""), "{gateway}");
    assert!(
        gateway.contains("subscription_broker = \"cliproxyapi\""),
        "{gateway}"
    );
    assert!(gateway.contains("[accounts.spare]"), "{gateway}");
    assert!(gateway.contains("provider = \"openrouter\""), "{gateway}");
    assert!(
        !gateway.contains("native_harness") && !gateway.contains("deny_tiers"),
        "policy is Glasshouse's and must not cross:\n{gateway}"
    );

    // Glasshouse's own file keeps its comment and its policy, and has lost
    // exactly the moved keys — and the table that held nothing else.
    let rewritten = std::fs::read_to_string(fixture.config_file()).unwrap();
    assert!(
        rewritten.contains("# The note I wrote about this account."),
        "the comment must survive a format-preserving rewrite:\n{rewritten}"
    );
    assert!(
        rewritten.contains("native_harness = \"claude-code\""),
        "{rewritten}"
    );
    assert!(
        rewritten.contains("deny_tiers = [\"frontier\"]"),
        "{rewritten}"
    );
    for moved in ["kind =", "vendor =", "subscription_broker =", "provider ="] {
        assert!(
            !rewritten.contains(moved),
            "`{moved}` survived:\n{rewritten}"
        );
    }
    assert!(
        !rewritten.contains("[entitlements.spare]"),
        "a table with nothing left in it goes:\n{rewritten}"
    );

    // The backup holds what the file said before.
    let backups: Vec<PathBuf> = std::fs::read_dir(fixture.base.join("config"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(".before-gateway-store-"))
        })
        .collect();
    assert_eq!(backups.len(), 1, "one backup, named for what it precedes");
    assert_eq!(
        std::fs::read_to_string(&backups[0]).unwrap(),
        LEGACY_CONFIG,
        "the backup is the file as it was"
    );

    // The five directories are under the gateway's root, `providers/`
    // renamed to the name the gateway uses for it.
    for name in [
        "subscription-brokers",
        "tools",
        "model-catalogues",
        "gateway-quota",
        "gateway-health",
    ] {
        assert!(
            fixture.gateway_data().join(name).is_dir(),
            "{name} did not arrive under {}",
            fixture.gateway_data().display()
        );
    }
    assert!(
        fixture
            .gateway_data()
            .join("subscription-brokers/entitlement-6161/auth/account.json")
            .is_file(),
        "a move carries the contents"
    );
    for name in [
        "subscription-brokers",
        "tools",
        "providers",
        "gateway-quota",
        "gateway-health",
    ] {
        assert!(
            !fixture.data().join(name).exists(),
            "{name} is still in Glasshouse's own data directory"
        );
    }

    // ---- and again ------------------------------------------------------
    let again = fixture.run(&["migrate-gateway-state"]);
    assert!(again.status.success(), "{again:?}");
    let second = String::from_utf8_lossy(&again.stdout).into_owned();
    assert!(second.contains("nothing to move"), "{second}");
    assert_eq!(
        std::fs::read_to_string(fixture.config_file()).unwrap(),
        rewritten,
        "a second run changes nothing"
    );

    // The migrated tree is one this build can now load: `glasshouse
    // entitlements` resolves the account from the gateway and the rules
    // from Glasshouse.
    let listed = fixture.run(&["entitlements"]);
    assert!(listed.status.success(), "{listed:?}");
    let report = String::from_utf8_lossy(&listed.stdout).into_owned();
    assert!(report.contains("claude-a"), "{report}");
}

/// **A configuration still holding an account key is refused by name, and
/// the refusal names the command that fixes it.** Through the binary,
/// because the point is what a person sees when they run one.
#[test]
fn a_configuration_with_a_legacy_account_key_is_refused_with_the_migration_named() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    write(&fixture.config_file(), LEGACY_CONFIG);

    let refused = fixture.run(&["entitlements"]);
    assert!(!refused.status.success(), "{refused:?}");
    let stderr = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(
        stderr.contains("glasshouse migrate-gateway-state"),
        "the refusal must name the command that moves it: {stderr}"
    );
    assert!(
        stderr.contains("gateway.toml"),
        "and where the key belongs: {stderr}"
    );

    // And `doctor` reports it rather than refusing, because `doctor` is what
    // a person runs when something is wrong.
    let doctor = fixture.run(&["doctor"]);
    assert!(doctor.status.success(), "{doctor:?}");
    let report = String::from_utf8_lossy(&doctor.stdout).into_owned();
    assert!(report.contains("Gateway-owned state"), "{report}");
    assert!(report.contains("still states"), "{report}");
    assert!(
        report.contains("glasshouse migrate-gateway-state"),
        "{report}"
    );
}

// ===========================================================================
// The forwards
// ===========================================================================

/// A stand-in `inference-gateway` that records its argv, its stdin and the
/// two environment variables the forward sets, then exits with `code`.
#[cfg(unix)]
fn fake_gateway(dir: &Path, record: &Path, code: i32) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join("inference-gateway");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n\
             {{ printf 'argv:%s\\n' \"$*\"; \
                printf 'config:%s\\n' \"$INFERENCE_GATEWAY_CONFIG\"; \
                printf 'data:%s\\n' \"$INFERENCE_GATEWAY_DATA_DIR\"; \
                printf 'stdin:'; cat; printf '\\n'; }} > '{}'\n\
             exit {code}\n",
            record.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// **`subscriptions connect` is the gateway's command, run by the gateway.**
/// The argv shape, the store the child is pointed at, the one stderr line
/// that says what happened, and the exit status coming back are all one
/// forward's observable behaviour.
#[cfg(unix)]
#[test]
fn subscriptions_connect_forwards_to_the_gateway_and_propagates_its_status() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let record = tmp.path().join("record");
    let fake = fake_gateway(tmp.path(), &record, 0);

    let output = fixture
        .command(&[
            "subscriptions",
            "connect",
            "anthropic",
            "--entitlement",
            "claude-a",
            "--json",
        ])
        .env("INFERENCE_GATEWAY_BIN", &fake)
        .output()
        .expect("spawn glasshouse");
    assert!(output.status.success(), "{output:?}");

    let recorded = std::fs::read_to_string(&record).unwrap();
    assert!(
        recorded.contains("argv:subscriptions connect anthropic --entitlement claude-a --json"),
        "{recorded}"
    );
    assert!(
        recorded.contains(&format!("config:{}", fixture.gateway_file().display())),
        "the child is pointed at the same store this process resolved: {recorded}"
    );
    assert!(
        recorded.contains(&format!("data:{}", fixture.gateway_data().display())),
        "{recorded}"
    );

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        stderr.contains("is the gateway's; forwarding to"),
        "one line saying it forwarded and to what: {stderr}"
    );
    assert!(stderr.contains("subscriptions connect"), "{stderr}");

    // A failure comes back as this process's own.
    let failing = fake_gateway(tmp.path(), &record, 7);
    let output = fixture
        .command(&[
            "subscriptions",
            "logout",
            "anthropic",
            "--entitlement",
            "claude-a",
        ])
        .env("INFERENCE_GATEWAY_BIN", &failing)
        .output()
        .expect("spawn glasshouse");
    assert_eq!(output.status.code(), Some(7), "{output:?}");
    let recorded = std::fs::read_to_string(&record).unwrap();
    assert!(
        recorded.contains("argv:subscriptions logout anthropic --entitlement claude-a"),
        "{recorded}"
    );
}

/// **`credentials store` hands the key to the gateway without reading it.**
/// The value is piped to *this* process's stdin, which the forward inherits;
/// the recording proves the gateway received it, and Glasshouse's own output
/// is asserted never to contain it.
#[cfg(unix)]
#[test]
fn credentials_store_forwards_the_variable_and_never_sees_the_value() {
    const PLANTED: &str = "fabricated-not-a-real-key-0123456789abcdef";

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let record = tmp.path().join("record");
    let fake = fake_gateway(tmp.path(), &record, 0);

    let mut child = fixture
        .command(&["credentials", "store", "ALPHA_KEY", "--stdin"])
        .env("INFERENCE_GATEWAY_BIN", &fake)
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn glasshouse");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(PLANTED.as_bytes())
        .unwrap();
    let output = child.wait_with_output().expect("wait for glasshouse");
    assert!(output.status.success(), "{output:?}");

    let recorded = std::fs::read_to_string(&record).unwrap();
    assert!(
        recorded.contains("argv:credentials set --variable ALPHA_KEY"),
        "{recorded}"
    );
    assert!(
        recorded.contains(&format!("stdin:{PLANTED}")),
        "the key reaches the gateway through the inherited stdin: {recorded}"
    );

    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !said.contains(PLANTED),
        "Glasshouse printed the value it forwarded: {said}"
    );

    // A value on the command line is still refused here, before anything is
    // run — the one check a forward cannot do later.
    let refused = fixture
        .command(&["credentials", "store", "ALPHA_KEY", PLANTED])
        .env("INFERENCE_GATEWAY_BIN", &fake)
        .output()
        .expect("spawn glasshouse");
    assert!(!refused.status.success(), "{refused:?}");
    let stderr = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(stderr.contains("never an argument"), "{stderr}");
    assert!(!stderr.contains(PLANTED), "{stderr}");
}
