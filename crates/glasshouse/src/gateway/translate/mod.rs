//! Wire-protocol translation at the gateway — Phase 56, lines 1948–1950 and
//! 1956, under the ruling recorded in `docs/product/design-decisions.md` as
//! *"the user's answer on pairs: all of them"*.
//!
//! # Codecs around one canonical form, and a table of pairs
//!
//! [`canonical`] is the one form. `anthropic`, `openai_chat` and
//! `openai_responses` are the codecs, each decoding its wire into that form
//! and encoding out of it, in both the request and the response direction
//! and for streams. A **pair**
//! is a decoder and an encoder meeting in the middle, and [`pairs`] is the
//! table that lists every ordered pair of wire protocols exactly once —
//! supported, or refused with its reason. The table is consulted by two
//! production callers: `crate::provider::translation_available`, through
//! which `harness::pairing::protocol_fit` classes a pairing as translated,
//! and `super::ingress`, which answers a target the provider does not
//! serve either by translating it or with a `404` whose body names the
//! refused pair and the table's reason.
//!
//! # The relay rule, narrowed and not repealed
//!
//! A request whose target belongs to a protocol the provider serves is
//! relayed byte for byte, exactly as before this module existed, and never
//! enters a codec — `place` is asked only from the branch that used to
//! answer `404`, and refuses a served protocol a second time on its own
//! account. Only an unserved target with a supported pair is translated.
//! Parsing is bounded ([`MAX_BODY_BYTES`], [`stream::MAX_EVENT_BYTES`]);
//! streaming stays streaming, one event translated and flushed at a time;
//! and nothing is guessed from a body's shape, because the target decided
//! the protocol before a byte of the body was read.
//!
//! # Refused by name, never dropped
//!
//! A field a codec cannot carry is a [`TranslationRefusal`] naming the pair,
//! the field and the reason, sent to the harness as a `4xx` in its own
//! protocol's error shape **before anything is opened upstream**. There is
//! no path through this module that drops a field silently: the decoders
//! refuse unknown keys, and the handful of response fields they ignore are
//! listed by name so the table can show them.
//!
//! # Structurally not a harness
//!
//! This directory keeps `gateway/`'s rule: no file here names
//! `crate::harness`, so the table is keyed by protocol **slug** — the same
//! spelling `WireProtocol::slug` produces and [`super::upstream::Route`]
//! already carries — and `crate::provider` is the caller that turns a
//! `WireProtocol` into one.

pub mod canonical;
pub mod stream;

mod anthropic;
mod openai_chat;
mod openai_responses;

use std::io::{BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};

use ureq::http::{HeaderValue, Request as HttpRequest, StatusCode, header};
use ureq::{Agent, SendBody};

use crate::provider::telemetry::RateLimitHeaders;

use super::http::{self, RequestHead};
use super::ingress::{Exchange, Framing, Outcome, StreamEnd, Tokens, transport_detail};
use super::upstream::{Route, Upstream, UpstreamBackend, VERSION_SEGMENT, path_of};
use canonical::{Request, Response, StreamEvent, Unsupported};
use stream::{SseEvent, SseReader};

pub use openai_chat::TOOL_ERROR_MARKER;

/// The largest request or response document the translator will hold whole.
///
/// The relay beside this module holds nothing and so bounds nothing; a codec
/// has to hold one document to translate it, and 32 MiB is Anthropic's own
/// request limit.
pub const MAX_BODY_BYTES: u64 = 32 * 1024 * 1024;

/// The three wire protocols, by slug, in the gateway's own order.
pub const PROTOCOLS: [&str; 3] = ["anthropic-messages", "openai-responses", "openai-chat"];

/// Whether an ordered pair is offered, and if not, why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairStatus {
    /// Both codecs exist and the pair's end-to-end test is green — the only
    /// way a row becomes supported (capability map line 1956).
    Supported,
    Refused(&'static str),
}

/// One ordered pair of wire protocols: the harness's protocol, the
/// provider's, and whether the gateway translates between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pair {
    /// The protocol the harness speaks — the request target's.
    pub from: &'static str,
    /// The protocol the provider serves.
    pub to: &'static str,
    pub status: PairStatus,
}

impl Pair {
    pub fn is_supported(&self) -> bool {
        matches!(self.status, PairStatus::Supported)
    }

    /// `from->to`, the spelling a diagnostic and the evidence ledger's
    /// `route` column carry.
    pub fn slug(&self) -> String {
        format!("{}->{}", self.from, self.to)
    }

    /// The reason a refused pair is refused, or `None` when it is supported.
    pub fn refusal(&self) -> Option<&'static str> {
        match self.status {
            PairStatus::Supported => None,
            PairStatus::Refused(reason) => Some(reason),
        }
    }
}

const NOT_YET_REVERSE: &str = "not yet: both codecs exist, but the pair has no end-to-end test through the shipped \
     binary against a fixture upstream, and no pair is offered before its test (1956)";
const SAME_PROTOCOL: &str =
    "same protocol: the relay carries it byte for byte and no codec is entered";

/// The API version header `api.anthropic.com` requires on every request —
/// the same value real clients send and the relay path already forwards
/// verbatim (`gateway/mod.rs`'s fixture tests pin it). A translated request
/// has no client header to relay this from, so [`serve`] states it itself,
/// and only toward an Anthropic-serving outbound protocol (T2 finding 2).
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// The pair table. Every ordered pair of [`PROTOCOLS`], including each
/// protocol with itself, exactly once — `every_ordered_pair_appears_exactly_once`
/// holds it to that, and `crate::provider`'s own test holds it against
/// `WireProtocol`, which this file may not name.
const TABLE: [Pair; 9] = [
    Pair {
        from: "anthropic-messages",
        to: "anthropic-messages",
        status: PairStatus::Refused(SAME_PROTOCOL),
    },
    // T1: Claude Code served by an OpenAI-Chat entitlement — OpenRouter and
    // every OpenAI-compatible key. The first pair, and the end-to-end test
    // in `tests/gateway_translate.rs` is what lets this row say Supported.
    Pair {
        from: "anthropic-messages",
        to: "openai-chat",
        status: PairStatus::Supported,
    },
    // T2: Claude Code served by an OpenAI-Responses entitlement — a
    // ChatGPT/Codex-plan-shaped upstream. Supported only because its own
    // end-to-end test exists: `tests/gateway_translate_responses.rs`,
    // `a_claude_code_request_is_translated_to_openai_responses_and_back_with_ids_preserved`.
    Pair {
        from: "anthropic-messages",
        to: "openai-responses",
        status: PairStatus::Supported,
    },
    Pair {
        from: "openai-chat",
        to: "anthropic-messages",
        status: PairStatus::Refused(NOT_YET_REVERSE),
    },
    Pair {
        from: "openai-chat",
        to: "openai-chat",
        status: PairStatus::Refused(SAME_PROTOCOL),
    },
    // T2b: an OpenCode-shaped (openai-chat) client served by an
    // OpenAI-Responses entitlement — a ChatGPT/Codex-plan-shaped upstream.
    // Supported only because its own end-to-end test exists:
    // `tests/gateway_translate_t2b.rs`,
    // `an_opencode_request_is_translated_to_openai_responses_and_back_with_tool_call_ids_preserved`.
    Pair {
        from: "openai-chat",
        to: "openai-responses",
        status: PairStatus::Supported,
    },
    // T2's mirror: a Codex-shaped client served by an Anthropic Messages
    // entitlement. Supported only because its own end-to-end test exists:
    // `tests/gateway_translate_responses.rs`,
    // `a_codex_request_is_translated_to_anthropic_messages_and_back_with_ids_preserved`.
    Pair {
        from: "openai-responses",
        to: "anthropic-messages",
        status: PairStatus::Supported,
    },
    // T2b's mirror: a Codex-shaped (openai-responses) client served by an
    // OpenAI-Chat entitlement. Supported only because its own end-to-end
    // test exists: `tests/gateway_translate_t2b.rs`,
    // `a_codex_shaped_request_is_translated_to_openai_chat_and_back_with_tool_call_ids_preserved`.
    Pair {
        from: "openai-responses",
        to: "openai-chat",
        status: PairStatus::Supported,
    },
    Pair {
        from: "openai-responses",
        to: "openai-responses",
        status: PairStatus::Refused(SAME_PROTOCOL),
    },
];

