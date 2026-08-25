//! The smallest amount of HTTP the gateway has to understand, and not one
//! byte more.
//!
//! # Why this file is so small
//!
//! The gateway is a pass-through. Routing needs the request line and the
//! headers; framing needs `content-length`. **The body is bytes** — nothing
//! here parses it, decodes it, or even looks at it, which is what makes a
//! tool-call payload survive the trip: a payload nothing inspected cannot be
//! rewritten.
//!
//! So this module deliberately does *not* contain: a JSON parser, a
//! content-type table, a URL parser, a cookie jar, or any notion of what an
//! Anthropic Messages request looks like. Adding one would be the first step
//! towards the cross-protocol translation the capability map refuses until a
//! concrete pair needs it.
//!
//! # What "byte-for-byte" honestly means
//!
//! A proxy terminates one connection and opens another, so *connection*
//! framing cannot survive: `content-length` is re-derived, `transfer-encoding`
//! is re-applied, and hop-by-hop headers belong to the hop they were written
//! for. What survives untouched is the part that carries meaning — the
//! method, the request target, every end-to-end header, and every byte of
//! the body, in order.
//!
//! Header *names* arrive here through [`HeaderName`], which lower-cases them.
//! That is the same normalisation HTTP/2 mandates and is semantically the
//! identity, so it is not a rewrite in any sense a client can observe.

use std::io::{BufRead, Read, Write};

use ureq::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};

/// The largest request head the gateway will read before giving up.
///
/// A cap rather than a preference: `read_until` on a client that never sends
/// a newline is unbounded, and the gateway runs in the user's own process.
/// 64 KiB is far above any real harness request head — Claude Code's is a
/// few hundred bytes — and far below anything that could hurt.
pub(super) const MAX_HEAD_BYTES: usize = 64 * 1024;

/// How much of a response body is moved per write.
///
/// Small on purpose. This is the granularity at which a streamed response
/// reaches the harness, so a large buffer would turn incremental delivery
/// into something that merely arrives eventually — see
/// [`pump`](self::pump), which flushes after every one of these.
const STREAM_CHUNK: usize = 8 * 1024;

/// A parsed request head: everything the gateway needs and nothing it does
/// not.
pub(super) struct RequestHead {
    pub(super) method: Method,
    /// The request target exactly as the client wrote it, path and query
    /// together. Never re-encoded, never normalised — a query string is the
    /// harness's business, not the gateway's.
    pub(super) target: String,
    pub(super) headers: HeaderMap,
    /// The declared body length, or `None` when the request carries no body.
    pub(super) content_length: Option<u64>,
}

/// Why a request head could not be read.
///
/// Each variant maps to exactly one status in
/// [`super::ingress`](super::ingress), and none of them carries any part of
/// the request: a malformed head is still a head someone wrote, and quoting
/// it back into a diagnostic is how prompt text leaks.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum HeadError {
    /// The client opened a connection and closed it without sending a
    /// request. Not an error to answer — there is nobody to answer.
    Empty,
    /// The head exceeded [`MAX_HEAD_BYTES`].
    TooLarge,
    /// The request line or a header line did not parse.
    Malformed,
    /// A body framed with `transfer-encoding` rather than `content-length`.
    ///
    /// Its own variant rather than [`HeadError::Malformed`] because it is a
    /// well-formed request the gateway declines, which is a different fact
    /// and gets a different status. See [`read_head`] for why.
    ChunkedRequest,
    /// The connection failed while the head was being read.
    Io,
}

/// Headers that belong to one hop and must never be copied to the next.
///
/// RFC 9110 §7.6.1's list, plus the two framing headers, which this gateway
/// re-derives for the connection it is actually writing to. Copying any of
/// these forward is the classic proxy defect: `connection: keep-alive` from
/// a client would be applied to the upstream socket, and a forwarded
/// `content-length` would contradict the framing the outbound layer chose.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "content-length",
];

