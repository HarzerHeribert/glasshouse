//! `glasshouse credentials`, driven as a user drives it: the shipped binary,
//! a real value on standard input, and the assertion that the value comes
//! back out of **nothing** — not stdout, not stderr, not an error.
//!
//! Since the 2026-09-11 ruling a provider key is the gateway's, so `store`
//! and `remove` are forwards. Every fixture here therefore points
//! `INFERENCE_GATEWAY_BIN` at a **fake** gateway that records what it was
//! given: without that, a forward would find whichever `inference-gateway`
//! happens to be installed on the machine running the test and write to the
//! developer's own store. The two checks that stayed Glasshouse's — a value
//! on the command line, and a variable name no shell could set — are
//! asserted here; what the gateway does with a key is the gateway's own
//! test.
//!
//! The planted values here are placeholders. They are shaped like keys so
//! that a `contains` assertion is meaningful, and every one of them is
//! marked `glasshouse:not-a-secret` for the repository's key guard.

use std::io::Write as _;
use std::process::{Command, Output, Stdio};

/// A value shaped like a provider key, and belonging to nobody.
const PLANTED: &str = "gsk-planted-by-a-test-never-a-real-key-4a7f2b"; // glasshouse:not-a-secret

/// A temporary project, data directory and configuration directory — so no
/// test here can read or write the developer's own Glasshouse state.
struct Fixture {
    base: tempfile::TempDir,
    root: tempfile::TempDir,
    gateway: std::path::PathBuf,
    record: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("a temporary project root");
        std::fs::create_dir_all(root.path().join(".git")).expect("a project marker");
        let base = tempfile::tempdir().expect("a temporary base");
        let record = base.path().join("forwarded");
        let gateway = install_fake_gateway(base.path(), &record);
        Self {
            base,
            root,
            gateway,
            record,
        }
    }

    /// What the fake gateway was asked to do, or the empty string when it
    /// was never run.
    fn forwarded(&self) -> String {
        std::fs::read_to_string(&self.record).unwrap_or_default()
    }

    /// Configure one provider naming `var` as its credential variable, which
    /// is what puts `var` in front of `doctor` and `credentials list`.
    fn with_provider_credential(self, var: &str) -> Self {
        let config = self.base.path().join("config");
        std::fs::create_dir_all(&config).expect("a config directory");
        std::fs::write(
            config.join("config.toml"),
            format!(
                "version = 1\n\n[providers.test-router]\ntemplate = \"openrouter\"\n\
                 credential_env = [\"{var}\"]\n"
            ),
        )
        .expect("a configuration file");
        self
    }

    /// The shipped binary, pointed at this project and nothing else.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command
            .arg("--scope")
            .arg(self.root.path())
            .arg("--data-dir")
            .arg(self.base.path().join("data"))
            .arg("--config-dir")
            .arg(self.base.path().join("config"))
            .env("INFERENCE_GATEWAY_BIN", &self.gateway)
            .args(args);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args)
            .stdin(Stdio::null())
            .output()
            .expect("the glasshouse binary must be runnable")
    }

    /// Run with one line on standard input, as a user piping a key does.
    fn run_with_stdin(&self, args: &[&str], input: &str) -> Output {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the glasshouse binary must be runnable");
        child
            .stdin
            .as_mut()
            .expect("a piped standard input")
            .write_all(input.as_bytes())
            .expect("the value must reach the child");
        child.wait_with_output().expect("the child must finish")
    }
}

