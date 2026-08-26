//! The local Glasshouse gateway: the process, and now its protocol (Phase
//! 9G).
//!
//! # What the gateway is, and the one thing it must never become
//!
//! The gateway is an **optional local proxy** — a transport, credential,
//! telemetry, reliability and backend-routing hop for requests that originate
//! in a real harness. It is never a coding harness, never an agent loop,
//! never the owner of an interactive session, and never a replacement for a
//! harness's own tools. Glasshouse's whole premise is that the harness stays
//! the harness; a gateway that started driving a model would quietly undo
//! that.
//!
//! That rule is **structural here rather than promised**. No file in this
//! directory imports `crate::session`, `crate::shell`, `crate::tui` or
//! `crate::harness`, and
//! `tests::the_gateway_imports_none_of_the_modules_that_would_make_it_a_harness`
//! scans every one of them to keep it that way. A module that cannot see the
//! session model cannot own a session, and a reviewer can check that with a
//! source scan instead of reading for intent — the same move
//! `harness::no_adapter_depends_on_the_session_model` already makes for the
//! adapters.
//!
//! # What this module owns, and what the ingress owns
//!
//! Here: a listener, an address, a token, an upstream, and the moment each of
//! them stops existing. In `ingress`: what happens on one connection. In
//! `http`: the small amount of HTTP that routing needs. In [`upstream`]:
//! where a request goes and the credential it goes with.
//!
//! # Loopback, and an ephemeral port
//!
//! The listener binds `127.0.0.1:0`. Loopback is not a default waiting to be
//! overridden: there is no configuration in this module that could bind
//! anywhere else, so a gateway that is reachable from the network cannot be
//! produced by getting a setting wrong. Port `0` asks the operating system
//! for a free port and is what lets two Glasshouse instances on one machine
//! coexist — neither one names a port, so neither can contend for one. The
//! port that was actually chosen is read back with `local_addr` and kept.
//!
//! # The token is an authentication secret, not an identifier
//!
//! `session::store`'s native session identifiers come from SQLite's
//! `randomblob`, and that is right for an identifier: it needs to be unique.
//! An authentication token needs to be **unpredictable to an attacker**,
//! which is a different requirement, so this one comes from the operating
//! system's cryptographic generator via the `getrandom` crate instead — 32
//! bytes of it, rendered as hex.
//!
//! [`GatewayToken`] is then treated exactly the way
//! [`crate::secret::Secret`] treats a credential: no `Display`, no `Deref`,
//! no `AsRef<str>`, no serde, and a manual [`Debug`](std::fmt::Debug) that
//! prints [`crate::secret::REDACTED`] — the same marker, not a second one
//! invented here. It lives in memory for the lifetime of one instance and is
//! never written to a log, a diagnostic, or a file.
//!
//! # The credential the child never sees
//!
//! A gateway-backed child harness is given [`GatewayToken`] and **not** the
//! provider's key. The gateway checks that token on arrival and attaches the
//! real credential itself, from an [`upstream::Upstream`] that holds it in
//! this process's memory. So the value in the child's environment is
//! worthless off this machine and dies with the instance — which is the
//! whole of "never expose provider API keys to a child harness when the
//! local gateway can hold the credential itself".
//!
//! # Blocking threads, deliberately
//!
//! Glasshouse has no async runtime and this phase does not add one for a
//! single-user loopback proxy. One thread accepts; each accepted connection
//! gets a thread of its own and blocks on it. The cost is one thread per
//! in-flight request, which for one developer's harness is a number in the
//! low single digits.

mod http;
mod ingress;
pub mod upstream;

use std::fmt;
use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::profile::{BackendResource, LaunchProfile};
use crate::secret::REDACTED;

pub use upstream::{Route, Upstream, UpstreamError};

/// The only interface a Glasshouse gateway ever binds.
///
/// Named as a constant so that "loopback only" is one greppable fact rather
/// than a literal repeated at a call site and a test.
const GATEWAY_INTERFACE: Ipv4Addr = Ipv4Addr::LOCALHOST;

/// Ask the operating system for a free port.
///
/// This is what "multiple Glasshouse instances can coexist" actually rests
/// on: an instance that never names a port cannot collide with one that also
/// never names a port.
const EPHEMERAL_PORT: u16 = 0;

/// How much entropy the per-instance token carries.
///
/// 32 bytes — 256 bits — is the size at which guessing is not a strategy,
/// and it is the size a reader can recognise without having to do the
/// arithmetic.
const TOKEN_BYTES: usize = 32;

/// How long the accept loop sleeps between polls of the stop flag.
///
/// This is the whole shutdown mechanism's only cost, and the number is a
/// trade: it bounds how long dropping a [`Gateway`] can take, and it is how
/// often a thread wakes while a gateway-backed session is open. 25ms puts
/// shutdown well inside "immediately" for a human and costs forty wakeups a
/// second on a thread that does nothing else — see [`Gateway`] for why this
/// approach and not one of the alternatives.
const ACCEPT_POLL: Duration = Duration::from_millis(25);