/// Whether `name` is a hop-by-hop header — see [`HOP_BY_HOP`].
pub(super) fn is_hop_by_hop(name: &HeaderName) -> bool {
    HOP_BY_HOP.contains(&name.as_str())
}

/// Read and parse one request head from `reader`, stopping at the blank line
/// that ends it.
///
/// `reader` is left positioned exactly at the first body byte, so the caller
/// can hand the same reader on as the body without a copy — which is what
/// keeps a request body streamed rather than buffered.
///
/// # `transfer-encoding` is declined rather than de-chunked
///
/// A chunked request body would have to be de-chunked here and re-framed on
/// the way out, and a de-chunker is a body parser: the one thing this module
/// exists not to have. Claude Code sends `content-length` — a Messages
/// request is one JSON document the harness has already serialised, so there
/// is nothing to stream incrementally in that direction. So the gateway
/// answers `411 Length Required`, which is exactly what that status means,
/// rather than growing a parser for a case no harness in scope produces.
pub(super) fn read_head(reader: &mut impl BufRead) -> Result<RequestHead, HeadError> {
    let raw = read_raw_head(reader)?;
    let text = std::str::from_utf8(&raw).map_err(|_| HeadError::Malformed)?;

    let mut lines = text.split("\r\n");
    let request_line = lines.next().ok_or(HeadError::Malformed)?;

    // `METHOD SP request-target SP HTTP-version`. Split from the left twice
    // rather than on whitespace: a request target may not contain a space,
    // but splitting whichever way happens to work on the input is how a
    // parser starts accepting things it should refuse.
    let (method, rest) = request_line.split_once(' ').ok_or(HeadError::Malformed)?;
    let (target, version) = rest.split_once(' ').ok_or(HeadError::Malformed)?;
    if !version.starts_with("HTTP/1.") || target.is_empty() {
        return Err(HeadError::Malformed);
    }
    let method = Method::from_bytes(method.as_bytes()).map_err(|_| HeadError::Malformed)?;

    let mut headers = HeaderMap::new();
    let mut content_length = None;
    let mut chunked = false;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':').ok_or(HeadError::Malformed)?;
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| HeadError::Malformed)?;
        // Only the optional whitespace after the colon is stripped, per RFC
        // 9112 §5. The value itself is untouched.
        let value = HeaderValue::from_str(value.trim_matches([' ', '\t']))
            .map_err(|_| HeadError::Malformed)?;

        if name == "content-length" {
            let declared: u64 = value
                .to_str()
                .map_err(|_| HeadError::Malformed)?
                .parse()
                .map_err(|_| HeadError::Malformed)?;
            // A second, disagreeing `content-length` is a request smuggling
            // primitive, so two of them is a refusal rather than a choice.
            if content_length.is_some_and(|already| already != declared) {
                return Err(HeadError::Malformed);
            }
            content_length = Some(declared);
        }
        if name == "transfer-encoding" {
            chunked = true;
        }
        headers.append(name, value);
    }

    if chunked {
        return Err(HeadError::ChunkedRequest);
    }

    Ok(RequestHead {
        method,
        target: target.to_owned(),
        headers,
        content_length: content_length.filter(|length| *length > 0),
    })
}

/// The head's raw bytes, up to and including the blank line.
///
/// Bounded by [`MAX_HEAD_BYTES`] through a [`Read::take`] around the reader
/// rather than by checking afterwards: `read_until` on a client that sends a
/// megabyte with no newline in it would have already allocated the megabyte
/// by the time a check could run.
fn read_raw_head(reader: &mut impl BufRead) -> Result<Vec<u8>, HeadError> {
    let mut head = Vec::new();
    let mut limited = reader.take(MAX_HEAD_BYTES as u64);
    loop {
        let mut line = Vec::new();
        let read = limited
            .read_until(b'\n', &mut line)
            .map_err(|_| HeadError::Io)?;
        if read == 0 {
            return Err(if head.is_empty() {
                HeadError::Empty
            } else {
                // The limit ran out, or the client hung up mid-head. The
                // first is the interesting one and the only one worth its
                // own status.
                HeadError::TooLarge
            });
        }
        let blank = line == b"\r\n" || line == b"\n";
        head.extend_from_slice(&line);
        if blank {
            return Ok(head);
        }
    }
}