/// Every stream the command produced, as one string — what a user, a log
/// file and a terminal scrollback would all see.
fn streams(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The assertion this whole file exists for. Deliberately does **not** print
/// the value when it fails: a test that reported the leak by leaking it
/// again would put the key in CI output.
fn assert_no_value_anywhere(output: &Output, value: &str, what: &str) {
    let seen = streams(output);
    assert!(
        !seen.contains(value),
        "{what} put the value in its output (value withheld from this message); \
         {} bytes of output were produced",
        seen.len()
    );
}

/// A stand-in `inference-gateway`: it records its argv and its stdin and
/// succeeds. Nothing here reaches a real store, on any developer's machine.
#[cfg(unix)]
fn install_fake_gateway(dir: &std::path::Path, record: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join("fake-inference-gateway");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n{{ printf 'argv:%s\\n' \"$*\"; printf 'stdin:'; cat; }} > '{}'\nexit 0\n",
            record.display()
        ),
    )
    .expect("write the fake gateway");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("make the fake gateway executable");
    path
}

#[cfg(not(unix))]
fn install_fake_gateway(dir: &std::path::Path, record: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("fake-inference-gateway.cmd");
    std::fs::write(
        &path,
        format!(
            "@echo off\r\necho argv:%*> \"{}\"\r\nexit /b 0\r\n",
            record.display()
        ),
    )
    .expect("write the fake gateway");
    path
}

/// A value on the command line is refused, the refusal says why, and the
/// refused word is never echoed.
///
/// The echo is the part that needs a test rather than a reading: without the
/// hidden `value_on_argv` argument, clap answers an unexpected positional by
/// printing it, so the refusal itself would be the leak.
#[test]
fn a_value_on_the_command_line_is_refused_and_never_echoed() {
    let fixture = Fixture::new();
    let output = fixture.run(&["credentials", "store", "GLASSHOUSE_TEST_ARGV_VAR", PLANTED]);

    assert!(
        !output.status.success(),
        "a value on argv must fail: {}",
        streams(&output)
    );
    let seen = streams(&output);
    assert!(
        seen.contains("never an argument"),
        "the refusal must say what is wrong: {seen}"
    );
    assert!(
        seen.contains("--stdin"),
        "the refusal must name the way that works: {seen}"
    );
    assert_no_value_anywhere(&output, PLANTED, "the argv refusal");
}

/// **The value goes to the gateway and never through Glasshouse.** The key
/// is piped to this process's standard input, which the forward inherits to
/// the child; the recording proves the child got it, and Glasshouse's own
/// output is asserted never to contain it.
#[cfg(unix)]
#[test]
fn store_forwards_the_variable_and_the_value_never_passes_through_glasshouse() {
    let fixture = Fixture::new();
    let output = fixture.run_with_stdin(
        &[
            "credentials",
            "store",
            "GLASSHOUSE_TEST_NO_TTY_VAR",
            "--stdin",
        ],
        PLANTED,
    );

    assert!(output.status.success(), "{}", streams(&output));
    let forwarded = fixture.forwarded();
    assert!(
        forwarded.contains("argv:credentials set --variable GLASSHOUSE_TEST_NO_TTY_VAR"),
        "{forwarded}"
    );
    assert!(
        forwarded.contains(&format!("stdin:{PLANTED}")),
        "the key reaches the gateway on the inherited standard input"
    );
    assert_no_value_anywhere(&output, PLANTED, "the store forward");
}

/// A value that reaches the store and is refused there is still a value the
/// error must not carry.
///
/// The empty variable name is the lever: every backend refuses to build an
/// entry for it, so this runs the whole `--stdin` path — read the line, hand
/// it to the store, report the failure — with a real value in hand, on every
/// platform, without writing anything anywhere.
#[test]
fn a_store_failure_reports_the_stores_own_words_and_never_the_value() {
    let fixture = Fixture::new();
    let output = fixture.run_with_stdin(
        &["credentials", "store", "", "--stdin"],
        &format!("{PLANTED}\n"),
    );

    assert!(
        !output.status.success(),
        "an unusable name must fail: {}",
        streams(&output)
    );
    assert_no_value_anywhere(&output, PLANTED, "a store failure");
}

/// A name no shell could set — a space in it — is refused on every platform
/// before any store is probed or any value is asked for, and the refusal
/// carries neither the value nor the name. The empty-name test above only
/// held on macOS, whose Keychain happened to refuse an empty account; the
/// Secret Service and Windows Credential Manager filed it (sweep
/// 34013598118), which is why the guard is the command's and this test exists.
#[test]
fn a_name_no_shell_could_set_is_refused_before_any_prompt() {
    let fixture = Fixture::new();
    let output = fixture.run_with_stdin(
        &["credentials", "store", "NOT A NAME", "--stdin"],
        &format!("{PLANTED}\n"),
    );

    assert!(
        !output.status.success(),
        "a name with a space must fail: {}",
        streams(&output)
    );
    let seen = streams(&output);
    assert!(
        seen.contains("not usable"),
        "the refusal must say the name is unusable: {seen}"
    );
    assert!(
        !seen.contains("NOT A NAME"),
        "the refusal must not echo the name, which may be a value: {seen}"
    );
    assert_no_value_anywhere(&output, PLANTED, "an unusable name");
}

/// `credentials list` prints names and sources. A credential variable set in
/// this test's own environment appears by name, with its source named, and
/// its value nowhere.
#[test]
fn list_prints_names_and_sources_and_never_a_value() {
    const VAR: &str = "GLASSHOUSE_TEST_CREDENTIALS_LIST_VAR";

    let fixture = Fixture::new().with_provider_credential(VAR);
    let output = fixture
        .command(&["credentials", "list"])
        .env(VAR, PLANTED)
        .stdin(Stdio::null())
        .output()
        .expect("the glasshouse binary must be runnable");

    assert!(
        output.status.success(),
        "`credentials list` must succeed: {}",
        streams(&output)
    );
    let seen = streams(&output);
    assert!(seen.contains(VAR), "the variable must be named: {seen}");
    assert!(
        seen.contains("value hidden"),
        "every line says the value is withheld: {seen}"
    );
    assert!(
        seen.contains("process environment"),
        "the source must be named: {seen}"
    );
    assert_no_value_anywhere(&output, PLANTED, "`credentials list`");
}

/// **`remove` is the gateway's too**, and what a removal that found nothing
/// should say is the gateway's answer, not Glasshouse's. What stays here is
/// the forward's shape and the propagated status.
#[cfg(unix)]
#[test]
fn remove_forwards_the_variable_to_the_gateway() {
    let fixture = Fixture::new();
    let account = format!("GLASSHOUSE_TEST_NEVER_STORED_{}", std::process::id());
    let output = fixture.run(&["credentials", "remove", &account]);

    assert!(output.status.success(), "{}", streams(&output));
    let forwarded = fixture.forwarded();
    assert!(
        forwarded.contains(&format!("argv:credentials remove --variable {account}")),
        "{forwarded}"
    );
    assert!(
        streams(&output).contains("is the gateway's; forwarding to"),
        "one line says what happened: {}",
        streams(&output)
    );
}

// The two `#[ignore]`d round-trip tests that used to live here — the
// shipped binary storing an item in the login keychain, and every other
// program being refused it — were removed with the 2026-09-11 ruling. Their
// subject is gone: Glasshouse no longer writes a credential store, so there
// is nothing here for them to be about, and a test kept alive past its
// premise is worse than no test. What replaces them is the gateway's own
// coverage of its store, and the forward tests above.
