//! A canned HTTP upstream, for the gateway's own tests.
//!
//! # Why this exists rather than a real provider
//!
//! A test that called Anthropic or OpenRouter would spend the user's
//! credential, would fail when the network did, and could not assert what
//! the *upstream* received — which is the whole point of most of these
//! tests. So the gateway is pointed at a `TcpListener` in this process
//! speaking canned HTTP, and every assertion about what was forwarded is
//! made against what that listener actually read off the wire.
//!
//! # It parses HTTP independently on purpose
//!
//! Nothing here calls [`super::http`]. A fixture that re-used the parser
//! under test would agree with it about a request it had mis-framed, and the
//! test would pass. This one reads the head byte by byte and finds
//! `content-length` itself, so "the body arrived byte-for-byte" is a claim
//! about the wire and not about two copies of the same code.

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// How long the fixture's accept loop sleeps between polls of its stop flag.
const POLL: Duration = Duration::from_millis(10);

/// One request as it actually arrived on the wire.
#[derive(Debug, Clone)]
pub(super) struct RecordedRequest {
    pub(super) method: String,
    pub(super) target: String,
    /// Header names lower-cased; values exactly as received.
    pub(super) headers: Vec<(String, String)>,
    pub(super) body: Vec<u8>,
}

impl RecordedRequest {
    /// The first value of `name`, or `None`.
    pub(super) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }
}

/// What the fixture writes back. Given the request it just read and the
/// socket it read it from, so a responder can stream, stall, or hang up.
type Responder = Box<dyn Fn(&RecordedRequest, &mut TcpStream) + Send + Sync + 'static>;

/// A canned upstream: an address to point a gateway at, plus a record of
/// everything that reached it.
pub(super) struct FixtureUpstream {
    address: SocketAddr,
    connections: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl FixtureUpstream {
    /// Bind a fixture on loopback that answers every request with
    /// `responder`.
    pub(super) fn start(
        responder: impl Fn(&RecordedRequest, &mut TcpStream) + Send + Sync + 'static,
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
            accept: Some(accept),
        }
    }

    /// A fixture that answers every request with one canned response.
    pub(super) fn answering(
        status_line: &'static str,
        headers: &'static str,
        body: &'static str,
    ) -> Self {
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

    /// Where to point a gateway.
    pub(super) fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// How many TCP connections have ever been accepted here.
    ///
    /// The assertion behind "a refused request opens nothing upstream" is
    /// made on this rather than on the request count: a connection that was
    /// opened and then abandoned would leave no request but would still be a
    /// connection, and it is the connection that must not happen.
    pub(super) fn connections(&self) -> usize {
        self.connections.load(Ordering::Relaxed)
    }

    /// Every request that arrived, in order.
    pub(super) fn requests(&self) -> Vec<RecordedRequest> {
        self.requests
            .lock()
            .expect("no test panics while holding this")
            .clone()
    }

    /// The single request that arrived, failing loudly on any other count.
    pub(super) fn only_request(&self) -> RecordedRequest {
        let mut requests = self.requests();
        assert_eq!(
            requests.len(),
            1,
            "expected exactly one request at the fixture upstream"
        );
        requests.remove(0)
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
    // The gateway turns Nagle off on its own client-facing socket; a fixture
    // that left it on here would add the very stall the tests are looking
    // for, and would be measuring itself.
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
