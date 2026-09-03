//! `GH-RELAY-USAGE`: a **relayed** exchange records the provider's own usage
//! figures and its first-token timing, extracted from bytes already going
//! past — through a real socket and a real [`Gateway`], the same door
//! `tests/gateway_first_events.rs` enters for the translated path.
//!
//! The user's ruling of 2026-09-03 (`docs/product/design-decisions.md`,
//! *Steering decisions of record* §1) approved this and named the constraints
//! that come with it. Each constraint that can fail at runtime has a test
//! here, and they are named after the constraint rather than after the code:
//!
//! - *the forwarded bytes and protocol semantics are preserved* —
//!   [`the_bytes_the_client_receives_are_exactly_what_the_provider_sent`];
//! - *an unsupported provider or format records unknown, never an estimate* —
//!   [`a_protocol_whose_usage_spelling_is_unknown_records_no_usage`] and
//!   [`a_supported_protocol_that_states_no_usage_records_none`];
//! - *never an estimate*, on a stream that stopped part-way —
//!   [`a_truncated_stream_records_no_usage_however_much_of_it_arrived`];
//! - *no relayed response content is persisted by this producer* —
//!   [`no_relayed_response_content_reaches_the_project_or_a_log`].
//!
//! The bounded-memory half of the ruling is checked where the bound lives,
//! against the observer's own state:
//! `gateway::usage::tests::the_window_never_grows_with_the_length_of_the_response`.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use glasshouse::gateway::{Gateway, Route, Upstream, UpstreamBackend};
use glasshouse::integrations::IntegrationId;
use glasshouse::profile::{BackendResource, LaunchProfile};
use glasshouse::routing::evidence::{
    EvidenceLedger, ObservationQuery, Outcome, RoutingObservation,
};
use glasshouse::routing::{AssignedModel, Cost, CredentialId};
use glasshouse::secret::{EnvironmentSecretStore, SecretRef, SecretStore};

/// A planted provider credential, unique to this test binary. Never a real
/// key.
const PLANTED_KEY: &str = "sk-planted-relay-usage-4455667788";
const CREDENTIAL_VAR: &str = "GLASSHOUSE_RELAY_USAGE_TEST_KEY";
const MODEL: &str = "fixture-model";

/// Text the fixture puts in the response's generated output. Planted so that
/// `!contains` on it is a real assertion rather than a shape check — the
/// extractor walks these very bytes to decide the delta is not padding, and
/// nothing may keep them.
const PLANTED_TEXT: &str = "aardvark-quixotic-7731";

const CLIENT_TIMEOUT: Duration = Duration::from_secs(20);

/// The pause before the real text delta and again before the tool call — long
/// enough that a unix-second clock reading taken before and after cannot
/// round to the same second, and never asserted against as an exact duration.
const PAUSE: Duration = Duration::from_millis(1200);

/// Every set→resolve→remove of [`CREDENTIAL_VAR`] happens under this lock —
/// the environment is process state shared by every test in this binary.
static ENV_LOCK: Mutex<()> = Mutex::new(());

// --- the fixture upstream ---------------------------------------------------------

/// What the fixture answers with.
#[derive(Clone, Copy)]
enum Script {
    /// A complete Anthropic message stream: usage in `message_start`, a
    /// whitespace-only delta, a real text delta, a `tool_use` block, and the
    /// final `output_tokens` in `message_delta`.
    AnthropicStream,
    /// The same stream, delivered under a `content-length` that promises more
    /// than is sent and then cut — the provider's stream failing short.
    AnthropicStreamTruncated,
    /// A complete, well-formed response that simply states no usage at all.
    NoUsageStated,
    /// A long, realistic Anthropic stream for [`proxy_only_overhead`] — the
    /// same bytes on both arms of the benchmark, with no pauses, so the only
    /// thing being timed is the proxy.
    Benchmark,
}

/// How many text deltas [`Script::Benchmark`]'s body carries. Enough that the
/// body is a few hundred kilobytes and the scan has real work to do, rather
/// than a length at which the socket setup dominates.
const BENCHMARK_DELTAS: usize = 2_000;