/// A per-instance gateway authentication token.
///
/// Handed to child harnesses so that a request arriving at the gateway can be
/// shown to have come from *this* Glasshouse instance. It is minted fresh at
/// start, held only in memory, and dies with the instance: nothing writes it
/// to disk, so nothing can leave it behind.
///
/// Everything about this type mirrors [`crate::secret::Secret`], deliberately
/// and item for item: no `Display`, no `Deref`, no `AsRef<str>`, no
/// `Clone`, no serde, and a manual [`Debug`](std::fmt::Debug) rendering
/// [`crate::secret::REDACTED`]. The only way out is [`GatewayToken::expose`],
/// whose name is the point.
///
/// It is not *itself* a [`crate::secret::Secret`] only because that type's
/// field is private to its own module, so nothing outside `crate::secret`
/// can mint one — see this module's report for that finding. Widening that
/// module's API to make this possible was not this slice's call to make.
pub struct GatewayToken(String);

impl GatewayToken {
    /// 32 bytes from the operating system's cryptographic generator,
    /// rendered as hex.
    ///
    /// `getrandom` rather than a hand-rolled read of `/dev/urandom`,
    /// `getrandom(2)`, `BCryptGenRandom` and whatever the next platform
    /// wants: entropy is the one place where the failure mode of "it
    /// silently returned something predictable on the platform nobody
    /// tested" is unrecoverable, and that is exactly what a hand-rolled
    /// version gets wrong.
    ///
    /// The error is propagated rather than swallowed with a fallback. A
    /// gateway that started with a guessable token would be worse than one
    /// that refused to start.
    fn generate() -> Result<Self> {
        let mut bytes = [0u8; TOKEN_BYTES];
        getrandom::fill(&mut bytes)
            .context("could not read cryptographic randomness for the gateway token")?;
        Ok(Self(hex::encode(bytes)))
    }

    /// Hand the token to something that genuinely needs it — the environment
    /// of a child harness, and the ingress's own comparison against it.
    ///
    /// Every call site is a place the token leaves this type, so each one
    /// should be short-lived, obvious, and easy to count. There are two:
    /// [`crate::profile::resolve_with_gateway`], which writes it into one
    /// child process's environment, and `ingress`'s check that an arriving
    /// request carries it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Prints [`crate::secret::REDACTED`] and nothing else.
///
/// Not derived, and not a prefix, a suffix or a length either: each of those
/// narrows the space an attacker has to search, so this rendering is
/// identical for every token.
impl fmt::Debug for GatewayToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

/// A running local gateway: a bound loopback listener, the token that
/// authenticates against it, the upstream it forwards to, and the thread
/// that accepts.
///
/// # Shutdown, and why it is a polled flag
///
/// `accept` blocks, and a blocked `accept` cannot be interrupted portably.
/// Dropping a `Gateway` must still return promptly, so the listener is set
/// **non-blocking** and the accept thread polls a stop flag every
/// `ACCEPT_POLL` (25ms), then joins.
///
/// **Why this and not the alternative.** The other portable trick is to
/// connect to your own listener to wake the accept. It is worse here on
/// every platform and worst on Windows: the wake-up connection races with a
/// real client's, so the loop may accept the client and leave the wake-up in
/// the backlog; and a self-connect on Windows can be delayed or refused by
/// local filtering software, which turns "shut down" into "hang until a
/// firewall decides". Non-blocking accept, by contrast, is the same code on
/// all three platforms — `ioctlsocket(FIONBIO)` on Windows, `O_NONBLOCK`
/// elsewhere — and `WSAEWOULDBLOCK` reaches Rust as
/// [`ErrorKind::WouldBlock`] exactly as `EWOULDBLOCK` does. Nothing here is
/// conditional on the platform, so there is no platform-specific path to get
/// wrong.
///
/// The consequence that *is* platform-specific is handled where it lands: on
/// Windows and the BSDs (including macOS) an accepted socket inherits the
/// listener's non-blocking flag, while on Linux it does not — so `ingress`
/// clears it on every accepted stream rather than assuming.
///
/// # Lifetime
///
/// Dropping this value stops the accept loop, joins its thread, and with it
/// drops the listener, which releases the port. That covers a normal return
/// and an unwinding panic alike.
///
/// In-flight connection threads are **not** joined. A streaming response can
/// legitimately be minutes long, and a shutdown that waited for one would be
/// the hang this design exists to avoid; those threads own their own sockets
/// and end when their exchange does, or when the process exits.
///
/// It deliberately registers **no** [`crate::shutdown::on_forced_exit`]
/// cleanup, for two reasons that both matter. First, that hook exists for
/// resources which *survive* [`std::process::exit`] — a harness left running
/// in its own session with nothing to hang it up. A listening socket is not
/// one of those: it is a descriptor owned by this process, and process exit
/// closes it and releases the port on every platform Glasshouse supports.
/// Second, that registry holds exactly one callback, so registering here
/// would silently displace the one an attached session installs to kill its
/// harness — trading a cleanup that is unnecessary for one that is not.
#[derive(Debug)]
pub struct Gateway {
    address: SocketAddr,
    token: Arc<GatewayToken>,
    /// Shared with the accept loop rather than moved into it, so that a
    /// launch profile can ask what this gateway actually serves. Its
    /// [`Debug`](fmt::Debug) renders the credential's redaction marker, not
    /// the credential — see [`Upstream`].
    upstream: Arc<Upstream>,
    /// Set by [`Drop`]; read by the accept loop every [`ACCEPT_POLL`].
    stop: Arc<AtomicBool>,
    /// `None` only after [`Drop`] has taken it.
    accept: Option<JoinHandle<()>>,
}

impl Gateway {
    /// Bind the listener, mint the token, and start accepting.
    ///
    /// Private on purpose. [`start_if_required`] is the only way in from
    /// outside this module, which is what stops a gateway from being started
    /// by a caller that simply decided to, rather than by a profile that
    /// asked for one.
    fn start(upstream: Upstream) -> Result<Self> {
        let listener = TcpListener::bind((GATEWAY_INTERFACE, EPHEMERAL_PORT))
            .context("could not bind the local Glasshouse gateway to loopback")?;
        // Port 0 was a request, not an address. This is the answer, and it is
        // the only place the real port ever comes from.
        let address = listener
            .local_addr()
            .context("could not read the local Glasshouse gateway's bound address")?;
        listener
            .set_nonblocking(true)
            .context("could not put the local Glasshouse gateway's listener in polling mode")?;

        let token = Arc::new(GatewayToken::generate()?);
        let stop = Arc::new(AtomicBool::new(false));
        let upstream = Arc::new(upstream);

        let accept = std::thread::Builder::new()
            .name("glasshouse-gateway-accept".to_owned())
            .spawn({
                let token = Arc::clone(&token);
                let stop = Arc::clone(&stop);
                let upstream = Arc::clone(&upstream);
                move || accept_loop(listener, stop, token, upstream)
            })
            .context("could not start the local Glasshouse gateway's accept thread")?;

        Ok(Self {
            address,
            token,
            upstream,
            stop,
            accept: Some(accept),
        })
    }

