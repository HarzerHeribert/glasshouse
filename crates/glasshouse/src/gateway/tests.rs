use super::*;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Instant;

use super::fixture::FixtureUpstream;
use crate::integrations::IntegrationId;
use crate::secret::Secret;

/// A source file's production code: everything before the first
/// `#[cfg(test)]`, with `//` comments stripped — the idiom
/// `harness/mod.rs` introduced and that `main.rs`, `shim.rs`,
/// `secret/mod.rs` and `session/lifecycle.rs` each keep their own copy
/// of.
///
/// Dropping comment lines is not a convenience here, it is the point:
/// this module's doc comments *name* every path it must not import,
/// while explaining why it does not import them.
fn production_code(source: &str) -> String {
    source
        .split("#[cfg(test)]")
        .next()
        .expect("split always yields at least one part")
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every production source file in this directory, for the scans below.
///
/// Listed rather than walked: `include_str!` needs a literal, and a list
/// that has to be added to when a file is added is a list a reviewer can
/// see is complete.
///
/// `fixture.rs` and `conformance.rs` are absent because both are
/// `#[cfg(test)]` in their entirety: they are not production code, and
/// scanning them would be scanning the tests for the rules the tests
/// exist to check.
fn gateway_sources() -> Vec<(&'static str, &'static str)> {
    let mut sources = relay_sources();
    sources.extend(translate_sources());
    sources
}

/// The relay: the files that move bytes and may never read them.
fn relay_sources() -> Vec<(&'static str, &'static str)> {
    vec![
        ("gateway/mod.rs", include_str!("mod.rs")),
        ("gateway/http.rs", include_str!("http.rs")),
        ("gateway/ingress.rs", include_str!("ingress.rs")),
        ("gateway/session.rs", include_str!("session/mod.rs")),
        ("gateway/upstream.rs", include_str!("upstream.rs")),
    ]
}

/// The codecs: the one part of this directory that parses a body, by
/// the Phase 56 ruling — and only for a target the provider does not
/// serve. Held to the harness-import rule like every other file here,
/// and deliberately **not** to the no-deserialization rule, which is the
/// relay's.
fn translate_sources() -> Vec<(&'static str, &'static str)> {
    vec![
        ("gateway/translate/mod.rs", include_str!("translate/mod.rs")),
        (
            "gateway/translate/canonical.rs",
            include_str!("translate/canonical.rs"),
        ),
        (
            "gateway/translate/anthropic.rs",
            include_str!("translate/anthropic.rs"),
        ),
        (
            "gateway/translate/openai_chat.rs",
            include_str!("translate/openai_chat.rs"),
        ),
        (
            "gateway/translate/stream.rs",
            include_str!("translate/stream.rs"),
        ),
    ]
}

/// A profile with the given backend, for the start predicate.
fn profile_backed_by(backend: BackendResource) -> LaunchProfile {
    let mut profile = LaunchProfile::native(IntegrationId::ClaudeCode);
    profile.backend = backend;
    profile
}

/// The credential a fixture upstream expects to see attached. Planted,
/// so that `!contains` on it is a real assertion rather than a shape
/// check.
const PROVIDER_CREDENTIAL: &str = "sk-planted-provider-key-qqqqwwwweeeerrrr";

/// A gateway pointed at `fixture`, holding [`PROVIDER_CREDENTIAL`].
fn gateway_to(fixture: &FixtureUpstream) -> Gateway {
    Gateway::start(anthropic_upstream_to(&fixture.base_url())).expect("loopback is bindable")
}

/// An upstream serving Anthropic Messages at `base_url` and nothing
/// else — the shape every test in this module written before the
/// ingress served more than one protocol assumes.
fn anthropic_upstream_to(base_url: &str) -> Upstream {
    Upstream::new(
        "fixture".to_owned(),
        vec![Route::new(
            "anthropic-messages".to_owned(),
            &["/messages"],
            base_url,
        )],
        Secret::mint_for_test(PROVIDER_CREDENTIAL),
        crate::routing::CredentialId::new(
            "fixture",
            crate::secret::SecretRef::Environment {
                var: "FIXTURE_API_KEY".to_owned(),
            },
        ),
    )
    .expect("the fixture's base URL is absolute")
}

/// The bytes a Claude Code child sends: a bearer token, a JSON body, and
/// a length.
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

/// Send `raw` to `address` and hand back the still-open connection.
fn send(address: SocketAddr, raw: &[u8]) -> TcpStream {
    let mut client = TcpStream::connect(address).expect("the gateway accepts connections");
    // Generous on purpose, and it costs a correct implementation
    // nothing: every exchange here completes in microseconds. The
    // margin exists so that a loaded machine cannot turn a passing test
    // into a failing one, and it has to stay larger than the fixture's
    // own wait in `a_streamed_response_...` so that a *buffering*
    // implementation is still observed failing rather than timing out
    // here first.
    client
        .set_read_timeout(Some(Duration::from_secs(60)))
        .expect("a non-zero read timeout is valid");
    client
        .write_all(raw)
        .expect("the gateway reads the request");
    client.flush().expect("the gateway reads the request");
    client
}

/// Everything the gateway wrote back, to the close.
fn read_all(mut client: TcpStream) -> String {
    let mut out = Vec::new();
    client
        .read_to_end(&mut out)
        .expect("the gateway answers and then closes");
    String::from_utf8_lossy(&out).into_owned()
}

// --- the token is a credential, and is shaped like one ----------------

/// A length is a real leak: it narrows a key space. So the rendering is
/// identical for every token, and no prefix or suffix of one — however
/// short — survives into it. Lose this and the first `tracing` field
/// that takes a `Gateway` publishes the instance's authentication token
/// to a log file.
#[test]
fn debug_on_a_gateway_token_prints_a_fixed_marker_and_never_the_token() {
    // A stand-in value rather than a generated one, and built through the
    // private field the way `secret`'s twin of this test builds a
    // `Secret`. A real token is 64 hex characters, and `[redacted]`
    // itself contains `a`, `c`, `d` and `e` — so a prefix scan over a
    // *generated* token reports a one-character "leak" roughly a quarter
    // of the time. That is the scan colliding with the marker, not a
    // leak, and a test that fails at random is worth less than no test.
    const VALUE: &str = "ghp_qqqqwwwweeeerrrrttttyyyyuuuu9999";

    let rendered = format!("{:?}", GatewayToken(VALUE.to_owned()));
    assert_eq!(rendered, REDACTED, "the marker must be fixed");
    for n in 1..=VALUE.len() {
        assert!(
            !rendered.contains(&VALUE[..n]),
            "the first {n} characters of the token survived into {rendered:?}"
        );
        assert!(
            !rendered.contains(&VALUE[VALUE.len() - n..]),
            "the last {n} characters of the token survived into {rendered:?}"
        );
    }
    assert!(
        !rendered.contains(&VALUE.len().to_string()),
        "the token's length appeared in {rendered:?}"
    );
    assert_eq!(
        format!("{:?}", GatewayToken(String::new())),
        format!("{:?}", GatewayToken("x".repeat(4096))),
        "an empty token and a 4096-character one must be indistinguishable in Debug output"
    );

    // ... and the same holds for a token that really came from the
    // generator. `expose` is used to *check for* the value, never to
    // print it: the message renders only the marker.
    let minted = GatewayToken::generate().expect("the OS has entropy");
    let rendered = format!("{minted:?}");
    assert_eq!(rendered, REDACTED);
    assert!(
        !rendered.contains(minted.expose()),
        "a minted token survived into {rendered:?}"
    );
}

/// The token is reachable through the whole gateway, so the whole
/// gateway has to be safe to render — a `Debug` on the owner is exactly
/// how a redacted field gets printed anyway. Since this slice the
/// gateway also *holds a provider credential*, so the same rendering has
/// to withhold two different secrets at once.
#[test]
fn debug_on_a_gateway_never_reaches_its_token_or_its_credential() {
    let fixture = FixtureUpstream::answering("HTTP/1.1 200 OK", "", "{}");
    let gateway = gateway_to(&fixture);
    let rendered = format!("{gateway:?}");
    assert!(
        !rendered.contains(gateway.token().expose()),
        "the gateway's own Debug leaked its token"
    );
    assert!(
        !rendered.contains(PROVIDER_CREDENTIAL),
        "the gateway's own Debug leaked the provider credential it holds"
    );
    assert!(
        rendered.contains(REDACTED),
        "the gateway's Debug must show the token's redaction marker, not omit the field"
    );
}

/// The compile-fail guard this codebase can express: a source scan of
/// production code, the same idiom as
/// `secret::a_secret_has_no_display_no_deref_and_no_asref`, which this
/// deliberately mirrors — the packet's rule is that the gateway token is
/// treated *exactly* as a credential, and "exactly" is only checkable if
/// the same check exists.
#[test]
fn a_gateway_token_has_no_display_no_deref_and_no_asref() {
    let code = production_code(include_str!("mod.rs"));
    for forbidden in [
        "Display",
        "Deref",
        "AsRef",
        "Borrow",
        "ToString",
        "Serialize",
        "Deserialize",
        "serde",
    ] {
        assert!(
            !code.contains(forbidden),
            "gateway/mod.rs names `{forbidden}` in production code: the gateway token must \
             not be printable, dereferenceable, borrowable as a str or serializable, \
             because every one of those is a way for a credential to reach output by \
             accident. `expose` is the only door."
        );
    }
}

// --- the profiles decide, not a flag ----------------------------------

/// The predicate is the whole of "only when at least one active launch
/// profile requires it", so it has to read the backend rather than
/// anything that merely travels alongside it. A profile that reaches its
/// backend directly must never cause a socket to exist.
#[test]
fn only_a_gateway_backed_profile_requires_a_gateway() {
    assert!(!gateway_is_required(&[]));
    assert!(!gateway_is_required(&[profile_backed_by(
        BackendResource::Native
    )]));
    assert!(!gateway_is_required(&[profile_backed_by(
        BackendResource::DirectProvider {
            provider: "openrouter".to_owned(),
        }
    )]));

    assert!(gateway_is_required(&[profile_backed_by(
        BackendResource::GlasshouseGateway
    )]));
    // One among several is enough: "at least one" is the rule.
    assert!(gateway_is_required(&[
        profile_backed_by(BackendResource::Native),
        profile_backed_by(BackendResource::GlasshouseGateway),
    ]));
}

/// Asserted on the *absence* of a gateway rather than on a boolean: the
/// promise is that no listener is bound at all, and a predicate that
/// answered `false` while something still bound a socket would satisfy a
/// boolean assertion and break the promise.
///
/// It also asserts that the upstream was never built. Resolving a
/// credential for a launch that needs no gateway would read a secret
/// nothing was going to use, which is the kind of thing that is only
/// ever noticed after it has been logged somewhere.
#[test]
fn no_profile_needing_a_gateway_binds_no_listener_and_resolves_no_credential() {
    let profiles = [
        profile_backed_by(BackendResource::Native),
        profile_backed_by(BackendResource::DirectProvider {
            provider: "openrouter".to_owned(),
        }),
    ];
    let mut built = false;
    let started = start_if_required(&profiles, || {
        built = true;
        unreachable!("the upstream must not be built for profiles that need no gateway")
    })
    .expect("deciding not to start cannot fail");
    assert!(
        started.is_none(),
        "a gateway was bound for profiles that never asked for one"
    );
    assert!(!built);
}

/// The other half of the same rule, and the one that keeps it from being
/// satisfied by a function that simply never starts anything.
#[test]
fn a_profile_backed_by_the_gateway_binds_a_listener() {
    let fixture = FixtureUpstream::answering("HTTP/1.1 200 OK", "", "{}");
    let profiles = [profile_backed_by(BackendResource::GlasshouseGateway)];
    let started = start_if_required(&profiles, || Ok(anthropic_upstream_to(&fixture.base_url())))
        .expect("loopback is bindable");
    assert!(
        started.is_some(),
        "a gateway-backed profile did not produce a gateway"
    );
}

// --- the ingress: what the upstream sees ------------------------------

/// The heart of lines 2 and 3. The upstream must see the *provider's*
/// credential, attached by the gateway; the child's own token must not
/// reach it in any header at all.
///
/// Both halves are asserted, and the second is the one that matters: a
/// gateway that attached the provider key while *also* forwarding the
/// child's `authorization` would pass a test that only checked the
/// first, and would be handing an upstream a Glasshouse instance's
/// authentication token.
#[test]
fn a_request_carrying_the_gateway_token_reaches_the_upstream_with_the_provider_credential() {
    let fixture = FixtureUpstream::answering("HTTP/1.1 200 OK", "", "{\"ok\":true}");
    let gateway = gateway_to(&fixture);

    let response = read_all(send(
        gateway.address(),
        &messages_request(gateway.token().expose(), "{\"model\":\"probe\"}"),
    ));
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("{\"ok\":true}"), "{response}");

    let request = fixture.only_request();
    assert_eq!(
        request.header("authorization"),
        Some(format!("Bearer {PROVIDER_CREDENTIAL}").as_str()),
        "the gateway did not attach the provider's own credential"
    );
    let rendered = format!("{request:?}");
    assert!(
        !rendered.contains(gateway.token().expose()),
        "the child's gateway token reached the upstream"
    );

    // The request target was appended to the provider's base URL with
    // its query intact, and the method and end-to-end headers survived.
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/v1/messages?beta=true");
    assert_eq!(
        request.body, b"{\"model\":\"probe\"}",
        "the request body did not arrive byte-for-byte"
    );
    assert_eq!(request.header("anthropic-version"), Some("2023-06-01"));
    // ... and `host` names the upstream rather than the loopback address
    // the child was pointed at.
    assert_eq!(
        request.header("host"),
        Some(fixture.base_url().trim_start_matches("http://")),
        "the host header was not corrected to the upstream's"
    );
}

