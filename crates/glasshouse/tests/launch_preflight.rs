//! Phase 9F lines 468 and 469 — the pre-flight capability check, and the
//! requirement that a harness executable be installed before a direct-provider
//! or gateway-backed launch profile is offered.
//!
//! # Everything here enters through the shipped binary, on purpose
//!
//! `profile/mod.rs`'s own tests already prove that `capability_probe` builds
//! the right request and that `connectivity` sends it over a real socket.
//! Re-proving that seam here would prove nothing new. What no unit test can
//! answer is whether the **binary a person runs** performs the check at all —
//! and that is exactly the question line 468 was scaffolded-but-unchecked on,
//! because the probe had zero production callers.
//!
//! So every test in this file runs `glasshouse launch`, and each one fails if
//! the production call disappears:
//!
//! - a launch against a provider that is not there prints the warning, and
//!   **still starts the session**;
//! - a launch against a provider that answers `401` says the credential was
//!   refused rather than that the host was missing;
//! - a launch against a provider that answers prints no warning at all, and
//!   the server still saw the request;
//! - a `Native` profile — line 468's own "when a check is available"
//!   qualifier — makes **no request whatsoever**;
//! - and the check's outcome cannot change which backend the session records.
//!
//! # The credential
//!
//! The planted value below is deliberately **not** credential-shaped:
//! `crate::secret::redact` would catch an `sk-` token, and a test that passed
//! only because the redactor caught the value would prove the net works
//! rather than that the structure keeps the value out. This one would survive
//! redaction, so its absence from the report is a claim about how the report
//! is built.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The variable the fixture declares the provider's credential under.
const CREDENTIAL_VAR: &str = "GLASSHOUSE_PREFLIGHT_TEST_KEY";

/// A value the redactor does not recognise — see the module doc.
const PLANTED_CREDENTIAL: &str = "planted-opaque-preflight-value-9f";

/// The exit code the fake harness uses, so "the session actually started" is
/// a distinctive observation rather than a zero that a launch which started
/// nothing would also produce.
const HARNESS_EXIT: i32 = 23;

/// Write each distinct fixture executable once per test binary instead of
/// once per test, so macOS Gatekeeper (`syspolicyd`/XProtect) validates it
/// once per `launch_preflight` run instead of once per test — see the
/// project memory `gatekeeper-scans-make-pty-fixtures-flaky` and
/// GH-FIXTURE-REUSE. `install_fake_harness` below writes fixed bytes
/// (`HARNESS_EXIT` is a compile-time constant), so every one of this file's
/// tests collapses onto the one file the first caller writes.
///
/// Sharing is keyed by content, never by the caller's requested name, so a
/// name never causes two distinct fixtures to collide, and a repeated name
/// with the same bytes never causes a second write. Race-free the way
/// `provider/cache.rs::write_json_atomically` is: one process-wide mutex
/// serialises the check-and-write, and the write itself lands in a
/// same-directory temporary name before an atomic rename.
fn shared_fixture(unique_name: &str, contents: &str) -> PathBuf {
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};
    use std::sync::{Mutex, OnceLock};

    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    static CACHE: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();

    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().expect("shared fixture cache poisoned");
    if let Some(path) = guard.get(contents) {
        return path.clone();
    }

    let dir = DIR.get_or_init(|| tempfile::tempdir().expect("shared fixture dir"));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    contents.hash(&mut hasher);
    let digest = format!("{:016x}", hasher.finish());
    let named = Path::new(unique_name);
    let stem = named
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(unique_name);
    let filename = match named.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem}-{digest}.{ext}"),
        None => format!("{stem}-{digest}"),
    };
    let path = dir.path().join(&filename);
    let temporary = dir.path().join(format!("{filename}.writing"));
    std::fs::write(&temporary, contents).expect("write shared fixture");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&temporary).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&temporary, perms).unwrap();
    }
    std::fs::rename(&temporary, &path).expect("rename shared fixture into place");
    guard.insert(contents.to_string(), path.clone());
    path
}

