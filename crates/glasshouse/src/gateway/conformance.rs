//! The Phase 9G conformance suite: the properties of the gateway that only a
//! whole exchange can demonstrate, each asserted over a real socket.
//!
//! # Why these are here rather than beside the code they exercise
//!
//! [`super::http`], [`super::ingress`] and [`super::upstream`] each test the
//! piece they own, and every one of those tests can pass while the
//! *composition* is wrong. A head parser that is byte-exact, an upstream that
//! appends a request target verbatim, and a pump that moves every byte it is
//! given still add up to a proxy that re-serialises a body if the seam
//! between them buffers, re-encodes or re-frames — and no test that stops at
//! a function boundary can see that. So nothing below calls a parsing
//! helper: a request is written to a socket as bytes, and every assertion is
//! made against what came off the wire at the other end.
//!
//! # The properties
//!
//! 1. **A request body arrives byte-for-byte.** The payload carries a
//!    `tool_use` block with nested objects and arrays, and text in several
//!    scripts plus an emoji, so that its byte length and its character length
//!    differ. The assertion is on bytes and on that byte length, which is
//!    what makes it fail for a gateway that preserved *meaning* — a JSON
//!    round-trip that changed whitespace, key order or escaping would still
//!    parse to the same document and is exactly the regression the capability
//!    map forbids.
//! 2. **A provider's error reaches the harness intact, and the diagnostic
//!    keeps only its status.** Those are two halves of one rule and they pull
//!    in opposite directions: the harness must see the whole body, and the
//!    log must see none of it. Both are asserted on the same exchange, so an
//!    implementation cannot satisfy one by giving up the other.
//! 3. **No rendering carries either secret.** Every `Debug` this module can
//!    reach, every response byte the client was sent, and the transport
//!    error's own detail, scanned for a planted provider credential and for a
//!    gateway token. Asserted twice: once over the paths a single-protocol
//!    gateway had, and once over the three-protocol ones, because a routed
//!    exchange and a refused-before-routing one render different fields.
//! 4. **A request reaches the base URL its own protocol declared, and no
//!    other.** The gateway serves up to three wire protocols from one
//!    provider, each with its own base URL, and chooses between them on the
//!    request target alone. The load-bearing half of every assertion here is
//!    the negative one — the *other* base URLs were never connected to —
//!    because the implementation this replaced appended every target to a
//!    single base URL and would pass the positive half for all three.
//! 5. **Streaming survives on every ingress.** The Anthropic path's twin of
//!    this lives in [`mod@super`]; a gateway that started buffering only the
//!    two new ones would leave that test green. The fixture blocks until the
//!    client says it has the first chunk, so a buffering implementation
//!    cannot produce the second one at all.
//! 6. **A target belonging to no served protocol is refused, and nothing is
//!    opened upstream.** Claude Code sends one such target before its first
//!    request. The assertion is on the fixtures' *connection counts*: a
//!    gateway that opened a connection, thought better of it and answered
//!    `404` would pass an assertion on the status and would still have sent
//!    a request somewhere nobody asked for it to go.
//!
//! # Two planted values, and why the token is planted twice
//!
//! [`PROVIDER_CREDENTIAL`] and [`PLANTED_TOKEN`] are known strings, so
//! `!contains` on them is a real assertion rather than a shape check.
//!
//! The token is planted *and* a real minted one is used, because the two
//! answer different questions. A minted token is 64 hex characters, and
//! `mod.rs`'s `debug_on_a_gateway_token_prints_a_fixed_marker_and_never_the_token`
//! records what goes wrong when short fragments of one are scanned for: hex
//! runs occur in ordinary text, so the scan reports leaks that are
//! coincidences and the test fails at random. A test that fails at random is
//! worth less than no test. So the minted token — held by a real
//! [`Gateway`] that really answered a request — is scanned for whole, and the
//! *fragment* scan runs against a planted value drawn from an alphabet that
//! makes a coincidence impossible rather than merely unlikely.

#![cfg(test)]

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{Mutex, mpsc};
use std::time::Duration;

use ureq::Agent;

use super::fixture::{FixtureUpstream, RecordedRequest};
use super::ingress::{Exchange, Outcome, serve};
use super::upstream::{Route, Upstream, agent};
use super::{Gateway, GatewayToken};
use crate::secret::{REDACTED, Secret, redact};

/// The three protocols the ingress can serve, spelled as `crate::profile`
/// composes them: a [`crate::harness::WireProtocol`] slug, and the request
/// targets that belong to it.
///
/// Written out here rather than imported because this suite must be able to
/// build an upstream that serves an arbitrary subset of them, and because
/// the module under test cannot name `crate::harness` at all.
/// `crate::profile`'s `the_ingress_target_table_covers_every_protocol_the_gateway_serves`
/// is what holds these two spellings together.
const ANTHROPIC_MESSAGES: &str = "anthropic-messages";
const OPENAI_RESPONSES: &str = "openai-responses";
const OPENAI_CHAT: &str = "openai-chat";

/// The request targets that belong to a protocol slug.
fn targets_for(protocol: &str) -> &'static [&'static str] {
    match protocol {
        ANTHROPIC_MESSAGES => &["/messages"],
        OPENAI_RESPONSES => &["/responses"],
        OPENAI_CHAT => &["/chat/completions"],
        other => panic!("no ingress targets are declared for {other:?}"),
    }
}

/// The provider credential every upstream here holds.
///
/// Planted rather than shaped like one, so that every `!contains` below is a
/// statement about this exact string. It is also a credential [`redact`]
/// recognises — `sk-` and a long tail — which is what lets the last test show
/// the redactor working rather than assert it was never needed.
const PROVIDER_CREDENTIAL: &str = "sk-ant-PLANTED-PROVIDER-CREDENTIAL-000111222333";

/// A stand-in gateway token, for the fragment scan a minted one cannot
/// survive.
///
/// Every character is drawn from `Z X Q V W Y`, and that is the whole point:
/// the renderings scanned in
/// [`no_rendering_the_gateway_can_produce_carries_either_planted_secret`] are
/// English prose, JSON, HTTP reason phrases and an operating system's error
/// text, and a four-character run from this alphabet does not occur in any of
/// them. The haystacks are fixed except for port numbers and byte counts,
/// which are digits, so the scan is deterministic: it either always passes or
/// always fails, and it can never fail for a run that a generated hex token
/// would have made likely.
const PLANTED_TOKEN: &str = "ZXQVWYZXQVWYZXQVWYZXQVWYZXQVWYZX";

/// The shortest fragment of [`PLANTED_TOKEN`] that is scanned for.
///
/// Four, and not one, because the haystacks here are whole HTTP responses and
/// whole `Debug` renderings rather than the ten characters of a redaction
/// marker. Four characters of this alphabet is already far past coincidence,
/// and it is well below any leak that could actually happen: a `Debug` that
/// kept "the first half", a log line that kept a prefix, a diagnostic that
/// quoted the last few characters "to tell instances apart" are all caught.
const SHORTEST_TOKEN_FRAGMENT: usize = 4;

/// The shortest fragment of [`PROVIDER_CREDENTIAL`] that is scanned for.
///
/// Longer than the token's, because this value ends in digits and the
/// renderings below contain a port and a byte count. A four-digit suffix
/// would collide with a port number and report a leak that is arithmetic; an
/// eight-character one cannot.
const SHORTEST_CREDENTIAL_FRAGMENT: usize = 8;

/// How long a client here waits for the gateway to answer.
///
/// Generous by orders of magnitude, and it costs a correct implementation
/// nothing: every exchange below completes in microseconds. The margin is
/// there so a loaded machine cannot turn a passing test into a failing one.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(60);

/// A distinctive string planted inside the provider's error body.
///
/// The one the diagnostic is scanned for. A marker rather than a plausible
/// phrase, so that finding it anywhere is unambiguous.
const RATE_LIMIT_SENTINEL: &str = "PROVIDER-ERROR-BODY-SENTINEL";