/// Pass-through means the provider sees the harness's own headers and
/// **nothing the gateway or its HTTP client decided to add**.
///
/// This is a real hazard rather than a hypothetical one: `ureq` adds a
/// `user-agent`, an `accept` and an `accept-encoding` of its own unless
/// told not to, and the `gzip` feature would additionally advertise an
/// encoding and then transparently decode the response — leaving a
/// `content-encoding` header describing something the client is no
/// longer being sent. `upstream::agent` turns all four off. Lose any of
/// them and the provider sees a client the harness is not, which is
/// exactly what "keep the first gateway implementation protocol
/// pass-through" forbids.
#[test]
fn the_gateway_adds_no_headers_of_its_own_to_a_forwarded_request() {
    let fixture = FixtureUpstream::answering("HTTP/1.1 200 OK", "", "{}");
    let gateway = gateway_to(&fixture);

    read_all(send(
        gateway.address(),
        &messages_request(gateway.token().expose(), "{}"),
    ));

    let request = fixture.only_request();
    let names: Vec<&str> = request
        .headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();

    for invented in ["user-agent", "accept", "accept-encoding"] {
        assert!(
            !names.contains(&invented),
            "the gateway's HTTP client added `{invented}` to a request the harness did not \
             send it on: {names:?}"
        );
    }
    // Exactly the harness's own end-to-end headers, plus the framing and
    // routing the next hop requires. Asserted as a set so that an added
    // header fails here rather than being noticed years later in a
    // provider's logs.
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        vec![
            "anthropic-version",
            "authorization",
            "content-length",
            "content-type",
            "host",
        ],
        "the forwarded header set changed"
    );
}

