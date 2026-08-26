//! A canned HTTP server bound on loopback, for the provider module's own tests.
//!
//! # Why this exists rather than a real provider
//!
//! A test that called OpenRouter or Anthropic would spend the user's credential,
//! fail when the network did, and could not assert what the upstream received.
//! So tests point a provider client at a [`FixtureProvider`] in this process
//! speaking canned HTTP, and every assertion about what was probed is made against
//! what the fixture actually read off the wire.
//!
//! # It parses HTTP independently on purpose
//!
//! Nothing here calls any HTTP parser in this crate. A fixture that re-used
//! production parsing code would agree with it about a request it had mis-framed,
//! and the test would pass. This fixture reads the request head byte by byte and
//! finds `content-length` itself, so "the request arrived byte-for-byte" is a
//! claim about the wire and not about two copies of the same parser.

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// How long the fixture's accept loop sleeps between polls of its stop flag.
const POLL: Duration = Duration::from_millis(10);

/// One request as it actually arrived on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordedRequest {
    pub(crate) method: String,
    pub(crate) target: String,
    /// Header names lower-cased; values exactly as received.
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
}

impl RecordedRequest {
    /// The first value of `name`, or `None`. Lookup matches header names
    /// case-insensitively against the lower-cased recorded headers.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// What the fixture writes back. Given the request it just read and the
/// socket it read it from, so a responder can reply, stream, stall, or hang up.
type Responder = Box<dyn Fn(&RecordedRequest, &mut TcpStream) + Send + Sync + 'static>;

/// A canned HTTP server bound on loopback: an address to point a provider client at,
/// plus a record of everything that reached it.
pub(crate) struct FixtureProvider {
    address: SocketAddr,
    connections: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    stop: Arc<AtomicBool>,
    hang_gate: Arc<(Mutex<bool>, Condvar)>,
    accept: Option<JoinHandle<()>>,
}

impl FixtureProvider {
    /// Bind on loopback; `responder` writes the reply for each request.
    pub(crate) fn start(
        responder: impl Fn(&RecordedRequest, &mut TcpStream) + Send + Sync + 'static,
    ) -> Self {
        let hang_gate = Arc::new((Mutex::new(false), Condvar::new()));
        Self::start_with_gate(responder, hang_gate)
    }

    fn start_with_gate(
        responder: impl Fn(&RecordedRequest, &mut TcpStream) + Send + Sync + 'static,
        hang_gate: Arc<(Mutex<bool>, Condvar)>,
    ) -> Self {
        let listener =
            TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("loopback is bindable");
        let address = listener
            .local_addr()
            .expect("a bound listener has an address");
        listener
            .set_nonblocking(true)
            .expect("a bound listener can be put in polling mode");

        let connections = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let responder: Responder = Box::new(responder);

        let accept = std::thread::spawn({
            let connections = Arc::clone(&connections);
            let requests = Arc::clone(&requests);
            let stop = Arc::clone(&stop);
            let responder = Arc::new(responder);
            move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            connections.fetch_add(1, Ordering::Relaxed);
                            let requests = Arc::clone(&requests);
                            let responder = Arc::clone(&responder);
                            std::thread::spawn(move || serve(stream, &requests, &responder));
                        }
                        Err(_) => std::thread::sleep(POLL),
                    }
                }
            }
        });

        Self {
            address,
            connections,
            requests,
            stop,
            hang_gate,
            accept: Some(accept),
        }
    }

    /// Answers every request with one canned response.
    /// `status_line` is e.g. "HTTP/1.1 200 OK"; `headers` is a possibly
    /// empty string of extra "name: value\r\n" lines; `content-length` is
    /// computed here, never by the caller.
    pub(crate) fn answering(status_line: &str, headers: &str, body: &str) -> Self {
        let status_line = status_line.to_owned();
        let headers = if headers.is_empty() || headers.ends_with("\r\n") {
            headers.to_owned()
        } else {
            format!("{headers}\r\n")
        };
        let body = body.to_owned();
        Self::start(move |_request, out| {
            let _ = write!(
                out,
                "{status_line}\r\n{headers}content-length: {}\r\n\r\n{body}",
                body.len()
            );
            let _ = out.flush();
            let _ = out.shutdown(Shutdown::Write);
        })
    }

    /// Accepts the connection, reads the request, and then NEVER writes a
    /// byte and never closes — until the fixture is dropped. This is the
    /// case the whole batch exists to bound. It must NOT be a refused
    /// connection and it must NOT be an immediate hangup.
    pub(crate) fn hanging() -> Self {
        let hang_gate = Arc::new((Mutex::new(false), Condvar::new()));
        let gate = Arc::clone(&hang_gate);
        Self::start_with_gate(
            move |_request, stream| {
                let (lock, cvar) = &*gate;
                let mut stopped = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                while !*stopped {
                    stopped = cvar
                        .wait(stopped)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                let _ = stream.shutdown(Shutdown::Both);
            },
            hang_gate,
        )
    }

    /// `http://127.0.0.1:<port>` — no trailing slash.
    pub(crate) fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// TCP connections ever accepted.
    pub(crate) fn connections(&self) -> usize {
        self.connections.load(Ordering::Relaxed)
    }

    /// Every request that arrived, in order.
    pub(crate) fn requests(&self) -> Vec<RecordedRequest> {
        self.requests
            .lock()
            .expect("no test panics while holding this")
            .clone()
    }
}

impl Drop for FixtureProvider {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        {
            let (lock, cvar) = &*self.hang_gate;
            let mut stopped = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            *stopped = true;
            cvar.notify_all();
        }
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

/// Read one request, record it, and hand it to the responder.
fn serve(mut stream: TcpStream, requests: &Mutex<Vec<RecordedRequest>>, responder: &Responder) {
    // **The same trap the production ingress documents, and this fixture
    // walked straight into it.** The listener above is non-blocking so its
    // accept loop can poll a stop flag, and on macOS, the BSDs and Windows an
    // accepted socket *inherits* that flag — while on Linux it does not.
    // Without this line the read below returns `WouldBlock` whenever the
    // request bytes have not landed yet, the fixture abandons the connection,
    // and the gateway reports a perfectly accurate `502`. It reproduced
    // roughly twice in fifteen full-suite runs before this line existed,
    // which is exactly the shape of flake that gets blamed on "the network"
    // and never fixed.
    stream
        .set_nonblocking(false)
        .expect("an accepted socket can be put back in blocking mode");
    let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
    // Turn Nagle off immediately on accepted sockets. A fixture that left
    // Nagle on would create the very stall the tests are hunting, and would
    // be measuring itself.
    let _ = stream.set_nodelay(true);
    let Some(request) = read_request(&mut stream) else {
        return;
    };
    requests
        .lock()
        .expect("no test panics while holding this")
        .push(request.clone());
    responder(&request, &mut stream);
}

/// Read a whole request off the wire, head and body, without using any of
/// the code under test.
fn read_request(stream: &mut TcpStream) -> Option<RecordedRequest> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => return None,
            Ok(_) => head.push(byte[0]),
        }
        if head.len() > 64 * 1024 {
            return None;
        }
    }

    let text = String::from_utf8(head).ok()?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split(' ');
    let method = parts.next()?.to_owned();
    let target = parts.next()?.to_owned();

    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':')?;
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_owned();
        if name == "content-length" {
            content_length = value.parse().ok()?;
        }
        headers.push((name, value));
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 && stream.read_exact(&mut body).is_err() {
        return None;
    }

    Some(RecordedRequest {
        method,
        target,
        headers,
        body,
    })
}
