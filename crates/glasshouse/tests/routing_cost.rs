//! **`glasshouse routing-cost`** — capability map line 1464: what
//! Glasshouse's own routing model has consumed, in tokens and requests,
//! apart from every other row this project's evidence ledger holds.
//!
//! Every test here drives the shipped binary, not
//! `EvidenceLedger::consumption_by_purpose` directly — practice §35's *"a
//! caller every test bypasses is not a caller"*: the aggregate existing is
//! not the same fact as the command surface reading it correctly, and the
//! command is what map line 1464 is actually about.
//!
//! # The hazard this file exists to pin
//!
//! A row nobody counted (this build's gateway relay never parses a reply
//! body, so every coding-agent exchange leaves its token columns `NULL`)
//! must never print as `0`. "not counted" and "0" are different facts, and
//! [`section`]/[`value_after`] below assert the *exact* rendered value for a
//! token field precisely so a future change that coerces an absent count to
//! `0` fails a string-equality assertion rather than a loose `contains`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Parser;
use rusqlite::Connection;

use glasshouse::gateway::{Route, Upstream};
use glasshouse::integrations::IntegrationId;
use glasshouse::profile::{BackendResource, LaunchProfile};
use glasshouse::routing::evidence::{EvidenceLedger, NewObservation, ObservationQuery};
use glasshouse::routing::{AssignedModel, CredentialId};
use glasshouse::secret::{EnvironmentSecretStore, Secret, SecretRef, SecretStore};
use glasshouse::{Cli, Runtime, bootstrap};

/// A bootstrapped project inside `base`, sharing `base`'s data and config
/// roots — the same shape `tests/memory_project_scope.rs` uses, so that two
/// fixtures over one `base` are two real projects on one machine, each with
/// its own canonicalised root and its own `glasshouse.db`.
struct Fixture {
    base: PathBuf,
    root: PathBuf,
    runtime: Runtime,
}

/// What one `glasshouse routing-cost` run printed.
struct Report {
    stdout: String,
    stderr: String,
    status: std::process::ExitStatus,
}

impl Fixture {
    fn new(base: &Path, name: &str) -> Self {
        let root: PathBuf = base.join("workspace").join(name);
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
        let runtime = bootstrap(&cli, &root).unwrap();
        Self {
            base: base.to_path_buf(),
            root,
            runtime,
        }
    }

    fn project_id(&self) -> &str {
        self.runtime.project().id().as_str()
    }

    fn ledger(&self) -> EvidenceLedger {
        EvidenceLedger::open(&self.runtime).unwrap()
    }

    fn raw_connection(&self) -> Connection {
        Connection::open(self.runtime.database_path()).unwrap()
    }

    /// Record one observation through the real ledger API — every column a
    /// producer might set, so token counts left `None` become `NULL` exactly
    /// as `NewObservation::with_tokens`'s own doc comment requires.
    fn record(
        &self,
        provider: &str,
        model: &str,
        purpose: Option<&str>,
        tokens: Option<(i64, i64, i64)>,
        observed_at_unix: i64,
    ) {
        let mut observation = NewObservation::new(provider, model).with_purpose(purpose);
        if let Some((input, output, cached)) = tokens {
            observation = observation.with_tokens(Some(input), Some(output), Some(cached));
        }
        self.ledger().record(observation, observed_at_unix).unwrap();
    }

    /// Record one observation exactly the shape
    /// `crate::gateway::session::SessionRouting::record` writes for a real
    /// coding-agent exchange: no purpose, a named harness, and no token
    /// counts — the relay never parses a reply body to read any.
    fn record_gateway_exchange(&self, provider: &str, model: &str, harness: &str, at: i64) {
        let observation = NewObservation::new(provider, model).with_harness(Some(harness));
        self.ledger().record(observation, at).unwrap();
    }

    /// Run `glasshouse routing-cost`, exactly as a person runs it.
    fn routing_cost(&self, hours: Option<u32>) -> Report {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glasshouse"));
        command
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .arg("routing-cost");
        if let Some(hours) = hours {
            command.arg("--hours").arg(hours.to_string());
        }
        let output = command
            .output()
            .expect("the glasshouse binary must be runnable");
        Report {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status,
        }
    }
}

