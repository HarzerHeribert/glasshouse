//! Capability map line 1369 — "reduce or suppress active probes when
//! probing would consume a material fraction of a scarce request pool" —
//! proved through the shipped binary's `glasshouse resources --probe`.
//!
//! Every test here drives `Command::Resources`'s real `--probe` arm
//! (`main.rs::resources_report`) against a real loopback fixture, the same
//! shape `tests/v1_criteria_setup.rs`'s and `tests/provider_discovery.rs`'s
//! binary-level tests already use: the assertion that carries each line is
//! the fixture's own request counter, not just the printed report, because a
//! refusal that still opened a socket would pass a text-only assertion.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use glasshouse::provider::telemetry::{GatewayQuotaCache, RateLimitHeaders};

const TELEMETRY_OBSERVED: i64 = 1_787_800_000;

// ---------------------------------------------------------------------------
// A canned upstream on loopback, answering every request `200` with a fixed
// body and recording what it actually received — trimmed from
// `tests/v1_criteria_setup.rs::FixtureUpstream`, whose helpers are private
// to that file.
// ---------------------------------------------------------------------------

struct FixtureUpstream {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl FixtureUpstream {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("loopback must bind");
        listener.set_nonblocking(true).expect("polling mode");
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let thread_requests = Arc::clone(&requests);
        let thread_stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        serve_fixture(stream, &thread_requests);
                    }
                    Err(ref err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            address,
            requests,
            stop,
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Every request target (`/v1/models`, `/v1/key`, ...) this fixture
    /// actually received, in arrival order.
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for FixtureUpstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn serve_fixture(mut stream: TcpStream, requests: &Arc<Mutex<Vec<String>>>) {
    let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }
    let target = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned();

    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    let _ = reader.read_exact(&mut body);

    requests.lock().unwrap().push(target);

    // A body every reader in this module can parse without error: an empty
    // model list for `telemetry_probe`'s target, and no `data.limit*` fields
    // for `usage_probe`'s — `ProviderUsage::read` treats that as "answered,
    // but this reader found none of the fields it understands" rather than
    // as a failure.
    let document = r#"{"data":[]}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{document}",
        document.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

// ---------------------------------------------------------------------------
// A project directory and a private data/config directory the shipped
// binary can be pointed at — `tests/provider_discovery.rs::BinaryFixture`'s
// shape, reproduced because its helpers are private to that file.
// ---------------------------------------------------------------------------

struct BinaryFixture {
    project: tempfile::TempDir,
    home: tempfile::TempDir,
}

impl BinaryFixture {
    fn new() -> Self {
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".git")).unwrap();
        let home = tempfile::tempdir().unwrap();
        Self { project, home }
    }

    fn with_config(self, toml: &str) -> Self {
        std::fs::write(self.home.path().join("config.toml"), toml).unwrap();
        self
    }

    /// The directory `GatewayQuotaCache::new` would resolve its own root
    /// under, for a test planting a reading directly.
    fn quota_cache(&self) -> GatewayQuotaCache {
        GatewayQuotaCache::at(self.home.path().join("gateway-quota"))
    }

    /// Run the shipped binary and return `(stdout, exit success)`.
    fn run(&self, args: &[&str], envs: &[(&str, &str)]) -> (String, bool) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command
            .current_dir(self.project.path())
            .args([
                "--data-dir",
                self.home.path().to_str().unwrap(),
                "--config-dir",
                self.home.path().to_str().unwrap(),
            ])
            .args(args);
        for (key, value) in envs {
            command.env(key, value);
        }
        let output = command.output().expect("the glasshouse binary runs");
        (
            String::from_utf8(output.stdout).expect("stdout is UTF-8"),
            output.status.success(),
        )
    }
}

/// `[providers.openrouter]` pointed at `fixture`'s loopback address —
/// `openrouter` and no other name, because
/// `crate::provider::usage_endpoint` matches on the provider's own
/// **configured name**, not its template, and OpenRouter is the one
/// built-in provider with a declared usage endpoint (capability map
/// line 1230). Named `V1369_OPENROUTER_KEY` so the credential in the
/// process environment cannot be mistaken for a real one.
fn openrouter_config(fixture: &FixtureUpstream) -> String {
    format!(
        "version = 1\n\n[providers.openrouter]\ntemplate = \"openrouter\"\nbase_url = \"{}/v1\"\n\
         credential_env = [\"V1369_OPENROUTER_KEY\"]\n",
        fixture.base_url()
    )
}

const CREDENTIAL_ENV: (&str, &str) = ("V1369_OPENROUTER_KEY", "sk-planted-1369");

// ---------------------------------------------------------------------------
// (a) a thin planted pool refuses the probe by name, and the fixture is
// never touched.
// ---------------------------------------------------------------------------

