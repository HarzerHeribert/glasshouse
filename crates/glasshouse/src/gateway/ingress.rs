//! One connection, from the harness's request line to the last byte of the
//! provider's response.
//!
//! # The shape of the whole thing
//!
//! Read the request head. Check the bearer token against this instance's
//! own. Append the request target to the provider's base URL, attach the
//! *provider's* credential in place of whatever the child sent, and forward
//! every other header and every body byte unchanged. Then write the
//! provider's status and headers back and move its body across a piece at a
//! time.
//!
//! That is the entire ingress, and its shortness is the design. Three things
//! are rewritten and named as such in [`forward`]; nothing else is even
//! looked at. A tool-call payload survives because no code here can tell a
//! tool-call payload from any other bytes.
//!
//! # Line 9: the ingress is not a public API
//!
//! "Require every interactive gateway ingress to be consumed through a
//! compatible installed harness launch profile" is satisfied by the token
//! rather than by a registry. The token is minted per Glasshouse instance,
//! held only in memory, and reaches exactly one place: the environment of a
//! child harness started by [`crate::profile::resolve`] for a
//! gateway-backed profile. **Possession of it therefore is the proof** that
//! a request came from such a launch — there is no other way for a process
//! to have it.
//!
//! A second mechanism — a session registry, an allow-list, a handshake —
//! would add state without adding a fact, because it would have to be keyed
//! on something the token already establishes.
//!
//! # Line 10: what may be recorded, and what may not
//!
//! [`Exchange`] is the only thing that reaches `tracing`, and it is
//! structurally incapable of carrying a body: it holds an outcome, two
//! statuses, a byte count and two names. Glasshouse's logging is already off
//! unless `GLASSHOUSE_LOG` is set — see [`crate::logging`] — so "opt-in" is
//! the existing mechanism rather than a new flag.
//!
//! **The packet asked for the provider error's `error.type` and
//! `error.message` to reach the diagnostic. They deliberately do not.**
//! Extracting either means parsing the response body, which this module is
//! forbidden to do and which is a stop condition for the whole slice. The
//! status is recorded; the body is forwarded to the harness, which is the
//! thing that actually needed to read it.