/// A request without this instance's token is refused **before an
/// upstream connection exists**, which is asserted on the fixture's own
/// connection count rather than on the order of two statements.
///
/// The connection count and not the request count: a gateway that
/// opened a socket and then thought better of it would leave no request
/// behind and would still have told the provider that someone was here.
#[test]
fn a_request_without_this_instances_token_is_refused_and_opens_nothing_upstream() {
    let fixture = FixtureUpstream::answering("HTTP/1.1 200 OK", "", "{}");
    let gateway = gateway_to(&fixture);
    let other = GatewayToken::generate().expect("the OS has entropy");

    for wrong in [
        format!("Bearer {}", other.expose()),
        format!("Bearer {}", &gateway.token().expose()[..32]),
        "Bearer".to_owned(),
        String::new(),
    ] {
        let raw = if wrong.is_empty() {
            // No `authorization` header at all.
            b"POST /v1/messages HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2\r\n\r\n{}".to_vec()
        } else {
            format!(
                "POST /v1/messages HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: {wrong}\r\n\
                 Content-Length: 2\r\n\r\n{{}}"
            )
            .into_bytes()
        };
        let response = read_all(send(gateway.address(), &raw));
        assert!(
            response.starts_with("HTTP/1.1 401 Unauthorized"),
            "a request presenting {wrong:?} was not refused: {response}"
        );
        assert!(
            response.contains("authentication_error"),
            "the refusal must be in the shape the harness's own protocol uses: {response}"
        );
    }

    assert_eq!(
        fixture.connections(),
        0,
        "a refused request opened a connection to the provider"
    );
}

