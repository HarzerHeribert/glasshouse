//! Capability map line 1247's reachable half —
//! `docs/product/design-decisions.md`'s *"Re-calibrating the headroom
//! estimator when the quota regime changes."* A change in a provider's
//! **stated ceiling** between two persisted gateway readings is a regime
//! change; its instant is persisted with the reading; and every headroom
//! estimate for that provider from then on is derived only from routing
//! observations at or after it, and says so in its rendered line.
//!
//! Five tests. The first drives a real gateway exchange end to end
//! (`gateway_retry_after.rs`'s own shape — practice §35: a caller every test
//! bypasses is not a caller), because the detector's production entry point
//! is `GatewayQuotaCache::try_store`, called from the accept loop on every
//! forwarded exchange that carried rate-limit headers. The next two exercise
//! the comparison itself through the cache's public API, without a socket.
//! The fourth proves the floor at the estimator's caller and its rendered
//! line, through the resolver and the shipped binary. The fifth proves a
//! reading file written before this package still loads, and still
//! estimates over the whole window.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::Parser;

use glasshouse::Runtime;
use glasshouse::config::{EffectiveConfig, EntitlementTelemetry, UserConfig};
use glasshouse::gateway::{Route, Upstream, UpstreamBackend};
use glasshouse::integrations::IntegrationId;
use glasshouse::profile::{BackendResource, LaunchProfile};
use glasshouse::provider::telemetry::{GatewayQuotaCache, RateLimitHeaders};
use glasshouse::routing::evidence::{
    CLASSIFICATION_EVIDENCE_WINDOW_SECONDS, EvidenceLedger, FailureClass, HeadroomBand,
    NewObservation, Outcome, estimate_subscription_headroom,
};
use glasshouse::routing::{AssignedModel, Cost, CredentialId};
use glasshouse::secret::{EnvironmentSecretStore, Secret, SecretRef, SecretStore};

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the system clock is after the epoch")
        .as_secs() as i64
}

// ===========================================================================
// A real gateway exchange — `gateway_retry_after.rs`'s own fixture shape,
// adapted for a scripted sequence of successful responses carrying differing
// rate-limit headers rather than a single 429.
// ===========================================================================

const PROVIDER: &str = "fixture-estimator-reset";
const MODEL: &str = "stub-model";

/// A stand-in provider credential, resolved through the real environment
/// store — `gateway_retry_after.rs`'s own `test_credential`, for the same
/// reason: `secret::Secret` has no public way to mint one outside
/// `crate::secret`.
fn test_credential(var: &str) -> Secret {
    // SAFETY: `var` is unique to the one caller that set it, and it is
    // removed again immediately below, before the resolved value is even
    // inspected, so no other test can observe it set.
    unsafe {
        std::env::set_var(var, "sk-planted-not-a-real-key-estimator-reset");
    }
    let resolved = EnvironmentSecretStore::new()
        .resolve(&SecretRef::Environment {
            var: var.to_owned(),
        })
        .expect("the variable was just set");
    unsafe {
        std::env::remove_var(var);
    }
    resolved
}

fn credential_id(var: &str) -> CredentialId {
    CredentialId::new(
        PROVIDER,
        SecretRef::Environment {
            var: var.to_owned(),
        },
    )
}

/// Read a request whole before answering it — `gateway_retry_after.rs`'s own
/// helper and its own doc explains why a single small read is a race with
/// `ureq`'s separate head/body writes.
fn read_whole_request(stream: &mut TcpStream) {
    let mut reader = BufReader::new(stream);
    let mut declared = 0usize;
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            declared = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; declared];
    let _ = reader.read_exact(&mut body);
}

/// A full HTTP/1.1 `200 OK` response carrying `headers`, with a small fixed
/// JSON body — proven sufficient for a forwarded exchange to complete in
/// `gateway::conformance`'s own quota-cache fixture
/// (`a_real_forwarded_exchanges_rate_limit_headers_are_persisted_for_the_next_process`).
fn scripted_200_response(headers: &[(&str, &str)]) -> String {
    let body = "{\"ok\":true}";
    let mut response = String::from("HTTP/1.1 200 OK\r\n");
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    response
}