/// Every ordered pair, for a later CLI view. **No production caller reads
/// this enumeration yet**; the two production consumers of the table go
/// through [`lookup`].
pub fn pairs() -> &'static [Pair] {
    &TABLE
}

/// The row for `from -> to`, or `None` when either slug is not a wire
/// protocol this gateway knows.
pub fn lookup(from: &str, to: &str) -> Option<&'static Pair> {
    TABLE.iter().find(|pair| pair.from == from && pair.to == to)
}

/// Whether `from -> to` is a supported pair. The one function
/// `crate::provider::translation_available` calls.
pub fn is_supported(from: &str, to: &str) -> bool {
    lookup(from, to).is_some_and(Pair::is_supported)
}

/// The per-field rows of one codec — what it refuses, with reasons, and
/// what it ignores by name in a response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldRows {
    pub refused: &'static [(&'static str, &'static str)],
    pub ignored: &'static [&'static str],
}

/// The per-field rows for `protocol`'s codec, or `None` for a protocol with
/// no codec.
pub fn field_rows(protocol: &str) -> Option<FieldRows> {
    codec_for(protocol).map(|codec| FieldRows {
        refused: codec.refused_fields(),
        ignored: codec.ignored_fields(),
    })
}

/// A request this pair cannot carry, named: the pair, the field in the
/// wire's own spelling, and one sentence a user can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationRefusal {
    pub pair: String,
    pub field: String,
    pub reason: String,
}

impl TranslationRefusal {
    fn new(pair: &Pair, unsupported: Unsupported) -> Self {
        Self {
            pair: pair.slug(),
            field: unsupported.field,
            reason: unsupported.reason,
        }
    }
}

impl std::fmt::Display for TranslationRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the Glasshouse gateway cannot translate this request for the pair {}: `{}` — {}",
            self.pair, self.field, self.reason
        )
    }
}

// --- codecs ---------------------------------------------------------------------

/// One wire protocol's codec.
pub(super) trait Codec: Sync {
    fn protocol(&self) -> &'static str;
    /// The one request target this codec translates, version segment
    /// stripped — the path a client of this protocol posts an inference
    /// request to, and the path the gateway posts to a provider of it.
    fn endpoint(&self) -> &'static str;
    /// What this codec cannot encode out of the canonical form, refused by
    /// name before anything is opened upstream.
    ///
    /// The canonical form was built from fields both T1 wires carry, so the
    /// default refuses nothing — but [`Codec::encode_request`] is
    /// infallible, and a codec whose wire has no home for a canonical field
    /// must refuse it here rather than drop it there. OpenAI Responses is
    /// the first such wire: it has no stop-sequence parameter.
    fn refuse_unencodable(&self, _request: &Request) -> Result<(), Unsupported> {
        Ok(())
    }
    fn decode_request(&self, body: &[u8]) -> Result<Request, Unsupported>;
    fn encode_request(&self, request: &Request) -> Vec<u8>;
    fn decode_response(&self, body: &[u8]) -> Result<Response, Unsupported>;
    fn encode_response(&self, response: &Response) -> Vec<u8>;
    fn stream_decoder(&self) -> Box<dyn StreamDecoder + Send>;
    fn stream_encoder(&self) -> Box<dyn StreamEncoder + Send>;
    /// This protocol's error `type` for an HTTP status.
    fn error_kind(&self, status: u16) -> &'static str;
    /// An error document in this protocol's shape.
    fn encode_error(&self, kind: &str, message: &str) -> Vec<u8>;
    /// An error event in this protocol's stream shape.
    fn encode_stream_error(&self, kind: &str, message: &str) -> Vec<u8>;
    /// The message out of an error document in this protocol's shape.
    fn decode_error(&self, body: &[u8]) -> Option<String>;
    fn refused_fields(&self) -> &'static [(&'static str, &'static str)];
    fn ignored_fields(&self) -> &'static [&'static str];
}

/// Turns one wire's stream events into canonical events, as they arrive.
pub(super) trait StreamDecoder {
    fn feed(&mut self, event: &SseEvent) -> Result<Vec<StreamEvent>, Unsupported>;
    /// The stream ended cleanly; whatever closes the message.
    fn finish(&mut self) -> Result<Vec<StreamEvent>, Unsupported>;
    fn is_done(&self) -> bool;
}

/// Turns canonical events into one wire's stream bytes.
pub(super) trait StreamEncoder {
    fn encode(&mut self, event: &StreamEvent) -> Vec<u8>;
}

const CODECS: [&dyn Codec; 3] = [
    &anthropic::Anthropic,
    &openai_chat::OpenAiChat,
    &openai_responses::OpenAiResponses,
];

fn codec_for(protocol: &str) -> Option<&'static dyn Codec> {
    CODECS
        .iter()
        .copied()
        .find(|codec| codec.protocol() == protocol)
}