#[cfg(test)]
mod shared_fixture_proof {
    use super::{HARNESS_EXIT, install_fake_harness};

    /// **The once-per-binary proof, through the real caller.** Every test in
    /// this file calls `Fixture::new`, which unconditionally calls
    /// `install_fake_harness` — so two independent per-test tempdirs asking
    /// for it, the ordinary shape this binary runs under, must collapse to
    /// one file rather than each writing its own.
    #[test]
    fn two_tempdirs_installing_the_fake_harness_get_one_shared_file() {
        let tmp_a = tempfile::tempdir().expect("tempdir a");
        let tmp_b = tempfile::tempdir().expect("tempdir b");
        let a = install_fake_harness(tmp_a.path());
        let meta_before = std::fs::metadata(&a).expect("fixture exists after first install");

        let b = install_fake_harness(tmp_b.path());
        assert_eq!(
            a, b,
            "two different tempdirs installing the fixture must share one file"
        );
        assert!(
            !a.starts_with(tmp_a.path()) && !a.starts_with(tmp_b.path()),
            "the shared file must live in the per-binary fixture dir, not either \
             test's own tempdir: {a:?}"
        );

        let meta_after = std::fs::metadata(&b).expect("fixture exists after second install");
        assert_eq!(
            meta_before.modified().unwrap(),
            meta_after.modified().unwrap(),
            "a second install of the same fixture must not rewrite the file"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                meta_before.ino(),
                meta_after.ino(),
                "a second install of the same fixture must return the same inode, \
                 not a second copy"
            );
        }
    }

    /// **Bytes unchanged.** The shared fixture is byte-for-byte the same
    /// script every per-test write used to produce.
    #[cfg(unix)]
    #[test]
    fn the_shared_fixture_has_the_original_unshared_bytes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = install_fake_harness(tmp.path());
        let content = std::fs::read_to_string(&path).expect("read shared fixture");
        assert_eq!(
            content,
            format!("#!/bin/sh\nexit {HARNESS_EXIT}\n"),
            "the shared fixture's bytes must match the original per-test literal exactly"
        );
    }
}

// --- a loopback provider ----------------------------------------------------

/// A server that answers every request with one fixed status line, counting
/// what it was asked for.
///
/// The count is the half that makes the `Native` test meaningful: "no warning
/// was printed" is also true of a check that ran and passed, so the assertion
/// that matters there is that **nothing was requested**.
struct FakeProvider {
    base_url: String,
    hits: Arc<AtomicUsize>,
}