    /// Where this gateway is listening: always loopback, always the port the
    /// operating system chose.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// The base URL a child harness is pointed at.
    ///
    /// A root with no path: Claude Code appends `/v1/messages` to whatever
    /// it is given — see `crate::harness::claude_code`'s `BASE_URL_ENV`,
    /// where that was observed on a real binary — and this gateway appends
    /// whatever arrives to the provider's own root in turn.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// This instance's authentication token.
    pub fn token(&self) -> &GatewayToken {
        &self.token
    }

    /// The slug of every wire protocol this gateway's ingress can actually
    /// carry.
    ///
    /// Not the same list as [`crate::profile::GATEWAY_INGRESS_PROTOCOLS`],
    /// and that difference is the point: that constant says what the ingress
    /// *knows how to serve*, while this says what the one configured
    /// provider declared a base URL for. A launch profile has to refuse
    /// against the second, or a harness would be started against an ingress
    /// that would answer its first request with a `404`.
    ///
    /// Slugs rather than a protocol enum because no file in this directory
    /// may name [`mod@crate::harness`] — see this module's header. The
    /// caller that reads them is `crate::profile`, which can.
    pub fn served_protocols(&self) -> Vec<&str> {
        self.upstream.served_protocols()
    }
}

/// Stop accepting, join, and release the port.
impl Drop for Gateway {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(accept) = self.accept.take() {
            // Joining is what makes the port released *by the time this
            // returns*: the listener is owned by the loop's stack frame, so
            // it is dropped when that frame unwinds and not before.
            let _ = accept.join();
        }
    }
}