/// Now, in the same clock `EvidenceLedger::consumption_by_purpose` reads
/// its window against — every fixture below records observations a few
/// seconds in the past so they land comfortably inside the default 24-hour
/// window without this file needing to know the command's own default.
fn now() -> i64 {
    glasshouse::provider::cache::now_unix_seconds()
}

/// Insert one observation directly, bypassing `EvidenceLedger::record` and
/// the project-id trigger — the only way to plant a row belonging to another
/// project, which is exactly what the trigger exists to prevent. Models a
/// row that reached the file by a route the trigger never saw: a restored
/// backup, a hand-edited file, a build whose schema predates the guard —
/// the same premise `tests/memory_project_scope.rs::plant_foreign_memory`
/// uses for the memory store's own version of this boundary.
fn plant_foreign_observation(conn: &Connection, project_id: &str, purpose: Option<&str>, at: i64) {
    conn.execute_batch("DROP TRIGGER routing_observations_reject_foreign_project_insert;")
        .unwrap();
    conn.execute(
        "INSERT INTO routing_observations
            (project_id, observed_at, provider, model, purpose,
             input_tokens, output_tokens, cached_input_tokens)
         VALUES (?1, ?2, 'foreign-provider', 'foreign-model', ?3, 999, 999, 999)",
        rusqlite::params![project_id, at, purpose],
    )
    .unwrap();
    conn.execute_batch(
        "CREATE TRIGGER routing_observations_reject_foreign_project_insert
         BEFORE INSERT ON routing_observations
         FOR EACH ROW
         WHEN NEW.project_id IS NOT (
             SELECT value FROM project_metadata WHERE key = 'project_id'
         )
         BEGIN
             SELECT RAISE(ABORT, 'routing observation belongs to a different project');
         END;",
    )
    .unwrap();
}

/// The rendered block for one purpose group's label, exactly as
/// `main.rs::render_routing_cost` writes it: from the blank line before
/// `  {label}` to the next blank line (or the end of the report).
fn section(report: &str, label: &str) -> String {
    let marker = format!("\n  {label}\n");
    let start = report
        .find(&marker)
        .unwrap_or_else(|| panic!("no section for {label:?} in:\n{report}"));
    let rest = &report[start + 1..];
    let end = rest.find("\n\n").unwrap_or(rest.len());
    rest[..end].to_owned()
}

/// The exact value printed after one field's fixed-width label, up to the
/// end of its line — strict on purpose, so a render that slips a stray digit
/// or a different word into a "not counted" field fails a string comparison
/// rather than surviving a loose `contains`.
fn value_after(text: &str, field_prefix: &str) -> String {
    let start = text
        .find(field_prefix)
        .unwrap_or_else(|| panic!("missing {field_prefix:?} in:\n{text}"))
        + field_prefix.len();
    let rest = &text[start..];
    let end = rest.find('\n').unwrap_or(rest.len());
    rest[..end].to_owned()
}

const REQUESTS: &str = "    requests            : ";
const INPUT_TOKENS: &str = "    input tokens        : ";
const OUTPUT_TOKENS: &str = "    output tokens       : ";
const CACHED_TOKENS: &str = "    cached input tokens : ";
const FIRST_BYTE_SAMPLES: &str = "    first-byte samples  : ";
const TIME_TO_FIRST_BYTE: &str = "    time to first byte  : ";
const FIRST_TOKEN_SAMPLES: &str = "    first-token samples : ";
const TIME_TO_FIRST_TOKEN: &str = "    time to first token : ";
const FIRST_TOOL_CALL_SAMPLES: &str = "    first-tool-call samples : ";
const TIME_TO_FIRST_TOOL_CALL: &str = "    time to first tool call : ";

// ---------------------------------------------------------------------------
// 1. Attribution: the routing model's own spend, apart from every other row.
// ---------------------------------------------------------------------------