/// A real harness connects first and writes afterwards, so the gateway
/// routinely accepts a connection *before* its request exists. That is
/// the case where an accepted socket which inherited its listener's
/// non-blocking flag — as it does on macOS, the BSDs and Windows, and
/// does not on Linux — answers the first read with `WouldBlock`, and the
/// connection is dropped without a reply.
///
/// Every other test here writes before the gateway can accept, so the
/// bytes are already in the receive buffer and a non-blocking read
/// succeeds anyway. **Removing `set_nonblocking(false)` from the ingress
/// broke nothing until this test existed** — which is exactly the shape
/// of a platform defect that ships.
///
/// The pause is a bound, not a synchronisation: it only has to exceed
/// one `ACCEPT_POLL`, and a pause that turned out to be too short would
/// make this test *weaker* rather than flaky, because both a correct and
/// a broken gateway pass when the write wins the race.
#[test]
fn a_client_that_connects_before_it_writes_is_still_served() {
    let fixture = FixtureUpstream::answering("HTTP/1.1 200 OK", "", "{\"ok\":true}");
    let gateway = gateway_to(&fixture);

    let mut client =
        TcpStream::connect(gateway.address()).expect("the gateway accepts connections");
    client
        .set_read_timeout(Some(Duration::from_secs(60)))
        .expect("a non-zero read timeout is valid");
    std::thread::sleep(ACCEPT_POLL * 20);

    let raw = messages_request(gateway.token().expose(), "{\"model\":\"probe\"}");
    client
        .write_all(&raw)
        .expect("the gateway is still reading");
    client.flush().expect("the gateway is still reading");

    let response = read_all(client);
    assert!(
        response.starts_with("HTTP/1.1 200 OK"),
        "the gateway dropped a connection it had accepted before the request arrived: \
         {response:?}"
    );
    assert_eq!(fixture.only_request().target, "/v1/messages?beta=true");
}