impl FakeProvider {
    fn answering(status_line: &'static str) -> Self {
        let listener =
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback must be bindable");
        let port = listener
            .local_addr()
            .expect("a bound listener has an address")
            .port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        // Detached: the test process exits when its main thread returns,
        // whatever this one is blocked on.
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                counter.fetch_add(1, Ordering::SeqCst);
                // Read the request head and nothing more — the probe sends no
                // body, and a server that replied before reading would race
                // the client's write.
                let mut reader =
                    BufReader::new(stream.try_clone().expect("a loopback stream can be cloned"));
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    if line == "\r\n" || line == "\n" {
                        break;
                    }
                    line.clear();
                }
                let _ = stream.write_all(
                    format!("{status_line}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}")
                        .as_bytes(),
                );
                let _ = stream.flush();
            }
        });
        Self {
            base_url: format!("http://127.0.0.1:{port}"),
            hits,
        }
    }

    /// A port with nothing behind it. Bound and dropped, so the number is
    /// real and free — and what a probe finds there is the platform's
    /// choice, not Glasshouse's: a Unix stack refuses the connection at once
    /// while Windows drops the SYN and the probe waits out its own bound.
    /// Both are "nothing answered", which is why no test below asserts on
    /// which of the two words came back.
    fn absent() -> Self {
        let listener =
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback must be bindable");
        let port = listener
            .local_addr()
            .expect("a bound listener has an address")
            .port();
        drop(listener);
        Self {
            base_url: format!("http://127.0.0.1:{port}"),
            hits: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

// --- the project ------------------------------------------------------------

/// A project with its own data and config roots and a fake `claude-code`, so
/// `session::select` resolves an installed harness without the real one being
/// present on the machine running the tests.
struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl Fixture {
    /// `harness_executable` absent means the integration is enabled with no
    /// configured path at all, which is what line 469's refusal needs.
    fn new(base_url: &str, harness_executable: bool) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let harness = install_fake_harness(&bin_dir);

        let executable_line = if harness_executable {
            // TOML needs a Windows path's backslashes escaped, the same way
            // `launch_overlay.rs`'s fixture does it.
            let escaped = harness.display().to_string().replace('\\', "\\\\");
            format!("executable = \"{escaped}\"\n")
        } else {
            String::new()
        };

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\n{executable_line}\n\
                 [providers.preflight-probe]\ntemplate = \"anthropic-compatible\"\n\
                 base_url = \"{base_url}\"\n\
                 credential_env = [\"{CREDENTIAL_VAR}\"]\n\n\
                 [profiles.direct]\nharness = \"claude-code\"\n\n\
                 [profiles.direct.backend]\nkind = \"direct-provider\"\n\
                 provider = \"preflight-probe\"\n\n\
                 [profiles.parked]\nharness = \"claude-code\"\nenabled = false\n\n\
                 [profiles.parked.backend]\nkind = \"direct-provider\"\n\
                 provider = \"preflight-probe\"\n\n\
                 [routing]\nautomatic = false\n"
            ),
        )
        .expect("write user config");

        Self {
            _tmp: tmp,
            base,
            root,
        }
    }

    /// The binary, with the credential planted in the child's environment
    /// only — an integration test cannot mint a `Secret`, and exporting the
    /// value into *this* process would publish it to every other test in it.
    ///
    /// `PATH` is emptied to the fixture's own (empty) directory so that a
    /// harness with no configured executable is genuinely not installed,
    /// whatever the machine running the tests happens to have.
    fn glasshouse(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .env(CREDENTIAL_VAR, PLANTED_CREDENTIAL)
            .env("PATH", self.base.join("empty-path"))
            .output()
            .expect("the glasshouse binary must be runnable")
    }

    /// Everything the binary said, both streams, for an assertion that has to
    /// hold of the whole report rather than of one channel.
    fn both_streams(output: &Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    /// The single recorded session's detail report.
    fn only_session_detail(&self) -> String {
        let listing = self.glasshouse(&["sessions"]);
        let listing = String::from_utf8_lossy(&listing.stdout).into_owned();
        let id = listing
            .lines()
            .nth(1)
            .and_then(|line| line.split_whitespace().next())
            .unwrap_or_else(|| panic!("expected exactly one recorded session:\n{listing}"))
            .to_owned();
        let detail = self.glasshouse(&["sessions", "show", &id]);
        String::from_utf8_lossy(&detail.stdout).into_owned()
    }
}

#[cfg(unix)]
fn install_fake_harness(_bin_dir: &Path) -> PathBuf {
    shared_fixture(
        "fake-claude-code",
        &format!("#!/bin/sh\nexit {HARNESS_EXIT}\n"),
    )
}

#[cfg(windows)]
fn install_fake_harness(_bin_dir: &Path) -> PathBuf {
    shared_fixture(
        "fake-claude-code.cmd",
        &format!("@echo off\r\nexit /b {HARNESS_EXIT}\r\n"),
    )
}

// --- line 468 ---------------------------------------------------------------