/// Accept until told to stop, giving each connection a thread.
///
/// Errors other than [`ErrorKind::WouldBlock`] are transient by the time
/// anything can be done about them — a descriptor limit reached, a
/// connection reset between the handshake and the accept — so the loop
/// sleeps and tries again rather than dying and leaving a bound port with
/// nothing behind it.
fn accept_loop(
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    token: Arc<GatewayToken>,
    upstream: Arc<Upstream>,
) {
    // One agent for the life of the gateway: it owns the connection pool to
    // the provider, so a warm TLS connection survives from one request to
    // the next. Built here rather than per connection, which would throw
    // that away every time.
    let agent = Arc::new(upstream::agent());

    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _peer)) => {
                let token = Arc::clone(&token);
                let upstream = Arc::clone(&upstream);
                let agent = Arc::clone(&agent);
                let spawned = std::thread::Builder::new()
                    .name("glasshouse-gateway-exchange".to_owned())
                    .spawn(move || {
                        ingress::serve(stream, &token, &upstream, &agent).record();
                    });
                if spawned.is_err() {
                    // No thread to serve it: the connection closes as the
                    // stream drops. Better than blocking the accept loop.
                    tracing::debug!("the Glasshouse gateway could not start a connection thread");
                }
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => std::thread::sleep(ACCEPT_POLL),
            Err(_) => std::thread::sleep(ACCEPT_POLL),
        }
    }
}

/// Whether any of these launch profiles needs a local gateway.
///
/// This is the whole of "start the local gateway only when at least one
/// active launch profile requires it", and it is deliberately a function of
/// the **profiles** rather than of a flag someone remembered to set. A flag
/// can drift from the configuration it was meant to summarise; a predicate
/// read straight off [`BackendResource`] cannot.
///
/// A profile requires the gateway exactly when its backend *is* the gateway.
/// [`BackendResource::Native`] and [`BackendResource::DirectProvider`] reach
/// their backends without one, so neither should cause a socket to exist.
pub fn gateway_is_required(profiles: &[LaunchProfile]) -> bool {
    profiles
        .iter()
        .any(|profile| matches!(profile.backend, BackendResource::GlasshouseGateway))
}

/// Start a gateway if — and only if — one of `profiles` requires it.
///
/// `Ok(None)` means no active profile asked for a gateway, and so **no
/// listener was bound at all**. That absence is the behaviour, not an
/// optimisation of it.
///
/// `upstream` is a closure rather than a value because building one costs a
/// provider lookup and a credential resolution, and a launch that needs no
/// gateway must pay for neither. It is called at most once, and only after
/// the predicate has already said yes.
pub fn start_if_required(
    profiles: &[LaunchProfile],
    upstream: impl FnOnce() -> Result<Upstream>,
) -> Result<Option<Gateway>> {
    if !gateway_is_required(profiles) {
        return Ok(None);
    }
    Gateway::start(upstream()?).map(Some)
}

#[cfg(test)]
mod conformance;
#[cfg(test)]
mod fixture;

#[cfg(test)]
mod tests {
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
        vec![
            ("gateway/mod.rs", include_str!("mod.rs")),
            ("gateway/http.rs", include_str!("http.rs")),
            ("gateway/ingress.rs", include_str!("ingress.rs")),
            ("gateway/upstream.rs", include_str!("upstream.rs")),
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
        let started =
            start_if_required(&profiles, || Ok(anthropic_upstream_to(&fixture.base_url())))
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
                b"POST /v1/messages HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2\r\n\r\n{}"
                    .to_vec()
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
            let _ =
                out.write_all(format!("{:x}\r\n{second}\r\n0\r\n\r\n", second.len()).as_bytes());
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

        let rebound = TcpListener::bind(address);
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
        let tested =
            "fn start() {}\n#[cfg(test)]\nmod tests { use crate::session::SessionLifecycle; }";
        assert!(!production_code(tested).contains("crate::session"));
        // ... and the file list it runs over is not empty, which would make
        // every assertion in it vacuous.
        assert_eq!(gateway_sources().len(), 4);
    }

    /// No file in this directory may deserialize anything. The whole of
    /// "preserve tool-call payloads without lossy rewriting" and "keep the
    /// first gateway implementation protocol pass-through" rests on nothing
    /// here ever looking at a body, so a serialization crate reaching this
    /// directory is the change that would quietly undo both — and it is the
    /// change that would look most reasonable in a diff ("just read
    /// `error.type` for the log").
    ///
    /// A scan cannot prove the absence of a hand-rolled parser, and this one
    /// does not claim to. What it does catch is the realistic version: the
    /// `use serde_json` that a body inspection would be written on top of.
    #[test]
    fn no_part_of_the_gateway_deserializes_anything() {
        const FORBIDDEN: [&str; 5] = [
            "serde_json",
            "serde::",
            "Deserialize",
            "from_str::<",
            "toml::",
        ];
        for (name, source) in gateway_sources() {
            let code = production_code(source);
            for forbidden in FORBIDDEN {
                assert!(
                    !code.contains(forbidden),
                    "{name} names `{forbidden}` in production code: the gateway has started \
                     looking at a body it is supposed to be unable to distinguish from any \
                     other bytes"
                );
            }
        }

        // ... and the scan fires on the change it exists to catch, rather
        // than passing because the needle was misspelled.
        let violating = production_code("use serde_json::Value;\nfn peek() {}");
        assert!(FORBIDDEN.iter().any(|needle| violating.contains(needle)));
    }
}