/// A provider's `429` body, in the shape Anthropic actually sends one.
///
/// Held whole so the assertion can be equality on bytes rather than a search
/// for a fragment: a proxy that dropped a field, re-ordered keys or
/// re-escaped a character would pass the second and fail the first.
const RATE_LIMIT_BODY: &str = concat!(
    r#"{"type":"error","error":{"type":"rate_limit_error","message":"#,
    r#""Number of request tokens has exceeded your per-minute rate limit. "#,
    r#"PROVIDER-ERROR-BODY-SENTINEL"}}"#
);

/// An Anthropic Messages body carrying a tool call and four scripts.
///
/// Everything about it is deliberate. The `tool_use` block nests objects
/// inside arrays inside objects, which is where a re-serialising proxy
/// changes key order. `Grüße`, `日本語のテキスト`, the Greek letters and the
/// emoji are each a different width in UTF-8, so the byte length and the
/// character length disagree and a proxy that counted characters somewhere
/// would truncate. And the escaped forward slashes and the `\u` escape are
/// forms a JSON round-trip normalises away without changing what the document
/// means.
const TOOL_CALL_BODY: &str = concat!(
    r#"{"model":"claude-opus-4-1","max_tokens":4096,"#,
    r#""system":"Grüße — antworte auf 日本語のテキスト 🔧","#,
    r#""messages":[{"role":"assistant","content":["#,
    r#"{"type":"text","text":"Grüße — ich rufe das Werkzeug auf: 🔧"},"#,
    r#"{"type":"tool_use","id":"toolu_01Ab2Cd3Ef4Gh5Ij6Kl7Mn","name":"edit_file","input":"#,
    r#"{"path":"crates\/glasshouse\/src\/gateway\/ingress.rs","edits":["#,
    r#"{"old_string":"Grüße","new_string":"Grüße 🔧"},"#,
    r#"{"old_string":"日本語","new_string":"日本語のテキスト"}],"#,
    r#""options":{"dry_run":false,"depth":3,"labels":["α","β","γ","🔧"],"#,
    r#""nested":{"more":[1,2,3],"deep":{"deeper":{"deepest":"Grüße/日本語/🔧"}}}}}}]}],"#,
    r#""tools":[{"name":"edit_file","description":"Bearbeite eine Datei, mit Gr\u00fc\u00dfen","#,
    r#""input_schema":{"type":"object","properties":{"path":{"type":"string"}},"#,
    r#""required":["path"]}}]}"#
);

// --- fixtures, upstreams and clients ----------------------------------------

/// An [`Upstream`] holding [`PROVIDER_CREDENTIAL`], serving Anthropic
/// Messages at `base_url` and nothing else.
///
/// One route, because that is the shape every test written before there was
/// more than one ingress protocol assumes, and those tests are the record
/// that the Anthropic path did not change. The multi-protocol shape is
/// [`upstream_serving`].
fn upstream_at(base_url: &str) -> Upstream {
    upstream_serving(&[(ANTHROPIC_MESSAGES, base_url)])
}

/// An [`Upstream`] holding [`PROVIDER_CREDENTIAL`] and serving one route per
/// `(protocol slug, base URL)` pair.
fn upstream_serving(routes: &[(&str, &str)]) -> Upstream {
    Upstream::new(
        "fixture".to_owned(),
        routes
            .iter()
            .map(|(protocol, base_url)| {
                Route::new((*protocol).to_owned(), targets_for(protocol), base_url)
            })
            .collect(),
        Secret::mint_for_test(PROVIDER_CREDENTIAL),
    )
    .expect("a loopback http URL is absolute and this credential is header-safe")
}

/// An [`Upstream`] pointed at `fixture`.
fn upstream_to(fixture: &FixtureUpstream) -> Upstream {
    upstream_at(&fixture.base_url())
}

/// A real running [`Gateway`], with its own minted token, in front of
/// `fixture`.
fn gateway_to(fixture: &FixtureUpstream) -> Gateway {
    Gateway::start(upstream_to(fixture)).expect("loopback is bindable")
}

/// A loopback address with nothing listening on it.
///
/// Bound and released rather than picked, so the port is one this machine was
/// willing to hand out. The operating system could in principle re-issue it
/// to another test binding port zero in the same instant, which would turn an
/// unreachable provider into a reachable one; over the ephemeral range that is
/// a coincidence measured in parts per ten thousand, and the alternative —
/// naming a fixed low port — trades it for the certainty of failing on a
/// machine that happens to run something there.
fn closed_loopback_address() -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback is bindable");
    let address = listener
        .local_addr()
        .expect("a bound listener has an address");
    drop(listener);
    address
}