#[test]
fn a_probe_costing_a_material_fraction_of_a_thin_pool_is_refused_and_the_fixture_is_untouched() {
    let fixture = FixtureUpstream::start();
    let binary = BinaryFixture::new().with_config(&openrouter_config(&fixture));
    binary.quota_cache().store(
        "openrouter",
        &RateLimitHeaders::read(vec![
            ("ratelimit-limit", "10"),
            ("ratelimit-remaining", "3"),
        ]),
        TELEMETRY_OBSERVED,
    );

    let (stdout, success) = binary.run(
        &["resources", "--no-harness", "--probe", "openrouter"],
        &[CREDENTIAL_ENV],
    );
    assert!(success, "a refusal is not a command failure:\n{stdout}");

    let row = stdout
        .lines()
        .find(|line| line.contains("not probing openrouter"))
        .unwrap_or_else(|| panic!("no refusal row for openrouter:\n{stdout}"));
    assert!(
        row.contains("3 request(s) remain"),
        "the refusal must name the remainder: {row}"
    );
    assert!(
        row.contains("would spend 2"),
        "openrouter declares a usage endpoint, so the probe costs 2: {row}"
    );
    assert!(
        row.contains("--force"),
        "the refusal must name the override flag: {row}"
    );
    assert!(
        !row.contains("sk-planted-1369"),
        "a refusal must never print a credential: {row}"
    );

    assert!(
        fixture.requests().is_empty(),
        "a refused probe must open no socket at all: {:?}",
        fixture.requests()
    );
}

// ---------------------------------------------------------------------------
// (b) the same pool, with `--force`: probed, fixture hit twice, and the
// spending line is printed.
// ---------------------------------------------------------------------------

#[test]
fn force_overrides_the_refusal_and_spends_the_budget_it_announced() {
    let fixture = FixtureUpstream::start();
    let binary = BinaryFixture::new().with_config(&openrouter_config(&fixture));
    binary.quota_cache().store(
        "openrouter",
        &RateLimitHeaders::read(vec![
            ("ratelimit-limit", "10"),
            ("ratelimit-remaining", "3"),
        ]),
        TELEMETRY_OBSERVED,
    );

    let (stdout, success) = binary.run(
        &[
            "resources",
            "--no-harness",
            "--probe",
            "openrouter",
            "--force",
        ],
        &[CREDENTIAL_ENV],
    );
    assert!(success, "{stdout}");

    let spending_row = stdout
        .lines()
        .find(|line| line.contains("probing openrouter anyway"))
        .unwrap_or_else(|| panic!("no spending row for openrouter:\n{stdout}"));
    assert!(spending_row.contains("spending 2 of 3"), "{spending_row}");
    assert!(
        !stdout.contains("not probing openrouter"),
        "--force must not also print a refusal:\n{stdout}"
    );

    let probe_row = stdout
        .lines()
        .find(|line| line.trim_start().starts_with("openrouter:"))
        .unwrap_or_else(|| panic!("no probe result row for openrouter:\n{stdout}"));
    assert!(
        probe_row.contains("reached (status 200)"),
        "--force must still actually probe: {probe_row}"
    );

    let hits = fixture.requests();
    assert_eq!(
        hits.len(),
        2,
        "the connectivity read and the usage-endpoint read: {hits:?}"
    );
    assert!(hits.contains(&"/v1/models".to_owned()), "{hits:?}");
    assert!(hits.contains(&"/v1/key".to_owned()), "{hits:?}");
}

// ---------------------------------------------------------------------------
// (c) no planted reading at all: probed exactly as before this line existed.
// ---------------------------------------------------------------------------

#[test]
fn no_cache_row_is_probed_as_today() {
    let fixture = FixtureUpstream::start();
    let binary = BinaryFixture::new().with_config(&openrouter_config(&fixture));
    // No `quota_cache().store(...)` call: `GatewayQuotaCache::load` answers
    // `None`, exactly `FreePool::allowance`'s own `unknown_pool()` default —
    // capability map line 1369's own "unknown" case.

    let (stdout, success) = binary.run(
        &["resources", "--no-harness", "--probe", "openrouter"],
        &[CREDENTIAL_ENV],
    );
    assert!(success, "{stdout}");
    assert!(
        !stdout.contains("not probing openrouter"),
        "an unmeasured pool must never be refused:\n{stdout}"
    );

    let hits = fixture.requests();
    assert_eq!(hits.len(), 2, "{hits:?}");
}

// ---------------------------------------------------------------------------
// (d) a provider the user has explicitly declared token-priced (a manual
// plan on its `[providers.<name>.quota]` table, capability map line 1233's
// own seam) is probed exactly as today: a monetary plan carries no request
// count for the budget check to compare a cost against.
// ---------------------------------------------------------------------------

#[test]
fn a_token_priced_providers_declared_plan_does_not_trigger_a_refusal() {
    let fixture = FixtureUpstream::start();
    let config = format!(
        "{}\n[providers.openrouter.quota]\nplan = \"pay-as-you-go\"\n",
        openrouter_config(&fixture)
    );
    let binary = BinaryFixture::new().with_config(&config);
    // Still no `GatewayQuotaCache` row: a declared plan is a fact about
    // pricing, not a request-pool reading, and the two must not be
    // conflated (capability map line 1324's principle, applied to this
    // check).

    let (stdout, success) = binary.run(
        &["resources", "--no-harness", "--probe", "openrouter"],
        &[CREDENTIAL_ENV],
    );
    assert!(success, "{stdout}");
    assert!(
        !stdout.contains("not probing openrouter"),
        "a token-priced provider must never be refused on a request-pool budget:\n{stdout}"
    );

    let hits = fixture.requests();
    assert_eq!(hits.len(), 2, "{hits:?}");
}
