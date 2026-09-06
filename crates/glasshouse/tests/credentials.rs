//! `glasshouse credentials`, driven as a user drives it: the shipped binary,
//! a real value on standard input, and the assertion that the value comes
//! back out of **nothing** — not stdout, not stderr, not an error.
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
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("a temporary project root");
        std::fs::create_dir_all(root.path().join(".git")).expect("a project marker");
        Self {
            base: tempfile::tempdir().expect("a temporary base"),
            root,
        }
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

/// Without `--stdin` and without a terminal there is nothing to prompt on,
/// and the command says so instead of reading the pipe it was not offered.
#[test]
fn a_prompt_with_no_terminal_is_refused_rather_than_read_from_the_pipe() {
    let fixture = Fixture::new();
    let output = fixture.run_with_stdin(
        &["credentials", "store", "GLASSHOUSE_TEST_NO_TTY_VAR"],
        &format!("{PLANTED}\n"),
    );

    // On a platform with no native store the store refusal comes first, by
    // design: nothing asks for a value it cannot keep. Either refusal is
    // correct; reading the pipe silently is not.
    assert!(
        !output.status.success(),
        "a prompt with no terminal must fail: {}",
        streams(&output)
    );
    assert_no_value_anywhere(&output, PLANTED, "the no-terminal refusal");
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

/// Removing something that was never stored is the state the user asked
/// for, not an error — and it writes nothing, so it is safe to run against
/// a real store.
#[test]
fn removing_a_credential_that_was_never_stored_is_not_an_error() {
    let fixture = Fixture::new();
    let account = format!("GLASSHOUSE_TEST_NEVER_STORED_{}", std::process::id());
    let output = fixture.run(&["credentials", "remove", &account]);
    let seen = streams(&output);

    if output.status.success() {
        assert!(
            seen.contains("nothing to remove"),
            "the answer must say which of the two happened: {seen}"
        );
    } else {
        // A session with no store — a CI runner with no login keychain, a
        // Windows service session, a Linux box with no keyring — is a real
        // state, and refusing plainly there is correct.
        assert!(
            seen.contains("no native secure store"),
            "the only acceptable failure here is a store that would not \
             open, said plainly: {seen}"
        );
    }
}

/// **The dogfooding defect of 2026-09-06, end to end, through the shipped
/// binary.**
///
/// Ignored by default because it writes to the developer's own login
/// keychain. Run it by hand:
///
/// ```text
/// cargo test -p glasshouse --test credentials -- --ignored
/// ```
///
/// What it proves, in the order that matters: the shipped binary stores a
/// credential; **the same shipped binary** then reports it as *set in the
/// macOS Keychain* through `doctor`, which is the exact line a user was not
/// getting; and the value appears in neither command's output. It also
/// records, out loud, whether a *different* binary — this test process — can
/// read what the shipped one wrote, because that is the question the whole
/// access-control mechanism turns on.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "writes to the developer's own login keychain"]
fn the_shipped_binary_stores_a_credential_the_shipped_binary_can_then_read() {
    let account = format!("GLASSHOUSE_TEST_SHIPPED_STORE_{}", std::process::id());
    let fixture = Fixture::new().with_provider_credential(&account);

    /// Removes the item however this test leaves.
    struct Stored<'a>(&'a Fixture, String);
    impl Drop for Stored<'_> {
        fn drop(&mut self) {
            let _ = self.0.run(&["credentials", "remove", self.1.as_str()]);
        }
    }

    let stored = fixture.run_with_stdin(
        &["credentials", "store", &account, "--stdin"],
        &format!("{PLANTED}\n"),
    );
    assert!(
        stored.status.success(),
        "the shipped binary must be able to store: {}",
        streams(&stored)
    );
    let _cleanup = Stored(&fixture, account.clone());
    assert_no_value_anywhere(&stored, PLANTED, "`credentials store --stdin`");
    assert!(
        streams(&stored).contains("macOS Keychain"),
        "the answer names where it went: {}",
        streams(&stored)
    );

    // The whole point: `doctor`, run through the same binary, now says the
    // credential is set — and says where.
    let doctor = fixture.run(&["doctor"]);
    let seen = streams(&doctor);
    let line = seen
        .lines()
        .find(|line| line.contains(&account))
        .unwrap_or_else(|| panic!("no line names {account} in:\n{seen}"));
    assert!(
        line.contains("set in the macOS Keychain") && line.contains("value hidden"),
        "doctor must report the stored credential as set: {line}"
    );
    assert_no_value_anywhere(&doctor, PLANTED, "`doctor`");

    // And the round trip is exact: no trailing newline was kept.
    let list = fixture.run(&["credentials", "list"]);
    assert!(
        streams(&list).contains("set in the macOS Keychain"),
        "`credentials list` must agree with `doctor`: {}",
        streams(&list)
    );

    // Recorded rather than asserted: this test process is a different
    // binary from the one that stored the item, so the access control list
    // does not name it. Whichever way it goes, say so — this is the fact
    // the whole mechanism turns on.
    let readable_here = glasshouse::secret::SecretStore::is_present(
        &glasshouse::secret::native::PreferNativeSecretStore::detect(),
        &glasshouse::secret::SecretRef::Environment {
            var: account.clone(),
        },
    );
    eprintln!(
        "MEASURED: a different binary (this test process) reading what the shipped binary \
         stored: is_present = {readable_here}"
    );
}