/// The bytes a Claude Code child sends: a bearer token, a JSON body, and a
/// length.
///
/// The length is the body's **byte** length, which is the only framing an
/// HTTP body has and the thing a non-ASCII payload makes it possible to get
/// wrong.
fn messages_request(token: &str, body: &str) -> Vec<u8> {
    format!(
        "POST /v1/messages?beta=true HTTP/1.1\r\n\
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

/// Send `raw` to `address` and hand back the connection, still open.
///
/// The open socket is what a streaming assertion needs: reading it to the
/// close would answer "did every byte arrive" and never "did the first one
/// arrive before the last was written", and only the second is streaming.
fn send(address: SocketAddr, raw: &[u8]) -> TcpStream {
    let mut client = TcpStream::connect(address).expect("the gateway accepts connections");
    client
        .set_read_timeout(Some(CLIENT_TIMEOUT))
        .expect("a non-zero read timeout is valid");
    client
        .write_all(raw)
        .expect("the gateway reads the request");
    client.flush().expect("the gateway reads the request");
    client
}

/// Send `raw` to `address` and read everything that comes back, to the close.
fn send_and_read(address: SocketAddr, raw: &[u8]) -> Vec<u8> {
    let mut client = send(address, raw);
    let mut received = Vec::new();
    client
        .read_to_end(&mut received)
        .expect("the gateway answers and then closes");
    received
}

/// Serve exactly one request through [`serve`] and hand back both halves:
/// the [`Exchange`] the gateway recorded and the bytes the client received.
///
/// A real [`Gateway`] hands its [`Exchange`] straight to `record` and keeps
/// nothing, so the only way to assert on one is to accept a socket here and
/// call the ingress directly. The listener is this function's own, so no
/// gateway is involved and nothing else can arrive on it.
fn serve_one(
    token: &GatewayToken,
    upstream: &Upstream,
    agent: &Agent,
    raw: &[u8],
) -> (Exchange, Vec<u8>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback is bindable");
    let address = listener
        .local_addr()
        .expect("a bound listener has an address");

    // The client runs on its own thread because the exchange is a
    // conversation: it has to be writing while the ingress below is reading,
    // and reading while the ingress is writing. Nothing here is timed — the
    // accept blocks until the connection arrives and the join blocks until
    // the response is complete, so there is no sleep standing in for a wait.
    let raw = raw.to_vec();
    let client = std::thread::spawn(move || {
        let mut socket = TcpStream::connect(address).expect("the accept below is already waiting");
        socket
            .set_read_timeout(Some(CLIENT_TIMEOUT))
            .expect("a non-zero read timeout is valid");
        socket
            .write_all(&raw)
            .expect("the ingress reads the request");
        socket.flush().expect("the ingress reads the request");
        let mut received = Vec::new();
        socket
            .read_to_end(&mut received)
            .expect("the ingress answers and then closes");
        received
    });

    let (accepted, _peer) = listener.accept().expect("the client above connects");
    let exchange = serve(accepted, token, upstream, agent);
    let received = client.join().expect("the client thread does not panic");
    (exchange, received)
}

/// The body of an HTTP response: everything after the blank line that ends
/// the head.
fn body_of(response: &[u8]) -> &[u8] {
    let head_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("a response head ends with a blank line");
    &response[head_end + 4..]
}

/// A response's bytes as text, for a `contains` or a failure message.
///
/// Lossy conversion is safe for every scan here: the values searched for are
/// ASCII, and replacing an invalid sequence never creates an ASCII byte or
/// destroys one.
fn as_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

// --- 1. the body is bytes ---------------------------------------------------

/// Lose this and the gateway may quietly start round-tripping request bodies
/// through a JSON library, which produces a document that means the same
/// thing and is not the same bytes. That breaks tool calls in the way that is
/// hardest to diagnose — the provider still answers, the harness still parses
/// the reply, and only a signature, a cache key or a strict schema
/// somewhere fails. The assertion is on bytes and on a byte *length*, so a
/// re-serialisation that preserved meaning still fails it, and the payload is
/// built so that a proxy counting characters instead of bytes fails it too.
#[test]
fn a_request_body_arrives_byte_for_byte_including_a_tool_call_and_non_ascii() {
    // The payload is the test. If it ever stops carrying these, everything
    // below still passes and proves nothing, so it is checked first.
    for fragment in [
        "\"tool_use\"",
        "\"input\"",
        "Grüße",
        "日本語のテキスト",
        "🔧",
        "\\u00fc",
        "crates\\/glasshouse",
    ] {
        assert!(
            TOOL_CALL_BODY.contains(fragment),
            "the forwarded payload no longer contains {fragment:?}, so this test no longer \
             exercises the tool-call, non-ASCII or escaping cases it exists for"
        );
    }
    assert_ne!(
        TOOL_CALL_BODY.len(),
        TOOL_CALL_BODY.chars().count(),
        "the forwarded payload is pure ASCII, so its byte length and its character length agree \
         and this test can no longer tell a byte-preserving gateway from a character-preserving \
         one"
    );

    let fixture = FixtureUpstream::answering(
        "HTTP/1.1 200 OK",
        "content-type: application/json\r\n",
        "{\"ok\":true}",
    );
    let gateway = gateway_to(&fixture);

    let response = as_text(&send_and_read(
        gateway.address(),
        &messages_request(gateway.token().expose(), TOOL_CALL_BODY),
    ));
    assert!(
        response.starts_with("HTTP/1.1 200 OK"),
        "the exchange did not complete, so nothing below is a statement about a forwarded body: \
         {response}"
    );

    let request = fixture.only_request();
    assert_eq!(
        request.method, "POST",
        "the method was rewritten on the way to the provider"
    );
    assert_eq!(
        request.target, "/v1/messages?beta=true",
        "the request target did not reach the provider with its query intact"
    );

    assert_eq!(
        request.body.len(),
        TOOL_CALL_BODY.len(),
        "the request body reached the provider with a different byte length, so something on the \
         way re-serialised it: a JSON round-trip preserves the document and changes its \
         whitespace, its key order and its escaping"
    );
    // `assert!` rather than `assert_eq!` on the two slices: a failing
    // `assert_eq!` would print several hundred bytes as a list of integers,
    // which nobody can read. This prints the body as text instead.
    assert!(
        request.body == TOOL_CALL_BODY.as_bytes(),
        "the request body did not arrive byte-for-byte; the provider received \
         {:?}",
        as_text(&request.body)
    );

    // ... and it was framed with its byte length, not its character count. A
    // gateway that re-declared the length from anything else would truncate
    // every non-ASCII request body by exactly the number of continuation
    // bytes it holds.
    let declared = TOOL_CALL_BODY.len().to_string();
    assert_eq!(
        request.header("content-length"),
        Some(declared.as_str()),
        "the content-length the provider received is not the body's byte length"
    );
}

// --- 2. a provider error, and a diagnostic that carries none of it ----------

/// Two failures hide here, and they are opposites. A gateway that treated a
/// `4xx` as its own error would replace the provider's body with a generic
/// one, and the harness would lose the retry-after, the quota and the reason
/// — the whole content of a rate limit. A gateway that instead copied that
/// body into its diagnostic would put a provider's message, and whatever of
/// the user's prompt it quotes back, into a log file. Both are asserted on
/// the same exchange, so neither can be satisfied by giving up the other.
#[test]
fn a_provider_error_reaches_the_harness_byte_for_byte_while_the_diagnostic_keeps_only_its_status() {
    assert!(
        RATE_LIMIT_BODY.contains(RATE_LIMIT_SENTINEL)
            && RATE_LIMIT_BODY.contains("rate_limit_error"),
        "the planted error body no longer carries the markers the diagnostic is scanned for, so \
         the `!contains` assertions below would pass on any implementation"
    );

    let fixture = FixtureUpstream::answering(
        "HTTP/1.1 429 Too Many Requests",
        "content-type: application/json\r\nretry-after: 17\r\n",
        RATE_LIMIT_BODY,
    );
    let upstream = upstream_to(&fixture);
    let token = GatewayToken(PLANTED_TOKEN.to_owned());
    let agent = agent();

    let (exchange, response) = serve_one(
        &token,
        &upstream,
        &agent,
        &messages_request(PLANTED_TOKEN, "{\"model\":\"probe\"}"),
    );
    let received = as_text(&response);

    // The harness's half: the status, the provider's own headers, and the
    // body exactly as the provider wrote it.
    assert!(
        received.starts_with("HTTP/1.1 429 Too Many Requests\r\n"),
        "the provider's status did not reach the harness: {received}"
    );
    assert!(
        received.contains("retry-after: 17"),
        "the provider's retry-after did not reach the harness, so a client cannot back off the \
         way the provider asked: {received}"
    );
    assert_eq!(
        body_of(&response),
        RATE_LIMIT_BODY.as_bytes(),
        "the provider's error body did not reach the harness byte-for-byte; a gateway that \
         substitutes its own message for a provider's takes the reason with it"
    );

    // The diagnostic's half: the status, and nothing that was in the body.
    let rendered = format!("{exchange:?}");
    assert_eq!(
        exchange.status, 429,
        "the exchange recorded a status other than the one the harness was told"
    );
    let Outcome::Forwarded {
        upstream_status,
        bytes,
    } = exchange.outcome
    else {
        panic!("a provider that answered was not recorded as forwarded: {rendered}");
    };
    assert_eq!(
        upstream_status, 429,
        "the exchange did not record the status the provider actually returned"
    );
    assert_eq!(
        bytes,
        RATE_LIMIT_BODY.len() as u64,
        "the exchange recorded a byte count that is not the body it moved"
    );

    for forbidden in [
        RATE_LIMIT_SENTINEL,
        "rate_limit_error",
        "per-minute",
        "Number of request tokens",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "the exchange's diagnostic carried {forbidden:?} out of the provider's error body: an \
             Exchange may record that a request was refused upstream and never what the refusal \
             said, because a provider's message quotes back what the user sent it"
        );
    }
}

/// The other way a request ends: the provider was never reached at all, so
/// there is no status to forward and no body to pass on. Lose this and a
/// harness pointed at a gateway whose provider is down gets a connection
/// reset instead of an error it can parse — which it reports as a Glasshouse
/// bug, because from the harness's side that is what it looks like.
///
/// The second half is the diagnostic. A failed connection is the one place an
/// HTTP client is most likely to render the request it was about to make —
/// which is the credential, the target and the host together — so the detail
/// is scanned for both planted values and for the address the request was
/// addressed to. Asserted here on a real failure rather than against
/// [`super::ingress`]'s own vocabulary, so it goes on holding however that
/// detail comes to be written.
#[test]
fn an_unreachable_provider_answers_the_harness_and_leaves_no_credential_in_the_diagnostic() {
    let address = closed_loopback_address();
    let upstream = upstream_at(&format!("http://{address}"));
    let token = GatewayToken(PLANTED_TOKEN.to_owned());
    let agent = agent();

    let (exchange, response) = serve_one(
        &token,
        &upstream,
        &agent,
        &messages_request(PLANTED_TOKEN, "{\"model\":\"probe\"}"),
    );
    let received = as_text(&response);

    assert!(
        received.starts_with("HTTP/1.1 502 Bad Gateway\r\n"),
        "an unreachable provider did not become a gateway error the harness can read: {received}"
    );
    assert!(
        received.contains("content-type: application/json"),
        "the gateway's own error was not sent as JSON, so a harness expecting the provider's \
         protocol cannot parse it: {received}"
    );
    for shape in [
        "\"type\":\"error\"",
        "\"type\":\"api_error\"",
        "\"message\":",
    ] {
        assert!(
            received.contains(shape),
            "the gateway's own error is not in the shape the harness's protocol uses; {shape:?} \
             is missing: {received}"
        );
    }

    let rendered = format!("{exchange:?}");
    assert_eq!(
        exchange.status, 502,
        "the exchange recorded a status other than the one the harness was told"
    );
    let Outcome::Unreachable { detail } = &exchange.outcome else {
        panic!("a provider that was never reached was not recorded as unreachable: {rendered}");
    };
    assert!(
        !detail.is_empty(),
        "an unreachable provider recorded no detail at all, so every assertion about it below is \
         vacuous"
    );
    assert!(
        !detail.contains(PROVIDER_CREDENTIAL),
        "the transport error's detail carried the provider credential; a failure to connect is \
         the one path where an HTTP client is most likely to render the whole request it was \
         about to make"
    );
    assert!(
        !detail.contains(PLANTED_TOKEN),
        "the transport error's detail carried the gateway's own token"
    );
    assert!(
        !detail.contains(&address.to_string()),
        "the transport error's detail quoted the address the request was addressed to; a \
         diagnostic that echoes the request it failed to send is one provider away from echoing \
         a credential that travelled in a URL"
    );
}

// --- 3. nothing renders either secret ---------------------------------------

/// The gateway's entire reason to exist is that two secrets stay inside this
/// process: the provider's credential, which the child harness must never be
/// given, and the instance token, which is what the child is given instead.
/// Every one of them has a `Debug`, and a `Debug` is how a field reaches a
/// log without anybody deciding it should. Lose this and the first `tracing`
/// call that takes a gateway, an upstream or an exchange publishes one of the
/// two — and the response scan is the other half: a gateway that echoed the
/// credential it attached back to the child would have handed over the exact
/// thing the token exists to avoid handing over.
#[test]
fn no_rendering_the_gateway_can_produce_carries_either_planted_secret() {
    let fixture = FixtureUpstream::answering(
        "HTTP/1.1 200 OK",
        "content-type: application/json\r\n",
        "{\"ok\":true}",
    );
    let gateway = gateway_to(&fixture);
    let minted = gateway.token();
    let upstream = upstream_to(&fixture);
    let token = GatewayToken(PLANTED_TOKEN.to_owned());
    let agent = agent();

    // A real exchange through a real gateway, so the minted token scanned for
    // below is one that actually authenticated a request rather than one that
    // was merely generated.
    let through_the_gateway = send_and_read(
        gateway.address(),
        &messages_request(minted.expose(), "{\"model\":\"probe\"}"),
    );
    assert!(
        as_text(&through_the_gateway).starts_with("HTTP/1.1 200 OK"),
        "the gateway did not serve the exchange whose response is scanned below"
    );

    // Three outcomes, because they take three different paths out of the
    // ingress and only one of them has a provider response to copy from.
    let (forwarded, forwarded_response) = serve_one(
        &token,
        &upstream,
        &agent,
        &messages_request(PLANTED_TOKEN, "{\"model\":\"probe\"}"),
    );
    let (unauthenticated, unauthenticated_response) = serve_one(
        &token,
        &upstream,
        &agent,
        &messages_request("not-this-instances-token", "{\"model\":\"probe\"}"),
    );
    let unreachable_upstream = upstream_at(&format!("http://{}", closed_loopback_address()));
    let (unreachable, unreachable_response) = serve_one(
        &token,
        &unreachable_upstream,
        &agent,
        &messages_request(PLANTED_TOKEN, "{\"model\":\"probe\"}"),
    );

    assert!(
        matches!(forwarded.outcome, Outcome::Forwarded { .. }),
        "the forwarded exchange took another path, so its rendering is not the one this test \
         means to scan: {forwarded:?}"
    );
    assert!(
        matches!(unauthenticated.outcome, Outcome::Unauthenticated),
        "the refused exchange took another path, so its rendering is not the one this test means \
         to scan: {unauthenticated:?}"
    );
    let Outcome::Unreachable { detail } = &unreachable.outcome else {
        panic!(
            "the unreachable exchange took another path, so there is no transport detail to \
             scan: {unreachable:?}"
        );
    };

    for (what, rendering) in [
        ("the gateway's own Debug", format!("{gateway:?}")),
        ("the upstream's own Debug", format!("{upstream:?}")),
        (
            "the Debug of a forwarded exchange",
            format!("{forwarded:?}"),
        ),
        (
            "the Debug of an unauthenticated exchange",
            format!("{unauthenticated:?}"),
        ),
        (
            "the Debug of an unreachable exchange",
            format!("{unreachable:?}"),
        ),
        ("the transport-error detail", (*detail).to_owned()),
        ("the transport-error detail after redaction", redact(detail)),
        (
            "the response a real gateway sent its child",
            as_text(&through_the_gateway),
        ),
        (
            "the response to a forwarded request",
            as_text(&forwarded_response),
        ),
        (
            "the response to an unauthenticated request",
            as_text(&unauthenticated_response),
        ),
        (
            "the response to a request whose provider was unreachable",
            as_text(&unreachable_response),
        ),
    ] {
        carries_no_planted_secret(what, &rendering, minted);
    }

    // A redacted field is shown rather than omitted, in both places that hold
    // a secret. A `Debug` that simply dropped the field would pass every
    // `!contains` above and would leave a reader unable to tell a gateway
    // holding a credential from one holding none.
    assert!(
        format!("{gateway:?}").contains(REDACTED),
        "the gateway's Debug omits its token rather than showing the redaction marker"
    );
    assert!(
        format!("{upstream:?}").contains(REDACTED),
        "the upstream's Debug omits its credential rather than showing the redaction marker"
    );

    // ... and `redact` is the crate's second line of defence for text it did
    // not write, so it is checked here on text that really does name both
    // planted values. The transport detail above no longer needs it — the
    // ingress keeps a phrase of its own rather than an error's — but every
    // future diagnostic built out of somebody else's string will, and a
    // redactor nobody has watched work is a redactor nobody knows works.
    let foreign = format!(
        "connecting to https://provider.invalid/v1/messages failed after sending \
         `Authorization: Bearer {PLANTED_TOKEN}` and the key {PROVIDER_CREDENTIAL}"
    );
    let cleaned = redact(&foreign);
    assert!(
        foreign.contains(PROVIDER_CREDENTIAL) && foreign.contains(PLANTED_TOKEN),
        "the sample this checks the redactor against no longer contains either planted value, so \
         the two assertions below would pass on a redactor that did nothing"
    );
    assert!(
        !cleaned.contains(PROVIDER_CREDENTIAL),
        "redact left a provider credential in foreign error text; it is what stands between a \
         string this project did not write and a diagnostic this project keeps"
    );
    assert!(
        !cleaned.contains(PLANTED_TOKEN),
        "redact left a bearer token in foreign error text"
    );
    assert!(
        cleaned.contains(REDACTED),
        "redact removed the planted values without leaving the marker, so a reader cannot tell a \
         redacted diagnostic from one that never carried a credential"
    );
}

/// The scan above is worth having only if it can fail, and every assertion it
/// makes is a negative one — so a scan that looked at the wrong string, or
/// compared with the wrong needle, would pass every rendering in this module
/// and go on reporting nothing for the life of the project. Lose this and the
/// test above degrades silently into a test of nothing at all, which is the
/// worst state a security assertion can be in: still green, still cited.
#[test]
fn the_secret_scan_finds_every_planted_value_it_is_given() {
    // A fixed stand-in rather than a generated token: nothing here is a claim
    // about the generator, and a fixed value keeps this test deterministic.
    let minted =
        GatewayToken("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned());

    for (leaking, what) in [
        (
            format!("credential={PROVIDER_CREDENTIAL}"),
            "a whole provider credential",
        ),
        (format!("token={PLANTED_TOKEN}"), "a whole gateway token"),
        (
            format!("token={}", minted.expose()),
            "a whole token a gateway minted",
        ),
        (
            format!(
                "the token begins {}...",
                &PLANTED_TOKEN[..SHORTEST_TOKEN_FRAGMENT]
            ),
            "the shortest scanned prefix of a gateway token",
        ),
        (
            format!(
                "...{} ends it",
                &PLANTED_TOKEN[PLANTED_TOKEN.len() - SHORTEST_TOKEN_FRAGMENT..]
            ),
            "the shortest scanned suffix of a gateway token",
        ),
        (
            format!(
                "the key begins {}...",
                &PROVIDER_CREDENTIAL[..SHORTEST_CREDENTIAL_FRAGMENT]
            ),
            "the shortest scanned prefix of a provider credential",
        ),
        (
            format!(
                "...{} ends it",
                &PROVIDER_CREDENTIAL[PROVIDER_CREDENTIAL.len() - SHORTEST_CREDENTIAL_FRAGMENT..]
            ),
            "the shortest scanned suffix of a provider credential",
        ),
    ] {
        assert!(
            planted_secret_in(&leaking, &minted).is_some(),
            "the scan did not find {what}, so every negative assertion it makes elsewhere is a \
             statement about this function rather than about the gateway"
        );
    }

    // ... and it stays silent on a rendering of exactly the shape the test
    // above scans, which is what stops it from being a function that always
    // says yes — and is also where the claim behind [`PLANTED_TOKEN`]'s
    // alphabet gets checked against real text rather than asserted in prose.
    let clean = format!(
        "Gateway {{ address: 127.0.0.1:54321, token: {REDACTED} }} \
         HTTP/1.1 429 Too Many Requests \
         {{\"type\":\"error\",\"error\":{{\"type\":\"rate_limit_error\"}}}}"
    );
    assert!(
        planted_secret_in(&clean, &minted).is_none(),
        "the scan reported a leak in a rendering that carries no planted value at all, so what it \
         reports elsewhere is a coincidence rather than a secret"
    );
}

/// Which planted value `rendering` carries, described — or `None`.
///
/// A function that *answers* rather than one that asserts, so that
/// [`the_secret_scan_finds_every_planted_value_it_is_given`] can watch it
/// fire.
///
/// The description names what leaked and never renders it, and neither does
/// the caller. That is the same care `mod.rs`'s
/// `two_gateways_mint_different_tokens` takes: the single run that ever fails
/// is the run where a secret really is in the text, and printing the haystack
/// would publish it to whatever collected the output.
fn planted_secret_in(rendering: &str, minted: &GatewayToken) -> Option<String> {
    if rendering.contains(PROVIDER_CREDENTIAL) {
        return Some("the provider credential, whole".to_owned());
    }
    if rendering.contains(PLANTED_TOKEN) {
        return Some("this instance's gateway token, whole".to_owned());
    }
    // The minted token is scanned whole and never in fragments — see this
    // module's header, and `mod.rs`'s
    // `debug_on_a_gateway_token_prints_a_fixed_marker_and_never_the_token`,
    // for why fragments of 64 hex characters produce failures that are
    // coincidences rather than leaks.
    if rendering.contains(minted.expose()) {
        return Some("the token a running gateway minted, whole".to_owned());
    }

    for length in SHORTEST_TOKEN_FRAGMENT..=PLANTED_TOKEN.len() {
        if rendering.contains(&PLANTED_TOKEN[..length]) {
            return Some(format!(
                "the first {length} characters of the gateway token"
            ));
        }
        if rendering.contains(&PLANTED_TOKEN[PLANTED_TOKEN.len() - length..]) {
            return Some(format!("the last {length} characters of the gateway token"));
        }
    }

    for length in SHORTEST_CREDENTIAL_FRAGMENT..=PROVIDER_CREDENTIAL.len() {
        if rendering.contains(&PROVIDER_CREDENTIAL[..length]) {
            return Some(format!(
                "the first {length} characters of the provider credential"
            ));
        }
        if rendering.contains(&PROVIDER_CREDENTIAL[PROVIDER_CREDENTIAL.len() - length..]) {
            return Some(format!(
                "the last {length} characters of the provider credential"
            ));
        }
    }

    None
}

/// Fail if `rendering` carries either planted secret, whole or in part.
fn carries_no_planted_secret(what: &str, rendering: &str, minted: &GatewayToken) {
    assert!(
        !rendering.is_empty(),
        "{what} is empty, so every assertion made about it is vacuous"
    );
    if let Some(leak) = planted_secret_in(rendering, minted) {
        panic!(
            "{what} carried {leak}. A prefix or a suffix is not a cosmetic leak — it is the \
             search space an attacker no longer has to cover — and the value is deliberately not \
             printed here, because printing it would publish it to whatever collected this output."
        );
    }
}

// --- 4. one ingress per protocol, chosen by the request target --------------

/// Every protocol this suite builds a gateway over, in the order the routes
/// are declared.
///
/// Order matters to what these tests prove rather than to what they do: a
/// gateway that ignored the request target and appended everything to the
/// **first** declared base URL is the implementation this whole section
/// exists to fail, and it would pass every positive assertion below if the
/// protocol under test were always declared first.
const PROTOCOLS: [&str; 3] = [ANTHROPIC_MESSAGES, OPENAI_RESPONSES, OPENAI_CHAT];

/// What a base URL that must never be reached answers with.
///
/// A marker rather than an ordinary body, so a request that went to the
/// wrong ingress fails twice and legibly: once on the connection count, and
/// once on a client that reads this instead of what it asked for.
const WRONG_INGRESS_BODY: &str = "{\"reached\":\"WRONG-INGRESS-BASE-URL\"}";

/// An OpenAI Responses request body, in the shape Codex sends one.
///
/// Non-ASCII on purpose: its byte length and its character length disagree,
/// so the `content-length` assertions on the new ingresses are the same
/// statement about framing that [`TOOL_CALL_BODY`] makes about the Anthropic
/// one.
const RESPONSES_BODY: &str = concat!(
    r#"{"model":"gpt-5-codex","stream":true,"input":[{"role":"user","#,
    r#""content":[{"type":"input_text","text":"Grüße — 日本語 🔧"}]}]}"#
);

/// An OpenAI Chat Completions request body, non-ASCII for the same reason.
const CHAT_BODY: &str = concat!(
    r#"{"model":"gpt-4.1","stream":true,"messages":[{"role":"user","#,
    r#""content":"Grüße — 日本語 🔧"}]}"#
);

/// How long a streaming fixture waits for the client to say it has the first
/// chunk before giving up and writing the marker that fails the test.
///
/// Shorter than [`CLIENT_TIMEOUT`] on purpose, and by a wide margin: a
/// buffering gateway must be observed *failing this assertion* rather than
/// timing the client out first, because a client timeout is a flake and a
/// marker in the body is a diagnosis.
const STREAM_WAIT: Duration = Duration::from_secs(20);

/// One canned upstream per protocol the gateway serves.
///
/// Three, rather than the one every test above uses, because the
/// load-bearing half of a routing assertion is negative: a request reached
/// *this* base URL **and no other**. One fixture can only ever show the
/// first half, and the single-base-URL gateway this replaced satisfies the
/// first half for every target there is.
struct Fixtures {
    /// One `(protocol slug, fixture)` pair per served protocol, in
    /// [`PROTOCOLS`] order.
    served: Vec<(&'static str, FixtureUpstream)>,
}

impl Fixtures {
    /// Three fixtures, each answering every request with the same `200`.
    fn answering_ok() -> Self {
        Self {
            served: PROTOCOLS
                .iter()
                .map(|protocol| {
                    (
                        *protocol,
                        FixtureUpstream::answering(
                            "HTTP/1.1 200 OK",
                            "content-type: application/json\r\n",
                            "{\"ok\":true}",
                        ),
                    )
                })
                .collect(),
        }
    }

    /// Three fixtures, with `protocol`'s driven by `responder` and the other
    /// two answering [`WRONG_INGRESS_BODY`].
    ///
    /// The other two exist even though the test is about one of them: a
    /// streaming assertion that held only because the request went somewhere
    /// else entirely would otherwise be indistinguishable from one that held
    /// for the reason it claims.
    fn driven_by(
        protocol: &'static str,
        responder: impl Fn(&RecordedRequest, &mut TcpStream) + Send + Sync + 'static,
    ) -> Self {
        let mut responder = Some(responder);
        Self {
            served: PROTOCOLS
                .iter()
                .map(|served| {
                    let fixture = if *served == protocol {
                        FixtureUpstream::start(
                            responder
                                .take()
                                .expect("exactly one fixture stands in for the driven protocol"),
                        )
                    } else {
                        FixtureUpstream::answering(
                            "HTTP/1.1 200 OK",
                            "content-type: application/json\r\n",
                            WRONG_INGRESS_BODY,
                        )
                    };
                    (*served, fixture)
                })
                .collect(),
        }
    }

    /// An [`Upstream`] serving every protocol, each routed to its own
    /// fixture and all of them holding the one [`PROVIDER_CREDENTIAL`].
    fn upstream(&self) -> Upstream {
        let base_urls: Vec<(&str, String)> = self
            .served
            .iter()
            .map(|(protocol, fixture)| (*protocol, fixture.base_url()))
            .collect();
        let routes: Vec<(&str, &str)> = base_urls
            .iter()
            .map(|(protocol, base_url)| (*protocol, base_url.as_str()))
            .collect();
        upstream_serving(&routes)
    }

    /// A real running [`Gateway`], with its own minted token, in front of all
    /// three.
    fn gateway(&self) -> Gateway {
        Gateway::start(self.upstream()).expect("loopback is bindable")
    }

    /// The fixture standing in for `protocol`'s base URL.
    fn at(&self, protocol: &str) -> &FixtureUpstream {
        self.served
            .iter()
            .find(|(served, _)| *served == protocol)
            .map(|(_, fixture)| fixture)
            .unwrap_or_else(|| panic!("no fixture stands in for {protocol}"))
    }

    /// The one request that reached `protocol`'s base URL — failing unless
    /// no other base URL was ever **connected to**.
    fn only_request_to(&self, protocol: &str) -> RecordedRequest {
        for (served, fixture) in &self.served {
            if *served == protocol {
                continue;
            }
            assert_eq!(
                fixture.connections(),
                0,
                "a request that belongs to {protocol} opened a connection to the {served} base \
                 URL. A gateway serving several protocols from one provider has a different base \
                 URL for each, and a request sent to the wrong one is a request sent somewhere \
                 nobody asked for it to go"
            );
        }
        let mut arrived = self.at(protocol).requests();
        assert_eq!(
            arrived.len(),
            1,
            "the {protocol} base URL received a number of requests other than the one that was \
             sent to it"
        );
        arrived.remove(0)
    }

    /// Fail unless **nothing at all** was opened at any base URL.
    ///
    /// The assertion is on connections rather than on requests: a gateway
    /// that opened a socket to a provider and then thought better of it has
    /// still told that provider a Glasshouse instance is here, and would
    /// leave no request behind to say so.
    fn nothing_was_opened(&self, what: &str) {
        for (served, fixture) in &self.served {
            assert_eq!(
                fixture.connections(),
                0,
                "{what} opened a connection to the {served} base URL. A target the gateway cannot \
                 place must be refused before anything upstream exists — otherwise the provider \
                 sees traffic for a request the gateway had already decided it would not carry"
            );
        }
    }
}

/// The bytes a harness sends: a method, a request target, a bearer token and
/// an optional JSON body framed by its **byte** length.
///
/// Separate from [`messages_request`] rather than a generalisation of it.
/// That one carries `anthropic-version`, which belongs to one protocol and
/// would be a lie on the other two, and it is the exact spelling every test
/// written before there was more than one ingress asserts against.
fn request_for(method: &str, target: &str, token: &str, body: Option<&str>) -> Vec<u8> {
    let head = format!(
        "{method} {target} HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Authorization: Bearer {token}\r\n"
    );
    match body {
        Some(body) => format!(
            "{head}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
        None => format!("{head}\r\n"),
    }
    .into_bytes()
}

/// Fail unless the provider received the credential the upstream holds, and
/// nothing of the token the child presented.
///
/// Both halves are one property: the child is given the gateway's own token
/// *instead of* the credential, so a provider that received the token would
/// have been handed the value that proves a request came from a harness this
/// instance started.
///
/// Compared with `assert!` on an equality rather than with `assert_eq!`, and
/// the failure message prints neither side. A failing `assert_eq!` renders
/// both, and one of them is the credential — which would publish it to
/// whatever collected the test output.
fn carries_the_provider_credential_and_not_the_childs_token(
    request: &RecordedRequest,
    presented: &str,
) {
    let attached = request.header("authorization");
    assert!(
        attached.is_some(),
        "the provider received no authorization header at all, so the gateway forwarded a request \
         the provider has no way to authenticate"
    );
    let expected = format!("Bearer {PROVIDER_CREDENTIAL}");
    assert!(
        attached == Some(expected.as_str()),
        "the provider did not receive the credential the upstream holds; the two values are \
         deliberately not printed here"
    );
    for (name, value) in &request.headers {
        assert!(
            !value.contains(presented),
            "the token the child presented survived to the provider in the {name} header. The \
             gateway exists so that the child holds a value worthless off this machine and the \
             provider holds one that is not; forwarding the child's own reverses that"
        );
    }
}

/// Lose this and a Codex child pointed at a Glasshouse gateway has its
/// `/responses` request appended to whichever base URL happened to be
/// declared first — most likely the provider's Anthropic Messages endpoint,
/// which answers a Responses payload with a `4xx` naming neither the reason
/// nor the gateway. Worse, it is a request sent to an endpoint nobody asked
/// for it to go to, carrying the user's prompt.
///
/// Both spellings, because whether a harness sends `/v1` depends on its own
/// idea of where its base URL ends and not on the protocol: Codex 0.149.1
/// pointed at a path-less base URL — the only kind [`Gateway::base_url`]
/// hands out — really does send `POST /responses`.
#[test]
fn a_responses_request_reaches_the_responses_base_url_and_opens_no_other() {
    for target in ["/responses", "/v1/responses"] {
        let fixtures = Fixtures::answering_ok();
        let gateway = fixtures.gateway();
        let presented = gateway.token().expose().to_owned();

        let response = as_text(&send_and_read(
            gateway.address(),
            &request_for("POST", target, &presented, Some(RESPONSES_BODY)),
        ));
        assert!(
            response.starts_with("HTTP/1.1 200 OK"),
            "the exchange for {target} did not complete, so nothing below is a statement about \
             where it went: {response}"
        );

        let forwarded = fixtures.only_request_to(OPENAI_RESPONSES);
        assert_eq!(
            forwarded.method, "POST",
            "the method was rewritten on the way to the Responses base URL"
        );
        assert_eq!(
            forwarded.target, target,
            "the request target did not reach the Responses base URL as the harness wrote it"
        );
        assert!(
            forwarded.body == RESPONSES_BODY.as_bytes(),
            "the Responses body did not arrive byte-for-byte; the provider received {:?}",
            as_text(&forwarded.body)
        );
        carries_the_provider_credential_and_not_the_childs_token(&forwarded, &presented);
    }
}

/// The same property for the third ingress, and it is not the same test:
/// `/chat/completions` is two path segments where the other two are one, so
/// a router that matched on "the first segment" would place the other two
/// and refuse this one, and a gateway that served only the protocols it was
/// tested on would leave a Chat-only provider unreachable.
#[test]
fn a_chat_completions_request_reaches_the_chat_base_url_and_opens_no_other() {
    for target in ["/chat/completions", "/v1/chat/completions"] {
        let fixtures = Fixtures::answering_ok();
        let gateway = fixtures.gateway();
        let presented = gateway.token().expose().to_owned();

        let response = as_text(&send_and_read(
            gateway.address(),
            &request_for("POST", target, &presented, Some(CHAT_BODY)),
        ));
        assert!(
            response.starts_with("HTTP/1.1 200 OK"),
            "the exchange for {target} did not complete, so nothing below is a statement about \
             where it went: {response}"
        );

        let forwarded = fixtures.only_request_to(OPENAI_CHAT);
        assert_eq!(
            forwarded.method, "POST",
            "the method was rewritten on the way to the Chat base URL"
        );
        assert_eq!(
            forwarded.target, target,
            "the request target did not reach the Chat base URL as the harness wrote it"
        );
        assert!(
            forwarded.body == CHAT_BODY.as_bytes(),
            "the Chat body did not arrive byte-for-byte; the provider received {:?}",
            as_text(&forwarded.body)
        );
        carries_the_provider_credential_and_not_the_childs_token(&forwarded, &presented);
    }
}

// --- 5. streaming, on the two ingresses that are new ------------------------

/// One streaming exchange, asserted the way [`mod@super`]'s
/// `a_streamed_response_reaches_the_client_before_the_upstream_has_finished`
/// asserts the Anthropic one — and built so that a buffering implementation
/// cannot pass rather than so that a streaming one happens to.
///
/// The fixture writes its first event, then **blocks until the client says
/// it has received that event**, and only then writes the second. So the
/// second event exists at all only if the first reached the client while the
/// response was still open. A gateway that read the upstream body to the end
/// before writing anything deadlocks instead: the client never acknowledges,
/// the fixture's wait expires, and the marker it writes in place of the
/// second event is what fails.
///
/// Nothing here sleeps. The synchronisation is a channel, in both
/// directions: the fixture blocks on `recv_timeout` and the client blocks on
/// `read`.
fn a_stream_arrives_incrementally(protocol: &'static str, target: &str, body: &str) {
    let (saw_first, first_seen) = mpsc::channel::<()>();
    let first_seen = Mutex::new(first_seen);

    let fixtures = Fixtures::driven_by(protocol, move |_request, out| {
        let _ = out.write_all(
            b"HTTP/1.1 200 OK\r\n\
              content-type: text/event-stream\r\n\
              transfer-encoding: chunked\r\n\r\n",
        );
        let first = "event: one\ndata: {\"n\":1}\n\n";
        let _ = out.write_all(format!("{:x}\r\n{first}\r\n", first.len()).as_bytes());
        let _ = out.flush();

        let streamed = first_seen
            .lock()
            .expect("no test panics while holding this")
            .recv_timeout(STREAM_WAIT)
            .is_ok();
        let second = if streamed {
            "event: two\ndata: {\"n\":2}\n\n"
        } else {
            "event: BUFFERED-NOT-STREAMED\n\n"
        };
        let _ = out.write_all(format!("{:x}\r\n{second}\r\n0\r\n\r\n", second.len()).as_bytes());
        let _ = out.flush();
    });

    let gateway = fixtures.gateway();
    let mut client = send(
        gateway.address(),
        &request_for("POST", target, gateway.token().expose(), Some(body)),
    );

    let mut seen = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        let read = client.read(&mut buffer).unwrap_or_else(|err| {
            panic!(
                "the gateway did not deliver the first event of a {protocol} stream before the \
                 upstream had finished ({err}); {} bytes had arrived: {:?}",
                seen.len(),
                as_text(&seen)
            )
        });
        assert!(
            read > 0,
            "the gateway closed a {protocol} response before its first event arrived; {} bytes \
             had arrived: {:?}",
            seen.len(),
            as_text(&seen)
        );
        seen.extend_from_slice(&buffer[..read]);
        if as_text(&seen).contains("event: one") {
            break;
        }
    }
    saw_first.send(()).expect("the fixture is still writing");

    let mut rest = Vec::new();
    client.read_to_end(&mut rest).expect("the stream completes");
    seen.extend_from_slice(&rest);
    let text = as_text(&seen);

    assert!(
        text.contains("event: one"),
        "the first event of a {protocol} stream never reached the client: {text}"
    );
    // Asserted before the presence of the second event, because this is the
    // diagnosis and that is only the symptom: a run that prints "the second
    // event never arrived" leaves a reader to work out why, and a run that
    // prints this one has already said.
    assert!(
        !text.contains("BUFFERED-NOT-STREAMED"),
        "the upstream's wait for the first event to reach the client timed out, so the gateway is \
         buffering a {protocol} response rather than streaming it: {text}"
    );
    assert!(
        text.contains("event: two"),
        "the second event of a {protocol} stream never reached the client: {text}"
    );
    assert!(
        !text.contains("WRONG-INGRESS-BASE-URL"),
        "a {protocol} request was answered by another protocol's base URL, so everything above is \
         a statement about the wrong ingress: {text}"
    );

    // ... and it really did travel the ingress this test names. Without
    // this the assertions above would go on holding for a gateway that
    // routed every target to one base URL, which is the defect the whole
    // section exists for.
    let forwarded = fixtures.only_request_to(protocol);
    assert_eq!(
        forwarded.target, target,
        "the streamed request did not reach {protocol}'s base URL as the harness wrote it"
    );
}

/// Line 4 of the capability map is "streaming is preserved end-to-end", and
/// [`mod@super`]'s twin of this asserts it only for Anthropic Messages. Lose
/// this and a gateway that buffers a Codex stream leaves that test green:
/// the child shows nothing for the whole generation and then everything at
/// once, which reads as a hang and is reported as one.
#[test]
fn a_streamed_responses_reply_reaches_the_client_before_the_upstream_has_finished() {
    a_stream_arrives_incrementally(OPENAI_RESPONSES, "/responses", RESPONSES_BODY);
}

/// The same, for the third ingress. Separate from the Responses test rather
/// than folded into it so that a regression names the protocol it broke.
#[test]
fn a_streamed_chat_reply_reaches_the_client_before_the_upstream_has_finished() {
    a_stream_arrives_incrementally(OPENAI_CHAT, "/v1/chat/completions", CHAT_BODY);
}

// --- 6. a target that belongs to nothing is refused, and forwarded nowhere --

/// The narrowing that came with serving more than one protocol, asserted on
/// the thing that actually matters. A single-protocol gateway appended every
/// request target to its one base URL; with three, "append it to the first
/// one" would send a `/v1/models` — or a `HEAD /api/hello`, which Claude
/// Code 2.1.245 really does send before its first `/v1/messages` — to
/// whichever provider endpoint happened to be declared first.
///
/// So the load-bearing assertion is **every fixture's connection count**,
/// not the status. A gateway that opened a connection upstream, read the
/// answer and then decided to write its own `404` would pass an assertion on
/// the status and would still have sent a request nobody asked for.
///
/// The refusal is also asserted on requests that carry a body: the gateway
/// has to drain what is still in flight before closing, or the client sees a
/// connection reset — a network error — in place of the status that would
/// have told it what was wrong.
#[test]
fn a_target_belonging_to_no_served_protocol_is_refused_and_nothing_is_opened_upstream() {
    let fixtures = Fixtures::answering_ok();
    let upstream = fixtures.upstream();
    let token = GatewayToken(PLANTED_TOKEN.to_owned());
    let agent = agent();

    for (method, target, body) in [
        // Observed against a recording listener: Claude Code 2.1.245 sends
        // this before its first `/v1/messages`, and carries on after a
        // non-2xx answer to it.
        ("HEAD", "/api/hello", None),
        ("GET", "/v1/models", None),
        // The same target with a body, because a refusal that resets the
        // connection is not a refusal the client can read.
        ("POST", "/v1/models", Some("{\"probe\":true}")),
        // Prefix matches that are not on a path-segment boundary — one per
        // served protocol, because each declares its own prefix and each
        // could be matched with `starts_with` alone.
        ("POST", "/v1/messagesomethingelse", Some(TOOL_CALL_BODY)),
        ("POST", "/responsesomethingelse", Some(RESPONSES_BODY)),
        ("POST", "/chat/completionsomethingelse", Some(CHAT_BODY)),
    ] {
        let (exchange, response) = serve_one(
            &token,
            &upstream,
            &agent,
            &request_for(method, target, PLANTED_TOKEN, body),
        );
        let received = as_text(&response);
        let what = format!("{method} {target}");

        assert!(
            received.starts_with("HTTP/1.1 404 Not Found\r\n"),
            "{what} was not refused with the status a harness can read: {received}"
        );
        assert!(
            received.contains("content-type: application/json"),
            "{what} was refused with something other than JSON, so a harness expecting a \
             provider's protocol cannot parse it: {received}"
        );

        let body_bytes = body_of(&response);
        if method == "HEAD" {
            // A response to `HEAD` carries a `GET`'s headers and none of its
            // body. Writing one is not a harmless extra: a client reads the
            // declared length, finds bytes it was told would not be there,
            // and takes them for the start of the next response. This is the
            // first response in this gateway's life a `HEAD` can reach —
            // Claude Code's `HEAD /api/hello` belongs to no protocol, so it
            // is refused here rather than forwarded, and `forward`'s own
            // rule never gets to apply to it.
            assert!(
                received.contains("content-length: "),
                "the refusal of {what} dropped the length a HEAD response still declares: \
                 {received}"
            );
            assert!(
                body_bytes.is_empty(),
                "the refusal of {what} carried a body; a client that reads it reads the start \
                 of a response that does not exist: {}",
                as_text(body_bytes)
            );
            fixtures.nothing_was_opened(&what);
            continue;
        }

        let parsed: serde_json::Value = serde_json::from_slice(body_bytes).unwrap_or_else(|err| {
            panic!(
                "the refusal of {what} did not carry readable JSON ({err}), so a harness sees \
                     a truncated or malformed answer rather than an error: {}",
                as_text(body_bytes)
            )
        });
        assert_eq!(
            parsed["type"], "error",
            "the refusal of {what} is not in the error shape a harness's protocol uses: {parsed}"
        );
        assert_eq!(
            parsed["error"]["type"], "not_found_error",
            "the refusal of {what} did not name the kind of error it is: {parsed}"
        );
        assert!(
            parsed["error"]["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "the refusal of {what} carried no message, so a user reading it learns only that \
             something was not found: {parsed}"
        );

        assert_eq!(
            exchange.status, 404,
            "the exchange for {what} recorded a status other than the one the harness was told"
        );
        assert!(
            matches!(exchange.outcome, Outcome::Unrouted),
            "{what} was refused but was not recorded as unrouted, so the one line that says a \
             request was never placed says something else: {exchange:?}"
        );
        assert!(
            exchange.protocol.is_none(),
            "the exchange for {what} named a protocol for a request that belongs to none: \
             {exchange:?}"
        );
        assert!(
            exchange.host.is_empty(),
            "the exchange for {what} named an upstream host the request was never going to reach; \
             a log that invents the one fact it exists to record is worse than no log: \
             {exchange:?}"
        );

        fixtures.nothing_was_opened(&what);
    }

    // ... and none of those zeroes is because nothing here could have
    // reached a fixture in the first place. Same upstream, same fixtures,
    // one target that *is* served: if this does not arrive, every count
    // above was a statement about a broken test rather than about a refusal.
    let (served, response) = serve_one(
        &token,
        &upstream,
        &agent,
        &messages_request(PLANTED_TOKEN, "{\"model\":\"probe\"}"),
    );
    assert!(
        as_text(&response).starts_with("HTTP/1.1 200 OK"),
        "the control exchange did not complete, so the zero connection counts above prove nothing"
    );
    assert!(
        matches!(served.outcome, Outcome::Forwarded { .. }),
        "the control exchange was not forwarded, so the zero connection counts above prove \
         nothing: {served:?}"
    );
    assert_eq!(
        served.protocol.as_deref(),
        Some(ANTHROPIC_MESSAGES),
        "the control exchange did not record the protocol that carried it: {served:?}"
    );
    assert_eq!(
        fixtures.at(ANTHROPIC_MESSAGES).connections(),
        1,
        "the served target opened no connection to its own base URL, so the refusals above were \
         asserted against fixtures that could not have been reached anyway"
    );
    assert_eq!(
        fixtures.at(OPENAI_RESPONSES).connections(),
        0,
        "a served Anthropic target also opened a connection to the Responses base URL"
    );
    assert_eq!(
        fixtures.at(OPENAI_CHAT).connections(),
        0,
        "a served Anthropic target also opened a connection to the Chat base URL"
    );
}

// --- 7. and none of the new paths renders either secret ---------------------

/// [`no_rendering_the_gateway_can_produce_carries_either_planted_secret`] is
/// the same assertion over the paths a single-protocol gateway had, and it
/// cannot cover these: a routed exchange now renders a protocol slug and the
/// host of *the route that carried it*, an unrouted one renders neither, and
/// an [`Upstream`] now holds three base URLs beside the one credential. Each
/// of those is a field that did not exist when that test was written, and a
/// field is how a secret reaches a log without anybody deciding it should.
///
/// Lose this and the first `tracing` call taking a three-protocol upstream,
/// or the first `Debug` of a Codex exchange, publishes the provider
/// credential — the exact thing the child harness is given a token instead
/// of.
#[test]
fn no_rendering_the_new_ingresses_produce_carries_either_planted_secret() {
    let fixtures = Fixtures::answering_ok();
    let gateway = fixtures.gateway();
    let minted = gateway.token();
    let upstream = fixtures.upstream();
    let token = GatewayToken(PLANTED_TOKEN.to_owned());
    let agent = agent();

    // A real exchange on a new ingress through a real gateway, so the minted
    // token scanned for below is one that actually authenticated a Responses
    // request rather than one that was merely generated.
    let through_the_gateway = send_and_read(
        gateway.address(),
        &request_for("POST", "/responses", minted.expose(), Some(RESPONSES_BODY)),
    );
    assert!(
        as_text(&through_the_gateway).starts_with("HTTP/1.1 200 OK"),
        "the gateway did not serve the exchange whose response is scanned below"
    );

    // Three paths out of the ingress that only exist now that it routes:
    // two that were placed on different routes, and one that was refused
    // before a route could be chosen.
    let (responses, responses_response) = serve_one(
        &token,
        &upstream,
        &agent,
        &request_for("POST", "/v1/responses", PLANTED_TOKEN, Some(RESPONSES_BODY)),
    );
    let (chat, chat_response) = serve_one(
        &token,
        &upstream,
        &agent,
        &request_for("POST", "/chat/completions", PLANTED_TOKEN, Some(CHAT_BODY)),
    );
    let (unrouted, unrouted_response) = serve_one(
        &token,
        &upstream,
        &agent,
        &request_for(
            "POST",
            "/v1/models",
            PLANTED_TOKEN,
            Some("{\"probe\":true}"),
        ),
    );

    assert!(
        matches!(responses.outcome, Outcome::Forwarded { .. }),
        "the Responses exchange took another path, so its rendering is not the one this test \
         means to scan: {responses:?}"
    );
    assert!(
        matches!(chat.outcome, Outcome::Forwarded { .. }),
        "the Chat exchange took another path, so its rendering is not the one this test means to \
         scan: {chat:?}"
    );
    assert!(
        matches!(unrouted.outcome, Outcome::Unrouted),
        "the refused exchange took another path, so its rendering is not the one this test means \
         to scan: {unrouted:?}"
    );

    for (what, rendering) in [
        (
            "the Debug of an upstream serving three protocols",
            format!("{upstream:?}"),
        ),
        (
            "the Debug of an exchange carried by the Responses route",
            format!("{responses:?}"),
        ),
        (
            "the Debug of an exchange carried by the Chat route",
            format!("{chat:?}"),
        ),
        (
            "the Debug of an exchange that belonged to no route",
            format!("{unrouted:?}"),
        ),
        (
            "the response a real gateway sent a Responses request",
            as_text(&through_the_gateway),
        ),
        (
            "the response to a Responses request",
            as_text(&responses_response),
        ),
        ("the response to a Chat request", as_text(&chat_response)),
        (
            "the response to a request that belonged to no route",
            as_text(&unrouted_response),
        ),
    ] {
        carries_no_planted_secret(what, &rendering, minted);
    }

    // ... and the credential is shown as redacted rather than dropped, while
    // everything a diagnostic is actually for survives. A `Debug` that
    // simply omitted the field would pass every `!contains` above and would
    // leave a reader unable to tell an upstream holding a credential from
    // one holding none.
    let rendered = format!("{upstream:?}");
    assert!(
        rendered.contains(REDACTED),
        "the upstream's Debug omits its credential rather than showing the redaction marker: \
         {rendered}"
    );
    for name in PROTOCOLS {
        assert!(
            rendered.contains(name),
            "the upstream's Debug no longer names {name}, so a reader cannot tell which protocols \
             a gateway is serving and the scan above is over a rendering that carries less than \
             the real one"
        );
    }
}
