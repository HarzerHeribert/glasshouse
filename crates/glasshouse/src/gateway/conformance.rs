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
//!    gateway token.
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
use std::time::Duration;

use ureq::Agent;

use super::fixture::FixtureUpstream;
use super::ingress::{Exchange, Outcome, serve};
use super::upstream::{Upstream, agent};
use super::{Gateway, GatewayToken};
use crate::secret::{REDACTED, Secret, redact};

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

/// An [`Upstream`] holding [`PROVIDER_CREDENTIAL`] and pointed at `base_url`.
fn upstream_at(base_url: &str) -> Upstream {
    Upstream::new(
        "fixture".to_owned(),
        base_url,
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

/// Send `raw` to `address` and read everything that comes back, to the close.
fn send_and_read(address: SocketAddr, raw: &[u8]) -> Vec<u8> {
    let mut client = TcpStream::connect(address).expect("the gateway accepts connections");
    client
        .set_read_timeout(Some(CLIENT_TIMEOUT))
        .expect("a non-zero read timeout is valid");
    client
        .write_all(raw)
        .expect("the gateway reads the request");
    client.flush().expect("the gateway reads the request");
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