/// **The joined link.** A ledger holding one `classification` row with real
/// token counts and one row with no purpose and no counts: the report
/// attributes the counted tokens to `classification` and does not smear them
/// onto the other group. Asserts the exact numbers, not just the labels.
#[test]
fn the_classification_group_is_attributed_its_own_tokens_and_no_others() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let at = now() - 60;

    fixture.record(
        "alpha-runner",
        "alpha-model",
        Some("classification"),
        Some((111, 222, 333)),
        at,
    );
    fixture.record_gateway_exchange("gateway-provider", "gateway-model", "claude-code", at);

    let run = fixture.routing_cost(None);
    assert!(run.status.success(), "stderr: {}", run.stderr);

    let classification = section(&run.stdout, "classification");
    assert_eq!(value_after(&classification, REQUESTS), "1");
    assert_eq!(value_after(&classification, INPUT_TOKENS), "111");
    assert_eq!(value_after(&classification, OUTPUT_TOKENS), "222");
    assert_eq!(value_after(&classification, CACHED_TOKENS), "333");

    let coding_agent = section(&run.stdout, "coding-agent (gateway relay)");
    assert_eq!(value_after(&coding_agent, REQUESTS), "1");
    assert!(
        !coding_agent.contains("111")
            && !coding_agent.contains("222")
            && !coding_agent.contains("333"),
        "the coding-agent group must never carry the classification group's own numbers:\n{}",
        run.stdout
    );
}

// ---------------------------------------------------------------------------
// 2. The hazard: an uncounted group renders "not counted", never a digit.
// ---------------------------------------------------------------------------

/// **The hazard this package exists to pin.** A group whose every row left
/// its token columns `NULL` — the coding-agent shape, gateway rows this
/// build never parses — renders the words *not counted*, and the token
/// fields carry no digit at all, even though its request count is a real,
/// nonzero number.
#[test]
fn a_group_with_no_counted_tokens_never_renders_a_digit_for_them() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let at = now() - 60;

    fixture.record_gateway_exchange("gateway-provider", "gateway-model", "claude-code", at);
    fixture.record_gateway_exchange("gateway-provider", "gateway-model", "claude-code", at - 1);

    let run = fixture.routing_cost(None);
    assert!(run.status.success(), "stderr: {}", run.stderr);

    let coding_agent = section(&run.stdout, "coding-agent (gateway relay)");
    assert_eq!(value_after(&coding_agent, REQUESTS), "2");
    for field in [INPUT_TOKENS, OUTPUT_TOKENS, CACHED_TOKENS] {
        let value = value_after(&coding_agent, field);
        assert_eq!(
            value, "not counted",
            "a group with no counted rows must say so, never a number: {field:?} was {value:?}"
        );
        assert!(
            !value.chars().any(|c| c.is_ascii_digit()),
            "\"not counted\" must never carry a stray digit: {field:?} was {value:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 3. An empty ledger is an honest, zero-exit report — never an error.
// ---------------------------------------------------------------------------

/// A brand-new project with no routing observations at all exits `0` and
/// says so in words, rather than erroring or printing nothing.
#[test]
fn an_empty_ledger_reports_honestly_and_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");

    let run = fixture.routing_cost(None);
    assert!(
        run.status.success(),
        "an empty ledger is not an error: {}",
        run.stderr
    );
    assert!(
        run.stdout
            .contains("no routing observations recorded in this window"),
        "an empty ledger must say so rather than printing a blank report:\n{}",
        run.stdout
    );
}

/// The same, for a project that has observations under other purposes but
/// none at all under `classification` — the other half of requirement 4.
#[test]
fn a_ledger_with_no_classification_row_still_reports_honestly() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let at = now() - 60;
    fixture.record_gateway_exchange("gateway-provider", "gateway-model", "claude-code", at);

    let run = fixture.routing_cost(None);
    assert!(run.status.success(), "stderr: {}", run.stderr);
    assert!(
        !run.stdout.contains("\n  classification\n"),
        "a ledger with no classification row must not fabricate one:\n{}",
        run.stdout
    );
    let coding_agent = section(&run.stdout, "coding-agent (gateway relay)");
    assert_eq!(value_after(&coding_agent, REQUESTS), "1");
}