/// The target a translated request is posted to at a provider of `codec`'s
/// protocol: the exact path that protocol's own native client sends, because
/// every provider's declared base URL is composed for that client.
///
/// Claude Code sends `POST /v1/messages` and the Anthropic-serving base URLs
/// carry no `/v1`; Codex sends `POST /responses` and OpenAI-Chat clients
/// `POST /chat/completions` against base URLs that already carry it (see the
/// provider templates and `profile::ingress_targets`, each read off real
/// request lines). Composing `base + endpoint()` alone would mis-address an
/// Anthropic-serving provider — `…/api/messages` instead of
/// `…/api/v1/messages` — which the T2 mirror pair was the first to reach.
fn outbound_target(codec: &dyn Codec) -> String {
    if codec.protocol() == anthropic::PROTOCOL {
        format!("{VERSION_SEGMENT}{}", codec.endpoint())
    } else {
        codec.endpoint().to_owned()
    }
}

// --- placement ------------------------------------------------------------------

/// What the ingress does with a target the provider does not serve.
#[derive(Debug)]
pub(super) enum Placement {
    /// Translate it through this supported pair.
    Translate(&'static Pair),
    /// The target belongs to a codec's protocol, but every pair to a served
    /// protocol is refused — answered with the pairs and their reasons.
    PairRefused {
        from: &'static str,
        refused: Vec<&'static Pair>,
    },
    /// The target lies under a codec's protocol but is not the one endpoint
    /// that codec translates.
    TargetRefused { from: &'static str },
    /// Not a target any codec claims: the plain `404` stays.
    Unplaceable,
}

/// Decide, from the target alone, whether an unserved request is translated.
///
/// `served` is the serving backend's protocol slugs. A target under a served
/// protocol is [`Placement::Unplaceable`] here even if a codec claims it —
/// the caller's route lookup owns served targets, and this is the second
/// lock on the byte-for-byte rule.
pub(super) fn place(target: &str, served: &[&str]) -> Placement {
    let path = path_of(target);
    let path = match path.strip_prefix(VERSION_SEGMENT) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => rest,
        _ => path,
    };
    let Some(codec) = CODECS.iter().copied().find(|codec| {
        path == codec.endpoint()
            || (path.starts_with(codec.endpoint())
                && path[codec.endpoint().len()..].starts_with('/'))
    }) else {
        return Placement::Unplaceable;
    };
    let from = codec.protocol();
    if served.contains(&from) {
        return Placement::Unplaceable;
    }
    if path != codec.endpoint() {
        return Placement::TargetRefused { from };
    }
    let mut refused = Vec::new();
    for to in served {
        match lookup(from, to) {
            Some(pair) if pair.is_supported() => return Placement::Translate(pair),
            Some(pair) => refused.push(pair),
            None => {}
        }
    }
    Placement::PairRefused { from, refused }
}

/// The `404` body for a target whose every pair is refused: the pairs by
/// name and the table's reason for each. Built from the table's own text and
/// the protocol slugs; nothing from the request.
pub(super) fn pair_refusal_message(from: &str, refused: &[&Pair]) -> String {
    if refused.is_empty() {
        return format!(
            "this request speaks {from}, which the configured provider does not serve, and no \
             protocol it does serve is one the Glasshouse gateway translates {from} to"
        );
    }
    let pairs = refused
        .iter()
        .map(|pair| format!("{} ({})", pair.slug(), pair.refusal().unwrap_or("refused")))
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "this request speaks {from}, which the configured provider does not serve, and the \
         Glasshouse gateway refuses the translation by name: {pairs}"
    )
}

/// The `404` body for a target under a translated protocol that is not the
/// one endpoint translated.
pub(super) fn target_refusal_message(from: &str) -> String {
    let endpoint = codec_for(from).map(Codec::endpoint).unwrap_or("");
    format!(
        "this request speaks {from}, which the configured provider does not serve, and only its \
         `{endpoint}` endpoint is translated; the requested endpoint has no equivalent on the \
         provider's protocol"
    )
}

// --- the pipeline -----------------------------------------------------------------