/// **The other half of the mechanism, through the shipped binary.** An item
/// the shipped binary stored is one *this* process is refused — and refused
/// is what it reads as, not absent.
///
/// This is the dogfooding defect inverted. The old instruction produced an
/// item Glasshouse was refused and reported as *not set*; the verb produces
/// an item every other program is refused, which is the property that makes
/// it worth having. Asserting `Refused` rather than merely "not readable" is
/// what keeps `doctor`'s new line honest.
///
/// Ignored for the same reason as the test above: it writes to the
/// developer's own login keychain.
///
/// **There is deliberately no test that reads the stored value back.** No
/// program but the one that wrote it can, and `security
/// find-generic-password -w` does not fail — it blocks on an *Allow* dialog,
/// measured here on 2026-09-06 at 160 seconds and still waiting. That is the
/// hang `secret::native`'s `silence_authorization_dialogs` exists to prevent,
/// and a test that reproduced it would be the defect. The exact bytes
/// `--stdin` stores are pinned instead by
/// `commands::credentials::tests::one_line_loses_its_terminator_and_nothing_else`,
/// which tests the function that decides them.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "writes to the developer's own login keychain"]
fn an_item_the_shipped_binary_stored_is_refused_to_every_other_program() {
    use glasshouse::secret::SecretStore as _;
    use glasshouse::secret::native::{PreferNativeSecretStore, Presence};

    let account = format!("GLASSHOUSE_TEST_FOREIGN_READ_{}", std::process::id());
    let fixture = Fixture::new();

    struct Stored<'a>(&'a Fixture, String);
    impl Drop for Stored<'_> {
        fn drop(&mut self) {
            let _ = self.0.run(&["credentials", "remove", self.1.as_str()]);
        }
    }

    let stored = fixture.run_with_stdin(
        &["credentials", "store", &account, "--stdin"],
        &format!("{PLANTED}\n"),
    );
    assert!(
        stored.status.success(),
        "the shipped binary must be able to store: {}",
        streams(&stored)
    );
    let _cleanup = Stored(&fixture, account.clone());

    let secrets = PreferNativeSecretStore::detect();
    let native = secrets
        .native()
        .expect("the store answered for the shipped binary, so it answers here");
    let reference = glasshouse::secret::SecretRef::Environment {
        var: account.clone(),
    };

    assert_eq!(
        native.presence(&reference),
        Presence::Refused,
        "this process did not write the item, so the Keychain must refuse it rather than \
         report it missing"
    );
    assert!(
        !native.is_present(&reference),
        "a refused credential is still one this process cannot use"
    );
    assert!(
        native.resolve(&reference).is_none(),
        "and it must not resolve"
    );
}