// ---------------------------------------------------------------------------
// 2b. `purpose` alone cannot tell two `NULL`-purpose producers apart — only
// `harness_recorded` can, and the two must never be merged.
// ---------------------------------------------------------------------------

/// **The orchestrator's own correction to this package.** `routing_observations`
/// has three production writers, and two of them — memory extraction and the
/// gateway relay — both leave `purpose` `NULL`. Extraction's rows carry real
/// token counts; the gateway's never do (`gateway/ingress.rs` never parses a
/// reply body). Grouping on `purpose` alone would fold a genuinely counted
/// total into the one group line 1464 asks to be reported as *not counted*.
/// `harness_recorded` — set only by the gateway's own producer — is what
/// keeps them apart.
#[test]
fn coding_agent_rows_and_other_unpurposed_rows_are_never_merged() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let at = now() - 60;

    // The extraction shape: no purpose, no harness, real tokens.
    fixture.record(
        "omega-runner",
        "extraction-model",
        None,
        Some((40, 41, 42)),
        at,
    );
    // The gateway shape: no purpose, a named harness, no tokens.
    fixture.record_gateway_exchange("gateway-provider", "gateway-model", "claude-code", at - 1);

    let run = fixture.routing_cost(None);
    assert!(run.status.success(), "stderr: {}", run.stderr);

    let other = section(&run.stdout, "(no purpose or harness recorded)");
    assert_eq!(value_after(&other, REQUESTS), "1");
    assert_eq!(value_after(&other, INPUT_TOKENS), "40");
    assert_eq!(value_after(&other, OUTPUT_TOKENS), "41");
    assert_eq!(value_after(&other, CACHED_TOKENS), "42");

    let coding_agent = section(&run.stdout, "coding-agent (gateway relay)");
    assert_eq!(value_after(&coding_agent, REQUESTS), "1");
    assert_eq!(value_after(&coding_agent, INPUT_TOKENS), "not counted");
    assert_eq!(value_after(&coding_agent, OUTPUT_TOKENS), "not counted");
    assert_eq!(value_after(&coding_agent, CACHED_TOKENS), "not counted");
    assert!(
        !coding_agent.contains("40")
            && !coding_agent.contains("41")
            && !coding_agent.contains("42"),
        "the coding-agent group must never inherit another producer's counted tokens:\n{}",
        run.stdout
    );
}

// ---------------------------------------------------------------------------
// 4. Cross-project isolation.
// ---------------------------------------------------------------------------

/// **Line 1343's "physically project-scoped," proved against the aggregate
/// itself, not just against the file boundary.** Two real projects share one
/// `--data-dir`, so each still gets its own `glasshouse.db`
/// (`Runtime::state_dir` is keyed by project id) — but that alone would let
/// this test pass even if `consumption_by_purpose`'s own `WHERE project_id =
/// ?1` were deleted, because there would be nothing in the same file to leak.
/// So the foreign row is planted **inside beta's own database file**, under
/// the *same* purpose as beta's real row, which is what makes the SQL
/// `WHERE` clause the only thing that can keep the totals apart.
#[test]
fn a_row_planted_under_another_projects_id_never_contributes_to_this_projects_totals() {
    let tmp = tempfile::tempdir().unwrap();
    let alpha = Fixture::new(tmp.path(), "alpha");
    let beta = Fixture::new(tmp.path(), "beta");
    let at = now() - 60;

    // beta's own, legitimate row — so a totals report of nothing at all
    // could not pass this test by accident.
    beta.record(
        "beta-runner",
        "beta-model",
        Some("classification"),
        Some((5, 6, 7)),
        at,
    );

    let conn = beta.raw_connection();
    plant_foreign_observation(&conn, alpha.project_id(), Some("classification"), at);
    drop(conn);

    let run = beta.routing_cost(None);
    assert!(run.status.success(), "stderr: {}", run.stderr);

    let classification = section(&run.stdout, "classification");
    assert_eq!(
        value_after(&classification, REQUESTS),
        "1",
        "a foreign-project row must not inflate this project's request count:\n{}",
        run.stdout
    );
    assert_eq!(value_after(&classification, INPUT_TOKENS), "5");
    assert_eq!(value_after(&classification, OUTPUT_TOKENS), "6");
    assert_eq!(value_after(&classification, CACHED_TOKENS), "7");
    assert!(
        !run.stdout.contains("999"),
        "a row planted under another project's id must never appear in this project's totals:\n{}",
        run.stdout
    );
}