/// Line 4, and the test is built so a buffered implementation cannot
/// pass it rather than so that a streaming one happens to.
///
/// The fixture writes its first event, then **blocks until the client
/// says it has received that event**, and only then writes the second.
/// So the second event exists only if the first reached the client while
/// the response was still open. A gateway that read the upstream body to
/// the end before writing anything would deadlock: the client would
/// never acknowledge, the fixture's wait would time out, and the marker
/// it writes instead is asserted on below.
#[test]
fn a_streamed_response_reaches_the_client_before_the_upstream_has_finished() {
    let (saw_first, first_seen) = mpsc::channel::<()>();
    let first_seen = Mutex::new(first_seen);

    let fixture = FixtureUpstream::start(move |_request, out| {
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
            .recv_timeout(Duration::from_secs(20))
            .is_ok();
        let second = if streamed {
            "event: two\ndata: {\"n\":2}\n\n"
        } else {
            "event: BUFFERED-NOT-STREAMED\n\n"
        };
        let _ = out.write_all(format!("{:x}\r\n{second}\r\n0\r\n\r\n", second.len()).as_bytes());
        let _ = out.flush();
    });

    let gateway = gateway_to(&fixture);
    let mut client = send(
        gateway.address(),
        &messages_request(gateway.token().expose(), "{\"stream\":true}"),
    );

    let mut seen = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        let read = client.read(&mut buffer).unwrap_or_else(|err| {
            panic!(
                "the gateway did not deliver the first event before the upstream finished \
                 ({err}); {} bytes had arrived: {:?}",
                seen.len(),
                String::from_utf8_lossy(&seen)
            )
        });
        assert!(
            read > 0,
            "the gateway closed the response before the first event arrived; {} bytes had \
             arrived: {:?}",
            seen.len(),
            String::from_utf8_lossy(&seen)
        );
        seen.extend_from_slice(&buffer[..read]);
        if String::from_utf8_lossy(&seen).contains("event: one") {
            break;
        }
    }
    saw_first.send(()).expect("the fixture is still writing");

    let mut rest = Vec::new();
    client.read_to_end(&mut rest).expect("the stream completes");
    seen.extend_from_slice(&rest);
    let text = String::from_utf8_lossy(&seen);

    assert!(text.contains("event: one"), "{text}");
    assert!(text.contains("event: two"), "{text}");
    assert!(
        !text.contains("BUFFERED-NOT-STREAMED"),
        "the upstream's wait for the first event to reach the client timed out, so the \
         gateway is buffering the response rather than streaming it: {text}"
    );
}

