//! Real network probes against a configured provider: connectivity, and the
//! model catalogue behind a `GET <base>/models`.
//!
//! # This replaces a precondition check
//!
//! Phase 9D line 1 asks that a user be able to test a provider *before*
//! enabling it for routing. The first version of that check could not make a
//! request — the batch that shipped it had no HTTP client on its branch — so
//! it proved what could be proven without one (the template resolves, a base
//! URL exists, a credential variable is set) and said so on screen. `ureq` is
//! here now, for the gateway, so the check is a request. The preconditions
//! are still checked first, because a request that cannot possibly work is
//! not worth opening a socket for, but they are no longer the answer.
//!
//! # Exactly one request, and only when asked
//!
//! Nothing in this module runs on a timer, on start, or on a cache expiry.
//! Every function here makes exactly one HTTP request and only because a
//! keystroke asked for it — see [`mod@crate::provider::cache`] for the other
//! half of that rule, which is what makes starting Glasshouse silent.
//!
//! # Nothing here blocks the interface
//!
//! Every function in this module blocks its own thread, and every one of them
//! is bounded — see [`ProbeTimeouts`]. The caller is responsible for running
//! them somewhere other than the thread drawing the terminal;
//! `shell::spawn_provider_probe` is the one place that does it, and its doc
//! comment explains why a blocking call on the draw thread is the specific
//! bug this batch existed to avoid.
//!
//! # The credential
//!
//! A [`ProbeRequest`] may carry a resolved [`Secret`]. It has a hand-written
//! [`Debug`](std::fmt::Debug) that prints [`crate::secret::REDACTED`] in its
//! place, it is never written to the cache, and no failure message in this
//! module is built from text that passed anywhere near it — see
//! the private `unreachable_reason`, which is deliberately built from a fixed set of
//! phrases rather than from an error's own words.

use std::fmt;
use std::time::{Duration, Instant};

use ureq::Agent;
use ureq::config::AutoHeaderValue;

use crate::harness::WireProtocol;
use crate::provider::cache::ModelEntry;
use crate::secret::{REDACTED, Secret};

/// How long a probe waits for the TCP connection and TLS handshake.
///
/// A handshake to a live host completes in well under a second on any
/// ordinary connection; five seconds is enough for a slow resolver and a
/// congested link and is still short enough that a host which is simply not
/// there is reported rather than waited on.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a probe waits for the response head once the request is sent.
///
/// This is the timeout that matters, and the one whose absence would be a
/// hang: a server that accepts a connection and then says nothing is
/// indistinguishable from a healthy one until this expires. A model
/// catalogue is a read, not a generation — every one of the five catalogues
/// probed on 2026-08-26 answered in well under a second — so ten seconds is
/// already generous by a factor of ten.
pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

/// A ceiling on the whole call, body included.
///
/// The other two bound the phases a stall is *likely* in. This bounds the
/// one nobody thinks of: a server that answers its head promptly and then
/// dribbles the body one byte at a time forever would satisfy both of the
/// others indefinitely. With this set, no probe can outlive it whatever the
/// peer does.
pub const TOTAL_TIMEOUT: Duration = Duration::from_secs(20);

/// The largest response body a probe will read.
///
/// OpenRouter's 417-model catalogue was 687,721 bytes when read on
/// 2026-08-26 and Nous's 372-model catalogue was 660,362; eight mebibytes is
/// an order of magnitude of headroom over the largest real catalogue anyone
/// has measured here and is still a bound. `ureq`'s own default is ten
/// mebibytes, so this is not relying on that default staying what it is.
const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;

/// Every timeout a probe is bounded by.
///
/// A type rather than three constants read directly, for one reason: the
/// tests that prove a stalled endpoint is bounded have to be able to pick
/// short values, and a test that had to wait out [`RESPONSE_TIMEOUT`] would
/// be a ten-second test that people would eventually mark `#[ignore]`. The
/// production path uses [`ProbeTimeouts::default`] and nothing else, and a
/// test asserts that default is exactly the three constants above — because
/// "no timeout" is a hang, and a default that quietly lost one would be the
/// regression this whole batch exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeTimeouts {
    pub connect: Duration,
    pub response: Duration,
    pub total: Duration,
}

impl Default for ProbeTimeouts {
    fn default() -> Self {
        Self {
            connect: CONNECT_TIMEOUT,
            response: RESPONSE_TIMEOUT,
            total: TOTAL_TIMEOUT,
        }
    }
}

/// Which URL a probe actually requests.
///
/// Named rather than inferred, because the difference is visible to the user
/// in the result line and a probe that quietly chose a different URL than the
/// one it reported would be worse than no probe at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeTarget {
    /// `<base>/models` — the provider's own model list.
    ///
    /// Only for a provider whose [`crate::provider::Provider::model_list_endpoint`]
    /// is verified. It is the better probe when it is available: one request
    /// exercises the base URL, TLS, the credential and a real route, instead
    /// of only proving something answered.
    ModelList,
    /// The base URL itself, with no path appended.
    ///
    /// For a provider whose model-list endpoint nobody has established. A
    /// probe that appended `/models` anyway would be guessing at a path, and
    /// this module refuses that for exactly the reason
    /// [`mod@crate::provider`] refuses to guess at a base URL.
    BaseUrl,
}

/// One provider probe: where to go, what to send, and the credential to send
/// with it.
///
/// The credential is private and there is no accessor. It reaches exactly one
/// place — the header this module builds — and its [`Debug`] shows
/// [`REDACTED`].
pub struct ProbeRequest {
    provider: String,
    protocol: WireProtocol,
    base_url: String,
    target: ProbeTarget,
    headers: Vec<(String, String)>,
    credential: Option<Secret>,
}