// ---------------------------------------------------------------------------
// 5. Capability map line 1331's gateway half: a real first-byte instant,
//    honestly absent when nothing ever answered.
// ---------------------------------------------------------------------------

/// The provider, model and harness every test below binds an assignment to
/// and reads the recorded row back with. Arbitrary — no stub upstream reads
/// any of them — but each must be the one string used everywhere below it.
const FIRST_BYTE_PROVIDER: &str = "routing-cost-first-byte";
const FIRST_BYTE_MODEL: &str = "stub-model";
const FIRST_BYTE_HARNESS: &str = "claude-code";

/// A stand-in provider credential, resolved through the real environment
/// store rather than a crate-private test constructor — the same shape
/// `tests/gateway_retry_after.rs`'s own `test_credential` uses, and for the
/// same reason: `secret::Secret` has no public way to mint one outside
/// `crate::secret`. `var` is unique per call site so the two tests below,
/// which may run concurrently, never race on the same environment variable.
fn first_byte_test_credential(var: &str) -> Secret {
    // SAFETY: `var` is unique to the one caller that set it, and it is
    // removed again immediately below, before the resolved value is even
    // inspected, so no other test can observe it set.
    unsafe {
        std::env::set_var(var, "sk-planted-not-a-real-key-firstbyte");
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

/// A gateway pointed at `upstream_address`, with `evidence_ledger` wired in
/// exactly the way `crate::gateway::start_if_required_with_telemetry`'s own
/// production callers wire it. Driving a real accept loop this way — rather
/// than calling `SessionRouting::record_routing_observation` or
/// `NewObservation::with_first_byte_at` directly — is what makes the tests
/// below reach the production call practice §35 asks for, not a helper's own
/// shortcut around it.
fn gateway_to_stub(
    credential_var: &str,
    upstream_address: SocketAddr,
    evidence_ledger: Arc<EvidenceLedger>,
) -> glasshouse::gateway::Gateway {
    let upstream = Upstream::new(
        FIRST_BYTE_PROVIDER.to_owned(),
        vec![Route::new(
            "anthropic-messages".to_owned(),
            &["/messages"],
            &format!("http://{upstream_address}"),
        )],
        first_byte_test_credential(credential_var),
        CredentialId::new(
            FIRST_BYTE_PROVIDER,
            SecretRef::Environment {
                var: credential_var.to_owned(),
            },
        ),
    )
    .expect("a loopback http URL is absolute and this credential is header-safe");

    let mut profile = LaunchProfile::native(IntegrationId::ClaudeCode);
    profile.backend = BackendResource::GlasshouseGateway;
    let gateway = glasshouse::gateway::start_if_required_with_telemetry(
        &[profile],
        || Ok(upstream),
        None,
        Some(evidence_ledger),
        None,
    )
    .expect("loopback is bindable")
    .expect("a gateway-backed profile requires a gateway");

    gateway.routing().bind(
        FIRST_BYTE_HARNESS,
        "anthropic-messages",
        AssignedModel::named(FIRST_BYTE_MODEL),
        gateway.upstream(),
    );

    gateway
}

fn first_byte_messages_request(token: &str) -> Vec<u8> {
    let body = format!(r#"{{"model":"{FIRST_BYTE_MODEL}"}}"#);
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
/// the same shape `tests/gateway_retry_after.rs`'s own `send_and_read` uses.
fn first_byte_send_and_read(address: SocketAddr, raw: &[u8]) -> String {
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

/// How long a real answer sits between its head landing and its body
/// following it — long enough, at this build's second-granularity clock, for
/// `dispatched_at`, `first_byte_at` and `completed_at` to have a real chance
/// to land in different seconds rather than merely satisfying `>=`/`<=` by
/// coincidence.
const FIRST_BYTE_STUB_DELAY: Duration = Duration::from_millis(1_200);

/// Read the request whole — its head, then exactly the body its
/// `Content-Length` declares — before the caller answers it.
///
/// This used to be a single `read` into a 4 KiB buffer, on the reasoning
/// that a stub which never parses the request need not read it. That is a
/// race with any client that writes its head and its body separately, and
/// the gateway is one: `ureq` sends the head, then streams the relayed body
/// from the client socket (`gateway::ingress`'s
/// `SendBody::from_owned_reader`). When the stub's single read lands
/// between those two writes it takes the head alone, and the body is still
/// in the socket's receive queue when the stub answers and drops the
/// stream.
///
/// Closing a socket that still holds unread data is an *abortive* close:
/// the stack sends RST instead of FIN. Winsock then discards whatever it
/// had already buffered for the peer, so the gateway's read of the response
/// this stub had just written failed with a connection reset, `agent.run`
/// returned `Err`, and the gateway answered its own `502 Bad Gateway`
/// (`ingress::serve`'s `Outcome::Unreachable`) rather than relaying the
/// scripted status. Unix hands the buffered bytes back first and only
/// reports the reset once they are drained, which is why the same stub was
/// reliable on macOS and Linux and flaked on the Windows ARM64 CI VM.
///
/// Nothing here is conditional on the platform, and no assertion moves:
/// reading a request before answering it is what any HTTP server does, and
/// it is already what `evaluation_producers.rs`'s `serve_json` does in this
/// same suite. On Unix it only reads bytes that were arriving anyway.
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

/// A stub upstream that answers one connection with `200 OK`, writing its
/// status and headers immediately and then holding the body back for
/// [`FIRST_BYTE_STUB_DELAY`] — so `ingress::forward`'s own read of the clock
/// right after `Agent::run` returns (`crate::gateway::ingress`'s own "a third
/// thing may now be recorded") lands well before the exchange actually
/// completes, rather than at the same instant as a matter of course.
fn stub_ok_server_with_delayed_body() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback is bindable");
    let address = listener
        .local_addr()
        .expect("a bound listener has a local address");
    listener
        .set_nonblocking(true)
        .expect("a listener can be put in polling mode");

    std::thread::Builder::new()
        .name("routing-cost-first-byte-stub".to_owned())
        .spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _peer)) => break Some(stream),
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            break None;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break None,
                }
            };
            let Some(stream) = stream.as_mut() else {
                return;
            };
            let _ = stream.set_nonblocking(false);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));

            read_whole_request(stream);

            let body = b"ok";
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.flush();

            std::thread::sleep(FIRST_BYTE_STUB_DELAY);

            let _ = stream.write_all(body);
            let _ = stream.flush();
        })
        .expect("can spawn the stub server thread");

    address
}