/// Write a status line and headers to `out`, ending with the blank line.
///
/// Always `HTTP/1.1`: that is the version this gateway speaks to the harness
/// on its own loopback connection, independently of whatever version the
/// upstream answered on.
pub(super) fn write_head(
    out: &mut impl Write,
    status: StatusCode,
    headers: &[(String, Vec<u8>)],
) -> std::io::Result<()> {
    // Assembled in memory and written **once**. Not tidiness: a `TcpStream`
    // issues one segment per `write`, so a head written field by field is a
    // dozen tiny segments, and Nagle's algorithm on the sender plus delayed
    // acknowledgement on the receiver then hold the *next* small write —
    // the first event of a stream — for tens or hundreds of milliseconds.
    // That is a latency defect in exactly the thing this gateway promises to
    // preserve. See `super::ingress::serve`, which also turns Nagle off.
    let reason = status.canonical_reason().unwrap_or("Unknown");
    let mut head = format!("HTTP/1.1 {} {reason}\r\n", status.as_u16()).into_bytes();
    for (name, value) in headers {
        head.extend_from_slice(name.as_bytes());
        head.extend_from_slice(b": ");
        head.extend_from_slice(value);
        head.extend_from_slice(b"\r\n");
    }
    head.extend_from_slice(b"\r\n");
    out.write_all(&head)?;
    out.flush()
}