use std::io::{BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

use ureq::http::{HeaderValue, Request, StatusCode, header};
use ureq::{Agent, SendBody};

use super::GatewayToken;
use super::http::{self, HeadError};
use super::upstream::{Route, Upstream};

/// The `authorization` scheme the gateway accepts from a child harness.
///
/// Claude Code 2.1.245 launched with `ANTHROPIC_AUTH_TOKEN=<value>` was
/// observed sending exactly `authorization: Bearer <value>` — see
/// `crate::harness::claude_code`'s `CREDENTIAL_ENV`, where that observation
/// is recorded.
const BEARER_PREFIX: &str = "Bearer ";

/// How long the gateway waits for a request head before hanging up.
///
/// A connection that has been opened but has sent nothing holds a thread.
/// Loopback clients are not slow, so this is generous by two orders of
/// magnitude and still bounds the damage a stuck client can do.
const HEAD_TIMEOUT: Duration = Duration::from_secs(30);

/// How much of a refused request's body is drained before the socket closes.
///
/// Closing a socket while the client is still writing its body gets the
/// client a connection reset instead of the `401` it was sent, so the body
/// is drained first. Capped, because draining is work done on behalf of a
/// request that has already been refused.
const DRAIN_CAP: u64 = 1024 * 1024;

/// How long a refused request is given to finish sending before the socket
/// closes underneath it.
///
/// Short, and much shorter than [`HEAD_TIMEOUT`]: this is politeness owed to
/// a request that is not going to be served, and a thread should not be held
/// for it.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(2);

/// What happened on one connection. **The only value that reaches a log.**
///
/// Every field is an outcome, a status, a count or a name. There is nowhere
/// here to put a request body, a response body, a header value, a token or a
/// credential — which is a stronger statement than a promise not to log one,
/// and `an_exchange_has_nowhere_to_put_a_body` checks it against this
/// declaration.
#[derive(Debug)]
pub(super) struct Exchange {
    pub(super) outcome: Outcome,
    /// What the harness was told.
    pub(super) status: u16,
    /// The configured provider this gateway forwards to. A name.
    pub(super) provider: String,
    /// The slug of the protocol the request target was placed in, or `None`
    /// when it was refused before it could be placed. A name.
    pub(super) protocol: Option<String>,
    /// The upstream host **of the route that carried it**. A host: never a
    /// path, never a query. Empty when no route was chosen, because there is
    /// then no host this request was ever going to reach — and naming one
    /// anyway would be the log asserting something that did not happen.
    pub(super) host: String,
}

/// How a connection ended.
#[derive(Debug)]
pub(super) enum Outcome {
    /// Forwarded, and the provider answered. `upstream_status` is what the
    /// provider said and `bytes` is how much body was moved — a size, never
    /// a content.
    Forwarded { upstream_status: u16, bytes: u64 },
    /// The bearer token was absent or wrong. **Nothing was opened
    /// upstream**; the refusal happens before a connection exists.
    Unauthenticated,
    /// A request the gateway will not carry: malformed, oversized, or framed
    /// with `transfer-encoding` — see [`super::http::read_head`].
    Declined,
    /// The request target belongs to none of the protocols this gateway's
    /// upstream serves. **Nothing was opened upstream**: a target the
    /// gateway cannot place is one it must not append to whichever base URL
    /// was declared first.
    Unrouted,
    /// The provider could not be reached at all, so there is no status to
    /// forward. `detail` is one of [`transport_detail`]'s fixed phrases —
    /// `&'static str`, so it is a string written in this file and can never
    /// be text something else produced.
    Unreachable { detail: &'static str },
    /// The client hung up part-way through.
    ClientGone,
    /// A connection that sent no request at all — a port scan, or a health
    /// check. Not worth a response.
    Idle,
}

impl Exchange {
    /// Record this exchange at debug level.
    ///
    /// Every field is named explicitly rather than rendered as one blob, so
    /// the event is structured and a reader can see, field by field, that
    /// there is nothing here but an outcome, two statuses, a size and two
    /// names. Widening what is logged means widening a type that has nowhere
    /// to put a body.
    ///
    /// Debug level, and Glasshouse's logging is off unless `GLASSHOUSE_LOG`
    /// is set — see [`crate::logging`]. That is what "gateway logs are
    /// opt-in" means here: the existing mechanism, not a second switch.
    pub(super) fn record(&self) {
        let (outcome, upstream_status, bytes, detail) = match &self.outcome {
            Outcome::Forwarded {
                upstream_status,
                bytes,
            } => ("forwarded", Some(*upstream_status), Some(*bytes), None),
            Outcome::Unauthenticated => ("unauthenticated", None, None, None),
            Outcome::Declined => ("declined", None, None, None),
            Outcome::Unrouted => ("unrouted", None, None, None),
            Outcome::Unreachable { detail } => ("unreachable", None, None, Some(*detail)),
            Outcome::ClientGone => ("client-gone", None, None, None),
            Outcome::Idle => ("idle", None, None, None),
        };
        tracing::debug!(
            outcome,
            status = self.status,
            upstream_status = ?upstream_status,
            bytes = ?bytes,
            detail = ?detail,
            provider = %self.provider,
            protocol = ?self.protocol,
            host = %self.host,
            "glasshouse gateway exchange"
        );
    }
}

/// Serve exactly one request on `stream`, and close.
///
/// # Why one request per connection
///
/// The inbound hop is loopback, where a new connection costs a syscall pair
/// and no handshake. The outbound hop is where reconnecting is expensive —
/// a TLS handshake to the provider — and that one *is* pooled, by `ureq`'s
/// own connection pool inside the shared [`Agent`]. So the cheap hop is kept
/// simple and the expensive hop is kept warm, which is the opposite of what
/// implementing keep-alive here would have optimised.
pub(super) fn serve(
    stream: TcpStream,
    token: &GatewayToken,
    upstream: &Upstream,
    agent: &Agent,
) -> Exchange {
    // **Not optional, and not tidiness.** The listener is non-blocking so
    // that shutdown cannot hang on `accept` — and on macOS, the BSDs and
    // Windows an accepted socket inherits that flag from its listener, while
    // on Linux it does not. A non-blocking stream would turn every read here
    // into `WouldBlock` on two of the three platforms Glasshouse supports.
    if stream.set_nonblocking(false).is_err() {
        return exchange(Outcome::ClientGone, 0, upstream, None);
    }
    let _ = stream.set_read_timeout(Some(HEAD_TIMEOUT));
    // Nagle's algorithm coalesces small writes and waits for an
    // acknowledgement before sending the next one; the receiver's delayed
    // acknowledgement then waits too. On a stream of small server-sent
    // events that pair adds a stall to every event, which is a latency
    // defect in exactly the property this gateway exists to preserve.
    // `ureq` already turns it off on the connection it makes to the
    // provider; this is the same decision on the connection the harness
    // makes to us.
    let _ = stream.set_nodelay(true);

    let Ok(mut out) = stream.try_clone() else {
        return exchange(Outcome::ClientGone, 0, upstream, None);
    };
    let mut reader = BufReader::new(stream);

    let head = match http::read_head(&mut reader) {
        Ok(head) => head,
        Err(HeadError::Empty) => return exchange(Outcome::Idle, 0, upstream, None),
        Err(HeadError::Io) => return exchange(Outcome::ClientGone, 0, upstream, None),
        Err(error) => {
            let (status, kind, message) = decline(&error);
            refuse(&mut out, status, kind, message, None);
            // The client may still be writing the body of a request whose
            // head was already refused — a chunked one, for instance. Closing
            // now would reset the connection and the client would see a
            // network error instead of the status explaining what was wrong.
            settle(&mut reader, &mut out, None);
            return exchange(Outcome::Declined, status.as_u16(), upstream, None);
        }
    };

    if !presented_token_matches(&head, token) {
        // Before any upstream connection is opened — which is asserted on the
        // upstream's own connection count, not on this ordering.
        refuse(
            &mut out,
            StatusCode::UNAUTHORIZED,
            "authentication_error",
            "this request did not carry the Glasshouse gateway's own token; a gateway ingress \
             is reachable only from a harness Glasshouse started under a gateway-backed launch \
             profile",
            Some(&head.method),
        );
        settle(&mut reader, &mut out, head.content_length);
        return exchange(Outcome::Unauthenticated, 401, upstream, None);
    }

    forward(head, reader, &mut out, upstream, agent)
}

/// The three rewrites, and everything that is not one.
///
/// Rewritten, and named here so that the list can be counted:
///
/// 1. **the request target** is appended to the base URL the provider
///    declared *for the protocol that target belongs to* —
///    [`Upstream::route_for`], then [`Route::uri_for`];
/// 2. **`authorization`** is replaced by the provider's credential, attached
///    by the gateway — [`Upstream::authorization`];
/// 3. **`host`** is dropped so that the outbound layer derives the upstream's
///    own, which is the correction the next hop requires.
///
/// Not rewritten: the method, every other header, and every byte of the
/// body.
///
/// Not *forwarded*, which is a different thing from rewritten: the hop-by-hop
/// headers of [`super::http::HOP_BY_HOP`]. Those describe the connection they
/// arrived on, and this is a different connection. `content-length` is among
/// them and is re-stated below from the value the client declared, so the
/// body is framed outbound exactly as it was framed inbound.
///
/// # Which protocol, and how that is decided
///
/// **By the request target, and by nothing else.** The gateway may serve
/// Anthropic Messages, OpenAI Responses and OpenAI Chat at once, each with
/// the base URL the one configured provider declared for it, and the target
/// the harness wrote is what says which of them this request is. The
/// alternative — looking at the body to see which protocol it reads like —
/// is forbidden here twice over: it would make this module a parser of the
/// payload it exists to be unable to distinguish from any other bytes, and a
/// request whose shape was ambiguous would be *guessed* rather than placed.
///
/// A target belonging to no served protocol is answered with a `404` and
/// **nothing is opened upstream**. That is a narrowing of what this gateway
/// used to do — a single-protocol gateway appended every target to its one
/// base URL — and it is the point rather than a side effect: with several
/// base URLs available, "append it to the first one" sends a request
/// somewhere nobody asked for it to go.
///
/// # What the narrowing costs, measured rather than assumed
///
/// Real harnesses do send targets outside their own protocol, and both were
/// observed against a listener that recorded the request line:
///
/// - Claude Code 2.1.245 sends `HEAD /api/hello` before its first
///   `/v1/messages`, and carries on unaffected after a non-2xx answer to it.
/// - Codex 0.149.1 sends `GET /models?client_version=0.149.1` when it does
///   not already hold metadata for the configured model. Under this rule
///   those are refused, and Codex logs
///   `failed to refresh available models: unexpected status 404 Not
///   Found: <this refusal's message>` — twice per session, at `ERROR`
///   level and visible to the user — then completes the session normally.
///   A full live run through this gateway to OpenRouter returned its
///   answer with exactly those two refusals recorded.
///
/// So the cost is real, it is user-visible, and it is a degradation rather
/// than a breakage. It is **not** silently accepted, and the reason it is
/// not simply routed is worth stating: `/models` is a catalogue endpoint
/// that all three protocols define, and the two spellings a harness may use
/// need *different* base URLs. Codex asks for `/models`, which only resolves
/// against a base URL carrying `/v1`; Anthropic Messages is declared at a
/// root without one, so the same request routed to that protocol would reach
/// a path the provider answers `404` for anyway. Placing it therefore means
/// choosing between OpenAI Responses and OpenAI Chat for a request that
/// names neither — a tie-break invented without a concrete provider pair
/// requiring it, which is the move the capability map's pass-through lines
/// forbid.
///
/// The change, if a later phase decides the tie-break: add `/models` to
/// `crate::profile::ingress_targets`' OpenAI Responses entry.
fn forward(
    head: http::RequestHead,
    mut reader: BufReader<TcpStream>,
    out: &mut TcpStream,
    upstream: &Upstream,
    agent: &Agent,
) -> Exchange {
    // The serving backend is read **once**, here, and used for the whole of
    // this exchange. Phase 9H's failover moves which backend serves from
    // another thread; reading it twice would let one request take its route
    // from one provider and its credential from another.
    let serving = upstream.serving();
    let Some(route) = serving.route_for(&head.target) else {
        refuse(
            out,
            StatusCode::NOT_FOUND,
            "not_found_error",
            "this request target does not belong to any protocol the Glasshouse gateway is \
             serving; a gateway ingress carries only the protocols the configured provider \
             declares a base URL for, and forwards nothing it cannot place",
            Some(&head.method),
        );
        // Drained before the socket closes for the same reason the `401`
        // path drains: a client still writing a body would get a connection
        // reset instead of the status that explains what was wrong.
        settle(&mut reader, out, head.content_length);
        return exchange(Outcome::Unrouted, 404, upstream, None);
    };

    let Some(uri) = route.uri_for(&head.target) else {
        refuse(
            out,
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "the request target could not be appended to the configured provider's base URL",
            Some(&head.method),
        );
        return exchange(Outcome::Declined, 400, upstream, Some(route));
    };

    let mut request = Request::builder().method(head.method.clone()).uri(uri);
    for (name, value) in head.headers.iter() {
        if http::is_hop_by_hop(name) || name == header::HOST || name == header::AUTHORIZATION {
            continue;
        }
        request = request.header(name.clone(), value.clone());
    }
    request = request.header(header::AUTHORIZATION, serving.authorization());

    let body = match head.content_length {
        Some(length) => {
            request = request.header(header::CONTENT_LENGTH, HeaderValue::from(length));
            // The body is moved from the client socket to the provider
            // socket without ever being held whole: `take` bounds it at the
            // length the client declared and nothing copies it into a
            // buffer of its own.
            SendBody::from_owned_reader(reader.take(length))
        }
        None => SendBody::none(),
    };

    let Ok(request) = request.body(body) else {
        refuse(
            out,
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "the request could not be rebuilt for the configured provider",
            Some(&head.method),
        );
        return exchange(Outcome::Declined, 400, upstream, Some(route));
    };

    let response = match agent.run(request) {
        Ok(response) => response,
        Err(err) => {
            let detail = transport_detail(&err);
            refuse(
                out,
                StatusCode::BAD_GATEWAY,
                "api_error",
                "the Glasshouse gateway could not reach the configured provider",
                Some(&head.method),
            );
            return exchange(Outcome::Unreachable { detail }, 502, upstream, Some(route));
        }
    };

    let (parts, mut body) = response.into_parts();
    let status = parts.status;
    let declared_length = body.content_length();
    // A `HEAD` response carries no body however ordinary its status is, and
    // writing one would be read by the client as the start of the *next*
    // response. No harness in scope sends `HEAD`; the method is forwarded
    // rather than vetted, so this is here because the method can arrive and
    // not because something sends it.
    let carries_body = status_carries_a_body(status) && head.method != ureq::http::Method::HEAD;

    let mut headers: Vec<(String, Vec<u8>)> = parts
        .headers
        .iter()
        .filter(|(name, _)| !http::is_hop_by_hop(name))
        .map(|(name, value)| (name.as_str().to_owned(), value.as_bytes().to_vec()))
        .collect();

    // Framing belongs to this hop, so it is decided here rather than copied.
    // A length the provider declared is re-stated; anything else — a chunked
    // response, an HTTP/2 one, a close-delimited one — is re-framed as
    // chunked, which is the only framing that can carry a stream whose
    // length nobody knows yet.
    let chunked = carries_body && declared_length.is_none();
    if carries_body {
        match declared_length {
            Some(length) => {
                headers.push(("content-length".to_owned(), length.to_string().into_bytes()))
            }
            None => headers.push(("transfer-encoding".to_owned(), b"chunked".to_vec())),
        }
    }
    // One request per connection — see `serve`. Said out loud so the client
    // does not wait for a second response on a socket that is about to
    // close.
    headers.push(("connection".to_owned(), b"close".to_vec()));

    if http::write_head(out, status, &headers).is_err() {
        return exchange(Outcome::ClientGone, status.as_u16(), upstream, Some(route));
    }

    let mut moved = 0;
    if carries_body {
        match http::pump(body.as_reader(), out, chunked) {
            Ok(bytes) => moved = bytes,
            Err(_) => return exchange(Outcome::ClientGone, status.as_u16(), upstream, Some(route)),
        }
    } else {
        let _ = out.flush();
    }
    let _ = out.shutdown(Shutdown::Both);

    exchange(
        Outcome::Forwarded {
            upstream_status: status.as_u16(),
            bytes: moved,
        },
        status.as_u16(),
        upstream,
        Some(route),
    )
}

/// Whether the presented bearer token is this instance's own.
///
/// # Constant time, as far as safe Rust can promise it
///
/// The comparison below folds every byte before answering, so it does not
/// return early on the first mismatch and its running time does not depend
/// on how many leading characters an attacker guessed. That is the property
/// that matters: a token is 256 bits of entropy and the only realistic
/// attack on it is one that learns a prefix.
///
/// It is **not** a hardware guarantee. Nothing in safe Rust stops an
/// optimiser from introducing a branch, and the honest fix is a crate whose
/// job that is — `subtle`, which is already in this workspace's lock file as
/// a transitive dependency of `rustls`. Promoting it to a direct dependency
/// was outside this slice's remit, so this is what is here and this comment
/// is the disclosure rather than a claim that it is equivalent.
fn presented_token_matches(head: &http::RequestHead, token: &GatewayToken) -> bool {
    let Some(value) = head.headers.get(header::AUTHORIZATION) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    let Some(presented) = value.strip_prefix(BEARER_PREFIX) else {
        return false;
    };
    constant_time_eq(presented.as_bytes(), token.expose().as_bytes())
}

/// `a == b`, without returning early.
///
/// A length mismatch is folded in rather than short-circuited, and the loop
/// still runs over the presented value, so an attacker learns no more from
/// the timing than "wrong".
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if b.is_empty() {
        // An empty expected token would make the index below divide by zero,
        // and would accept everything. It cannot happen — a `GatewayToken`
        // is 64 hex characters — but "cannot happen" is not a guard.
        return false;
    }
    let mut difference = u8::from(a.len() != b.len());
    for (index, byte) in a.iter().enumerate() {
        difference |= byte ^ b[index % b.len()];
    }
    difference == 0
}

/// Why the provider could not be reached, as one of a **fixed vocabulary**.
///
/// # Not the error's own text, and that is the point
///
/// The obvious implementation is `crate::secret::redact(&err.to_string())`,
/// and it was the first one here. It is not enough.
/// [`crate::secret::redact`] removes things that *look like credentials* —
/// an `sk-` key, a `Bearer` token — and it makes no claim at all about the
/// rest of a string. `ureq` wrote that string, `ureq` never had this
/// project's rules, and a diagnostic that keeps foreign text keeps whatever
/// the next version of that crate decides to put in it. The test
/// `a_recorded_exchange_writes_a_line_with_no_secret_in_it` caught exactly
/// that: the credential was redacted and everything around it went to the
/// log verbatim.
///
/// So nothing foreign is kept. Each variant maps to a phrase written here,
/// which means a leak is not something to be careful about — it is something
/// this function has no way to express. The categories are still the ones a
/// user needs to tell apart: a refused connection, a name that does not
/// resolve, a TLS failure and a timeout have completely different fixes.
///
/// The variant *is* read from the error, so the answer is a real
/// observation and not a constant.
fn transport_detail(err: &ureq::Error) -> &'static str {
    match err {
        ureq::Error::HostNotFound => "the provider's host name did not resolve",
        ureq::Error::ConnectionFailed => "the connection to the provider could not be made",
        ureq::Error::Io(_) => "the connection to the provider failed",
        ureq::Error::Timeout(_) => "the provider did not answer in time",
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) | ureq::Error::Pem(_) => {
            "the TLS connection to the provider could not be established"
        }
        ureq::Error::BadUri(_) | ureq::Error::Http(_) => {
            "the request could not be addressed to the provider"
        }
        ureq::Error::Protocol(_) => "the provider's answer was not valid HTTP",
        _ => "the provider could not be reached",
    }
}