/// Serve one request the ingress placed for translation.
///
/// Everything the relay does not do happens here and only here: the body is
/// read whole (bounded), decoded by the harness's codec, encoded by the
/// provider's, sent with the provider's credential exactly as the relay
/// attaches it, and the answer is decoded by the provider's codec and
/// encoded by the harness's — as a document, or event by event as a stream.
/// A refusal at any point before the upstream request is built answers the
/// harness with **nothing opened upstream**.
#[allow(clippy::too_many_arguments)]
pub(super) fn serve(
    head: RequestHead,
    mut reader: BufReader<TcpStream>,
    out: &mut TcpStream,
    upstream: &Upstream,
    serving: &UpstreamBackend,
    agent: &Agent,
    pair: &'static Pair,
) -> (Exchange, RateLimitHeaders) {
    let from = codec_for(pair.from).expect("a supported pair has a codec on its harness side");
    let to = codec_for(pair.to).expect("a supported pair has a codec on its provider side");
    let route = serving
        .route_named(pair.to)
        .expect("a pair is placed only against a protocol the serving backend routes");

    // Written with the socket left open: every caller below either drains
    // the rest of the client's body through `settle` (which closes once it
    // has drained) or has already consumed the whole body itself, so a
    // shutdown here would only race the drain — killing the read half the
    // client is still writing into and turning the refusal `settle` exists
    // to deliver into the network error it exists to prevent.
    let refuse = |out: &mut TcpStream, status: StatusCode, message: &str| {
        let body = from.encode_error(from.error_kind(status.as_u16()), message);
        let _ = write_document_open(out, status, &body);
    };

    if head.method != ureq::http::Method::POST {
        refuse(
            out,
            StatusCode::METHOD_NOT_ALLOWED,
            "only a POST is translated; the provider's protocol has no equivalent for this method",
        );
        settle(&mut reader, out, head.content_length);
        return (
            exchange(Outcome::Declined, 405, upstream, pair, route),
            RateLimitHeaders::default(),
        );
    }
    let Some(length) = head.content_length else {
        refuse(
            out,
            StatusCode::BAD_REQUEST,
            "a request to translate must carry a body framed with content-length",
        );
        settle(&mut reader, out, None);
        return (
            exchange(Outcome::Declined, 400, upstream, pair, route),
            RateLimitHeaders::default(),
        );
    };
    if length > MAX_BODY_BYTES {
        refuse(
            out,
            StatusCode::PAYLOAD_TOO_LARGE,
            "the request body exceeds the size the Glasshouse gateway will translate",
        );
        settle(&mut reader, out, Some(length));
        return (
            exchange(Outcome::Declined, 413, upstream, pair, route),
            RateLimitHeaders::default(),
        );
    }
    // Reserved for what a real request looks like, not for what this one
    // declared: `take` below bounds the result either way, and a
    // declaration is not a delivery. 64 KiB covers a Claude Code request
    // head-on and the vector grows for anything larger.
    let mut body = Vec::with_capacity(length.min(64 * 1024) as usize);
    if (&mut reader).take(length).read_to_end(&mut body).is_err() || body.len() as u64 != length {
        return (
            exchange(Outcome::ClientGone, 0, upstream, pair, route),
            RateLimitHeaders::default(),
        );
    }

    // Decode on the harness's codec, then let the provider's codec refuse,
    // by name, any canonical field its wire has no home for — both before
    // anything is opened upstream.
    let request = match from.decode_request(&body).and_then(|request| {
        to.refuse_unencodable(&request)?;
        Ok(request)
    }) {
        Ok(request) => request,
        Err(unsupported) => {
            let refusal = TranslationRefusal::new(pair, unsupported);
            refuse(out, StatusCode::BAD_REQUEST, &refusal.to_string());
            let _ = out.shutdown(Shutdown::Both);
            return (
                exchange(Outcome::Declined, 400, upstream, pair, route),
                RateLimitHeaders::default(),
            );
        }
    };
    let translated = to.encode_request(&request);

    let Some(uri) = route.uri_for(&outbound_target(to)) else {
        refuse(
            out,
            StatusCode::BAD_REQUEST,
            "the translated request could not be addressed to the configured provider",
        );
        return (
            exchange(Outcome::Declined, 400, upstream, pair, route),
            RateLimitHeaders::default(),
        );
    };
    let mut outbound = HttpRequest::builder()
        .method(ureq::http::Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::ACCEPT,
            if request.stream {
                "text/event-stream"
            } else {
                "application/json"
            },
        )
        .header(header::CONTENT_LENGTH, HeaderValue::from(translated.len()));
    if let Some(agent_name) = head.headers.get(header::USER_AGENT) {
        outbound = outbound.header(header::USER_AGENT, agent_name.clone());
    }
    // The one credential, attached exactly where the relay attaches it.
    outbound = outbound.header(header::AUTHORIZATION, serving.authorization());
    // api.anthropic.com requires this header; no client header exists to
    // relay it from on a translated request, so it is stated here, and only
    // toward an Anthropic-serving outbound protocol (T2 finding 2).
    if to.protocol() == anthropic::PROTOCOL {
        outbound = outbound.header("anthropic-version", ANTHROPIC_VERSION);
    }
    let Ok(outbound) = outbound.body(SendBody::from_owned_reader(std::io::Cursor::new(
        translated,
    ))) else {
        refuse(
            out,
            StatusCode::BAD_REQUEST,
            "the translated request could not be built for the configured provider",
        );
        return (
            exchange(Outcome::Declined, 400, upstream, pair, route),
            RateLimitHeaders::default(),
        );
    };

    let response = match agent.run(outbound) {
        Ok(response) => response,
        Err(err) => {
            let detail = transport_detail(&err);
            refuse(
                out,
                StatusCode::BAD_GATEWAY,
                "the Glasshouse gateway could not reach the configured provider",
            );
            return (
                exchange(Outcome::Unreachable { detail }, 502, upstream, pair, route),
                RateLimitHeaders::default(),
            );
        }
    };
    let first_byte_at = Some(crate::provider::cache::now_unix_seconds());
    let (parts, mut body) = response.into_parts();
    let status = parts.status;
    let quota = RateLimitHeaders::read(
        parts
            .headers
            .iter()
            .filter_map(|(name, value)| Some((name.as_str(), value.to_str().ok()?))),
    );
    let is_event_stream = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim_start().starts_with("text/event-stream"));
    let upstream_status = status.as_u16();

    let finish = |outcome: Outcome, status: u16, framing: Framing, tokens: Option<Tokens>| {
        (
            Exchange {
                first_byte_at,
                framing: Some(framing),
                tokens,
                ..exchange(outcome, status, upstream, pair, route)
            },
            quota.clone(),
        )
    };

    // A provider error, in the harness's own error shape and with the
    // provider's own status: the status is what routing reads, and the
    // message is what the harness needed to show.
    if !status.is_success() {
        let raw = body
            .with_config()
            .limit(MAX_BODY_BYTES)
            .read_to_vec()
            .unwrap_or_default();
        let message = to
            .decode_error(&raw)
            .unwrap_or_else(|| String::from_utf8_lossy(&raw).into_owned());
        let document = from.encode_error(from.error_kind(upstream_status), &message);
        let written = document.len() as u64;
        let ended = match write_document(out, status, &document) {
            Ok(()) => StreamEnd::Complete,
            Err(_) => StreamEnd::ClientClosed,
        };
        let outcome = if ended == StreamEnd::ClientClosed {
            Outcome::ClientGone
        } else {
            Outcome::Forwarded {
                upstream_status,
                bytes: written,
            }
        };
        return finish(
            outcome,
            upstream_status,
            Framing {
                declared: None,
                relayed: Some(written),
                ended,
            },
            None,
        );
    }

    if is_event_stream {
        let mut events = SseReader::new(BufReader::new(body.as_reader()));
        let mut decoder = to.stream_decoder();
        if request.stream {
            return stream_events(
                out,
                &mut events,
                decoder.as_mut(),
                from,
                &finish,
                upstream_status,
            );
        }
        // The harness asked for a document and the provider streamed anyway:
        // gather the stream into the document it delivered.
        let mut gathered = Vec::new();
        // One event is bounded by `stream::MAX_EVENT_BYTES`; the number of
        // events accumulated here is not, and this is the one branch that
        // holds them all rather than writing each one out as it arrives.
        let mut held = 0u64;
        loop {
            match events.next_event() {
                Ok(Some(event)) => {
                    held += event.data.len() as u64;
                    if held > MAX_BODY_BYTES {
                        return untranslatable(
                            out,
                            from,
                            pair,
                            Unsupported::new(
                                "body",
                                "the provider's stream exceeded the size the Glasshouse \
                                 gateway will hold to answer a request that did not ask for \
                                 a stream",
                            ),
                            &finish,
                        );
                    }
                    match decoder.feed(&event) {
                        Ok(more) => gathered.extend(more),
                        Err(unsupported) => {
                            return untranslatable(out, from, pair, unsupported, &finish);
                        }
                    }
                }
                Ok(None) => match decoder.finish() {
                    Ok(more) => {
                        gathered.extend(more);
                        break;
                    }
                    Err(unsupported) => {
                        return untranslatable(out, from, pair, unsupported, &finish);
                    }
                },
                Err(_) => {
                    return untranslatable(
                        out,
                        from,
                        pair,
                        Unsupported::new("stream", "the provider's stream failed"),
                        &finish,
                    );
                }
            }
            if decoder.is_done() {
                break;
            }
        }
        return match canonical::accumulate(&gathered) {
            Ok(response) => deliver_document(out, from, &response, &finish, upstream_status),
            Err(unsupported) => untranslatable(out, from, pair, unsupported, &finish),
        };
    }

    let raw = match body.with_config().limit(MAX_BODY_BYTES).read_to_vec() {
        Ok(raw) => raw,
        Err(_) => {
            return untranslatable(
                out,
                from,
                pair,
                Unsupported::new("body", "the provider's response could not be read whole"),
                &finish,
            );
        }
    };
    let response = match to.decode_response(&raw) {
        Ok(response) => response,
        Err(unsupported) => return untranslatable(out, from, pair, unsupported, &finish),
    };
    if request.stream {
        // The harness asked for a stream and the provider answered with a
        // document: deliver it as the event sequence it would have streamed.
        let events = response.as_events();
        let mut encoder = from.stream_encoder();
        let mut written = 0u64;
        if write_stream_head(out).is_err() {
            return finish(
                Outcome::ClientGone,
                upstream_status,
                Framing {
                    declared: None,
                    relayed: Some(0),
                    ended: StreamEnd::ClientClosed,
                },
                None,
            );
        }
        for event in &events {
            let bytes = encoder.encode(event);
            if write_chunk(out, &bytes).is_err() {
                return finish(
                    Outcome::ClientGone,
                    upstream_status,
                    Framing {
                        declared: None,
                        relayed: Some(written),
                        ended: StreamEnd::ClientClosed,
                    },
                    None,
                );
            }
            written += bytes.len() as u64;
        }
        let _ = out.write_all(b"0\r\n\r\n");
        let _ = out.flush();
        let _ = out.shutdown(Shutdown::Both);
        return finish(
            Outcome::Forwarded {
                upstream_status,
                bytes: written,
            },
            upstream_status,
            Framing {
                declared: None,
                relayed: Some(written),
                ended: StreamEnd::Complete,
            },
            Some(tokens_of(&response)),
        );
    }
    deliver_document(out, from, &response, &finish, upstream_status)
}