/// Move every byte of `body` to `out`, flushing as it goes.
///
/// The flush is the whole point and is not tidiness: it is what makes a
/// server-sent-event stream arrive as events rather than as one delivery at
/// the end. `std::io::copy` would be shorter and would silently defeat
/// [`super::ingress`]'s streaming contract, which is why it is not used.
///
/// When `chunked` is set, each read is framed as one HTTP chunk, and the
/// terminating zero-length chunk is written at the end.
pub(super) fn pump(
    mut body: impl Read,
    out: &mut impl Write,
    chunked: bool,
) -> std::io::Result<u64> {
    let mut buffer = vec![0u8; STREAM_CHUNK];
    let mut moved = 0u64;
    loop {
        let read = match body.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        // One write per read, framing included, for the same reason
        // `write_head` assembles its head: three small writes where one
        // would do is three segments a receiver has to acknowledge.
        if chunked {
            let mut framed = format!("{read:x}\r\n").into_bytes();
            framed.extend_from_slice(&buffer[..read]);
            framed.extend_from_slice(b"\r\n");
            out.write_all(&framed)?;
        } else {
            out.write_all(&buffer[..read])?;
        }
        out.flush()?;
        moved += read as u64;
    }
    if chunked {
        out.write_all(b"0\r\n\r\n")?;
        out.flush()?;
    }
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::BufReader;

    fn head_of(raw: &str) -> Result<RequestHead, HeadError> {
        let mut reader = BufReader::new(raw.as_bytes());
        read_head(&mut reader)
    }

    #[test]
    fn a_request_line_and_headers_parse_into_exactly_what_routing_needs() {
        let head = head_of(
            "POST /v1/messages?beta=true HTTP/1.1\r\n\
             Host: 127.0.0.1:8731\r\n\
             Authorization: Bearer abc\r\n\
             Content-Length: 4\r\n\
             \r\nbody",
        )
        .expect("a well-formed head parses");

        assert_eq!(head.method, Method::POST);
        assert_eq!(
            head.target, "/v1/messages?beta=true",
            "the request target must survive with its query intact"
        );
        assert_eq!(head.content_length, Some(4));
        assert_eq!(
            head.headers.get("authorization").map(|v| v.as_bytes()),
            Some(b"Bearer abc".as_slice())
        );
    }

    #[test]
    fn the_reader_is_left_on_the_first_body_byte() {
        let raw = "POST /v1/messages HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello";
        let mut reader = BufReader::new(raw.as_bytes());
        let head = read_head(&mut reader).expect("a well-formed head parses");
        assert_eq!(head.content_length, Some(5));

        let mut body = String::new();
        reader.read_to_string(&mut body).expect("the body is there");
        assert_eq!(
            body, "hello",
            "the head reader over-read into the body, so a streamed body would lose its start"
        );
    }

    #[test]
    fn a_body_framed_with_transfer_encoding_is_declined_rather_than_parsed() {
        assert_eq!(
            head_of("POST /v1/messages HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").err(),
            Some(HeadError::ChunkedRequest)
        );
    }

    #[test]
    fn two_disagreeing_content_lengths_are_refused() {
        assert_eq!(
            head_of("POST /v1/messages HTTP/1.1\r\nContent-Length: 4\r\nContent-Length: 9\r\n\r\n")
                .err(),
            Some(HeadError::Malformed)
        );
        // ... while two that agree are merely redundant, not an attack.
        assert!(
            head_of("POST /v1/messages HTTP/1.1\r\nContent-Length: 4\r\nContent-Length: 4\r\n\r\n")
                .is_ok()
        );
    }

    #[test]
    fn a_head_larger_than_the_cap_is_refused_rather_than_read() {
        let filler = "x".repeat(MAX_HEAD_BYTES);
        let raw = format!("POST /v1/messages HTTP/1.1\r\nX-Big: {filler}\r\n\r\n");
        assert_eq!(head_of(&raw).err(), Some(HeadError::TooLarge));
    }

    #[test]
    fn a_client_that_sends_nothing_is_not_an_error_to_answer() {
        assert_eq!(head_of("").err(), Some(HeadError::Empty));
    }

    #[test]
    fn malformed_request_lines_are_refused() {
        for raw in [
            "GARBAGE\r\n\r\n",
            "POST /v1/messages\r\n\r\n",
            "POST /v1/messages HTTP/9.9\r\n\r\n",
            "POST  HTTP/1.1\r\n\r\n",
            "POST /v1/messages HTTP/1.1\r\nNoColonHere\r\n\r\n",
        ] {
            assert_eq!(
                head_of(raw).err(),
                Some(HeadError::Malformed),
                "accepted {raw:?}"
            );
        }
    }

    #[test]
    fn every_framing_and_connection_header_is_hop_by_hop() {
        for name in [
            "connection",
            "keep-alive",
            "transfer-encoding",
            "content-length",
            "proxy-authorization",
            "upgrade",
        ] {
            assert!(is_hop_by_hop(&HeaderName::from_static(name)), "{name}");
        }
        // ... and an ordinary end-to-end header is not, or nothing would be
        // forwarded at all.
        for name in ["authorization", "content-type", "anthropic-version"] {
            assert!(!is_hop_by_hop(&HeaderName::from_static(name)), "{name}");
        }
    }

    #[test]
    fn a_chunked_pump_frames_every_read_and_terminates_the_stream() {
        let mut out = Vec::new();
        let moved = pump(b"hello".as_slice(), &mut out, true).expect("writing to a Vec succeeds");
        assert_eq!(moved, 5);
        assert_eq!(String::from_utf8(out).unwrap(), "5\r\nhello\r\n0\r\n\r\n");
    }

    #[test]
    fn an_unframed_pump_writes_the_body_and_nothing_else() {
        let mut out = Vec::new();
        let moved = pump(b"hello".as_slice(), &mut out, false).expect("writing to a Vec succeeds");
        assert_eq!(moved, 5);
        assert_eq!(out, b"hello");
    }

    #[test]
    fn a_written_head_is_a_status_line_headers_and_a_blank_line() {
        let mut out = Vec::new();
        write_head(
            &mut out,
            StatusCode::TOO_MANY_REQUESTS,
            &[("content-type".to_owned(), b"application/json".to_vec())],
        )
        .expect("writing to a Vec succeeds");
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\n\r\n"
        );
    }
}