/// [`Script::Benchmark`]'s body: a full message stream with usage at both
/// ends, one tool call, and [`BENCHMARK_DELTAS`] text deltas between them.
fn benchmark_body() -> String {
    let mut body = String::with_capacity(512 * 1024);
    body.push_str(&anthropic_events()[0]);
    body.push_str(&anthropic_events()[1]);
    for index in 0..BENCHMARK_DELTAS {
        body.push_str(&format!(
            "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"token {index} of the benchmark body\"}}}}\n\n"
        ));
    }
    body.push_str(&anthropic_events()[4]);
    body.push_str(&anthropic_events()[5]);
    body
}

/// The events of [`Script::AnthropicStream`], each written as its own chunk in
/// order.
fn anthropic_events() -> Vec<String> {
    vec![
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_fixture\",\"usage\":{\"input_tokens\":120,\"cache_read_input_tokens\":100,\"output_tokens\":1}}}\n\n".to_owned(),
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n".to_owned(),
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"   \"}}\n\n".to_owned(),
        format!("event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{PLANTED_TEXT}\"}}}}\n\n"),
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_fixture\",\"name\":\"Bash\"}}\n\n".to_owned(),
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":33}}\n\n".to_owned(),
    ]
}

/// A complete response body that states no usage — the same shape, minus
/// every count.
const NO_USAGE_BODY: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_fixture\"}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

struct FixtureUpstream {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl FixtureUpstream {
    fn start(script: Script) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback is bindable");
        let address = listener.local_addr().expect("bound");
        listener.set_nonblocking(true).expect("polling mode");
        let stop = Arc::new(AtomicBool::new(false));
        let accept = std::thread::spawn({
            let stop = Arc::clone(&stop);
            move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            std::thread::spawn(move || serve(stream, script));
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(10)),
                    }
                }
            }
        });
        Self {
            address,
            stop,
            accept: Some(accept),
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }
}

impl Drop for FixtureUpstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

/// Read the request head and body off `stream` and discard both — the
/// fixture answers from its script, not from what was asked.
fn read_request(stream: &mut TcpStream) {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => return,
            Ok(_) => head.push(byte[0]),
        }
        if head.len() > 64 * 1024 {
            return;
        }
    }
    let text = String::from_utf8_lossy(&head);
    let mut content_length = 0usize;
    for line in text.split("\r\n") {
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        let _ = stream.read_exact(&mut body);
    }
}

fn write_chunk(stream: &mut TcpStream, bytes: &[u8]) {
    let mut framed = format!("{:x}\r\n", bytes.len()).into_bytes();
    framed.extend_from_slice(bytes);
    framed.extend_from_slice(b"\r\n");
    let _ = stream.write_all(&framed);
    let _ = stream.flush();
}

fn serve(mut stream: TcpStream, script: Script) {
    stream.set_nonblocking(false).expect("blocking mode");
    let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
    read_request(&mut stream);
    match script {
        Script::AnthropicStream => {
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n",
            );
            for (index, event) in anthropic_events().into_iter().enumerate() {
                // A gap before the real text delta and again before the tool
                // call, so the row's instants can be asserted as ordered with
                // real gaps rather than as exact durations.
                if index == 3 || index == 4 {
                    std::thread::sleep(PAUSE);
                }
                write_chunk(&mut stream, event.as_bytes());
            }
            let _ = stream.write_all(b"0\r\n\r\n");
        }
        Script::AnthropicStreamTruncated => {
            // A declared length the provider then fails to deliver: the
            // events all arrive, the usage is fully stated, and the stream
            // still ends short of what it promised.
            let body: String = anthropic_events().concat();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
                body.len() + 64
            );
            let _ = stream.write_all(body.as_bytes());
        }
        Script::Benchmark => {
            let body = benchmark_body();
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n",
            );
            for piece in body.as_bytes().chunks(8 * 1024) {
                write_chunk(&mut stream, piece);
            }
            let _ = stream.write_all(b"0\r\n\r\n");
        }
        Script::NoUsageStated => {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n{NO_USAGE_BODY}",
                NO_USAGE_BODY.len()
            );
        }
    }
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
}

// --- the gateway, built the way the binary builds it -------------------------------