type Finish<'a> = &'a dyn Fn(Outcome, u16, Framing, Option<Tokens>) -> (Exchange, RateLimitHeaders);

fn tokens_of(response: &Response) -> Tokens {
    Tokens {
        input: response.usage.input,
        output: response.usage.output,
        cached: response.usage.cached,
    }
}

/// A translated document, written whole.
fn deliver_document(
    out: &mut TcpStream,
    from: &dyn Codec,
    response: &Response,
    finish: Finish<'_>,
    upstream_status: u16,
) -> (Exchange, RateLimitHeaders) {
    let document = from.encode_response(response);
    let written = document.len() as u64;
    match write_document(out, StatusCode::OK, &document) {
        Ok(()) => finish(
            Outcome::Forwarded {
                upstream_status,
                bytes: written,
            },
            200,
            Framing {
                declared: None,
                relayed: Some(written),
                ended: StreamEnd::Complete,
            },
            Some(tokens_of(response)),
        ),
        Err(_) => finish(
            Outcome::ClientGone,
            200,
            Framing {
                declared: None,
                relayed: Some(0),
                ended: StreamEnd::ClientClosed,
            },
            None,
        ),
    }
}

/// The provider answered, and its answer is not one the pair can carry.
///
/// A `502` rather than a `4xx`: the harness's request was fine, and what
/// cannot be translated is the provider's side. Nothing of the provider's
/// body reaches the harness — the refusal names the field.
fn untranslatable(
    out: &mut TcpStream,
    from: &dyn Codec,
    pair: &Pair,
    unsupported: Unsupported,
    finish: Finish<'_>,
) -> (Exchange, RateLimitHeaders) {
    let refusal = TranslationRefusal::new(pair, unsupported);
    let message = format!("the provider's answer could not be translated — {refusal}");
    let document = from.encode_error(from.error_kind(502), &message);
    let written = document.len() as u64;
    let ended = match write_document(out, StatusCode::BAD_GATEWAY, &document) {
        Ok(()) => StreamEnd::Complete,
        Err(_) => StreamEnd::ClientClosed,
    };
    finish(
        Outcome::Declined,
        502,
        Framing {
            declared: None,
            relayed: Some(written),
            ended,
        },
        None,
    )
}

/// Translate a provider's stream to the harness, one event at a time.
fn stream_events<R: Read>(
    out: &mut TcpStream,
    events: &mut SseReader<BufReader<R>>,
    decoder: &mut dyn StreamDecoder,
    from: &dyn Codec,
    finish: Finish<'_>,
    upstream_status: u16,
) -> (Exchange, RateLimitHeaders) {
    let mut encoder = from.stream_encoder();
    let mut written = 0u64;
    let mut usage = None;
    let mut order = canonical::Order::default();
    let client_gone = |written: u64| {
        finish(
            Outcome::ClientGone,
            upstream_status,
            Framing {
                declared: None,
                relayed: Some(written),
                ended: StreamEnd::ClientClosed,
            },
            None,
        )
    };
    if write_stream_head(out).is_err() {
        return client_gone(0);
    }
    let mut ended = StreamEnd::Complete;
    loop {
        let translated = match events.next_event() {
            Ok(Some(event)) => decoder.feed(&event),
            Ok(None) => decoder.finish(),
            Err(_) => Err(Unsupported::new(
                "stream",
                "the provider's stream failed before it finished",
            )),
        };
        let at_end = decoder.is_done();
        // The encoders write bytes and cannot refuse, so the one place an
        // out-of-order provider stream can still be refused by name is here,
        // before a delta is handed to an encoder that would attach it to
        // whichever block is open — see `canonical::Order`.
        let translated = translated.and_then(|events| {
            for event in &events {
                order.check(event)?;
            }
            Ok(events)
        });
        match translated {
            Ok(canonical_events) => {
                for event in &canonical_events {
                    if let StreamEvent::MessageDelta {
                        usage: final_usage, ..
                    } = event
                    {
                        usage = Some(Tokens {
                            input: final_usage.input,
                            output: final_usage.output,
                            cached: final_usage.cached,
                        });
                    }
                    let bytes = encoder.encode(event);
                    if bytes.is_empty() {
                        continue;
                    }
                    if write_chunk(out, &bytes).is_err() {
                        return client_gone(written);
                    }
                    written += bytes.len() as u64;
                }
            }
            Err(unsupported) => {
                // The head has been sent: the only channel left is the
                // stream itself, so the refusal goes down it, by name, and
                // the stream ends.
                let message =
                    format!("the provider's stream could not be translated: {unsupported}");
                let bytes = from.encode_stream_error(from.error_kind(502), &message);
                if write_chunk(out, &bytes).is_err() {
                    return client_gone(written);
                }
                written += bytes.len() as u64;
                ended = StreamEnd::Aborted;
                break;
            }
        }
        if at_end {
            break;
        }
    }
    let _ = out.write_all(b"0\r\n\r\n");
    let _ = out.flush();
    let _ = out.shutdown(Shutdown::Both);
    finish(
        Outcome::Forwarded {
            upstream_status,
            bytes: written,
        },
        upstream_status,
        Framing {
            declared: None,
            relayed: Some(written),
            ended,
        },
        if ended == StreamEnd::Complete {
            usage
        } else {
            None
        },
    )
}