/// A local HTTP server answering each successive connection with the next
/// response in `responses`, repeating the last one if more connections
/// arrive than were scripted.
fn stub_scripted_server(responses: Vec<String>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback is bindable");
    let address = listener
        .local_addr()
        .expect("a bound listener has a local address");
    listener
        .set_nonblocking(true)
        .expect("a listener can be put in polling mode");

    std::thread::Builder::new()
        .name("estimator-reset-stub".to_owned())
        .spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut served = 0usize;
            loop {
                let mut stream = match listener.accept() {
                    Ok((stream, _peer)) => stream,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(_) => return,
                };
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));

                read_whole_request(&mut stream);

                let response = responses
                    .get(served)
                    .or_else(|| responses.last())
                    .cloned()
                    .unwrap_or_else(|| "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_owned());
                served += 1;
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        })
        .expect("can spawn the stub server thread");

    address
}

fn messages_request(token: &str) -> Vec<u8> {
    let body = format!(r#"{{"model":"{MODEL}"}}"#);
    format!(
        "POST /v1/messages HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Authorization: Bearer {token}\r\n\
         Content-Type: application/json\r\n\
         Anthropic-Version: 2023-06-01\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {body}",
        body.len()
    )
    .into_bytes()
}

/// Send `raw` and return everything the gateway wrote back, to the close —
/// `gateway_degrade.rs`'s and `gateway_retry_after.rs`'s own `send_and_read`.
fn send_and_read(address: SocketAddr, raw: &[u8]) -> String {
    let mut client = TcpStream::connect(address).expect("the gateway accepts connections");
    client
        .set_read_timeout(Some(Duration::from_secs(20)))
        .expect("a non-zero read timeout is valid");
    client
        .write_all(raw)
        .expect("the gateway reads the request");
    client.flush().expect("the gateway reads the request");
    let mut out = Vec::new();
    client
        .read_to_end(&mut out)
        .expect("the gateway answers and then closes");
    String::from_utf8_lossy(&out).into_owned()
}

/// A gateway pointed at a stub upstream, persisting every forwarded
/// exchange's rate-limit headers to `quota_cache` — `gateway_retry_after.rs`'s
/// own `gateway_to_stub`, with a quota cache in the telemetry slot
/// `start_if_required_with_telemetry` reserves for it instead of a health
/// cache.
fn gateway_to_stub(
    credential_var: &str,
    upstream_address: SocketAddr,
    quota_cache: GatewayQuotaCache,
) -> glasshouse::gateway::Gateway {
    let backend = UpstreamBackend::new(
        PROVIDER.to_owned(),
        vec![Route::new(
            "anthropic-messages".to_owned(),
            &["/messages"],
            &format!("http://{upstream_address}"),
        )],
        test_credential(credential_var),
        credential_id(credential_var),
        Cost::Metered,
    )
    .expect("a loopback http URL is absolute and this credential is header-safe");
    let upstream = Upstream::with_failover(vec![backend]).expect("one backend is not none");

    let mut profile = LaunchProfile::native(IntegrationId::ClaudeCode);
    profile.backend = BackendResource::GlasshouseGateway;
    let gateway = glasshouse::gateway::start_if_required_with_telemetry(
        &[profile],
        || Ok(upstream),
        Some(quota_cache),
        None,
        None,
    )
    .expect("loopback is bindable")
    .expect("a gateway-backed profile requires a gateway");

    gateway.routing().bind(
        "claude-code",
        "anthropic-messages",
        AssignedModel::named(MODEL),
        gateway.upstream(),
    );

    gateway
}

/// Poll `cache` until it holds a reading whose `remaining` count matches
/// `expected_remaining` — a marker distinct across every scripted exchange
/// even when two of them share the same `limit`, so this does not depend on
/// two exchanges landing in different wall-clock seconds the way polling on
/// `observed_at_unix` would. The write happens on the connection thread,
/// after the response is already back on the client's wire
/// (`gateway::conformance`'s own documented race), so `send_and_read`
/// returning is not proof the cache has been written yet.
fn wait_for_quota_reading(
    cache: &GatewayQuotaCache,
    provider: &str,
    expected_remaining: i64,
) -> (RateLimitHeaders, i64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(found) = cache.load(provider)
            && found.0.remaining() == Some(expected_remaining)
        {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "no reading with remaining={expected_remaining} for provider {provider} within 5s"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// **Test (a).** Two exchanges through a real gateway whose upstream states
/// `limit: 100` then `limit: 50`: the second's differing stated ceiling is a
/// regime change, and the cache file must carry its own instant. A third
/// exchange restating `limit: 50` must carry that same instant forward,
/// never replace it with its own later one.
#[test]
fn a_real_gateways_stated_ceiling_change_is_detected_and_the_new_instant_is_carried_forward() {
    const CREDENTIAL_VAR: &str = "GLASSHOUSE_ESTIMATOR_RESET_TEST_KEY_CEILING";

    let quota_dir = tempfile::tempdir().expect("a temp directory can be created");
    let quota_cache = GatewayQuotaCache::at(quota_dir.path());

    let responses = vec![
        scripted_200_response(&[("ratelimit-limit", "100"), ("ratelimit-remaining", "99")]),
        scripted_200_response(&[("ratelimit-limit", "50"), ("ratelimit-remaining", "49")]),
        scripted_200_response(&[("ratelimit-limit", "50"), ("ratelimit-remaining", "48")]),
    ];
    let upstream_address = stub_scripted_server(responses);
    let gateway = gateway_to_stub(CREDENTIAL_VAR, upstream_address, quota_cache.clone());

    assert!(
        quota_cache.load(PROVIDER).is_none(),
        "no reading should exist for this provider before any exchange has run"
    );

    let first = send_and_read(
        gateway.address(),
        &messages_request(gateway.token().expose()),
    );
    assert!(
        first.starts_with("HTTP/1.1 200"),
        "the gateway must relay the stub's own 200: {first}"
    );
    let (first_headers, _) = wait_for_quota_reading(&quota_cache, PROVIDER, 99);
    assert_eq!(first_headers.limit(), Some(100));
    assert_eq!(
        quota_cache.regime_changed_at(PROVIDER),
        None,
        "a first reading must record no change"
    );

    let second = send_and_read(
        gateway.address(),
        &messages_request(gateway.token().expose()),
    );
    assert!(second.starts_with("HTTP/1.1 200"), "{second}");
    let (second_headers, second_observed_at) = wait_for_quota_reading(&quota_cache, PROVIDER, 49);
    assert_eq!(second_headers.limit(), Some(50));
    assert_eq!(
        quota_cache.regime_changed_at(PROVIDER),
        Some(second_observed_at),
        "a stated ceiling difference must record the instant of the exchange that detected it"
    );

    let third = send_and_read(
        gateway.address(),
        &messages_request(gateway.token().expose()),
    );
    assert!(third.starts_with("HTTP/1.1 200"), "{third}");
    let (third_headers, _) = wait_for_quota_reading(&quota_cache, PROVIDER, 48);
    assert_eq!(third_headers.limit(), Some(50));
    assert_eq!(
        quota_cache.regime_changed_at(PROVIDER),
        Some(second_observed_at),
        "an unchanged ceiling must carry the earlier regime-change instant forward, never the \
         newest exchange's own instant"
    );
}

// ===========================================================================
// The detector's own comparison, through the cache's public API — no socket.
// ===========================================================================

/// **Test (b).** `remaining` and `reset` moving, with `limit` unchanged,
/// must never read as the stated ceiling changing — the pool being spent is
/// not the ceiling itself.
#[test]
fn a_ceiling_that_is_unchanged_but_the_pool_itself_moving_records_no_change() {
    let dir = tempfile::tempdir().expect("a temp directory can be created");
    let cache = GatewayQuotaCache::at(dir.path());
    let observed_first = 1_800_000_000_i64;

    cache.store(
        PROVIDER,
        &RateLimitHeaders::read(vec![
            ("ratelimit-limit", "300"),
            ("ratelimit-remaining", "300"),
            ("ratelimit-reset", "60"),
        ]),
        observed_first,
    );
    assert_eq!(
        cache.regime_changed_at(PROVIDER),
        None,
        "a first reading must record no change"
    );

    cache.store(
        PROVIDER,
        &RateLimitHeaders::read(vec![
            ("ratelimit-limit", "300"),
            ("ratelimit-remaining", "150"),
            ("ratelimit-reset", "30"),
        ]),
        observed_first + 30,
    );
    assert_eq!(
        cache
            .load(PROVIDER)
            .and_then(|(headers, _)| headers.remaining()),
        Some(150),
        "the pool reading must still have moved, or this test proves nothing"
    );
    assert_eq!(
        cache.regime_changed_at(PROVIDER),
        None,
        "remaining and reset moving alone must never read as the stated ceiling changing"
    );
}

/// **Test (e), unit.** The comparison only ever looks at `limit`,
/// `window_seconds` and `token_limit`, and only when **both** readings state
/// a value for the one being compared — a ceiling present on one side and
/// absent on the other is not evidence of a change, in either direction.
#[test]
fn stated_ceiling_comparison_ignores_a_field_absent_on_either_side() {
    let dir = tempfile::tempdir().expect("a temp directory can be created");
    let cache = GatewayQuotaCache::at(dir.path());

    // limit=100, window_seconds=60, token_limit absent.
    cache.store(
        PROVIDER,
        &RateLimitHeaders::read(vec![
            ("ratelimit-limit", "100"),
            ("x-ratelimit-window", "60"),
        ]),
        1_000,
    );
    assert_eq!(cache.regime_changed_at(PROVIDER), None);

    // window_seconds now absent — present on the previous reading only, so
    // it cannot be compared and is not evidence of a change; limit itself
    // is unchanged too.
    cache.store(
        PROVIDER,
        &RateLimitHeaders::read(vec![("ratelimit-limit", "100")]),
        1_060,
    );
    assert_eq!(
        cache.regime_changed_at(PROVIDER),
        None,
        "a ceiling present on only one side is not evidence of a change"
    );

    // token_limit appears for the first time — present on the new reading
    // only, so it is likewise not evidence of a change.
    cache.store(
        PROVIDER,
        &RateLimitHeaders::read(vec![
            ("ratelimit-limit", "100"),
            ("x-ratelimit-limit-tokens", "500"),
        ]),
        1_120,
    );
    assert_eq!(
        cache.regime_changed_at(PROVIDER),
        None,
        "a ceiling appearing for the first time, absent on the reading it replaces, is not \
         evidence of a change"
    );

    // Now `limit` itself actually differs with both sides stating a value —
    // a real regime change.
    cache.store(
        PROVIDER,
        &RateLimitHeaders::read(vec![
            ("ratelimit-limit", "50"),
            ("x-ratelimit-limit-tokens", "500"),
        ]),
        1_180,
    );
    assert_eq!(
        cache.regime_changed_at(PROVIDER),
        Some(1_180),
        "a ceiling stated on both sides that actually differs must be detected"
    );
}

// ===========================================================================
// The estimator's floor, and its rendered line — through the resolver and
// the shipped binary.
// ===========================================================================

const RESOLVER_PROVIDER: &str = "estimator-reset-probe";
const RESOLVER_VAR: &str = "GLASSHOUSE_ESTIMATOR_RESET_PROBE_KEY";
const RESOLVER_LABEL: &str = "estimator-reset-probe/GLASSHOUSE_ESTIMATOR_RESET_PROBE_KEY";

fn pool_config() -> String {
    format!(
        "[providers.{RESOLVER_PROVIDER}]\ntemplate = \"openrouter\"\n\
         credential_env = [\"{RESOLVER_VAR}\"]\n\n\
         [entitlements.acct]\nkind = \"claude\"\nvendor = \"claude\"\n\
         provider = \"{RESOLVER_PROVIDER}\"\ncredential = {{ env = \"{RESOLVER_VAR}\" }}\n"
    )
}

fn user_config() -> UserConfig {
    toml::from_str(&format!("version = 1\n\n{}", pool_config())).expect("the fixture parses")
}

/// A bootstrapped project inside `base` — `subscription_estimator.rs`'s own
/// idiom.
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

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.toml"),
            format!("version = 1\n\n{}", pool_config()),
        )
        .unwrap();

        let cli = glasshouse::Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            config_dir.to_str().unwrap(),
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
    fn glasshouse(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .env("PATH", self.base.join("empty-path"))
            .output()
            .expect("the glasshouse binary must be runnable")
    }
}

fn accepted(account: Option<&str>, at: i64) -> NewObservation {
    NewObservation::new(RESOLVER_PROVIDER, "some-model")
        .with_route(Some("anthropic-messages"))
        .with_harness(Some("claude-code"))
        .with_quota_context(account)
        .with_timing(Some(at), Some(at + 5))
        .with_outcome(Outcome::Succeeded)
}

fn throttle(account: Option<&str>, at: i64) -> NewObservation {
    NewObservation::new(RESOLVER_PROVIDER, "some-model")
        .with_route(Some("anthropic-messages"))
        .with_harness(Some("claude-code"))
        .with_quota_context(account)
        .with_timing(Some(at), Some(at + 5))
        .with_outcome(Outcome::Failed)
        .with_failure_class(Some(FailureClass::Throttle))
}

/// The `glasshouse entitlements` view renders one entry as `` `name`
/// (describe) `` followed by its facets on the next line —
/// `subscription_estimator.rs`'s own `facets_line`.
fn facets_line(view: &str, name: &str) -> String {
    let mut lines = view.lines();
    while let Some(line) = lines.next() {
        if line.trim_start().starts_with(&format!("`{name}`")) {
            return lines
                .next()
                .unwrap_or_else(|| panic!("no facet line after `{name}` in:\n{view}"))
                .to_owned();
        }
    }
    panic!("no entry for `{name}` in:\n{view}");
}

/// **Test (c).** Rows planted before and after a regime change: the
/// estimate's band must reflect only the rows at or after it, and the
/// rendered line must say so.
#[test]
fn the_floor_keeps_only_rows_at_or_after_the_regime_change_and_the_render_says_so() {
    let tmp = tempfile::tempdir().expect("a temp directory can be created");
    let fixture = Fixture::new(tmp.path());
    let now = now_unix();

    let quota_dir = tmp.path().join("data").join("gateway-quota");
    let quota = GatewayQuotaCache::at(&quota_dir);
    quota.store(
        RESOLVER_PROVIDER,
        &RateLimitHeaders::read(vec![("ratelimit-limit", "100")]),
        now - 1_200,
    );
    quota.store(
        RESOLVER_PROVIDER,
        &RateLimitHeaders::read(vec![("ratelimit-limit", "50")]),
        now - 900,
    );
    let regime_changed_at = quota
        .regime_changed_at(RESOLVER_PROVIDER)
        .expect("the second store's differing limit must have recorded a regime change");
    assert_eq!(regime_changed_at, now - 900);

    let ledger = fixture.ledger();
    // Before the regime change: a throttled burst, recent enough on its own
    // to read as pressure.
    ledger
        .record(throttle(Some(RESOLVER_LABEL), now - 1_000), now - 995)
        .unwrap();
    // After the regime change: one clean accepted exchange.
    ledger
        .record(accepted(Some(RESOLVER_LABEL), now - 100), now - 95)
        .unwrap();
    let rows = ledger
        .observations_in_window(now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();

    // Premise (§17): without the floor, the pre-change throttle is still
    // "recent" and must read the whole window as under pressure — otherwise
    // this test would not distinguish a floor working from a floor absent.
    let unfiltered = estimate_subscription_headroom(
        &rows,
        RESOLVER_PROVIDER,
        Some(RESOLVER_LABEL),
        now,
        None,
        None,
    )
    .expect("a throttle and an accepted row are both real evidence");
    assert_eq!(
        unfiltered.band,
        HeadroomBand::Exhausted,
        "premise: without a floor the pre-change throttle must still read as pressure: \
         {unfiltered:?}"
    );

    let user = user_config();
    let effective = EffectiveConfig::new(&user, None);
    let telemetry = EntitlementTelemetry::new(now)
        .with_gateway_quota(&quota)
        .with_observations(&rows);
    let entries = effective
        .configured_entitlements_with_telemetry(&telemetry)
        .expect("the pool resolves");
    let entry = entries
        .iter()
        .find(|entry| entry.name() == "acct")
        .expect("the one configured entitlement");

    let floored = entry
        .headroom_estimate()
        .expect("the post-change accepted row is real evidence on its own");
    assert_eq!(
        floored.band,
        HeadroomBand::Ample,
        "the floor must drop the pre-change throttle, leaving only the clean post-change row: \
         {floored:?}"
    );
    assert_eq!(
        floored.since_unix,
        Some(regime_changed_at),
        "the estimate must carry the regime-change instant it was floored to: {floored:?}"
    );

    let stdout =
        String::from_utf8_lossy(&fixture.glasshouse(&["entitlements"]).stdout).into_owned();
    let facet_line = facets_line(&stdout, "acct");
    assert!(
        facet_line.contains("limits changed"),
        "the rendered line must say the estimate was floored to a regime change: {facet_line}"
    );
    assert!(
        !facet_line.to_lowercase().contains("exhausted"),
        "the rendered band must reflect the floored (post-change) evidence, not the pre-change \
         pressure the unfiltered premise above showed: {facet_line}"
    );
}

// ===========================================================================
// A reading file from before this field existed.
// ===========================================================================

/// **Test (d).** A cache file in the exact shape the format had before this
/// package — no `regime_changed_at_unix` key at all — must still load, must
/// read as no change recorded, and the estimator must draw on the whole
/// window rather than silently narrowing it.
#[test]
fn a_reading_file_written_before_this_field_existed_loads_as_no_change_recorded_and_estimates_the_whole_window()
 {
    let tmp = tempfile::tempdir().expect("a temp directory can be created");
    let fixture = Fixture::new(tmp.path());
    let now = now_unix();

    let quota_dir = tmp.path().join("data").join("gateway-quota");
    let quota = GatewayQuotaCache::at(&quota_dir);
    quota.store(
        RESOLVER_PROVIDER,
        &RateLimitHeaders::read(vec![("ratelimit-limit", "300")]),
        now - 5_000,
    );

    // Rewrite the file this store just wrote in the exact shape the format
    // had before this package: the same file, minus the one key this
    // package added.
    let path = std::fs::read_dir(&quota_dir)
        .expect("the store created its directory")
        .next()
        .expect("the store wrote one file")
        .expect("a readable directory entry")
        .path();
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(
        value
            .as_object_mut()
            .unwrap()
            .remove("regime_changed_at_unix")
            .is_some(),
        "the field this package added must actually be in the freshly written file, or this \
         test proves nothing"
    );
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

    let (headers, observed_at) = quota
        .load(RESOLVER_PROVIDER)
        .expect("a file missing only this new field must still parse");
    assert_eq!(headers.limit(), Some(300));
    assert_eq!(observed_at, now - 5_000);
    assert_eq!(
        quota.regime_changed_at(RESOLVER_PROVIDER),
        None,
        "a file written before this field existed must read as no change recorded"
    );

    // One row a floor would drop if this file somehow produced one —
    // proof the estimator drew on the whole window instead.
    let ledger = fixture.ledger();
    ledger
        .record(accepted(Some(RESOLVER_LABEL), now - 4_900), now - 4_895)
        .unwrap();
    let rows = ledger
        .observations_in_window(now, CLASSIFICATION_EVIDENCE_WINDOW_SECONDS)
        .unwrap();

    let user = user_config();
    let effective = EffectiveConfig::new(&user, None);
    let telemetry = EntitlementTelemetry::new(now)
        .with_gateway_quota(&quota)
        .with_observations(&rows);
    let entries = effective
        .configured_entitlements_with_telemetry(&telemetry)
        .expect("the pool resolves");
    let entry = entries
        .iter()
        .find(|entry| entry.name() == "acct")
        .expect("the one configured entitlement");
    let estimate = entry
        .headroom_estimate()
        .expect("the planted row is real evidence");
    assert_eq!(
        estimate.since_unix, None,
        "no change was ever recorded, so the estimate must not claim a regime instant: \
         {estimate:?}"
    );

    let stdout =
        String::from_utf8_lossy(&fixture.glasshouse(&["entitlements"]).stdout).into_owned();
    let facet_line = facets_line(&stdout, "acct");
    assert!(
        !facet_line.contains("limits changed"),
        "with no change ever recorded, the render must carry no floor clause at all: {facet_line}"
    );
}