// --- the listener's address, and its lifetime -------------------------

/// Two facts, each of which fails differently. An interface other than v4
/// loopback would put a Glasshouse instance's gateway on the network,
/// which is the outcome this module has no configuration to cause and
/// therefore no way to notice. A port still equal to the one that was
/// *asked for* would mean `local_addr` was never consulted, and the
/// address handed to a child harness would name a port nothing is
/// listening on.
///
/// `is_loopback()` is deliberately not what is asserted: it also accepts
/// `127.0.0.2` and `::1`, and neither of those is an address this module
/// is allowed to bind.
#[test]
fn the_gateway_binds_v4_loopback_on_a_port_the_operating_system_chose() {
    let fixture = FixtureUpstream::answering("HTTP/1.1 200 OK", "", "{}");
    let gateway = gateway_to(&fixture);
    let address = gateway.address();

    assert_eq!(
        address.ip(),
        Ipv4Addr::LOCALHOST,
        "the gateway bound an interface other than v4 loopback"
    );
    assert_ne!(
        address.port(),
        EPHEMERAL_PORT,
        "the address still carries the port that was requested, so the port the operating \
         system actually chose was never read back"
    );
    assert_eq!(gateway.base_url(), format!("http://{address}"));
}

/// "Multiple Glasshouse instances can coexist" is a claim about two
/// listeners being alive *at the same time*, so both are held across the
/// comparison. Drop the first before asking and the operating system is
/// entitled to reissue its port to the second: the assertion would still
/// pass and would have proved nothing.
#[test]
fn two_gateways_in_one_process_bind_different_ports() {
    let fixture = FixtureUpstream::answering("HTTP/1.1 200 OK", "", "{}");
    let first = gateway_to(&fixture);
    let second = gateway_to(&fixture);

    assert_ne!(
        first.address().port(),
        second.address().port(),
        "two gateways bound at the same time claimed the same port"
    );
}