/// One document, written with the connection left open: for a refusal that
/// still owes the client a drain of whatever it is still sending — `out` is
/// a `try_clone` of the same socket as the reader doing that draining, so a
/// shutdown here would close the read half out from under it too.
fn write_document_open(
    out: &mut TcpStream,
    status: StatusCode,
    body: &[u8],
) -> std::io::Result<()> {
    let headers = vec![
        ("content-type".to_owned(), b"application/json".to_vec()),
        (
            "content-length".to_owned(),
            body.len().to_string().into_bytes(),
        ),
        ("connection".to_owned(), b"close".to_vec()),
    ];
    http::write_head(out, status, &headers)?;
    out.write_all(body)?;
    out.flush()
}

/// One document, and the connection closed after it: for every answer that
/// is not followed by a drain of the client's own socket.
fn write_document(out: &mut TcpStream, status: StatusCode, body: &[u8]) -> std::io::Result<()> {
    write_document_open(out, status, body)?;
    let _ = out.shutdown(Shutdown::Both);
    Ok(())
}

fn write_stream_head(out: &mut TcpStream) -> std::io::Result<()> {
    let headers = vec![
        (
            "content-type".to_owned(),
            b"text/event-stream; charset=utf-8".to_vec(),
        ),
        ("cache-control".to_owned(), b"no-cache".to_vec()),
        ("transfer-encoding".to_owned(), b"chunked".to_vec()),
        ("connection".to_owned(), b"close".to_vec()),
    ];
    http::write_head(out, StatusCode::OK, &headers)
}

/// One HTTP chunk, written and flushed at once — the same one-write rule
/// `super::http::pump` keeps, for the same reason.
fn write_chunk(out: &mut TcpStream, bytes: &[u8]) -> std::io::Result<()> {
    let mut framed = format!("{:x}\r\n", bytes.len()).into_bytes();
    framed.extend_from_slice(bytes);
    framed.extend_from_slice(b"\r\n");
    out.write_all(&framed)?;
    out.flush()
}

/// Drain what the client is still sending, then close — `ingress::settle`,
/// for the refusals written here.
fn settle(reader: &mut BufReader<TcpStream>, out: &mut TcpStream, content_length: Option<u64>) {
    super::ingress::settle(reader, out, content_length);
}

fn exchange(
    outcome: Outcome,
    status: u16,
    upstream: &Upstream,
    pair: &Pair,
    route: &Route,
) -> Exchange {
    Exchange {
        outcome,
        status,
        provider: upstream.provider().to_owned(),
        protocol: Some(pair.slug()),
        host: route.host(),
        first_byte_at: None,
        framing: None,
        tokens: None,
    }
}

// --- field access -------------------------------------------------------------------

/// Strict, path-carrying access to a JSON object, for the decoders.
///
/// Every key is taken out as it is read; what is left at [`Fields::finish`]
/// is a key nobody looked at, and that is a refusal naming it. The path is
/// in the wire's own spelling — `messages[2].content[0].cache_control` — so
/// a refusal points at the exact field.
pub(super) mod fields {
    use serde_json::{Map, Value};

    use super::canonical::{Unsupported, json_kind};

    pub(crate) struct Fields {
        path: String,
        map: Map<String, Value>,
    }

    /// `path[index]`.
    pub(crate) fn element(path: &str, index: usize) -> String {
        format!("{path}[{index}]")
    }

    impl Fields {
        pub(crate) fn of(value: Value, path: impl Into<String>) -> Result<Self, Unsupported> {
            let path = path.into();
            match value {
                Value::Object(map) => Ok(Self { path, map }),
                other => Err(Unsupported::new(
                    if path.is_empty() {
                        "body".to_owned()
                    } else {
                        path
                    },
                    format!("expected a JSON object, not {}", json_kind(&other)),
                )),
            }
        }

        pub(crate) fn path(&self) -> &str {
            &self.path
        }

        /// The path of `key` under this object.
        pub(crate) fn at(&self, key: &str) -> String {
            if self.path.is_empty() {
                key.to_owned()
            } else {
                format!("{}.{key}", self.path)
            }
        }

        pub(crate) fn take(&mut self, key: &str) -> Option<Value> {
            self.map.remove(key)
        }

        pub(crate) fn take_string(&mut self, key: &str) -> Result<Option<String>, Unsupported> {
            match self.take(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(text)) => Ok(Some(text)),
                Some(other) => Err(self.wrong(key, "a string", &other)),
            }
        }

        pub(crate) fn require_string(&mut self, key: &str) -> Result<String, Unsupported> {
            self.take_string(key)?
                .ok_or_else(|| Unsupported::new(self.at(key), "this field is required"))
        }