/// An upstream whose one route carries `protocol` and serves `/messages`.
///
/// The slug is the only thing that varies between the supported and
/// unsupported tests below, which is what makes the pair decisive: the
/// request, the fixture and the bytes are identical, and the reading changes
/// because the gateway has no usage spelling for that protocol.
fn upstream_serving(protocol: &str, base_url: &str) -> Upstream {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // SAFETY: `CREDENTIAL_VAR` is unique to this test binary, set and removed
    // around the one resolve that reads it, under `ENV_LOCK` for the whole
    // window.
    unsafe {
        std::env::set_var(CREDENTIAL_VAR, PLANTED_KEY);
    }
    let credential = EnvironmentSecretStore::new()
        .resolve(&SecretRef::Environment {
            var: CREDENTIAL_VAR.to_owned(),
        })
        .expect("just set");
    unsafe {
        std::env::remove_var(CREDENTIAL_VAR);
    }
    let backend = UpstreamBackend::new(
        "fixture".to_owned(),
        vec![Route::new(protocol.to_owned(), &["/messages"], base_url)],
        credential,
        CredentialId::new(
            "fixture",
            SecretRef::Environment {
                var: CREDENTIAL_VAR.to_owned(),
            },
        ),
        Cost::Metered,
    )
    .expect("a loopback URL is absolute and the planted key is header-safe");
    Upstream::with_failover(vec![backend]).expect("one backend")
}

struct LedgerFixture {
    tmp: tempfile::TempDir,
    ledger: Arc<EvidenceLedger>,
}

fn ledger_fixture() -> LedgerFixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = std::fs::canonicalize(tmp.path()).expect("canonicalize");
    let root = base.join("workspace");
    std::fs::create_dir_all(root.join(".git")).expect("project root");
    let cli = glasshouse::Cli {
        scope: Some(root.clone()),
        allow_unsafe_scope: false,
        data_dir: Some(base.join("data")),
        config_dir: Some(base.join("config")),
        log_level: None,
        log_file: None,
        log_stderr: false,
        command: None,
    };
    let runtime = glasshouse::bootstrap(&cli, &root).expect("bootstrap");
    LedgerFixture {
        tmp,
        ledger: Arc::new(EvidenceLedger::open(&runtime).expect("open the ledger")),
    }
}

fn start_gateway(upstream: Upstream, ledger: Arc<EvidenceLedger>, protocol: &str) -> Gateway {
    let mut profile = LaunchProfile::native(IntegrationId::ClaudeCode);
    profile.backend = BackendResource::GlasshouseGateway;
    let gateway = glasshouse::gateway::start_if_required_with_degrade_sink(
        &[profile],
        || Ok(upstream),
        None,
        Some(ledger),
        None,
        None,
        None,
    )
    .expect("loopback is bindable")
    .expect("a gateway-backed profile requires a gateway");
    gateway.routing().bind(
        "claude-code",
        protocol,
        AssignedModel::named(MODEL),
        gateway.upstream(),
    );
    gateway
}

fn messages_request(token: &str) -> Vec<u8> {
    let body = "{\"model\":\"claude-x\",\"max_tokens\":10,\"stream\":true,\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}";
    format!(
        "POST /v1/messages HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Authorization: Bearer {token}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {body}",
        body.len()
    )
    .into_bytes()
}

fn send_and_read(address: SocketAddr, raw: &[u8]) -> Vec<u8> {
    let mut client = TcpStream::connect(address).expect("the gateway accepts connections");
    client
        .set_read_timeout(Some(CLIENT_TIMEOUT))
        .expect("a non-zero timeout is valid");
    client
        .write_all(raw)
        .expect("the gateway reads the request");
    client.flush().expect("flush");
    let mut received = Vec::new();
    let _ = client.read_to_end(&mut received);
    received
}