/// A loopback address nothing listens on — bound, then immediately dropped,
/// so a connection attempt is refused rather than merely slow. Models
/// `Outcome::Unreachable`: a route was chosen and a connection was
/// attempted, but no response — and so no first byte — ever arrived.
fn unreachable_address() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback is bindable");
    let address = listener
        .local_addr()
        .expect("a bound listener has a local address");
    drop(listener);
    address
}

/// Poll the ledger for the one row `FIRST_BYTE_PROVIDER`/`FIRST_BYTE_MODEL`/
/// route/harness names, giving the same margin `tests/gateway_retry_after.rs`'s
/// own `wait_for_readings` gives: the accept loop's connection thread writes
/// the observation *after* `send_and_read` has already returned, so the
/// client side finishing is not proof the row has landed yet.
fn wait_for_observation(
    ledger: &EvidenceLedger,
) -> glasshouse::routing::evidence::RoutingObservation {
    let query = ObservationQuery {
        provider: FIRST_BYTE_PROVIDER,
        model: FIRST_BYTE_MODEL,
        route: Some("anthropic-messages"),
        harness: Some(FIRST_BYTE_HARNESS),
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let rows = ledger.recent(query, 1).unwrap();
        if let Some(row) = rows.into_iter().next() {
            return row;
        }
        assert!(
            Instant::now() < deadline,
            "no routing observation was recorded within 5s of a completed exchange"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// **Acceptance test 1.** A real relayed exchange, driven through a real
/// `Gateway` and a real accept loop — never
/// `SessionRouting::record_routing_observation` called directly (practice
/// §35) — records a `first_byte_at` that lands between `dispatched_at` and
/// `completed_at`, not merely present. The stub's own delay between its head
/// and its body ([`FIRST_BYTE_STUB_DELAY`]) is what makes the ordering
/// assertion mean something rather than being trivially true at this
/// build's second-granularity clock.
///
/// Mutation target (§35): delete `exchange.first_byte_at` from
/// `SessionRouting::record_routing_observation`'s own
/// `.with_first_byte_at(exchange.first_byte_at)` call — the production call
/// this test reaches — and this assertion must fail.
#[test]
fn a_real_relayed_exchange_records_first_byte_at_between_dispatch_and_completion() {
    const CREDENTIAL_VAR: &str = "GLASSHOUSE_ROUTING_COST_FIRST_BYTE_TEST_KEY_OK";

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = Arc::new(fixture.ledger());

    let upstream_address = stub_ok_server_with_delayed_body();
    let gateway = gateway_to_stub(CREDENTIAL_VAR, upstream_address, ledger);

    let response = first_byte_send_and_read(
        gateway.address(),
        &first_byte_messages_request(gateway.token().expose()),
    );
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "the gateway must relay the stub's own 200: {response}"
    );

    let row = wait_for_observation(&fixture.ledger());
    let dispatched = row
        .dispatched_at_unix
        .expect("a forwarded exchange always records dispatched_at");
    let completed = row
        .completed_at_unix
        .expect("a forwarded exchange always records completed_at");
    let first_byte = row
        .first_byte_at_unix
        .expect("a real forwarded exchange must record when the first response byte arrived");
    assert!(
        first_byte >= dispatched,
        "first_byte_at ({first_byte}) must not precede dispatched_at ({dispatched})"
    );
    assert!(
        first_byte <= completed,
        "first_byte_at ({first_byte}) must not follow completed_at ({completed})"
    );

    let run = fixture.routing_cost(None);
    assert!(run.status.success(), "stderr: {}", run.stderr);
    let coding_agent = section(&run.stdout, "coding-agent (gateway relay)");
    assert_eq!(value_after(&coding_agent, FIRST_BYTE_SAMPLES), "1");
    let rendered = value_after(&coding_agent, TIME_TO_FIRST_BYTE);
    assert!(
        rendered.ends_with("ms (mean)") && rendered != "not recorded",
        "a timed row must render a real reading, not the absence phrase: {rendered:?}"
    );
}

/// **Acceptance test 2.** An exchange that never reached a provider —
/// `Outcome::Unreachable`, a real connection attempt that was refused —
/// records `first_byte_at` as `NULL`, and the reader prints the *not
/// recorded* phrase: no digit anywhere in that field, and never `0` or
/// `0ms`.
///
/// Mutation target: render an absent first-byte time as `0` in
/// `main.rs::render_time_to_first_byte` and this test must fail.
#[test]
fn an_exchange_that_never_reached_a_provider_records_no_first_byte_and_the_reader_says_so() {
    const CREDENTIAL_VAR: &str = "GLASSHOUSE_ROUTING_COST_FIRST_BYTE_TEST_KEY_UNREACHABLE";

    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let ledger = Arc::new(fixture.ledger());

    let upstream_address = unreachable_address();
    let gateway = gateway_to_stub(CREDENTIAL_VAR, upstream_address, ledger);

    let response = first_byte_send_and_read(
        gateway.address(),
        &first_byte_messages_request(gateway.token().expose()),
    );
    assert!(
        response.starts_with("HTTP/1.1 502"),
        "an unreachable provider must relay as a 502: {response}"
    );

    let row = wait_for_observation(&fixture.ledger());
    assert!(row.dispatched_at_unix.is_some());
    assert!(row.completed_at_unix.is_some());
    assert_eq!(
        row.first_byte_at_unix, None,
        "no response ever arrived, so there is no first byte to record"
    );

    let run = fixture.routing_cost(None);
    assert!(run.status.success(), "stderr: {}", run.stderr);
    let coding_agent = section(&run.stdout, "coding-agent (gateway relay)");
    assert_eq!(value_after(&coding_agent, FIRST_BYTE_SAMPLES), "0");
    let rendered = value_after(&coding_agent, TIME_TO_FIRST_BYTE);
    assert_eq!(
        rendered, "not recorded",
        "an untimed group must say so, never a number: {rendered:?}"
    );
    assert!(
        !rendered.chars().any(|c| c.is_ascii_digit()),
        "\"not recorded\" must never carry a stray digit: {rendered:?}"
    );
}

// ---------------------------------------------------------------------------
// 6. GH-STREAM-FIRST-EVENTS (lines 1331/1332): the first-token and
//    first-tool-call readout lines beside the first-byte pair above.
// ---------------------------------------------------------------------------

/// A seeded row carrying both `first_token_at` and `first_tool_call_at`
/// prints real figures for both new lines; a group whose rows never carried
/// either — the coding-agent shape, until a translated exchange supplies
/// them — prints "not recorded" for both, exactly as `time to first byte`
/// already does for an untimed group.
#[test]
fn a_row_carrying_first_token_and_first_tool_call_prints_real_figures_and_an_untimed_group_says_so_twice()
 {
    let tmp = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(tmp.path(), "alpha");
    let at = now() - 60;
    let dispatched = at - 5;
    let completed = at + 5;

    let timed = NewObservation::new("timed-provider", "timed-model")
        .with_purpose(Some("classification"))
        .with_timing(Some(dispatched), Some(completed))
        .with_first_token_at(Some(dispatched + 2))
        .with_first_tool_call_at(Some(dispatched + 3));
    fixture.ledger().record(timed, at).unwrap();

    // A real gateway-shaped row: harness-recorded, no purpose, and — like
    // every relayed exchange, and like every gateway exchange before
    // GH-STREAM-FIRST-EVENTS translated one — no first_token_at or
    // first_tool_call_at either.
    fixture.record_gateway_exchange("gateway-provider", "gateway-model", "claude-code", at);

    let run = fixture.routing_cost(None);
    assert!(run.status.success(), "stderr: {}", run.stderr);

    let classification = section(&run.stdout, "classification");
    assert_eq!(value_after(&classification, FIRST_TOKEN_SAMPLES), "1");
    assert!(
        value_after(&classification, TIME_TO_FIRST_TOKEN).ends_with("ms (mean)"),
        "{}",
        run.stdout
    );
    assert_eq!(value_after(&classification, FIRST_TOOL_CALL_SAMPLES), "1");
    assert!(
        value_after(&classification, TIME_TO_FIRST_TOOL_CALL).ends_with("ms (mean)"),
        "{}",
        run.stdout
    );

    let coding_agent = section(&run.stdout, "coding-agent (gateway relay)");
    assert_eq!(value_after(&coding_agent, FIRST_TOKEN_SAMPLES), "0");
    assert_eq!(
        value_after(&coding_agent, TIME_TO_FIRST_TOKEN),
        "not recorded"
    );
    assert_eq!(value_after(&coding_agent, FIRST_TOOL_CALL_SAMPLES), "0");
    assert_eq!(
        value_after(&coding_agent, TIME_TO_FIRST_TOOL_CALL),
        "not recorded"
    );
}
