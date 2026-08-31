//! Capability map line 1317's reader, exercised the way a caller outside
//! this crate reaches it: through a real `Runtime`, a real project database
//! on disk, and the shipped binary's own `glasshouse route` report.
//!
//! Behavioral contract: a throttle observed overlapping a throttle on
//! another model of the same provider reads as provider-wide; a throttle
//! observed only against a sibling model that kept serving reads as
//! model-specific; below the minimum sample it is *unknown* with the count,
//! never guessed; and `glasshouse route` prints the classification for every
//! route this project's ledger has seen throttled (capability map line
//! 1317). The production reader is proven directly against
//! `EvidenceLedger::throttle_scopes` — this file's own concern is that the
//! shipped binary reaches it and prints what it says.
//!
//! Line 1317 also names two scopes this build refuses to fabricate:
//! account-specific (no row carries an account identity) and
//! request-pool-specific (`routing::free::is_request_pool` has no
//! production caller — refusal register, row 531). Neither is built, and
//! neither string appears anywhere `glasshouse route` prints.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;

use glasshouse::routing::evidence::{
    CLASSIFICATION_EVIDENCE_WINDOW_SECONDS, EvidenceLedger, FailureClass, MIN_CORRELATION_SAMPLE,
    NewObservation, Outcome, RouteIdentity, ThrottleScope,
};
use glasshouse::{Cli, Runtime};

/// A bootstrapped project inside `base`, sharing `base`'s data and config
/// roots — the same idiom `tests/route_correlation.rs` uses.
struct Fixture {
    base: PathBuf,
    root: PathBuf,
    runtime: Runtime,
}

impl Fixture {
    fn new(base: &Path) -> Self {
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        let runtime = glasshouse::bootstrap(&cli, &root).unwrap();
        Fixture {
            base: base.to_path_buf(),
            root,
            runtime,
        }
    }

    fn ledger(&self) -> EvidenceLedger {
        EvidenceLedger::open(&self.runtime).unwrap()
    }

    /// The shipped binary, pointed at this project.
    fn glasshouse(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .env(
                "GLASSHOUSE_RATE_LIMIT_SCOPE_TEST_KEY",
                "planted-opaque-value",
            )
            .env("PATH", self.base.join("empty-path"))
            .output()
            .expect("the glasshouse binary must be runnable")
    }
}

fn exchange(
    provider: &str,
    model: &str,
    start: i64,
    class: Option<FailureClass>,
) -> NewObservation {
    NewObservation::new(provider, model)
        .with_route(Some("anthropic-messages"))
        .with_harness(Some("claude-code"))
        .with_timing(Some(start), Some(start + 5))
        .with_outcome(if class.is_some() {
            Outcome::Failed
        } else {
            Outcome::Succeeded
        })
        .with_failure_class(class)
}

fn route(provider: &str, model: &str) -> RouteIdentity {
    RouteIdentity::new(provider, model)
}

/// Plant, relative to `now`: five moments where `a/x` and `a/y` were
/// throttled together (provider-wide), two where `b/m` was throttled while
/// `b/n` kept serving (below the minimum sample), and one overlapping pair
/// of throttles on `c/p` and `c/q` from before the window.
fn plant(ledger: &EvidenceLedger, now: i64) {
    for i in 0..5 {
        let at = now - 3_600 + i * 300;
        ledger
            .record(exchange("a", "x", at, Some(FailureClass::Throttle)), at + 5)
            .unwrap();
        ledger
            .record(
                exchange("a", "y", at + 10, Some(FailureClass::Throttle)),
                at + 15,
            )
            .unwrap();
    }
    for i in 0..2 {
        let at = now - 1_800 + i * 300;
        ledger
            .record(exchange("b", "m", at, Some(FailureClass::Throttle)), at + 5)
            .unwrap();
        ledger
            .record(exchange("b", "n", at + 10, None), at + 15)
            .unwrap();
    }
    let stale = now - CLASSIFICATION_EVIDENCE_WINDOW_SECONDS - 1_000;
    ledger
        .record(
            exchange("c", "p", stale, Some(FailureClass::Throttle)),
            stale + 5,
        )
        .unwrap();
    ledger
        .record(
            exchange("c", "q", stale + 10, Some(FailureClass::Throttle)),
            stale + 15,
        )
        .unwrap();
}

#[test]
fn throttle_scopes_are_read_from_a_real_ledger_with_their_sample_size_and_window() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());
    let ledger = fixture.ledger();
    let now = 1_800_000_000_i64;
    plant(&ledger, now);

    let scopes = ledger
        .throttle_scopes(now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();

    assert_eq!(
        scopes.for_route(&route("a", "x")),
        ThrottleScope::ProviderWide,
        "every throttle on a/x overlapped a throttle on a/y"
    );
    assert_eq!(
        scopes.for_route(&route("a", "y")),
        ThrottleScope::ProviderWide,
        "the relationship is symmetric"
    );
    assert_eq!(
        scopes.for_route(&route("b", "m")),
        ThrottleScope::Unknown {
            sample_size: 2,
            required: MIN_CORRELATION_SAMPLE,
        },
        "two informative throttles are not enough to say anything, and the count is carried \
         rather than hidden"
    );
    assert_eq!(
        scopes.for_route(&route("c", "p")),
        ThrottleScope::Unknown {
            sample_size: 0,
            required: MIN_CORRELATION_SAMPLE,
        },
        "an overlap from before the window is not read"
    );
}

/// Line 1317 at the shipped binary: `glasshouse route` names every route
/// this project's ledger has seen throttled, with the classification the
/// evidence supports and the honest count when it does not support one yet.
#[test]
fn the_route_command_prints_every_routes_throttle_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path());

    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let fake = tmp.path().join("fake-claude");
    std::fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let escaped = fake.display().to_string().replace('\\', "\\\\");
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            "version = 1\n\n\
             [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\n\
             [providers.rate-limit-probe]\ntemplate = \"openrouter\"\n\
             credential_env = [\"GLASSHOUSE_RATE_LIMIT_SCOPE_TEST_KEY\"]\n\n\
             [profiles.metered]\nharness = \"claude-code\"\n\
             expected_protocol = \"anthropic-messages\"\n\n\
             [profiles.metered.backend]\nkind = \"direct-provider\"\n\
             provider = \"rate-limit-probe\"\n"
        ),
    )
    .unwrap();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    plant(&fixture.ledger(), now);

    let output = fixture.glasshouse(&["route"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "route must succeed: status {:?}\nstdout:\n{stdout}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("Throttle scope in this project, last 7 days (map line 1317)"),
        "the section must be printed:\n{stdout}"
    );
    assert!(
        stdout.contains(
            "a/x: provider-wide — a throttle on this route overlapped a throttle on another \
             model of the same provider"
        ),
        "a route whose throttles overlapped a sibling model's throttle reads as provider-wide:\n\
         {stdout}"
    );
    assert!(
        stdout.contains(
            "b/m: insufficient evidence — 2 of the 5 informative throttle events a scope needs; \
             treated as unknown"
        ),
        "a route below the minimum prints the count and says what it is treated as:\n{stdout}"
    );
    assert!(
        !stdout.contains("c/p") && !stdout.contains("c/q"),
        "a route whose only throttle predates the window is not listed:\n{stdout}"
    );
    assert!(
        !stdout.contains("account-specific") && !stdout.contains("request-pool-specific"),
        "the two scopes this build has no producer for are never printed:\n{stdout}"
    );
}
