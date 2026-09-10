//! Server-sent-event framing: the one piece of the stream both codecs share.
//!
//! Anthropic Messages and OpenAI Chat both stream as `text/event-stream`
//! (RFC-less, but WHATWG-specified) events whose `data:` lines carry one
//! JSON document each. What differs is the vocabulary inside the JSON, and
//! that is the codecs' business — this module only frames and unframes, and
//! it does so **one event at a time**: [`SseReader::next_event`] returns as
//! soon as one event's blank line has arrived, which is what lets a
//! translated stream stay a stream rather than a document assembled at the
//! end.
//!
//! # Bounded
//!
//! A single event is capped at [`MAX_EVENT_BYTES`]. The relay this sits
//! beside bounds nothing, because it holds nothing; a decoder holds one
//! event, so one event is what it bounds.

use std::io::{self, BufRead, Read};

/// The largest single event a stream decoder will hold before refusing the
/// stream. A megabyte is three orders of magnitude above any real delta and
/// far below what could hurt.
pub const MAX_EVENT_BYTES: usize = 1024 * 1024;

/// One parsed event: its `event:` name, when it had one, and its `data:`
/// lines joined with newlines, as the specification says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// Frame one event for the wire.
pub fn encode(event: Option<&str>, data: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 32);
    if let Some(event) = event {
        out.extend_from_slice(b"event: ");
        out.extend_from_slice(event.as_bytes());
        out.push(b'\n');
    }
    out.extend_from_slice(b"data: ");
    out.extend_from_slice(data.as_bytes());
    out.extend_from_slice(b"\n\n");
    out
}

/// Reads events off a byte stream as they complete.
pub struct SseReader<R> {
    inner: R,
}

impl<R: BufRead> SseReader<R> {
    pub fn new(inner: R) -> Self {
        Self { inner }
    }

    /// The next complete event, `Ok(None)` at a clean end of stream.
    ///
    /// An event still being assembled when the stream ends is delivered if it
    /// carries any data — a provider that closes after its last `data:` line
    /// without the terminating blank line has still said what it meant — and
    /// dropped when it carries none.
    pub fn next_event(&mut self) -> io::Result<Option<SseEvent>> {
        let mut event = None;
        let mut data: Option<String> = None;
        let mut held = 0usize;
        let mut line = Vec::new();
        loop {
            line.clear();
            // `take` bounds the read of one line: a stream that never sends
            // a newline would otherwise grow `line` without limit.
            let read = (&mut self.inner)
                .take((MAX_EVENT_BYTES - held + 1) as u64)
                .read_until(b'\n', &mut line)?;
            if read == 0 {
                return Ok(data.map(|data| SseEvent { event, data }));
            }
            held += read;
            if held > MAX_EVENT_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "a server-sent event exceeded the size the translator will hold",
                ));
            }
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if line.is_empty() {
                match data {
                    Some(data) => return Ok(Some(SseEvent { event, data })),
                    // A blank line with nothing pending is just a blank line.
                    None => continue,
                }
            }
            if line.starts_with(b":") {
                continue;
            }
            let text = String::from_utf8_lossy(&line);
            let (field, value) = match text.split_once(':') {
                Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                None => (text.as_ref(), ""),
            };
            match field {
                "data" => match &mut data {
                    Some(data) => {
                        data.push('\n');
                        data.push_str(value);
                    }
                    None => data = Some(value.to_owned()),
                },
                "event" => event = Some(value.to_owned()),
                // `id` and `retry` mean nothing to a one-shot response, and
                // an unknown field is ignored by specification.
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::BufReader;

    fn events_of(raw: &[u8]) -> Vec<SseEvent> {
        let mut reader = SseReader::new(BufReader::new(raw));
        let mut events = Vec::new();
        while let Some(event) = reader.next_event().expect("well-formed input") {
            events.push(event);
        }
        events
    }

    #[test]
    fn an_encoded_event_reads_back_as_itself() {
        let framed = encode(Some("content_block_delta"), r#"{"a":1}"#);
        assert_eq!(
            String::from_utf8(framed.clone()).unwrap(),
            "event: content_block_delta\ndata: {\"a\":1}\n\n"
        );
        assert_eq!(
            events_of(&framed),
            vec![SseEvent {
                event: Some("content_block_delta".to_owned()),
                data: r#"{"a":1}"#.to_owned(),
            }]
        );
    }

    #[test]
    fn openai_style_events_with_no_name_crlf_and_a_trailing_done_parse_one_at_a_time() {
        let raw = b": keep-alive\r\ndata: {\"n\":1}\r\n\r\ndata: {\"n\":2}\n\ndata: [DONE]\n";
        assert_eq!(
            events_of(raw),
            vec![
                SseEvent {
                    event: None,
                    data: "{\"n\":1}".to_owned()
                },
                SseEvent {
                    event: None,
                    data: "{\"n\":2}".to_owned()
                },
                // Delivered even though the stream closed without its blank
                // line: the provider said `[DONE]`, and dropping it would
                // turn a clean end into an abort.
                SseEvent {
                    event: None,
                    data: "[DONE]".to_owned()
                },
            ]
        );
    }

    #[test]
    fn multiple_data_lines_join_with_a_newline_and_a_line_without_a_colon_is_a_field() {
        assert_eq!(
            events_of(b"data: a\ndata: b\n\n"),
            vec![SseEvent {
                event: None,
                data: "a\nb".to_owned()
            }]
        );
        // `data` with no colon is the empty-value form.
        assert_eq!(
            events_of(b"data\n\n"),
            vec![SseEvent {
                event: None,
                data: String::new()
            }]
        );
    }

    #[test]
    fn an_event_larger_than_the_cap_is_refused_rather_than_held() {
        let huge = format!("data: {}\n\n", "x".repeat(MAX_EVENT_BYTES + 10));
        let mut reader = SseReader::new(BufReader::new(huge.as_bytes()));
        let err = reader.next_event().expect_err("over the cap");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);

        // ... and a line with no newline at all is bounded the same way.
        let endless = "x".repeat(MAX_EVENT_BYTES + 10);
        let mut reader = SseReader::new(BufReader::new(endless.as_bytes()));
        assert!(reader.next_event().is_err());
    }

    #[test]
    fn a_reader_returns_each_event_as_soon_as_it_is_complete() {
        // Two events in one buffer: the first `next_event` must return after
        // the first blank line, not after consuming both.
        let raw = b"data: 1\n\ndata: 2\n\n";
        let mut reader = SseReader::new(BufReader::new(&raw[..]));
        let first = reader.next_event().unwrap().unwrap();
        assert_eq!(first.data, "1");
        // `reader.inner` has consumed exactly through the first blank line.
        assert_eq!(reader.inner.buffer(), b"data: 2\n\n");
    }
}