/// `ObservationQuery::route` is `None` **for rows with no route**, not for
/// any route — so the protocol is named here rather than left out.
fn wait_for_row(ledger: &EvidenceLedger, protocol: &str) -> Vec<RoutingObservation> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let rows = ledger
            .recent(
                ObservationQuery {
                    provider: "fixture",
                    model: MODEL,
                    route: Some(protocol),
                    harness: Some("claude-code"),
                },
                10,
            )
            .expect("read the ledger");
        if !rows.is_empty() || std::time::Instant::now() >= deadline {
            return rows;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One relayed exchange against `protocol`, returning the row it wrote and
/// everything the client received.
fn one_exchange(script: Script, protocol: &str) -> (RoutingObservation, Vec<u8>, LedgerFixture) {
    let fixture = FixtureUpstream::start(script);
    let ledger = ledger_fixture();
    let upstream = upstream_serving(protocol, &fixture.base_url());
    let gateway = start_gateway(upstream, Arc::clone(&ledger.ledger), protocol);
    let received = send_and_read(
        gateway.address(),
        &messages_request(gateway.token().expose()),
    );
    let rows = wait_for_row(&ledger.ledger, protocol);
    assert_eq!(
        rows.len(),
        1,
        "one routing observation for the exchange; the gateway answered: {}",
        String::from_utf8_lossy(&received)
    );
    drop(gateway);
    (rows.into_iter().next().expect("checked"), received, ledger)
}

/// Everything after the response head — the body the client actually got,
/// with the gateway's own chunked framing removed.
fn dechunked_body(response: &[u8]) -> Vec<u8> {
    let head_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("a response has a head")
        + 4;
    let mut rest = &response[head_end..];
    let mut body = Vec::new();
    while let Some(line_end) = rest.windows(2).position(|window| window == b"\r\n") {
        let size = usize::from_str_radix(
            std::str::from_utf8(&rest[..line_end]).expect("a chunk size is ASCII"),
            16,
        )
        .expect("a chunk size is hexadecimal");
        if size == 0 {
            break;
        }
        let start = line_end + 2;
        body.extend_from_slice(&rest[start..start + size]);
        rest = &rest[start + size + 2..];
    }
    body
}

// ---------------------------------------------------------------------------
// The producer: a relayed exchange records what the provider stated.
// ---------------------------------------------------------------------------

/// The whole contract in one row: the counts the provider wrote, and the two
/// instants for events that passed the seam, on an exchange that never
/// entered a codec.
///
/// Mutation target `estimate-instead-of-unknown`: the counts here are exactly
/// the digits in the fixture's own events, so any figure derived from the
/// byte count rather than read out of them fails this test as well as the
/// unsupported-protocol one below.
#[test]
fn a_relayed_anthropic_stream_records_the_usage_the_provider_stated() {
    let (row, _received, _ledger) = one_exchange(Script::AnthropicStream, "anthropic-messages");

    assert_eq!(row.outcome, Some(Outcome::Succeeded));
    assert_eq!(
        row.route.as_deref(),
        Some("anthropic-messages"),
        "the exchange must have been relayed, not translated — a translated row's route \
         names a pair"
    );
    assert_eq!(
        (row.input_tokens, row.output_tokens, row.cached_input_tokens),
        (Some(120), Some(33), Some(100)),
        "the counts must be the ones the provider stated, with the later output figure \
         winning over `message_start`'s"
    );

    let first_byte = row
        .first_byte_at_unix
        .expect("a relayed exchange that reached the provider has a first byte");
    let first_token = row
        .first_token_at_unix
        .expect("the real text delta must have stamped first_token_at");
    let first_tool_call = row
        .first_tool_call_at_unix
        .expect("the tool-use block must have stamped first_tool_call_at");
    assert!(
        first_token - first_byte >= 1,
        "the pause before the real text delta must show as a real gap: \
         first_byte_at={first_byte}, first_token_at={first_token}"
    );
    assert!(
        first_tool_call - first_token >= 1,
        "the pause before the tool call must show as a real gap: \
         first_token_at={first_token}, first_tool_call_at={first_tool_call}"
    );
    // Migration 25's offsets describe the same two events, from the same
    // clock reading.
    assert!(row.first_token_ms.is_some_and(|ms| ms >= 0));
    assert!(row.first_tool_call_ms.is_some_and(|ms| ms >= 0));
}

// ---------------------------------------------------------------------------
// "The forwarded response bytes and protocol semantics are preserved."
// ---------------------------------------------------------------------------

/// The constraint the whole ruling is conditioned on, checked at the only
/// place it can stop being true: the observer is fed the same buffer the
/// relay is about to write, so anything it consumed, reordered or dropped
/// would show up here as a body that is not the fixture's.
///
/// Mutation target `swallow-a-byte`: shortening `Counted::read`'s return by
/// one after feeding the extractor must fail this test.
#[test]
fn the_bytes_the_client_receives_are_exactly_what_the_provider_sent() {
    let (_row, received, _ledger) = one_exchange(Script::AnthropicStream, "anthropic-messages");

    let expected: Vec<u8> = anthropic_events().concat().into_bytes();
    let body = dechunked_body(&received);
    assert_eq!(
        body.len(),
        expected.len(),
        "the relayed body is {} bytes and the provider sent {}",
        body.len(),
        expected.len()
    );
    assert_eq!(
        body,
        expected,
        "the client must receive precisely what the provider sent, byte for byte and in \
         order: {}",
        String::from_utf8_lossy(&body)
    );
}

// ---------------------------------------------------------------------------
// "An unsupported provider or response format records usage as unknown."
// ---------------------------------------------------------------------------

/// The same request, the same fixture, the same bytes — and a protocol slug
/// this relay has no usage spelling for. Nothing is looked at, and the row
/// says unknown rather than a figure derived from what did arrive.
///
/// Mutation target `estimate-instead-of-unknown`: this is the test that
/// fails first when the unsupported branch starts inventing a count.
#[test]
fn a_protocol_whose_usage_spelling_is_unknown_records_no_usage() {
    let (row, received, _ledger) = one_exchange(Script::AnthropicStream, "gemini-generate-content");

    assert_eq!(row.outcome, Some(Outcome::Succeeded));
    assert!(
        !received.is_empty(),
        "the exchange must have been served, or the assertions below are vacuous"
    );
    assert_eq!(
        (row.input_tokens, row.output_tokens, row.cached_input_tokens),
        (None, None, None),
        "an unsupported protocol records unknown — never a count derived from the {} bytes \
         that did arrive",
        received.len()
    );
    assert_eq!(
        (row.first_token_at_unix, row.first_tool_call_at_unix),
        (None, None),
        "a protocol with no spelling has no markers to recognise either"
    );
}

/// The other half of *unknown*: a protocol that **is** supported, on a
/// response that simply states nothing. Both counts stay `NULL` rather than
/// becoming zeroes.
#[test]
fn a_supported_protocol_that_states_no_usage_records_none() {
    let (row, _received, _ledger) = one_exchange(Script::NoUsageStated, "anthropic-messages");

    assert_eq!(row.outcome, Some(Outcome::Succeeded));
    assert_eq!(
        (row.input_tokens, row.output_tokens, row.cached_input_tokens),
        (None, None, None),
        "a response that stated no usage records unknown, not zero"
    );
}

/// A stream that stated its usage in full and then ended short of the length
/// it declared. The figures are there for the taking and are still not
/// recorded: pairing them with an ending the provider never reached would be
/// asserting a completed exchange that did not complete.
#[test]
fn a_truncated_stream_records_no_usage_however_much_of_it_arrived() {
    let (row, _received, _ledger) =
        one_exchange(Script::AnthropicStreamTruncated, "anthropic-messages");

    assert_eq!(
        (row.input_tokens, row.output_tokens, row.cached_input_tokens),
        (None, None, None),
        "a truncated stream records unknown even though every count was stated before it \
         was cut"
    );
    // The timings are observations of events that did pass the seam, so they
    // survive the truncation — the distinction the module header states.
    assert!(
        row.first_token_at_unix.is_some(),
        "a token that passed the seam passed it, whatever happened afterwards"
    );
}

// ---------------------------------------------------------------------------
// "No relayed response content is persisted by this producer."
// ---------------------------------------------------------------------------

/// The extractor walks the generated text to decide it is not padding. This
/// is the test that it kept none of it: every file the project wrote, the
/// database and its journal included, is scanned for the planted text.
#[test]
fn no_relayed_response_content_reaches_the_project_or_a_log() {
    let (row, received, ledger) = one_exchange(Script::AnthropicStream, "anthropic-messages");

    assert!(
        received
            .windows(PLANTED_TEXT.len())
            .any(|window| window == PLANTED_TEXT.as_bytes()),
        "the client must have received the planted text, or the scan below proves nothing"
    );
    assert!(
        row.first_token_at_unix.is_some(),
        "the extractor must have looked at that text, or the scan below proves nothing"
    );

    let LedgerFixture { tmp, ledger } = ledger;
    drop(ledger);
    let mut files = Vec::new();
    files_under(tmp.path(), &mut files);
    assert!(
        !files.is_empty(),
        "the project wrote files, or this scan is vacuous"
    );
    for file in files {
        let bytes = std::fs::read(&file).unwrap_or_default();
        assert!(
            !bytes
                .windows(PLANTED_TEXT.len())
                .any(|window| window == PLANTED_TEXT.as_bytes()),
            "relayed response content reached {}",
            file.display()
        );
    }
}

fn files_under(directory: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_under(&path, found);
        } else {
            found.push(path);
        }
    }
}