/// The status, error kind and message for a head that would not parse.
fn decline(error: &HeadError) -> (StatusCode, &'static str, &'static str) {
    match error {
        HeadError::TooLarge => (
            StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE,
            "invalid_request_error",
            "the request head exceeded the size the Glasshouse gateway will read",
        ),
        HeadError::ChunkedRequest => (
            StatusCode::LENGTH_REQUIRED,
            "invalid_request_error",
            "the Glasshouse gateway forwards request bodies framed with content-length; a \
             chunked request body would have to be parsed to be re-framed",
        ),
        _ => (
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "the Glasshouse gateway could not read this request",
        ),
    }
}

/// Write an error the harness can understand, in the shape its own protocol
/// uses.
///
/// The message is written here and never quotes the request: a malformed
/// head is still a head someone wrote, and echoing it back is how prompt
/// text ends up in a terminal scrollback. Nothing in this body is derived
/// from anything the client sent.
fn refuse(
    out: &mut TcpStream,
    status: StatusCode,
    kind: &str,
    message: &str,
    method: Option<&ureq::http::Method>,
) {
    let body = format!(
        "{{\"type\":\"error\",\"error\":{{\"type\":\"{kind}\",\"message\":\"{message}\"}}}}"
    );
    // A response to `HEAD` carries the headers a `GET` would have and none
    // of the body, and a client that reads the body anyway reads it as the
    // start of the next response. `forward` already applies this rule to a
    // provider's answer; a refusal written here needs it just as much, and
    // needs it more now — Claude Code 2.1.245's `HEAD /api/hello` belongs to
    // no protocol, so the refusal above is the first response in this
    // gateway's life that a `HEAD` can actually reach.
    //
    // `None` is a request whose head would not parse, so there is no method
    // to honour and the body is the only thing that can explain why.
    let carries_body = method != Some(&ureq::http::Method::HEAD);
    let headers = vec![
        ("content-type".to_owned(), b"application/json".to_vec()),
        (
            "content-length".to_owned(),
            body.len().to_string().into_bytes(),
        ),
        ("connection".to_owned(), b"close".to_vec()),
    ];
    if http::write_head(out, status, &headers).is_ok() {
        if carries_body {
            let _ = out.write_all(body.as_bytes());
        }
        let _ = out.flush();
    }
}