impl ProbeRequest {
    pub fn new(
        provider: impl Into<String>,
        protocol: WireProtocol,
        base_url: impl Into<String>,
        target: ProbeTarget,
        headers: Vec<(String, String)>,
        credential: Option<Secret>,
    ) -> Self {
        Self {
            provider: provider.into(),
            protocol,
            base_url: base_url.into(),
            target,
            headers,
            credential,
        }
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn protocol(&self) -> WireProtocol {
        self.protocol
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn target(&self) -> ProbeTarget {
        self.target
    }

    /// The exact URL this probe requests.
    ///
    /// Public because the result line names it: "reached X" is a claim a user
    /// can only check if they are told what was actually asked for.
    pub fn url(&self) -> String {
        match self.target {
            ProbeTarget::BaseUrl => self.base_url.clone(),
            ProbeTarget::ModelList => {
                format!("{}/models", self.base_url.trim_end_matches('/'))
            }
        }
    }
}

/// Prints [`REDACTED`] where the credential is, and whether there is one at
/// all — which is a fact a diagnostic legitimately needs and which reveals
/// nothing about the value.
impl fmt::Debug for ProbeRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProbeRequest")
            .field("provider", &self.provider)
            .field("protocol", &self.protocol)
            .field("base_url", &self.base_url)
            .field("target", &self.target)
            .field("headers", &self.headers)
            .field(
                "credential",
                &match self.credential {
                    Some(_) => REDACTED,
                    None => "(none)",
                },
            )
            .finish()
    }
}

/// What one probe found.
///
/// The distinction that earns this type its variants is
/// [`ProbeOutcome::Rejected`] against [`ProbeOutcome::Unreachable`]: a
/// provider that answered `401` and a provider that answered nothing are
/// different problems with different fixes — one is a credential, one is a
/// URL or a network — and a user told only "the test failed" has to guess
/// which they have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// Answered `2xx`. The endpoint is there and took the credential.
    Reached { status: u16 },
    /// Answered `401` or `403`. The endpoint is there and refused the
    /// credential — reachable, rejected.
    Rejected { status: u16 },
    /// Answered something else. Reported verbatim rather than translated: a
    /// `404` on a model list and a `503` from a provider having a bad day
    /// are both "it answered", and inventing a friendlier word for either
    /// would throw away the only number the user can act on.
    Unexpected { status: u16 },
    /// Bounded by a timeout instead of answering.
    TimedOut { waited_ms: u64 },
    /// Never answered at all — no route, no host, a refused connection.
    Unreachable { reason: String },
}

impl ProbeOutcome {
    /// Whether the endpoint produced an HTTP response of any kind.
    ///
    /// True for a `401` and a `503` alike. Deliberately not called
    /// "succeeded": reaching a provider is not the same as being able to
    /// route to it, and nothing in this module decides the second question.
    pub fn answered(&self) -> bool {
        matches!(
            self,
            Self::Reached { .. } | Self::Rejected { .. } | Self::Unexpected { .. }
        )
    }
}

/// What a model-catalogue fetch produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelFetch {
    /// A catalogue was read, in the order the provider listed it.
    Catalogue(Vec<ModelEntry>),
    /// The endpoint answered `2xx` with something that is not a catalogue.
    ///
    /// Separate from [`ProbeOutcome::Unexpected`] because the failure is in
    /// the body rather than the status, and the two have different fixes:
    /// one is "that URL is wrong", the other is "that URL is right and this
    /// parser does not understand what it said".
    NotACatalogue { status: u16, reason: String },
    /// Everything a connectivity probe can report, reported the same way.
    Probe(ProbeOutcome),
}

/// What one probe found, **with the rate-limit headers it came back with** —
/// capability map line 1229.
///
/// # Why the headers are here and not on [`ProbeOutcome`]
///
/// A [`ProbeOutcome`] answers "did this endpoint answer, and how". Adding a
/// header list to its variants would put quota telemetry inside the type
/// [`mod@crate::shell::state`] renders as a one-line connectivity result, and
/// every existing caller would have to learn to ignore it. This type wraps
/// that answer instead, so [`connectivity`] keeps its exact signature and
/// nothing that only wants the outcome changes at all.
///
/// # Only the headers Glasshouse asked for
///
/// `headers` is what [`crate::provider::telemetry::retain_rate_limit_headers`]
/// kept, which is an allowlist and not a filter — see
/// [`crate::provider::telemetry::RATE_LIMIT_HEADERS`] for why that matters,
/// and note in particular that OpenRouter's `GET /api/v1/models` answers with
/// a `set-cookie` header. A probe result is a diagnostic a user may be invited
/// to share, and a diagnostic that captured "the response headers" would carry
/// a session cookie into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResponse {
    outcome: ProbeOutcome,
    headers: Vec<(String, String)>,
}

impl ProbeResponse {
    pub fn new(outcome: ProbeOutcome, headers: Vec<(String, String)>) -> Self {
        Self { outcome, headers }
    }

    pub fn outcome(&self) -> &ProbeOutcome {
        &self.outcome
    }

    /// The rate-limit headers this response carried, lowercased, in the
    /// order [`crate::provider::telemetry::RATE_LIMIT_HEADERS`] lists them.
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// What those headers said — capability map line 1229.
    ///
    /// Empty when the response carried none, which was the answer for seven
    /// of the eight hosts Glasshouse ships templates for when this was
    /// measured; see [`mod@crate::provider::telemetry`].
    pub fn rate_limits(&self) -> crate::provider::telemetry::RateLimitHeaders {
        crate::provider::telemetry::RateLimitHeaders::read(
            self.headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
        )
    }
}

/// Make one request and report what happened, without reading a body.
///
/// This is Phase 9D line 1. It reports; it decides nothing. A failure here
/// must never disable a provider and a success must never enable one — see
/// the caller in [`mod@crate::shell::state`], where that separation is the
/// thing under test.
pub fn connectivity(request: &ProbeRequest, timeouts: ProbeTimeouts) -> ProbeOutcome {
    connectivity_with_headers(request, timeouts).outcome
}