// ---------------------------------------------------------------------------
// "CPU, memory, latency and throughput overhead on the hot path are
// benchmarked."
// ---------------------------------------------------------------------------

/// Proxy-only overhead, with and without extraction, on the same bytes.
///
/// `#[ignore]` because it is a measurement rather than an assertion: it takes
/// tens of seconds and its numbers are a property of the machine, so it must
/// not sit in the gate. Run it, and it is reproducible by anyone:
///
/// ```text
/// GH_RELAY_BENCH_PROTOCOL=anthropic-messages \
///   cargo test --release -p glasshouse --test relay_usage -- --ignored --nocapture
/// GH_RELAY_BENCH_PROTOCOL=gemini-generate-content \
///   cargo test --release -p glasshouse --test relay_usage -- --ignored --nocapture
/// ```
///
/// The two arms are the **controlled local fixture** the ruling asks for, and
/// the control is exact: identical fixture, identical bytes, identical code
/// path, and one difference — `anthropic-messages` has an entry in
/// `usage::format_for`'s table and `gemini-generate-content` does not, so the
/// second arm constructs no extractor and `Counted::read` never offers it a
/// chunk. Nothing else differs, and neither arm reaches a provider or a
/// network, so provider and network latency are excluded by construction
/// rather than subtracted afterwards.
///
/// Each arm is its own process, so CPU and peak resident memory can be read
/// off `/usr/bin/time -l` around the whole run rather than sampled from
/// inside it.
#[test]
#[ignore = "a measurement, not an assertion: run it explicitly, see this test's own doc"]
fn proxy_only_overhead() {
    let protocol = std::env::var("GH_RELAY_BENCH_PROTOCOL")
        .unwrap_or_else(|_| "anthropic-messages".to_owned());
    let iterations: usize = std::env::var("GH_RELAY_BENCH_ITERATIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(200);

    let fixture = FixtureUpstream::start(Script::Benchmark);
    let ledger = ledger_fixture();
    let upstream = upstream_serving(&protocol, &fixture.base_url());
    let gateway = start_gateway(upstream, Arc::clone(&ledger.ledger), &protocol);
    let request = messages_request(gateway.token().expose());

    // One untimed exchange first: the first one pays for a TLS-free but still
    // cold `ureq` connection pool and a cold ledger, and timing it would put
    // that cost in whichever arm ran it.
    let warmup = send_and_read(gateway.address(), &request);
    assert!(!warmup.is_empty(), "the fixture answered the warm-up");

    let mut latencies = Vec::with_capacity(iterations);
    let mut bytes = 0u64;
    let started = std::time::Instant::now();
    for _ in 0..iterations {
        let at = std::time::Instant::now();
        let received = send_and_read(gateway.address(), &request);
        latencies.push(at.elapsed());
        bytes += received.len() as u64;
    }
    let wall = started.elapsed();

    latencies.sort_unstable();
    let percentile = |p: usize| latencies[(latencies.len() * p / 100).min(latencies.len() - 1)];
    let mean = wall.as_secs_f64() / iterations as f64;

    println!("--- proxy-only overhead ---------------------------------------");
    println!("protocol            {protocol}");
    println!(
        "extraction          {}",
        if protocol == "gemini-generate-content" {
            "OFF (no entry in usage::format_for)"
        } else {
            "ON"
        }
    );
    println!("iterations          {iterations}");
    println!("body per exchange   {} bytes", bytes / iterations as u64);
    println!("wall total          {:.3} s", wall.as_secs_f64());
    println!("mean latency        {:.3} ms", mean * 1000.0);
    println!(
        "p50 latency         {:.3} ms",
        percentile(50).as_secs_f64() * 1000.0
    );
    println!(
        "p95 latency         {:.3} ms",
        percentile(95).as_secs_f64() * 1000.0
    );
    println!(
        "p99 latency         {:.3} ms",
        percentile(99).as_secs_f64() * 1000.0
    );
    println!(
        "throughput          {:.1} MiB/s",
        bytes as f64 / wall.as_secs_f64() / (1024.0 * 1024.0)
    );
    println!("---------------------------------------------------------------");
}