        pub(crate) fn take_u64(&mut self, key: &str) -> Result<Option<u64>, Unsupported> {
            match self.take(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Number(number)) if number.as_u64().is_some() => Ok(number.as_u64()),
                Some(other) => Err(self.wrong(key, "a non-negative integer", &other)),
            }
        }

        pub(crate) fn take_f64(&mut self, key: &str) -> Result<Option<f64>, Unsupported> {
            match self.take(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Number(number)) if number.as_f64().is_some() => Ok(number.as_f64()),
                Some(other) => Err(self.wrong(key, "a number", &other)),
            }
        }

        pub(crate) fn take_bool(&mut self, key: &str) -> Result<Option<bool>, Unsupported> {
            match self.take(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Bool(flag)) => Ok(Some(flag)),
                Some(other) => Err(self.wrong(key, "a boolean", &other)),
            }
        }

        pub(crate) fn take_array(&mut self, key: &str) -> Result<Option<Vec<Value>>, Unsupported> {
            match self.take(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Array(items)) => Ok(Some(items)),
                Some(other) => Err(self.wrong(key, "an array", &other)),
            }
        }

        pub(crate) fn take_object(&mut self, key: &str) -> Result<Option<Fields>, Unsupported> {
            match self.take(key) {
                None | Some(Value::Null) => Ok(None),
                Some(object @ Value::Object(_)) => Ok(Some(Fields::of(object, self.at(key))?)),
                Some(other) => Err(self.wrong(key, "an object", &other)),
            }
        }

        /// Refuse, by name, a key that is present — whatever its value.
        pub(crate) fn refuse_if_present(
            &mut self,
            key: &str,
            reason: &str,
        ) -> Result<(), Unsupported> {
            match self.take(key) {
                None => Ok(()),
                Some(_) => Err(Unsupported::new(self.at(key), reason)),
            }
        }

        /// Drop a key on purpose. Every call site is a named decision, and
        /// the codec's `IGNORED_FIELDS` lists it.
        pub(crate) fn ignore(&mut self, key: &str) {
            self.map.remove(key);
        }

        /// Refuse whatever nobody read, by name.
        pub(crate) fn finish(self) -> Result<(), Unsupported> {
            let mut left: Vec<&String> = self.map.keys().collect();
            left.sort();
            match left.first() {
                None => Ok(()),
                Some(key) => Err(Unsupported::new(
                    self.at(key),
                    "this field is not one the codec carries, and nothing is dropped silently",
                )),
            }
        }

        fn wrong(&self, key: &str, expected: &str, got: &Value) -> Unsupported {
            Unsupported::new(
                self.at(key),
                format!("expected {expected}, not {}", json_kind(got)),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ordered_pair_appears_exactly_once() {
        for from in PROTOCOLS {
            for to in PROTOCOLS {
                let rows = TABLE
                    .iter()
                    .filter(|pair| pair.from == from && pair.to == to)
                    .count();
                assert_eq!(rows, 1, "{from} -> {to} appears {rows} times");
            }
        }
        assert_eq!(TABLE.len(), PROTOCOLS.len() * PROTOCOLS.len());
        // ... and every slug in the table is a protocol.
        for pair in &TABLE {
            assert!(PROTOCOLS.contains(&pair.from), "{}", pair.from);
            assert!(PROTOCOLS.contains(&pair.to), "{}", pair.to);
        }
    }

    #[test]
    fn exactly_the_supported_pairs_are_supported_and_every_other_row_carries_a_reason() {
        let supported: Vec<String> = TABLE
            .iter()
            .filter(|pair| pair.is_supported())
            .map(Pair::slug)
            .collect();
        assert_eq!(
            supported,
            vec![
                "anthropic-messages->openai-chat".to_owned(),
                "anthropic-messages->openai-responses".to_owned(),
                "openai-chat->openai-responses".to_owned(),
                "openai-responses->anthropic-messages".to_owned(),
                "openai-responses->openai-chat".to_owned(),
            ]
        );
        for pair in TABLE.iter().filter(|pair| !pair.is_supported()) {
            let reason = pair.refusal().expect("a refused pair has a reason");
            assert!(!reason.is_empty(), "{}", pair.slug());
        }
        // A supported pair has a codec on both sides — the table cannot
        // promise what the codecs cannot do.
        for pair in TABLE.iter().filter(|pair| pair.is_supported()) {
            assert!(codec_for(pair.from).is_some(), "{}", pair.from);
            assert!(codec_for(pair.to).is_some(), "{}", pair.to);
        }
        assert!(is_supported("anthropic-messages", "openai-chat"));
        assert!(is_supported("anthropic-messages", "openai-responses"));
        assert!(is_supported("openai-responses", "anthropic-messages"));
        assert!(is_supported("openai-responses", "openai-chat"));
        assert!(is_supported("openai-chat", "openai-responses"));
        assert!(!is_supported("openai-chat", "anthropic-messages"));
        assert!(!is_supported("anthropic-messages", "anthropic-messages"));
        assert!(!is_supported("anthropic-messages", "gemini"));
    }

    #[test]
    fn a_target_is_placed_from_its_path_alone_and_a_served_protocol_never_enters_a_codec() {
        let served_chat = ["openai-chat"];
        assert!(matches!(
            place("/v1/messages?beta=true", &served_chat),
            Placement::Translate(pair) if pair.slug() == "anthropic-messages->openai-chat"
        ));
        assert!(matches!(
            place("/messages", &served_chat),
            Placement::Translate(_)
        ));
        // The endpoint's sub-targets are refused by name, not translated.
        assert!(matches!(
            place("/v1/messages/count_tokens", &served_chat),
            Placement::TargetRefused {
                from: "anthropic-messages"
            }
        ));
        // A served protocol is never placed, even though a codec claims it.
        assert!(matches!(
            place("/v1/messages", &["anthropic-messages", "openai-chat"]),
            Placement::Unplaceable
        ));
        assert!(matches!(
            place("/v1/chat/completions", &["openai-chat"]),
            Placement::Unplaceable
        ));
        // Not a codec's target at all: the plain 404 stays.
        assert!(matches!(
            place("/api/hello", &served_chat),
            Placement::Unplaceable
        ));
        assert!(matches!(
            place("/models?client_version=1", &served_chat),
            Placement::Unplaceable
        ));
        assert!(matches!(
            place("/v1/messagesomethingelse", &served_chat),
            Placement::Unplaceable
        ));
        // The two T2 pairs place: Claude Code at a Responses-only provider,
        // and a Codex-shaped client at an Anthropic-only one.
        assert!(matches!(
            place("/v1/messages", &["openai-responses"]),
            Placement::Translate(pair) if pair.slug() == "anthropic-messages->openai-responses"
        ));
        assert!(matches!(
            place("/responses", &["anthropic-messages"]),
            Placement::Translate(pair) if pair.slug() == "openai-responses->anthropic-messages"
        ));
        // ... and a served Responses target never enters the codec.
        assert!(matches!(
            place("/v1/responses", &["openai-responses", "anthropic-messages"]),
            Placement::Unplaceable
        ));
        // The two T2b pairs place too: an OpenCode-shaped client at a
        // Responses-only provider, and a Codex-shaped client at a Chat-only
        // one.
        assert!(matches!(
            place("/v1/chat/completions", &["openai-responses"]),
            Placement::Translate(pair) if pair.slug() == "openai-chat->openai-responses"
        ));
        assert!(matches!(
            place("/responses", &["openai-chat"]),
            Placement::Translate(pair) if pair.slug() == "openai-responses->openai-chat"
        ));
        // OpenAI Chat at an Anthropic-only provider: the reverse pair, refused
        // by name until its own end-to-end test exists.
        match place("/v1/chat/completions", &["anthropic-messages"]) {
            Placement::PairRefused { refused, .. } => {
                assert!(refused[0].refusal().unwrap().contains("1956"));
            }
            other => panic!("expected a refused pair, got {other:?}"),
        }
    }

    #[test]
    fn a_refusal_names_the_pair_the_field_and_the_reason() {
        let pair = lookup("anthropic-messages", "openai-chat").unwrap();
        let refusal = TranslationRefusal::new(
            pair,
            Unsupported::new("system[0].cache_control", "no home for it"),
        );
        let text = refusal.to_string();
        assert!(text.contains("anthropic-messages->openai-chat"));
        assert!(text.contains("`system[0].cache_control`"));
        assert!(text.contains("no home for it"));
    }

    #[test]
    fn field_rows_exist_for_every_codec_and_for_nothing_else() {
        let rows = field_rows("anthropic-messages").unwrap();
        assert!(
            rows.refused
                .iter()
                .any(|(field, _)| *field == "cache_control")
        );
        assert!(rows.ignored.contains(&"usage.service_tier"));
        let rows = field_rows("openai-chat").unwrap();
        assert!(rows.refused.iter().any(|(field, _)| *field == "n"));
        let rows = field_rows("openai-responses").unwrap();
        assert!(
            rows.refused
                .iter()
                .any(|(field, _)| *field == "previous_response_id")
        );
        assert!(rows.ignored.contains(&"output[].id"));
        assert!(field_rows("gemini").is_none());
    }

    #[test]
    fn a_translated_request_is_posted_to_the_targets_native_clients_send() {
        // The convention every provider base URL is composed for — see
        // `outbound_target`'s own doc. The Anthropic path carries the
        // version segment because Claude Code sends it and the Anthropic
        // base URLs omit it; the OpenAI paths omit it because their clients
        // do and their base URLs carry it.
        assert_eq!(
            outbound_target(codec_for("anthropic-messages").unwrap()),
            "/v1/messages"
        );
        assert_eq!(
            outbound_target(codec_for("openai-chat").unwrap()),
            "/chat/completions"
        );
        assert_eq!(
            outbound_target(codec_for("openai-responses").unwrap()),
            "/responses"
        );
    }

    #[test]
    fn strict_fields_refuse_what_nobody_read_by_its_full_path() {
        let value: serde_json::Value =
            serde_json::json!({"a": 1, "nested": {"b": "x", "surprise": true}});
        let mut top = fields::Fields::of(value, "").unwrap();
        assert_eq!(top.take_u64("a").unwrap(), Some(1));
        let mut nested = top.take_object("nested").unwrap().unwrap();
        assert_eq!(nested.require_string("b").unwrap(), "x");
        let refusal = nested.finish().unwrap_err();
        assert_eq!(refusal.field, "nested.surprise");
        assert!(top.finish().is_ok());

        let wrong = fields::Fields::of(serde_json::json!({"n": "not a number"}), "")
            .unwrap()
            .take_u64("n")
            .unwrap_err();
        assert_eq!(wrong.field, "n");
        assert!(wrong.reason.contains("a string"));
    }

    /// A decoder that replays a scripted sequence of already-canonical
    /// batches, ignoring the raw SSE bytes it is fed. The wire shape that
    /// produces this exact canonical sequence from a real provider is
    /// `anthropic::EventDecoder` (swarm finding break/gateway-translate#1):
    /// its `require_index` only range-checks the provider's index and never
    /// compares it to which blocks have started. Scripting the decoder here
    /// reproduces that output directly against the real `stream_events`
    /// path without duplicating the Anthropic wire format.
    struct Scripted {
        batches: std::collections::VecDeque<Result<Vec<StreamEvent>, Unsupported>>,
        finished: bool,
    }

    impl StreamDecoder for Scripted {
        fn feed(&mut self, _event: &SseEvent) -> Result<Vec<StreamEvent>, Unsupported> {
            self.batches.pop_front().unwrap_or(Ok(Vec::new()))
        }
        fn finish(&mut self) -> Result<Vec<StreamEvent>, Unsupported> {
            self.finished = true;
            Ok(vec![StreamEvent::MessageStop])
        }
        fn is_done(&self) -> bool {
            self.finished
        }
    }

    fn test_finish(
        outcome: Outcome,
        status: u16,
        framing: Framing,
        tokens: Option<Tokens>,
    ) -> (Exchange, RateLimitHeaders) {
        (
            Exchange {
                outcome,
                status,
                provider: String::new(),
                protocol: None,
                host: String::new(),
                first_byte_at: None,
                framing: Some(framing),
                tokens,
            },
            RateLimitHeaders::default(),
        )
    }

    /// The acceptance shape from break/gateway-translate#1: `call_A`'s block
    /// starts, `call_B`'s block starts before `call_A`'s stops, and a delta
    /// addressed to `call_A` (index 0) arrives while `call_B` (index 1) is
    /// the open block. Before `canonical::Order` this delta rode on whatever
    /// block `openai_responses::EventEncoder` had open — `call_B` — so the
    /// harness would have been told `call_B` received `call_A`'s arguments.
    /// It must instead be refused by name, before the encoder ever sees it.
    #[test]
    fn stream_events_refuses_a_delta_that_would_misfile_under_another_calls_id() {
        use std::collections::VecDeque;
        use std::io::Cursor;
        use std::net::TcpListener;

        let mut decoder = Scripted {
            batches: VecDeque::from([
                Ok(vec![StreamEvent::MessageStart {
                    id: "msg_fix".to_owned(),
                    model: "claude-x".to_owned(),
                    usage: canonical::Usage::default(),
                }]),
                Ok(vec![
                    StreamEvent::BlockStart {
                        index: 0,
                        block: canonical::BlockStart::ToolUse {
                            id: "call_A".to_owned(),
                            name: "Bash".to_owned(),
                        },
                    },
                    StreamEvent::BlockStart {
                        index: 1,
                        block: canonical::BlockStart::ToolUse {
                            id: "call_B".to_owned(),
                            name: "Read".to_owned(),
                        },
                    },
                ]),
                Ok(vec![StreamEvent::BlockDelta {
                    index: 0,
                    delta: canonical::Delta::InputJson("{\"command\": \"ls\"}".to_owned()),
                }]),
            ]),
            finished: false,
        };
        let from = codec_for("openai-responses").expect("openai-responses is a registered codec");

        // Three raw placeholder SSE events: `Scripted` ignores their content
        // and returns the canned batches above instead, one per `feed` call.
        let raw = b"event: x\ndata: {}\n\nevent: x\ndata: {}\n\nevent: x\ndata: {}\n\n".to_vec();
        let mut events = SseReader::new(BufReader::new(Cursor::new(raw)));

        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback is bindable");
        let addr = listener.local_addr().expect("bound");
        let mut client = TcpStream::connect(addr).expect("loopback connects");
        let (mut server, _) = listener.accept().expect("loopback accepts");

        let finish: Finish<'_> = &test_finish;
        stream_events(&mut server, &mut events, &mut decoder, from, finish, 200);
        drop(server);

        let mut received = Vec::new();
        client
            .read_to_end(&mut received)
            .expect("the client reads whatever the gateway wrote before closing");
        let text = String::from_utf8_lossy(&received);

        assert!(
            text.contains("a delta arrived for a block that is not the open one"),
            "expected the wrong-tool-call-id refusal, got: {text}"
        );
        assert!(
            !text.contains("\"command\": \"ls\""),
            "call_A's argument fragment must never reach the client attached to \
             call_B's item: {text}"
        );
    }
}