/// Let a refused request finish arriving, then close.
///
/// Both halves matter. Reading what is still in flight is what stops the
/// close below from becoming a connection reset that discards the response
/// the client was just sent — a client that got a reset sees a network
/// error, not the `401` or `411` that would have told it what was wrong.
/// And both the byte cap ([`DRAIN_CAP`]) and the time cap
/// ([`SETTLE_TIMEOUT`]) are there because this is work done on behalf of a
/// request that has already been refused: neither a large body nor a client
/// that stops sending may hold this thread.
fn settle(reader: &mut BufReader<TcpStream>, out: &mut TcpStream, content_length: Option<u64>) {
    let _ = reader.get_ref().set_read_timeout(Some(SETTLE_TIMEOUT));
    let cap = content_length.unwrap_or(DRAIN_CAP).min(DRAIN_CAP);
    let _ = std::io::copy(&mut reader.take(cap), &mut std::io::sink());
    let _ = out.shutdown(Shutdown::Both);
}

/// Whether a response with this status is allowed to carry a body at all.
///
/// A `204` or a `304` with a `content-length` or a chunked framing is a
/// protocol error that some clients treat as the start of the *next*
/// response, so this is a correctness rule rather than an optimisation.
fn status_carries_a_body(status: StatusCode) -> bool {
    !(status.is_informational()
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED)
}