/// [`connectivity`], keeping the rate-limit headers the response carried —
/// capability map line 1229, and the only place in Glasshouse that reads a
/// quota number off a response.
///
/// # Why here and not on the gateway's forwarding path
///
/// The gateway is deliberately excluded. Phase 9I line 528 settled that *the
/// gateway forwards headers without reading them, and a parser there would
/// make it a reader of the payload it exists to pass through*. This module is
/// the opposite kind of thing: it already makes a request **because a
/// keystroke asked it to**, it already holds the response, and until now it
/// discarded the headers. Reading them costs no extra request — which is the
/// other half of why this is the right seam, since capability map line 1230's
/// "without excessive request cost" applies to the whole telemetry story and
/// a probe that already ran is free.
pub fn connectivity_with_headers(request: &ProbeRequest, timeouts: ProbeTimeouts) -> ProbeResponse {
    let started = Instant::now();
    match send(request, timeouts) {
        Ok(response) => {
            let outcome = classify(response.status().as_u16());
            ProbeResponse::new(outcome, rate_limit_headers_of(&response))
        }
        Err(err) => ProbeResponse::new(transport_outcome(&err, started), Vec::new()),
    }
}

/// Pull the allowlisted rate-limit headers off a response.
///
/// A header whose value is not valid UTF-8 is skipped rather than escaped:
/// every field this module reads is an integer or a short parameter list, and
/// a non-UTF-8 value in one of them is not a number this parser could have
/// understood anyway.
fn rate_limit_headers_of(response: &ureq::http::Response<ureq::Body>) -> Vec<(String, String)> {
    let named: Vec<(&str, &str)> = response
        .headers()
        .iter()
        .filter_map(|(name, value)| Some((name.as_str(), value.to_str().ok()?)))
        .collect();
    crate::provider::telemetry::retain_rate_limit_headers(named)
}

/// Make one request to the provider's model list and read it.
///
/// This is Phase 9D line 2's network half. It is only ever called for a
/// provider whose model-list endpoint is established — the caller decides
/// that, and says so plainly when it is not, because "this provider does not
/// offer model discovery" is an answer and not an error.
pub fn model_catalogue(request: &ProbeRequest, timeouts: ProbeTimeouts) -> ModelFetch {
    let started = Instant::now();
    let mut response = match send(request, timeouts) {
        Ok(response) => response,
        Err(err) => return ModelFetch::Probe(transport_outcome(&err, started)),
    };

    let status = response.status().as_u16();
    let outcome = classify(status);
    if !matches!(outcome, ProbeOutcome::Reached { .. }) {
        return ModelFetch::Probe(outcome);
    }

    let body = match response
        .body_mut()
        .with_config()
        .limit(MAX_BODY_BYTES)
        .read_to_string()
    {
        Ok(body) => body,
        Err(ureq::Error::Timeout(_)) => {
            return ModelFetch::Probe(ProbeOutcome::TimedOut {
                waited_ms: elapsed_ms(started),
            });
        }
        Err(_) => {
            return ModelFetch::NotACatalogue {
                status,
                reason: "the response body could not be read".to_owned(),
            };
        }
    };

    match parse_catalogue(&body) {
        Ok(models) => ModelFetch::Catalogue(models),
        Err(reason) => ModelFetch::NotACatalogue { status, reason },
    }
}

/// Read a model catalogue out of a `GET /models` response body.
///
/// # The shape, as actually read
///
/// Five live catalogues were read unauthenticated on 2026-08-26 —
/// OpenRouter (417 entries), Nous (372), UnoRouter (374), Kilo (367) and
/// AnyRouter (102). All five put their entries under a top-level `data`
/// array and **every entry in all five carried a string `id`**. Nothing else
/// was universal: UnoRouter wraps the array in `{"success":…,"message":…}`,
/// AnyRouter adds `"object":"list"`, and UnoRouter's entries have no `name`
/// field at all where the other four do. So `id` is the one field this
/// parser reads and the one field [`ModelEntry`] stores; anything else would
/// be a field that is absent for at least one provider already shipping a
/// template here.
///
/// A bare top-level array is accepted too. That shape was not observed in
/// any of the five, but it costs one match arm and it is the obvious way a
/// small self-hosted proxy — the `llama-cpp` and `ollama` templates point at
/// two — answers this route.
fn parse_catalogue(body: &str) -> Result<Vec<ModelEntry>, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|_| "the response was not JSON".to_owned())?;

    let entries = match &value {
        serde_json::Value::Array(entries) => entries,
        serde_json::Value::Object(object) => match object.get("data") {
            Some(serde_json::Value::Array(entries)) => entries,
            _ => return Err("the response has no `data` array of models".to_owned()),
        },
        _ => return Err("the response was not a model list".to_owned()),
    };

    let models: Vec<ModelEntry> = entries
        .iter()
        .filter_map(|entry| match entry {
            serde_json::Value::String(id) => Some(ModelEntry::new(id)),
            serde_json::Value::Object(object) => object
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(ModelEntry::new),
            _ => None,
        })
        .collect();

    if models.is_empty() && !entries.is_empty() {
        return Err("no entry in the model list had an `id`".to_owned());
    }
    Ok(models)
}

/// Send the request, with every timeout set and every one of `ureq`'s
/// helpful behaviours turned off.
///
/// `http_status_as_error(false)` is the load-bearing one: with the default, a
/// `401` arrives as an `Err` indistinguishable in shape from a refused
/// connection, and telling those two apart is the whole point of
/// [`ProbeOutcome::Rejected`]. `max_redirects(0)` is the second: following a
/// redirect would mean re-attaching the credential to a host the provider
/// named at runtime, which is a decision this module has no business making
/// silently — and it is not hypothetical, because `kilocode.ai` answers
/// `308` to `kilo.ai` today.
fn send(
    request: &ProbeRequest,
    timeouts: ProbeTimeouts,
) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    let agent = Agent::new_with_config(
        Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .accept_encoding(AutoHeaderValue::None)
            .timeout_connect(Some(timeouts.connect))
            .timeout_recv_response(Some(timeouts.response))
            .timeout_global(Some(timeouts.total))
            .build(),
    );

    let mut builder = agent.get(request.url());
    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }
    if let Some(credential) = &request.credential {
        // The one place a resolved credential is read in this module. It goes
        // into a header value and nowhere else — not into the URL, not into
        // a log line, not into the error path below.
        builder = match request.protocol {
            WireProtocol::AnthropicMessages => builder.header("x-api-key", credential.expose()),
            WireProtocol::OpenAiChat | WireProtocol::OpenAiResponses => {
                builder.header("authorization", format!("Bearer {}", credential.expose()))
            }
        };
    }
    builder.call()
}