/// A token that repeated across instances would let one Glasshouse
/// authenticate against another's gateway, and would mean the value is
/// not coming from the operating system's generator at all.
///
/// Compared with a bare `assert!` rather than `assert_ne!`, and through
/// the private field that `mod tests` can see: `assert_ne!` renders both
/// operands when it fails, so the single run that ever failed would be
/// the run that published two live credentials into CI output — undoing
/// the hand-written [`Debug`](fmt::Debug) above. The message below names
/// no value and no part of one.
#[test]
fn two_gateways_mint_different_tokens() {
    let fixture = FixtureUpstream::answering("HTTP/1.1 200 OK", "", "{}");
    let first = gateway_to(&fixture);
    let second = gateway_to(&fixture);

    assert!(
        first.token().0 != second.token().0,
        "two gateways minted the same token"
    );
}

/// Nothing here calls a `close` or a `stop`: the port is released only
/// because dropping the [`Gateway`] stops its accept loop and joins it,
/// which drops the listener the loop owns. Lose that and a process which
/// started and finished with several gateways would hold every port it
/// had ever bound until it exited.
///
/// **Now with a live accept loop**, which is what makes this the
/// shutdown test rather than a statement about `Drop` on a struct: the
/// gateway has served a real exchange before it is dropped, so the loop
/// is running and blocked on nothing but its own poll.
///
/// Asserted as "the same address binds again", which is a direct
/// statement that the descriptor is gone. The alternative — "connecting
/// now fails" — depends on when the kernel gets around to refusing, and
/// that is a wait this test would have to encode as a timeout.
#[test]
fn dropping_the_gateway_releases_its_port() {
    let fixture = FixtureUpstream::answering("HTTP/1.1 200 OK", "", "{}");
    let gateway = gateway_to(&fixture);
    let address = gateway.address();

    let response = read_all(send(
        gateway.address(),
        &messages_request(gateway.token().expose(), "{}"),
    ));
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");

    let started = Instant::now();
    drop(gateway);
    let elapsed = started.elapsed();

    // Generous by two orders of magnitude over `ACCEPT_POLL`, because
    // this is a bound on "does not hang" and not a benchmark. A blocking
    // accept with no stop flag would sit here until the next connection,
    // which in a test is forever.
    assert!(
        elapsed < Duration::from_secs(2),
        "dropping a gateway with a running accept loop took {elapsed:?}"
    );

    // Bounded retry, and it does not weaken the assertion. The gateway
    // binds an *ephemeral* port, so between the drop above and this bind
    // the kernel is free to hand that same port to any other test thread
    // calling `bind(0)` — and this suite has many. That transient loss
    // races as `AddrInUse` and is not this gateway holding anything: two
    // workers hit it independently on 2026-08-26, once captured by name.
    //
    // If the gateway really had failed to release the descriptor, no
    // number of retries would ever succeed, so the loop still fails for
    // the reason the test exists. It only tolerates an unrelated binder
    // holding the port briefly.
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut rebound = TcpListener::bind(address);
    while rebound.is_err() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        rebound = TcpListener::bind(address);
    }
    assert!(
        rebound.is_ok(),
        "the gateway's port was still held after the gateway was dropped: {:?}",
        rebound.as_ref().err()
    );
}

// --- the rule the module is built to be unable to break ---------------