/// One [`Exchange`], with the upstream's non-secret identity filled in.
///
/// `route` is `None` for everything refused before a target could be placed.
/// It is not defaulted to the first route: a log that named a protocol and a
/// host for a request that never reached either would be inventing the one
/// fact it exists to record.
fn exchange(outcome: Outcome, status: u16, upstream: &Upstream, route: Option<&Route>) -> Exchange {
    Exchange {
        outcome,
        status,
        provider: upstream.provider().to_owned(),
        protocol: route.map(|route| route.protocol().to_owned()),
        host: route.map(Route::host).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use ureq::http::{HeaderMap, HeaderName, Method};

    fn head_with_authorization(value: &str) -> http::RequestHead {
        let mut headers = HeaderMap::new();
        headers.append(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(value).expect("a header-safe test value"),
        );
        http::RequestHead {
            method: Method::POST,
            target: "/v1/messages".to_owned(),
            headers,
            content_length: None,
        }
    }

    #[test]
    fn only_this_instances_own_token_is_accepted() {
        let token = GatewayToken::generate().expect("the OS has entropy");
        let other = GatewayToken::generate().expect("the OS has entropy");

        let presented = format!("Bearer {}", token.expose());
        assert!(presented_token_matches(
            &head_with_authorization(&presented),
            &token
        ));

        for wrong in [
            format!("Bearer {}", other.expose()),
            format!("Bearer {}", &token.expose()[..32]),
            format!("Bearer {}x", token.expose()),
            format!("bearer {}", token.expose()),
            format!("Basic {}", token.expose()),
            token.expose().to_owned(),
            "Bearer ".to_owned(),
            String::new(),
        ] {
            assert!(
                !presented_token_matches(&head_with_authorization(&wrong), &token),
                "a request presenting a token that is not this instance's own was accepted"
            );
        }

        // ... and no `authorization` header at all is not an accident that
        // passes.
        let bare = http::RequestHead {
            method: Method::POST,
            target: "/v1/messages".to_owned(),
            headers: HeaderMap::new(),
            content_length: None,
        };
        assert!(!presented_token_matches(&bare, &token));
    }

    #[test]
    fn the_comparison_folds_every_byte_rather_than_stopping_at_the_first() {
        assert!(constant_time_eq(b"abcd", b"abcd"));
        assert!(!constant_time_eq(b"abcd", b"abce"));
        assert!(!constant_time_eq(b"abcd", b"abcde"));
        assert!(!constant_time_eq(b"abcde", b"abcd"));
        assert!(!constant_time_eq(b"", b"abcd"));
        // An empty expected value must reject rather than accept, or a
        // gateway whose token failed to generate would accept everything.
        assert!(!constant_time_eq(b"anything", b""));
        assert!(!constant_time_eq(b"", b""));
    }

    /// Everything one call to [`Exchange::record`] actually writes.
    ///
    /// `tracing::subscriber::with_default` installs a **thread-local**
    /// default, not a global one, so this cannot race the other tests in
    /// this binary the way `set_global_default` would.
    fn recorded(exchange: &Exchange) -> String {
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct Capture(Arc<Mutex<Vec<u8>>>);

        impl std::io::Write for Capture {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .expect("no test panics while holding this")
                    .extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
            type Writer = Capture;
            fn make_writer(&'a self) -> Capture {
                self.clone()
            }
        }

        let sink = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(Capture(Arc::clone(&sink)))
            .with_max_level(tracing::Level::TRACE)
            .without_time()
            .finish();
        tracing::subscriber::with_default(subscriber, || exchange.record());

        let captured = sink
            .lock()
            .expect("no test panics while holding this")
            .clone();
        String::from_utf8_lossy(&captured).into_owned()
    }

    /// The packet asks that neither secret appear "in any Debug, log line, or
    /// error rendering". The `Debug` half is asserted in several places; this
    /// is the **log line** half, asserted against what `tracing` actually
    /// emitted rather than against the argument that it could not have
    /// emitted anything else.
    ///
    /// Lose this and the guard on what a gateway exchange writes to a log is
    /// a reading of [`Exchange`]'s declaration — which is a good guard, and
    /// is not the same as having seen the line.
    #[test]
    fn a_recorded_exchange_writes_a_line_with_no_secret_in_it() {
        const PROVIDER_CREDENTIAL: &str = "sk-ant-planted-provider-000111222333444";
        const PROMPT_BODY: &str = "PLANTED-PROMPT-BODY-DO-NOT-LOG";

        for outcome in [
            Outcome::Forwarded {
                upstream_status: 429,
                bytes: 4096,
            },
            Outcome::Unauthenticated,
            Outcome::Declined,
            Outcome::Unreachable {
                detail: transport_detail(&ureq::Error::Tls(
                    "planted certificate text carrying PLANTED-PROMPT-BODY-DO-NOT-LOG",
                )),
            },
            Outcome::ClientGone,
            Outcome::Idle,
            Outcome::Unrouted,
        ] {
            let exchange = Exchange {
                outcome,
                status: 429,
                provider: "openrouter".to_owned(),
                protocol: Some("anthropic-messages".to_owned()),
                host: "openrouter.ai".to_owned(),
            };
            let line = recorded(&exchange);

            assert!(
                !line.is_empty(),
                "nothing was recorded, so the scans below prove nothing"
            );
            assert!(
                !line.contains(PROVIDER_CREDENTIAL),
                "a provider credential reached a log line: {line}"
            );
            assert!(
                !line.contains(PROMPT_BODY),
                "text quoted out of a request reached a log line: {line}"
            );
            // ... and the line is still worth writing: the status and the
            // provider are what a user needs to see when a session starts
            // failing.
            assert!(line.contains("429"), "{line}");
            assert!(line.contains("openrouter"), "{line}");
        }
    }

    /// A structural rule, checked against the declaration rather than
    /// promised in prose: there is nowhere in an [`Exchange`] to put a body.
    /// Lose this and the first person who wants "just the error text for
    /// debugging" adds a `String` that holds a prompt.
    #[test]
    fn an_exchange_has_nowhere_to_put_a_body() {
        let source = include_str!("ingress.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("split always yields at least one part");
        // Comment lines are dropped, the same way every other source scan in
        // this crate drops them: these declarations *describe* what they must
        // not hold, in prose that names it. A scan that could not tell the
        // description from the thing would have to be deleted the first time
        // someone wrote the rule down.
        let declaration_of = |header: &str| {
            let start = production
                .find(header)
                .unwrap_or_else(|| panic!("`{header}` is declared in this module"));
            let rest = &production[start..];
            let end = rest.find("\n}").expect("the declaration ends");
            rest[..end]
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n")
        };

        // Both types are logged, so both are scanned — but the lists differ,
        // and the difference is the rule rather than an oversight.
        //
        // `Exchange` names *who*: a configured provider and a host, both of
        // which the user wrote in their own configuration file. Owned strings
        // are right there.
        //
        // `Outcome` says *what happened*, and every one of its fields is a
        // status, a count, or a phrase written in this file. It used to hold
        // an owned `String` built from `ureq`'s own error text — see
        // `transport_detail` — and that is exactly the shape this scan exists
        // to refuse: an owned string is somewhere foreign text can be kept,
        // and a borrowed static one is not.
        let body_shaped = ["Vec<u8>", "[u8]", "Bytes", "body", "payload", "content"];
        for (name, header, forbidden) in [
            (
                "Exchange",
                "pub(super) struct Exchange {",
                body_shaped.to_vec(),
            ),
            ("Outcome", "pub(super) enum Outcome {", {
                let mut list = body_shaped.to_vec();
                list.push("String");
                list
            }),
        ] {
            let declaration = declaration_of(header);
            assert!(
                !declaration.is_empty(),
                "{name}'s declaration was not found, so this scan would pass vacuously"
            );
            for needle in forbidden {
                assert!(
                    !declaration.contains(needle),
                    "{name}'s declaration names `{needle}`: what this gateway logs must be \
                     structurally unable to carry a body, or any other text produced outside \
                     this file"
                );
            }
        }
    }

    #[test]
    fn a_status_that_may_not_carry_a_body_is_framed_as_carrying_none() {
        for status in [
            StatusCode::NO_CONTENT,
            StatusCode::NOT_MODIFIED,
            StatusCode::CONTINUE,
        ] {
            assert!(!status_carries_a_body(status), "{status}");
        }
        for status in [
            StatusCode::OK,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            assert!(status_carries_a_body(status), "{status}");
        }
    }

    /// `Outcome::Unreachable`'s detail is the only place a transport failure
    /// says anything at all, and it says it in this file's own words.
    ///
    /// Two properties, and the second is the one that took a failing test to
    /// find. The phrase must distinguish the failures a user fixes
    /// differently — a refused connection and an unresolvable host have
    /// nothing to do with each other. And it must never be `ureq`'s own
    /// string: `crate::secret::redact` removes credential-shaped runs and
    /// makes no promise about the text around them, so a redacted foreign
    /// string still carries whatever else was in it.
    #[test]
    fn a_transport_detail_is_this_files_own_words_and_never_the_errors() {
        let refused = transport_detail(&ureq::Error::ConnectionFailed);
        let unresolved = transport_detail(&ureq::Error::HostNotFound);
        let tls = transport_detail(&ureq::Error::Tls("planted certificate text"));

        assert_ne!(
            refused, unresolved,
            "two transport failures with completely different fixes are reported identically"
        );
        assert_ne!(refused, tls);
        assert!(
            !tls.contains("planted certificate text"),
            "the error's own text reached the diagnostic: {tls}"
        );

        // Whatever a variant carries, the phrase is drawn from the fixed set
        // written above it. Checked as a set membership rather than one
        // string at a time, so a variant mapped to an interpolated string
        // fails here.
        let vocabulary = [
            "the provider's host name did not resolve",
            "the connection to the provider could not be made",
            "the connection to the provider failed",
            "the provider did not answer in time",
            "the TLS connection to the provider could not be established",
            "the request could not be addressed to the provider",
            "the provider's answer was not valid HTTP",
            "the provider could not be reached",
        ];
        for err in [
            ureq::Error::ConnectionFailed,
            ureq::Error::HostNotFound,
            ureq::Error::Tls("planted certificate text"),
            ureq::Error::BadUri("https://planted.example/sk-ant-planted".to_owned()),
            ureq::Error::BodyExceedsLimit(1),
        ] {
            let detail = transport_detail(&err);
            assert!(
                vocabulary.contains(&detail),
                "a transport detail escaped the fixed vocabulary: {detail:?}"
            );
        }
    }

    #[test]
    fn a_declined_head_maps_to_the_status_that_says_why() {
        assert_eq!(
            decline(&HeadError::TooLarge).0,
            StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE
        );
        assert_eq!(
            decline(&HeadError::ChunkedRequest).0,
            StatusCode::LENGTH_REQUIRED
        );
        assert_eq!(decline(&HeadError::Malformed).0, StatusCode::BAD_REQUEST);
    }
}