/// An HTTP status, as an outcome.
fn classify(status: u16) -> ProbeOutcome {
    match status {
        200..=299 => ProbeOutcome::Reached { status },
        401 | 403 => ProbeOutcome::Rejected { status },
        _ => ProbeOutcome::Unexpected { status },
    }
}

/// A transport failure, as an outcome.
fn transport_outcome(err: &ureq::Error, started: Instant) -> ProbeOutcome {
    if is_timeout(err) {
        ProbeOutcome::TimedOut {
            waited_ms: elapsed_ms(started),
        }
    } else {
        ProbeOutcome::Unreachable {
            reason: unreachable_reason(err),
        }
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Whether `err` is a timeout, however it was raised.
///
/// Two arms rather than one because `ureq` raises its own configured
/// timeouts as [`ureq::Error::Timeout`] but a socket-level deadline can
/// still surface as an `io::Error` — and a timeout reported as "unreachable"
/// would tell the user their URL is wrong when their network is slow.
fn is_timeout(err: &ureq::Error) -> bool {
    match err {
        ureq::Error::Timeout(_) => true,
        ureq::Error::Io(io) => matches!(
            io.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ),
        _ => false,
    }
}

/// Why a request never got an answer, in words chosen here.
///
/// **Built from a fixed set of phrases on purpose.** Every one of these is a
/// literal in this function, so no text from a peer, a header, a URL or an
/// error's own `Display` can reach a diagnostic through this path. That is a
/// structural guarantee rather than a careful habit, which is the same trade
/// [`crate::secret::Secret`]'s hand-written `Debug` makes: it costs a little
/// detail and it cannot be undone by someone later adding a field.
fn unreachable_reason(err: &ureq::Error) -> String {
    let phrase = match err {
        ureq::Error::HostNotFound => "the host name did not resolve",
        ureq::Error::ConnectionFailed => "the connection failed",
        ureq::Error::BadUri(_) => "the base URL is not a usable URL",
        ureq::Error::RedirectFailed | ureq::Error::TooManyRedirects => {
            "the endpoint redirected, and a probe will not follow one"
        }
        ureq::Error::Protocol(_) => "the reply was not valid HTTP",
        ureq::Error::Io(io) => match io.kind() {
            std::io::ErrorKind::ConnectionRefused => "the connection was refused",
            std::io::ErrorKind::ConnectionReset => "the connection was reset",
            std::io::ErrorKind::ConnectionAborted => "the connection was aborted",
            std::io::ErrorKind::UnexpectedEof => "the connection closed before answering",
            std::io::ErrorKind::NotConnected | std::io::ErrorKind::BrokenPipe => {
                "the connection dropped"
            }
            std::io::ErrorKind::PermissionDenied => "the connection was not permitted",
            _ => "the connection failed",
        },
        _ => "the request could not be made",
    };
    phrase.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::fixture::FixtureProvider;

    /// Short enough that the whole suite does not wait on them, long enough
    /// that a loopback round trip is never mistaken for a stall.
    fn quick() -> ProbeTimeouts {
        ProbeTimeouts {
            connect: Duration::from_millis(500),
            response: Duration::from_millis(400),
            total: Duration::from_millis(900),
        }
    }

    fn request_at(base_url: &str, target: ProbeTarget) -> ProbeRequest {
        ProbeRequest::new(
            "test-provider",
            WireProtocol::OpenAiChat,
            base_url,
            target,
            Vec::new(),
            Some(Secret::mint_for_test("sk-probe-credential")),
        )
    }

    // --- the timeouts themselves ----------------------------------------

    /// A default of "no timeout" is a hang, and this is the assertion that
    /// says so. Every field is checked against its own constant so that
    /// losing one — the response timeout especially — fails here rather than
    /// in a frozen terminal.
    #[test]
    fn the_default_timeouts_are_the_named_constants_and_none_is_unset() {
        let defaults = ProbeTimeouts::default();
        assert_eq!(defaults.connect, CONNECT_TIMEOUT);
        assert_eq!(defaults.response, RESPONSE_TIMEOUT);
        assert_eq!(defaults.total, TOTAL_TIMEOUT);
        for (what, value) in [
            ("connect", defaults.connect),
            ("response", defaults.response),
            ("total", defaults.total),
        ] {
            assert!(!value.is_zero(), "the {what} timeout must not be zero");
            assert!(
                value <= Duration::from_secs(30),
                "the {what} timeout must stay short enough that a user waits rather than \
                 wonders whether the interface has frozen"
            );
        }
        assert!(
            defaults.total >= defaults.response,
            "the whole-call ceiling must not be shorter than the phase it contains"
        );
    }

    // --- what a probe actually requests ----------------------------------

    #[test]
    fn a_model_list_probe_requests_models_under_the_base_url() {
        let request = request_at("https://a.example/v1", ProbeTarget::ModelList);
        assert_eq!(request.url(), "https://a.example/v1/models");
    }

    #[test]
    fn a_trailing_slash_on_the_base_url_does_not_double_the_separator() {
        let request = request_at("https://a.example/v1/", ProbeTarget::ModelList);
        assert_eq!(request.url(), "https://a.example/v1/models");
    }

    #[test]
    fn a_base_url_probe_appends_no_path_at_all() {
        let request = request_at("https://a.example/v1", ProbeTarget::BaseUrl);
        assert_eq!(request.url(), "https://a.example/v1");
    }

    // --- Phase 32B line 1229: the headers a response carried --------------

    /// The header block `https://anyrouter.dev/api/v1/models` really answered
    /// with on 2026-08-27, in wire format.
    ///
    /// Served by the fixture so that the capture is proven **through
    /// `connectivity_with_headers`** rather than by handing
    /// `RateLimitHeaders::read` a list a test built. Practice §35: the
    /// telemetry tests all enter below this function, and a capture nothing
    /// enters through is a capture the suite would not miss.
    const ANYROUTER_HEADER_BLOCK: &str = "ratelimit-limit: 300\r\n\
         ratelimit-policy: 300;w=60\r\n\
         x-ratelimit-limit: 300\r\n\
         x-ratelimit-tier: ip\r\n\
         x-ratelimit-window: 60\r\n\
         access-control-expose-headers: X-RateLimit-Limit,RateLimit-Remaining\r\n\
         set-cookie: __cf_bm=oGkHQJmsGX6wCH7Quh5JYzAK6KXu1icwUg5MExQ2LqQ\r\n";

    #[test]
    fn a_response_carrying_rate_limit_headers_hands_them_back_to_the_caller() {
        let fixture =
            FixtureProvider::answering("HTTP/1.1 200 OK", ANYROUTER_HEADER_BLOCK, "{\"data\":[]}");
        let request = request_at(&fixture.base_url(), ProbeTarget::ModelList);
        let response = connectivity_with_headers(&request, quick());

        assert_eq!(response.outcome(), &ProbeOutcome::Reached { status: 200 });
        let limits = response.rate_limits();
        assert_eq!(limits.limit(), Some(300));
        assert_eq!(limits.window_seconds(), Some(60));
        // The host advertises `RateLimit-Remaining` and did not send it.
        assert_eq!(limits.remaining(), None);
    }

    /// The allowlist, at the boundary it exists to guard. This response
    /// carries a `set-cookie` — OpenRouter's own `GET /api/v1/models` does,
    /// measured — and a capture that kept "the response headers" would put a
    /// session cookie into a diagnostic a user is invited to share.
    #[test]
    fn nothing_but_an_allowlisted_header_survives_the_capture() {
        let fixture =
            FixtureProvider::answering("HTTP/1.1 200 OK", ANYROUTER_HEADER_BLOCK, "{\"data\":[]}");
        let request = request_at(&fixture.base_url(), ProbeTarget::ModelList);
        let response = connectivity_with_headers(&request, quick());

        let names: Vec<&str> = response
            .headers()
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "ratelimit-limit",
                "ratelimit-policy",
                "x-ratelimit-limit",
                "x-ratelimit-window"
            ],
            "the capture kept a header nobody asked for"
        );
        let rendered = format!("{response:?}");
        for forbidden in ["set-cookie", "__cf_bm", "oGkHQJmsGX", "x-ratelimit-tier"] {
            assert!(
                !rendered.contains(forbidden),
                "`{forbidden}` survived the capture"
            );
        }
    }

    #[test]
    fn a_response_with_no_rate_limit_header_hands_back_an_empty_capture() {
        let fixture = FixtureProvider::answering(
            "HTTP/1.1 200 OK",
            "content-type: application/json\r\ncf-cache-status: HIT\r\n",
            "{\"data\":[]}",
        );
        let request = request_at(&fixture.base_url(), ProbeTarget::ModelList);
        let response = connectivity_with_headers(&request, quick());
        assert!(response.headers().is_empty());
        assert!(response.rate_limits().is_empty());
    }

    /// `connectivity` must keep answering exactly what it always did, so no
    /// existing caller changed behaviour when the capture was added.
    #[test]
    fn capturing_headers_did_not_change_what_a_plain_connectivity_probe_answers() {
        let fixture =
            FixtureProvider::answering("HTTP/1.1 200 OK", ANYROUTER_HEADER_BLOCK, "{\"data\":[]}");
        let request = request_at(&fixture.base_url(), ProbeTarget::ModelList);
        assert_eq!(
            connectivity(&request, quick()),
            ProbeOutcome::Reached { status: 200 }
        );
    }

    /// A refusal carries headers too, and `Retry-After` is the one a provider
    /// sends with one. Capability map line 1229 says *rate-limit and usage
    /// headers*, not *headers on a success*.
    #[test]
    fn a_refusal_that_carries_a_retry_after_still_yields_a_reading() {
        let fixture = FixtureProvider::answering(
            "HTTP/1.1 429 Too Many Requests",
            "retry-after: 30\r\nratelimit-remaining: 0\r\n",
            "",
        );
        let request = request_at(&fixture.base_url(), ProbeTarget::ModelList);
        let response = connectivity_with_headers(&request, quick());
        assert_eq!(
            response.outcome(),
            &ProbeOutcome::Unexpected { status: 429 }
        );
        assert_eq!(response.rate_limits().retry_after_seconds(), Some(30));
        assert_eq!(response.rate_limits().remaining(), Some(0));
    }

    /// A request that never got an answer has no headers to carry, and asking
    /// for them must not be a way to turn a transport failure into a panic.
    #[test]
    fn an_unreachable_endpoint_yields_an_outcome_and_an_empty_capture() {
        let request = request_at("http://127.0.0.1:1/v1", ProbeTarget::ModelList);
        let response = connectivity_with_headers(&request, quick());
        assert!(!response.outcome().answered());
        assert!(response.headers().is_empty());
        assert!(response.rate_limits().is_empty());
    }

    // --- line 1: connectivity --------------------------------------------

    #[test]
    fn a_reachable_endpoint_is_reported_as_reached_with_its_status() {
        let fixture = FixtureProvider::answering("HTTP/1.1 200 OK", "", "{\"data\":[]}");
        let request = request_at(&fixture.base_url(), ProbeTarget::ModelList);
        assert_eq!(
            connectivity(&request, quick()),
            ProbeOutcome::Reached { status: 200 }
        );
        assert_eq!(fixture.requests().len(), 1, "exactly one request, no other");
    }

    /// Acceptance test 2. `401` is not "the test failed" — it is "the
    /// endpoint is there and did not accept this credential", which is a
    /// different problem with a different fix.
    #[test]
    fn an_endpoint_answering_401_is_reachable_but_rejected_not_unreachable() {
        let fixture = FixtureProvider::answering("HTTP/1.1 401 Unauthorized", "", "{}");
        let request = request_at(&fixture.base_url(), ProbeTarget::ModelList);
        let outcome = connectivity(&request, quick());
        assert_eq!(outcome, ProbeOutcome::Rejected { status: 401 });
        assert!(outcome.answered(), "a 401 means something answered");
        assert!(
            !matches!(outcome, ProbeOutcome::Unreachable { .. }),
            "a rejected credential must never be reported as an unreachable host"
        );
    }

    #[test]
    fn an_endpoint_answering_403_is_also_reachable_but_rejected() {
        let fixture = FixtureProvider::answering("HTTP/1.1 403 Forbidden", "", "{}");
        let request = request_at(&fixture.base_url(), ProbeTarget::ModelList);
        assert_eq!(
            connectivity(&request, quick()),
            ProbeOutcome::Rejected { status: 403 }
        );
    }

    #[test]
    fn an_endpoint_answering_404_is_reported_with_the_status_it_gave() {
        let fixture = FixtureProvider::answering("HTTP/1.1 404 Not Found", "", "nope");
        let request = request_at(&fixture.base_url(), ProbeTarget::ModelList);
        assert_eq!(
            connectivity(&request, quick()),
            ProbeOutcome::Unexpected { status: 404 }
        );
    }

    /// **Acceptance test 3, and the one that matters.**
    ///
    /// Not a refused connection — that is the easy case and it proves very
    /// little. This fixture accepts the connection, reads the request, and
    /// then says nothing at all, which is exactly what a wedged provider
    /// looks like and exactly what would hang a client with no read timeout.
    /// The assertion is that the probe comes back *and* reports a timeout,
    /// and that it does so within a bound derived from the timeout rather
    /// than from hope.
    #[test]
    fn an_endpoint_that_accepts_and_never_answers_is_bounded_by_the_timeout() {
        let fixture = FixtureProvider::hanging();
        let base_url = fixture.base_url();

        // **The probe runs on a thread and the assertion waits with a
        // deadline**, rather than calling `connectivity` directly. That is
        // not ceremony: the mutation that proves this test — deleting the
        // read timeout — makes the call never return, and a direct call
        // would hang the whole test binary rather than failing it. A test
        // that wedges CI reports nothing; this one reports a failure in a
        // bounded time whatever the peer or the code does.
        let (done, answer) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let request = request_at(&base_url, ProbeTarget::ModelList);
            let started = Instant::now();
            let outcome = connectivity(&request, quick());
            let _ = done.send((outcome, started.elapsed()));
        });

        let (outcome, elapsed) = answer.recv_timeout(Duration::from_secs(5)).expect(
            "the probe never came back — an endpoint that accepts and then says \
                     nothing must be bounded by the read timeout, not waited on forever",
        );

        assert!(
            matches!(outcome, ProbeOutcome::TimedOut { .. }),
            "a stalled endpoint must be reported as a timeout, got {outcome:?}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "the probe must be bounded by its own timeout, not by the peer; took {elapsed:?}"
        );
        assert_eq!(
            fixture.connections(),
            1,
            "the probe must have actually connected — a test that proved a refused \
             connection would prove nothing about a stall"
        );
    }

    /// Nothing listening is reported as a non-answer, and *how* it is reported
    /// follows what the operating system actually did.
    ///
    /// # Why this is not simply `assert Unreachable`
    ///
    /// It was, and Windows CI failed it with
    /// `nothing listening is unreachable, got TimedOut { waited_ms: 502 }`.
    /// That is not a defect in the code under test — it is a difference in
    /// what the platforms *do*. A Unix host answers a connection to a closed
    /// port with an immediate RST, which surfaces as
    /// [`std::io::ErrorKind::ConnectionRefused`] and classifies as
    /// [`ProbeOutcome::Unreachable`]. On the Windows runner the attempt
    /// instead ran out the connect timeout (502 ms against a 500 ms budget),
    /// so there was no refusal to classify and `TimedOut` is the honest
    /// answer.
    ///
    /// So the assertion is split. **The part that is true everywhere** — a
    /// host that nothing is listening on never counts as having answered — is
    /// asserted unconditionally, and it is the property the product actually
    /// promises. The classification of a *refusal* is asserted only where a
    /// refusal is what the platform produces.
    ///
    /// This is the same shape as the pty rule in the practice file: local
    /// behaviour that a `cfg` flip cannot reproduce, because it is a runtime
    /// property of the platform rather than a compile-time one.
    #[test]
    fn nothing_listening_never_counts_as_an_answer() {
        // Bind and drop, so the port is one nothing is listening on.
        let port = {
            let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .expect("loopback is bindable");
            listener
                .local_addr()
                .expect("a bound listener has an address")
                .port()
        };
        let request = request_at(&format!("http://127.0.0.1:{port}"), ProbeTarget::BaseUrl);
        let outcome = connectivity(&request, quick());

        // True on every platform, and the property that matters: a probe that
        // never reached anything must not report that it did.
        assert!(
            !outcome.answered(),
            "nothing was listening, so nothing can have answered, got {outcome:?}"
        );
        assert!(
            matches!(
                outcome,
                ProbeOutcome::Unreachable { .. } | ProbeOutcome::TimedOut { .. }
            ),
            "a port nothing is listening on is either refused or never answers, got {outcome:?}"
        );

        // Where the platform refuses, the refusal must be classified as a
        // refusal and not as a stall — the two have different fixes and the
        // user is told which they have.
        #[cfg(unix)]
        assert!(
            matches!(outcome, ProbeOutcome::Unreachable { .. }),
            "on Unix a closed port answers with RST, so this must classify as unreachable \
             rather than as a timeout, got {outcome:?}"
        );
    }

    /// The classification itself, with no operating system involved.
    ///
    /// This is what the platform-dependent test above can no longer prove
    /// everywhere, so it is proved here instead: a refusal is `Unreachable`,
    /// and it says so in words this module chose.
    #[test]
    fn a_refused_connection_classifies_as_unreachable_rather_than_a_timeout() {
        let refused = ureq::Error::Io(std::io::Error::from(std::io::ErrorKind::ConnectionRefused));
        assert!(!is_timeout(&refused), "a refusal is not a timeout");
        assert_eq!(unreachable_reason(&refused), "the connection was refused");

        let outcome = transport_outcome(&refused, Instant::now());
        assert!(
            matches!(outcome, ProbeOutcome::Unreachable { .. }),
            "got {outcome:?}"
        );
        assert!(!outcome.answered());
    }

    // --- line 2: the catalogue -------------------------------------------

    #[test]
    fn a_catalogue_is_read_from_the_data_array_in_the_order_given() {
        let fixture = FixtureProvider::answering(
            "HTTP/1.1 200 OK",
            "content-type: application/json\r\n",
            r#"{"object":"list","data":[{"id":"b/one"},{"id":"a/two"}]}"#,
        );
        let request = request_at(&fixture.base_url(), ProbeTarget::ModelList);
        match model_catalogue(&request, quick()) {
            ModelFetch::Catalogue(models) => assert_eq!(
                models,
                vec![ModelEntry::new("b/one"), ModelEntry::new("a/two")],
                "the provider's own order is preserved"
            ),
            other => panic!("expected a catalogue, got {other:?}"),
        }
    }

    /// UnoRouter's real envelope, quoted from the body read on 2026-08-26.
    #[test]
    fn an_envelope_around_the_data_array_does_not_stop_the_catalogue_being_read() {
        let fixture = FixtureProvider::answering(
            "HTTP/1.1 200 OK",
            "",
            r#"{"success":true,"message":"","data":[{"id":"glm-deep-research-thinking:free","object":"model","owned_by":"custom"}]}"#,
        );
        let request = request_at(&fixture.base_url(), ProbeTarget::ModelList);
        match model_catalogue(&request, quick()) {
            ModelFetch::Catalogue(models) => {
                assert_eq!(
                    models,
                    vec![ModelEntry::new("glm-deep-research-thinking:free")]
                );
            }
            other => panic!("expected a catalogue, got {other:?}"),
        }
    }

    #[test]
    fn a_bare_top_level_array_is_read_as_a_catalogue_too() {
        let fixture =
            FixtureProvider::answering("HTTP/1.1 200 OK", "", r#"[{"id":"local/model"}]"#);
        let request = request_at(&fixture.base_url(), ProbeTarget::ModelList);
        match model_catalogue(&request, quick()) {
            ModelFetch::Catalogue(models) => {
                assert_eq!(models, vec![ModelEntry::new("local/model")]);
            }
            other => panic!("expected a catalogue, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_catalogue_is_a_catalogue_and_not_a_failure() {
        let fixture = FixtureProvider::answering("HTTP/1.1 200 OK", "", r#"{"data":[]}"#);
        let request = request_at(&fixture.base_url(), ProbeTarget::ModelList);
        assert_eq!(
            model_catalogue(&request, quick()),
            ModelFetch::Catalogue(Vec::new()),
            "a provider that genuinely offers no models said so; that is an answer"
        );
    }

    #[test]
    fn a_200_that_is_not_a_catalogue_says_so_rather_than_reporting_an_empty_list() {
        let fixture = FixtureProvider::answering("HTTP/1.1 200 OK", "", "<html>hello</html>");
        let request = request_at(&fixture.base_url(), ProbeTarget::ModelList);
        match model_catalogue(&request, quick()) {
            ModelFetch::NotACatalogue { status, reason } => {
                assert_eq!(status, 200);
                assert!(!reason.is_empty());
            }
            other => panic!("expected NotACatalogue, got {other:?}"),
        }
    }

    #[test]
    fn a_catalogue_fetch_that_is_rejected_reports_the_rejection_not_an_empty_list() {
        let fixture = FixtureProvider::answering("HTTP/1.1 401 Unauthorized", "", "{}");
        let request = request_at(&fixture.base_url(), ProbeTarget::ModelList);
        assert_eq!(
            model_catalogue(&request, quick()),
            ModelFetch::Probe(ProbeOutcome::Rejected { status: 401 })
        );
    }

    #[test]
    fn a_stalled_catalogue_fetch_is_bounded_by_the_timeout_too() {
        let fixture = FixtureProvider::hanging();
        let request = request_at(&fixture.base_url(), ProbeTarget::ModelList);
        let started = Instant::now();
        let fetched = model_catalogue(&request, quick());
        assert!(
            matches!(fetched, ModelFetch::Probe(ProbeOutcome::TimedOut { .. })),
            "got {fetched:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// The two-orders-of-magnitude range the packet named: nine models
    /// against four hundred and seventeen. Both ends are read from the same
    /// parser with no truncation and no cap.
    #[test]
    fn a_catalogue_of_nine_and_a_catalogue_of_four_hundred_and_seventeen_both_read_whole() {
        for count in [9usize, 417] {
            let entries: Vec<String> = (0..count)
                .map(|index| format!("{{\"id\":\"vendor/model-{index}\"}}"))
                .collect();
            let body = format!("{{\"data\":[{}]}}", entries.join(","));
            let fixture = FixtureProvider::start(move |_request, out| {
                use std::io::Write as _;
                let _ = write!(
                    out,
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = out.flush();
                let _ = out.shutdown(std::net::Shutdown::Write);
            });
            let request = request_at(&fixture.base_url(), ProbeTarget::ModelList);
            match model_catalogue(&request, quick()) {
                ModelFetch::Catalogue(models) => {
                    assert_eq!(models.len(), count, "every entry must survive the read");
                    assert_eq!(models[0], ModelEntry::new("vendor/model-0"));
                    assert_eq!(
                        models[count - 1],
                        ModelEntry::new(format!("vendor/model-{}", count - 1))
                    );
                }
                other => panic!("expected a catalogue of {count}, got {other:?}"),
            }
        }
    }

    // --- the credential ---------------------------------------------------

    /// Acceptance test 7, at this module's own boundary: the value goes into
    /// a header and nowhere else. Asserted with `!contains` rather than
    /// `assert_eq!`, because a failing `assert_eq!` on secret material
    /// prints both sides.
    #[test]
    fn the_credential_reaches_the_authorization_header_and_no_other_surface() {
        const VALUE: &str = "sk-planted-credential-value-9d";
        let fixture = FixtureProvider::answering("HTTP/1.1 200 OK", "", r#"{"data":[]}"#);
        let request = ProbeRequest::new(
            "leak-check",
            WireProtocol::OpenAiChat,
            fixture.base_url(),
            ProbeTarget::ModelList,
            vec![("x-config-header".to_owned(), "plain".to_owned())],
            Some(Secret::mint_for_test(VALUE)),
        );

        // It must not be in the type's own rendering...
        let debug = format!("{request:?}");
        assert!(
            !debug.contains(VALUE),
            "a credential reached ProbeRequest's Debug"
        );
        assert!(debug.contains(REDACTED), "{debug}");

        // ... nor in the URL ...
        assert!(!request.url().contains(VALUE));

        let outcome = connectivity(&request, quick());
        assert_eq!(outcome, ProbeOutcome::Reached { status: 200 });

        // ... and on the wire it is in exactly one header and no other.
        let sent = fixture.requests();
        assert_eq!(sent.len(), 1);
        let sent = &sent[0];
        assert_eq!(
            sent.header("authorization"),
            Some(format!("Bearer {VALUE}").as_str()),
            "the credential must be attached as a bearer token"
        );
        assert!(!sent.target.contains(VALUE), "a credential reached the URL");
        for (name, value) in &sent.headers {
            if name != "authorization" {
                assert!(
                    !value.contains(VALUE),
                    "a credential reached the `{name}` header"
                );
            }
        }
        assert!(
            !String::from_utf8_lossy(&sent.body).contains(VALUE),
            "a credential reached the request body"
        );

        // ... and none of the reported outcome carries it either.
        assert!(!format!("{outcome:?}").contains(VALUE));
    }

    #[test]
    fn an_anthropic_provider_sends_its_credential_as_x_api_key_not_a_bearer_token() {
        const VALUE: &str = "sk-ant-planted-9d";
        let fixture = FixtureProvider::answering("HTTP/1.1 200 OK", "", "{}");
        let request = ProbeRequest::new(
            "anthropic-ish",
            WireProtocol::AnthropicMessages,
            fixture.base_url(),
            ProbeTarget::BaseUrl,
            Vec::new(),
            Some(Secret::mint_for_test(VALUE)),
        );
        connectivity(&request, quick());
        let sent = fixture.requests();
        assert_eq!(sent[0].header("x-api-key"), Some(VALUE));
        assert!(
            sent[0].header("authorization").is_none(),
            "an Anthropic-protocol provider must not be sent a bearer token"
        );
    }

    #[test]
    fn a_provider_with_no_credential_sends_no_credential_header() {
        let fixture = FixtureProvider::answering("HTTP/1.1 200 OK", "", "{}");
        let request = ProbeRequest::new(
            "no-credential",
            WireProtocol::OpenAiChat,
            fixture.base_url(),
            ProbeTarget::BaseUrl,
            Vec::new(),
            None,
        );
        connectivity(&request, quick());
        let sent = fixture.requests();
        assert!(sent[0].header("authorization").is_none());
        assert!(format!("{request:?}").contains("(none)"));
    }

    /// A provider's configured headers are configuration, so they are sent —
    /// but they are sent as written, and adding one must not disturb the
    /// credential header beside it.
    #[test]
    fn a_providers_configured_headers_are_sent_as_written() {
        let fixture = FixtureProvider::answering("HTTP/1.1 200 OK", "", "{}");
        let request = ProbeRequest::new(
            "with-headers",
            WireProtocol::OpenAiChat,
            fixture.base_url(),
            ProbeTarget::BaseUrl,
            vec![
                (
                    "http-referer".to_owned(),
                    "https://glasshouse.dev".to_owned(),
                ),
                ("x-title".to_owned(), "Glasshouse".to_owned()),
            ],
            None,
        );
        connectivity(&request, quick());
        let sent = fixture.requests();
        assert_eq!(
            sent[0].header("http-referer"),
            Some("https://glasshouse.dev")
        );
        assert_eq!(sent[0].header("x-title"), Some("Glasshouse"));
    }

    // --- redirects are not followed ---------------------------------------

    /// `kilocode.ai` answers `308` pointing at `kilo.ai` today, which is why
    /// the `kilo` template declares the new host. A probe that quietly
    /// followed a redirect would hide exactly that fact, and would also be
    /// deciding on its own to hand the credential to whatever host the
    /// redirect named.
    #[test]
    fn a_redirect_is_reported_rather_than_followed() {
        let fixture = FixtureProvider::answering(
            "HTTP/1.1 308 Permanent Redirect",
            "location: https://elsewhere.example/api/models\r\n",
            "",
        );
        let request = request_at(&fixture.base_url(), ProbeTarget::ModelList);
        assert_eq!(
            connectivity(&request, quick()),
            ProbeOutcome::Unexpected { status: 308 },
            "a redirect is a fact about the endpoint, not something to chase"
        );
    }

    // --- the parser, without a socket -------------------------------------

    #[test]
    fn a_body_that_is_not_json_is_refused_by_name() {
        assert_eq!(
            parse_catalogue("not json at all"),
            Err("the response was not JSON".to_owned())
        );
    }

    #[test]
    fn a_json_object_with_no_data_array_is_refused_by_name() {
        assert!(parse_catalogue(r#"{"object":"list"}"#).is_err());
    }

    #[test]
    fn entries_without_an_id_do_not_silently_become_an_empty_catalogue() {
        assert!(parse_catalogue(r#"{"data":[{"name":"no id here"}]}"#).is_err());
    }
}