/// "The gateway is never a coding harness and never owns an interactive
/// session" is a promise until something makes it impossible to break by
/// accident, and this is that something. A module that cannot see the
/// session model cannot own a session, cannot drive a terminal and cannot
/// reach a harness adapter — so the rule survives a contributor who never
/// read the header, which is the only kind of rule worth having here.
///
/// Every file in this directory, not just this one: the ingress is where
/// a "just look up which session this belongs to" would be written.
#[test]
fn the_gateway_imports_none_of_the_modules_that_would_make_it_a_harness() {
    for (name, source) in gateway_sources() {
        let code = production_code(source);
        for forbidden in [
            "crate::session",
            "crate::shell",
            "crate::tui",
            "crate::harness",
        ] {
            assert!(
                !code.contains(forbidden),
                "{name} names `{forbidden}` in production code: the gateway has become \
                 able to see the session model it must never own, and \"the harness stays \
                 the harness\" is back to being a promise rather than something this \
                 module is structurally unable to break"
            );
        }
    }
}

/// The scan above is only worth having if it can fail — and here, more
/// than anywhere else in this crate, if it does not fire on the prose
/// that explains it: this file's own header names all four forbidden
/// paths in the course of saying it imports none of them. A scan that
/// could not tell those apart would have to be deleted the first time
/// someone wrote the rule down.
#[test]
fn the_gateway_dependency_scan_would_catch_a_violation() {
    let violating = "use crate::session::SessionLifecycle;\nfn start() {}";
    assert!(production_code(violating).contains("crate::session"));
    // ... and does not fire on a doc comment that merely mentions the
    // module, the way this file's own header legitimately does for all
    // four paths.
    let documented = "//! Imports none of `crate::session`.\nfn start() {}";
    assert!(!production_code(documented).contains("crate::session"));
    // ... nor on a mention inside a test.
    let tested = "fn start() {}\n#[cfg(test)]\nmod tests { use crate::session::SessionLifecycle; }";
    assert!(!production_code(tested).contains("crate::session"));
    // ... and the file list it runs over is not empty, which would make
    // every assertion in it vacuous.
    assert_eq!(gateway_sources().len(), 10);
}

/// No file of the **relay** may deserialize anything. The whole of
/// "preserve tool-call payloads without lossy rewriting" and "keep the
/// first gateway implementation protocol pass-through" rests on nothing
/// here ever looking at a body, so a serialization crate reaching these
/// files is the change that would quietly undo both — and it is the
/// change that would look most reasonable in a diff ("just read
/// `error.type` for the log").
///
/// Phase 56 narrowed this rule and did not repeal it: `translate/` is
/// the one place a body is parsed, entered only from the branch that
/// answered `404`, and it is held apart here on purpose. The second half
/// of this test is what keeps that split honest — the codecs *do*
/// deserialize, so a relay file that started to would be caught by the
/// first half and not excused by the second.
///
/// A scan cannot prove the absence of a hand-rolled parser, and this one
/// does not claim to. What it does catch is the realistic version: the
/// `use serde_json` that a body inspection would be written on top of.
#[test]
fn no_part_of_the_relay_deserializes_anything() {
    const FORBIDDEN: [&str; 5] = [
        "serde_json",
        "serde::",
        "Deserialize",
        "from_str::<",
        "toml::",
    ];
    for (name, source) in relay_sources() {
        let code = production_code(source);
        for forbidden in FORBIDDEN {
            assert!(
                !code.contains(forbidden),
                "{name} names `{forbidden}` in production code: the relay has started \
                 looking at a body it is supposed to be unable to distinguish from any \
                 other bytes"
            );
        }
    }
    // The exception is real and confined: the codecs deserialize, and
    // nothing outside `translate/` does.
    let codecs_parse = translate_sources()
        .iter()
        .any(|(_, source)| production_code(source).contains("serde_json"));
    assert!(
        codecs_parse,
        "translate/ no longer deserializes anything, so the split above proves nothing"
    );
    assert_eq!(relay_sources().len(), 5);

    // ... and the scan fires on the change it exists to catch, rather
    // than passing because the needle was misspelled.
    let violating = production_code("use serde_json::Value;\nfn peek() {}");
    assert!(FORBIDDEN.iter().any(|needle| violating.contains(needle)));
}