/// **The production caller, and the ruling, in one observation.**
///
/// A direct-provider profile whose provider is not there: the binary says so
/// **before** the session starts, and then starts it. Deleting
/// `profile::preflight`'s call from `main.rs::launch_session` removes the
/// first half and this fails; changing the ruling to a refusal removes the
/// second half and this fails too.
#[test]
fn an_unreachable_provider_is_reported_before_the_session_starts_and_does_not_refuse_it() {
    let provider = FakeProvider::absent();
    let fixture = Fixture::new(&provider.base_url, true);

    let output =
        fixture.glasshouse(&["launch", "claude-code", "--profile", "direct", "--headless"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stderr.contains("pre-flight check did not confirm"),
        "the launch must report the check it could not confirm:\n{stderr}"
    );
    // Named specifically enough to act on: which profile, which provider, and
    // the exact URL that was asked. "the check failed" is not a diagnostic.
    assert!(
        stderr.contains("launch profile `direct`")
            && stderr.contains("provider `preflight-probe`")
            && stderr.contains(&provider.base_url),
        "the report must name the profile, the provider and the URL:\n{stderr}"
    );
    assert!(
        stderr.contains("starting the session anyway"),
        "a warning that reads like a refusal is a refusal to the person reading it:\n{stderr}"
    );

    // And it really did start: the harness ran and its exit code came back.
    assert_eq!(
        output.status.code(),
        Some(HARNESS_EXIT),
        "an unconfirmed pre-flight check must not stop the session:\n{stderr}"
    );
}

/// The distinction that earns the check its cost: a host that is there and
/// refused the credential must not read like a host that is not there.
///
/// §63 is the reason this is a *report* and not a refusal — but it is also the
/// reason the two must stay distinguishable, because they have different
/// fixes.
#[test]
fn a_provider_that_rejects_the_credential_reads_differently_from_one_that_never_answered() {
    let rejecting = FakeProvider::answering("HTTP/1.1 401 Unauthorized");
    let fixture = Fixture::new(&rejecting.base_url, true);

    let output =
        fixture.glasshouse(&["launch", "claude-code", "--profile", "direct", "--headless"]);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert_eq!(rejecting.hits(), 1, "the check must have made its request");
    assert!(
        stderr.contains("rejected the credential") && stderr.contains("401"),
        "a 401 must be reported as a rejected credential, with its status:\n{stderr}"
    );
    assert!(
        !stderr.contains("never answered") && !stderr.contains("did not answer"),
        "a host that answered must never be reported as one that did not:\n{stderr}"
    );
    assert_eq!(
        output.status.code(),
        Some(HARNESS_EXIT),
        "a rejected credential is reported, not refused:\n{stderr}"
    );
}

/// A provider that answers produces **no** warning — and the server still saw
/// the request, so this is "the check ran and was quiet", not "the check was
/// removed".
///
/// The silence is the point. A channel that fires on every launch of a
/// working profile is a channel users stop reading, which would cost them the
/// two warnings above.
#[test]
fn a_provider_that_answers_produces_no_warning_and_the_check_still_ran() {
    let answering = FakeProvider::answering("HTTP/1.1 200 OK");
    let fixture = Fixture::new(&answering.base_url, true);

    let output =
        fixture.glasshouse(&["launch", "claude-code", "--profile", "direct", "--headless"]);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert_eq!(answering.hits(), 1, "the check must have made its request");
    assert!(
        !stderr.contains("pre-flight"),
        "a healthy provider must not put anything on the user's terminal:\n{stderr}"
    );
    assert_eq!(output.status.code(), Some(HARNESS_EXIT), "{stderr}");
}

/// **Line 468's own qualifier**: a profile with no check available launches
/// exactly as it does today, and the absent check does not become a refusal.
///
/// The assertion that carries this is the request count. "No warning was
/// printed" is also true of a check that ran and passed; "nothing was ever
/// requested" is only true of a launch that never had a check to make.
#[test]
fn a_native_profile_has_no_check_available_and_makes_no_request_at_all() {
    let provider = FakeProvider::answering("HTTP/1.1 200 OK");
    let fixture = Fixture::new(&provider.base_url, true);

    // No `--profile`: the implied Native profile — what an unadorned
    // `glasshouse launch` gets while automatic routing is off (the fixture
    // turns it off; since map line 372 closed, an unpinned launch under
    // automatic routing ranks every enabled profile and may land on
    // `direct`, which has a check to make).
    let output = fixture.glasshouse(&["launch", "claude-code", "--headless"]);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert_eq!(
        provider.hits(),
        0,
        "a launch with no check available must not touch the network:\n{stderr}"
    );
    assert!(
        !stderr.contains("pre-flight"),
        "an absent check is not a finding to report:\n{stderr}"
    );
    assert_eq!(
        output.status.code(),
        Some(HARNESS_EXIT),
        "a Native launch must be exactly what it was before:\n{stderr}"
    );

    // "Unavailable, not a failure" is a *decision*, and a launch log that
    // said nothing about it would be indistinguishable from one where the
    // check was never consulted. With logging on, the reason is there in the
    // words `capability_probe` gives for a Native backend.
    let logged = fixture.glasshouse(&[
        "--log-stderr",
        "--log-level",
        "info",
        "launch",
        "claude-code",
        "--headless",
    ]);
    let logged = Fixture::both_streams(&logged);
    assert!(
        logged.contains("pre-flight capability check")
            && logged.contains("a native profile uses the harness's own account"),
        "the launch log must say why there was nothing to check:\n{logged}"
    );
    assert_eq!(
        provider.hits(),
        0,
        "and it must still not have touched the network:\n{logged}"
    );
}

/// The boundary `a_capability_probe_cannot_influence_which_backend_resolve_selects`
/// holds at the unit level, asserted where it could actually be broken: the
/// session a real launch recorded.
///
/// The provider is unreachable, so the check is as negative as it gets. The
/// session is still recorded against the direct provider the profile named —
/// not demoted to `native`, not rerouted to a gateway. A pre-flight check that
/// steered resolution would be a router, and this is not a routing line.
#[test]
fn the_pre_flight_outcome_cannot_change_which_backend_the_session_records() {
    let provider = FakeProvider::absent();
    let fixture = Fixture::new(&provider.base_url, true);

    let output =
        fixture.glasshouse(&["launch", "claude-code", "--profile", "direct", "--headless"]);
    assert_eq!(
        output.status.code(),
        Some(HARNESS_EXIT),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let detail = fixture.only_session_detail();
    assert!(
        detail.contains("backend resource   direct-provider:preflight-probe"),
        "a failed check must not change the backend the profile named:\n{detail}"
    );
    assert!(
        detail.contains("launch profile     direct"),
        "the session must still be recorded under the profile that was asked for:\n{detail}"
    );
}

/// **The negative test the secret boundary needs.**
///
/// The check is handed a `Resolution` carrying a resolved credential and its
/// report reaches both the terminal and the log. The planted value is not
/// credential-shaped, so `secret::redact` would not save this test — its
/// absence is a fact about how the report is built, not about the redactor.
///
/// Asserted against the *unreachable* case on purpose: that is the report
/// built from the most text, and the one whose `reason` string a naive
/// implementation would take from the transport error's own words.
///
/// **Run with logging on.** The warning on the terminal is only half of where
/// this string goes; the other half is the `tracing` event, which carries the
/// same summary and fires on *every* launch rather than only on an
/// unconfirmed one. A test that watched only the terminal would leave the
/// wider channel unchecked.
#[test]
fn a_pre_flight_report_never_carries_the_credential_it_probed_with() {
    let provider = FakeProvider::absent();
    let fixture = Fixture::new(&provider.base_url, true);

    let output = fixture.glasshouse(&[
        "--log-stderr",
        "--log-level",
        "info",
        "launch",
        "claude-code",
        "--profile",
        "direct",
        "--headless",
    ]);
    let both = Fixture::both_streams(&output);

    // Not vacuous: both channels really did carry the report, and it really
    // does name the things it is supposed to name.
    assert!(
        both.contains("pre-flight check did not confirm") && both.contains(&provider.base_url),
        "the report must have been printed for its contents to be worth asserting on:\n{both}"
    );
    assert!(
        both.contains("pre-flight capability check") && both.contains("preflight="),
        "the logged event must have fired too, or this test watches one channel:\n{both}"
    );
    assert!(
        !both.contains(PLANTED_CREDENTIAL),
        "the credential reached the pre-flight report:\n{both}"
    );

    // And nothing Glasshouse wrote to disk for this project carries it either
    // — the same report goes to the log.
    let mut files = Vec::new();
    collect_files(&fixture.base.join("data"), &mut files);
    for path in files {
        let Ok(body) = std::fs::read(&path) else {
            continue;
        };
        assert!(
            !String::from_utf8_lossy(&body).contains(PLANTED_CREDENTIAL),
            "the credential reached {}",
            path.display()
        );
    }
}

// --- line 469 ---------------------------------------------------------------

/// **Line 469, established rather than added.**
///
/// A direct-provider launch profile cannot be reached at all while its
/// harness executable is missing: `session::select::select` resolves the
/// executable *before* `EffectiveConfig::launch_profile` is consulted, so the
/// refusal arrives one step earlier than the map's wording suggests — and it
/// names the harness rather than the profile, because at that point Glasshouse
/// has not yet looked at any profile.
///
/// The pair with the test below is what makes this "offered as unavailable,
/// and you can see what to fix" rather than "silently gone": the profile is
/// not deleted, not filtered, not hidden. Install the executable and the same
/// configuration, unchanged, launches.
///
/// # The assertions are ordered, and the order was paid for
///
/// A non-zero exit is the *weakest* evidence here and it is asserted last, on
/// purpose. The mutation that removes this guarantee — `select` falling back
/// to some other program instead of refusing — was run, and under it the
/// launch proceeds and the substituted program exits non-zero anyway. A test
/// that led with "it did not succeed" would have reported KILLED while its
/// real subject went unexercised, which is §80's fifth case.
///
/// So what leads is what only a refusal can produce: the provider was never
/// probed, and no session exists. Under the fallback mutation both are false —
/// the check runs, and a session is recorded — and both fire before anything
/// about an exit code is asked.
#[test]
fn a_direct_provider_profile_cannot_be_launched_while_its_harness_is_not_installed() {
    let provider = FakeProvider::answering("HTTP/1.1 200 OK");
    let fixture = Fixture::new(&provider.base_url, false);

    let output =
        fixture.glasshouse(&["launch", "claude-code", "--profile", "direct", "--headless"]);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    // Nothing got far enough to be checked: the refusal arrives before the
    // profile is consulted, so the pre-flight check never had a launch to
    // look at.
    assert_eq!(
        provider.hits(),
        0,
        "a profile that cannot be launched must not be probed:\n{stderr}"
    );
    // And nothing got far enough to be recorded.
    let listing = fixture.glasshouse(&["sessions"]);
    let listing = String::from_utf8_lossy(&listing.stdout).into_owned();
    assert!(
        !listing.contains("claude-code"),
        "a refused launch must record no session:\n{listing}"
    );
    // What the user is told: the harness, and what to do about it. Not the
    // profile — Glasshouse has not looked at one yet.
    assert!(
        stderr.contains("Claude Code") || stderr.contains("claude-code"),
        "the refusal must name the harness that is missing:\n{stderr}"
    );
    assert!(
        !output.status.success(),
        "an uninstalled harness must not start a session:\n{stderr}"
    );
}

/// GH-PROFILE-ENABLED acceptance test 3, in this file because this is where
/// "a refusal costs nothing" is provable.
///
/// `route_command.rs`'s
/// `explicitly_launching_a_disabled_profile_is_refused_and_says_how_to_undo_it`
/// covers what the person reads. What only this fixture can show is **when**
/// the refusal happens: `parked` is a direct-provider profile pointed at a
/// provider that answers, so a launch that got as far as the profile would
/// probe it. Zero hits means the refusal arrived before the profile was
/// resolved at all — the same ordering, and the same evidence, as the
/// uninstalled-harness refusal above.
///
/// # The assertions are ordered the same way, and for the same reason
///
/// A non-zero exit is asserted last (§80 case 5). The mutation that removes
/// this guarantee — dropping the `profile_enabled` check in `launch_session`
/// — lets the launch proceed, and the fake harness then exits `HARNESS_EXIT`,
/// which is also not success. A test that led with "it did not succeed" would
/// report KILLED with its real subject unexercised. So what leads is what
/// only a refusal produces: the provider was never probed, and no session
/// exists.
#[test]
fn an_explicitly_named_disabled_profile_is_refused_before_anything_is_probed_or_recorded() {
    let provider = FakeProvider::answering("HTTP/1.1 200 OK");
    let fixture = Fixture::new(&provider.base_url, true);

    let output =
        fixture.glasshouse(&["launch", "claude-code", "--profile", "parked", "--headless"]);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert_eq!(
        provider.hits(),
        0,
        "a disabled profile must be refused before it is resolved, so the pre-flight check \
         never has a launch to look at:\n{stderr}"
    );
    let listing = fixture.glasshouse(&["sessions"]);
    let listing = String::from_utf8_lossy(&listing.stdout).into_owned();
    assert!(
        !listing.contains("claude-code"),
        "a refused launch must record no session:\n{listing}"
    );
    assert!(
        stderr.contains("parked"),
        "the refusal must name the profile that was refused:\n{stderr}"
    );
    assert!(
        !output.status.success(),
        "a disabled profile must not start a session:\n{stderr}"
    );
}

/// The other half, and the same pairing `a_direct_provider_profile_cannot_be
/// _launched_while_its_harness_is_not_installed` has with the test below it:
/// disabling is not deleting. The identical launch against the profile that
/// carries no `enabled = false` runs, on the same fixture, in the same
/// process — so the refusal above is about that one key and not about
/// `parked`'s backend, its provider, or its harness.
#[test]
fn the_sibling_profile_that_is_not_disabled_launches_on_the_same_configuration() {
    let provider = FakeProvider::answering("HTTP/1.1 200 OK");
    let fixture = Fixture::new(&provider.base_url, true);

    let output =
        fixture.glasshouse(&["launch", "claude-code", "--profile", "direct", "--headless"]);
    assert_eq!(
        output.status.code(),
        Some(HARNESS_EXIT),
        "`direct` differs from `parked` only in that it has no `enabled = false`, and it \
         must launch:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        provider.hits() > 0,
        "and it must get far enough to be pre-flight checked, which is what makes the \
         zero-hit assertion above a statement about the refusal rather than about this \
         fixture"
    );
}

/// The other half: the profile was never hidden or discarded, so installing
/// the executable is all it takes.
///
/// Without this, the test above would also pass against a Glasshouse that
/// deleted the profile — which is exactly the failure line 469's "while
/// preserving the profile's visibility so the user can see what to fix" is
/// about.
#[test]
fn the_same_profile_launches_once_its_harness_executable_is_installed() {
    let provider = FakeProvider::answering("HTTP/1.1 200 OK");
    let fixture = Fixture::new(&provider.base_url, true);

    let output =
        fixture.glasshouse(&["launch", "claude-code", "--profile", "direct", "--headless"]);
    assert_eq!(
        output.status.code(),
        Some(HARNESS_EXIT),
        "the same profile, with the executable installed, must launch:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let detail = fixture.only_session_detail();
    assert!(
        detail.contains("backend resource   direct-provider:preflight-probe"),
        "and it must launch as the direct-provider profile it is:\n{detail}"
    );
}

/// Every regular file under `dir`, so a "nothing written carries this value"
/// assertion can name what it found.
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out);
        } else {
            out.push(path);
        }
    }
    out.sort();
}
