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
//! where a request goes and the credential it goes with. In [`translate`]:
//! the one branch of the ingress that may parse a body — a target the
//! provider does not serve, for a pair the table supports (Phase 56).
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
pub mod session;
pub mod translate;
pub mod upstream;

use std::fmt;
use std::io::{ErrorKind, Read};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::profile::{BackendResource, LaunchProfile};
use crate::routing::free::FreeResource;
use crate::routing::interactive::Assignment;
use crate::secret::REDACTED;

pub use session::SessionRouting;
pub use upstream::{Route, Upstream, UpstreamBackend, UpstreamError};

/// Told once per exchange whose outcome says the gateway's own upstream
/// failed — map line 1735, "detect gateway failure separately from harness
/// process failure" — with the resource's
/// [`crate::profile::BackendResource::slug`] and which kind of failure it
/// was.
///
/// A closure rather than a direct call to [`crate::events::degrade_resource`]
/// from inside this module, because that function's
/// `records: &[crate::session::SessionRecord]` parameter would require
/// naming `crate::session` here — exactly the import
/// `tests::the_gateway_imports_none_of_the_modules_that_would_make_it_a_harness`
/// exists to make impossible (see this module's header). The caller that
/// builds one closes over an [`crate::events::EventBus`] and a live session
/// list; this module only ever calls it with a resource name and a
/// [`crate::events::GatewayFailure`].
pub type DegradeSink = Arc<dyn Fn(&str, crate::events::GatewayFailure) + Send + Sync>;

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
    /// Which backend is serving this session, what has moved it, and what
    /// real work has said about each resource — Phase 9H and Phase 9I.
    ///
    /// Shared with every connection thread rather than owned by the accept
    /// loop, because a launch profile binds the assignment into it from the
    /// main thread while connection threads observe into it. It holds no
    /// credential value: an assignment names a credential, and
    /// [`crate::routing::CredentialId`] is two names.
    routing: Arc<SessionRouting>,
    /// Set by [`Drop`]; read by the accept loop every [`ACCEPT_POLL`].
    stop: Arc<AtomicBool>,
    /// `None` only after [`Drop`] has taken it.
    accept: Option<JoinHandle<()>>,
}

impl Gateway {
    /// Bind the listener, mint the token, and start accepting, with no
    /// durable quota cache — capability map line 1229's gateway half stays
    /// in memory only, exactly as before this package.
    ///
    /// Private on purpose. [`start_if_required`] is the only way in from
    /// outside this module, which is what stops a gateway from being started
    /// by a caller that simply decided to, rather than by a profile that
    /// asked for one.
    fn start(upstream: Upstream) -> Result<Self> {
        Self::start_with_quota_cache(upstream, None)
    }

    /// [`Self::start`], with a [`crate::provider::telemetry::GatewayQuotaCache`]
    /// a real forwarded exchange's rate-limit headers are persisted to —
    /// capability map lines 1217/1218/1229's bridge across the process
    /// boundary between this gateway and a later `glasshouse resources`
    /// invocation.
    ///
    /// `None` reproduces [`Self::start`] exactly: nothing is ever written to
    /// disk, and every existing caller of [`Self::start`] — including every
    /// test in [`super::conformance`] that runs a real accept loop — is
    /// unaffected. **No caller resolves
    /// [`crate::paths::RuntimePaths::resolve`] here, and none may be added
    /// here**: this module has never had a project or a data directory in
    /// scope, and a gateway that resolved its own OS-standard directory
    /// would write into whichever machine happens to be running `cargo test`
    /// every time a conformance test forwards a request with a rate-limit
    /// header — see [`crate::provider::telemetry::GatewayQuotaCache`]'s own
    /// doc for why that is the wrong owner for the resolve step. A caller
    /// that wants persistence resolves its own
    /// [`crate::paths::RuntimePaths`] and hands this a
    /// [`crate::provider::telemetry::GatewayQuotaCache::new`] built from it.
    ///
    /// Private on purpose, exactly as [`Self::start`] is: reached from
    /// outside this module only through [`start_if_required_with_quota_cache`].
    fn start_with_quota_cache(
        upstream: Upstream,
        quota_cache: Option<crate::provider::telemetry::GatewayQuotaCache>,
    ) -> Result<Self> {
        Self::start_with_telemetry(upstream, quota_cache, None, None)
    }

    /// [`Self::start_with_quota_cache`], with a
    /// [`crate::routing::evidence::EvidenceLedger`] every real forwarded
    /// exchange that has been bound to an assignment is recorded to —
    /// capability map Phase 33A, this package's own production producer. See
    /// [`crate::gateway::session::SessionRouting::record_routing_observation`]
    /// for exactly what is and is not recorded from one exchange.
    ///
    /// `None` reproduces [`Self::start_with_quota_cache`] exactly — the same
    /// additive guarantee that constructor already gives
    /// [`Self::start`], and for the same reason: this module has never had a
    /// project or a data directory in scope (see
    /// [`Self::start_with_quota_cache`]'s own doc), so a caller that wants a
    /// durable evidence ledger resolves its own [`crate::Runtime`] and hands
    /// this an already-opened
    /// [`crate::routing::evidence::EvidenceLedger::open`].
    ///
    /// **Not called from `crates/glasshouse/src/main.rs` today** — the same
    /// gap [`Self::start_with_quota_cache`]'s own doc records for the quota
    /// cache, and for the identical reason: `main.rs` is this package's
    /// `FORBIDDEN FILES`. See the report for the exact patch.
    ///
    /// `health_cache` is [`crate::provider::telemetry::GatewayHealthCache`],
    /// capability map lines 1311/1321/1322/1324's own bridge, additive the
    /// identical way `quota_cache` is: `None` writes nothing and reproduces
    /// this function's pre-health-cache behaviour exactly, so every existing
    /// caller — including every [`super::conformance`] test that does not
    /// pass one — is unaffected.
    ///
    /// Private on purpose: reached from outside this module only through
    /// [`start_if_required_with_telemetry`].
    fn start_with_telemetry(
        upstream: Upstream,
        quota_cache: Option<crate::provider::telemetry::GatewayQuotaCache>,
        evidence_ledger: Option<Arc<crate::routing::evidence::EvidenceLedger>>,
        health_cache: Option<crate::provider::telemetry::GatewayHealthCache>,
    ) -> Result<Self> {
        Self::start_with_degrade_sink(
            upstream,
            quota_cache,
            evidence_ledger,
            health_cache,
            None,
            None,
        )
    }

    /// [`Self::start_with_telemetry`], with a [`DegradeSink`] told about every
    /// exchange whose outcome is a genuine gateway failure — map line 1735.
    ///
    /// `None` reproduces [`Self::start_with_telemetry`] exactly, the same
    /// additive guarantee every sink before it gives: every existing caller,
    /// including every test in [`super::conformance`], is unaffected.
    ///
    /// **Not called from `crates/glasshouse/src/main.rs` today** — the same
    /// gap this module's other telemetry sinks record, and for the identical
    /// reason: `main.rs` is this package's `FORBIDDEN FILES`. See the report
    /// for the closure `main.rs` would need to build there, capturing its own
    /// `EventBus` and session records so it can call
    /// [`crate::events::degrade_resource`] itself.
    ///
    /// Private on purpose, exactly as [`Self::start_with_telemetry`] is:
    /// reached from outside this module only through
    /// [`start_if_required_with_degrade_sink`].
    fn start_with_degrade_sink(
        upstream: Upstream,
        quota_cache: Option<crate::provider::telemetry::GatewayQuotaCache>,
        evidence_ledger: Option<Arc<crate::routing::evidence::EvidenceLedger>>,
        health_cache: Option<crate::provider::telemetry::GatewayHealthCache>,
        degrade_sink: Option<DegradeSink>,
        prevention_sink: Option<session::FailoverPreventionSink>,
    ) -> Result<Self> {
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
        let routing = Arc::new(SessionRouting::new());
        let quota_cache = quota_cache.map(Arc::new);
        let health_cache = health_cache.map(Arc::new);

        let accept = std::thread::Builder::new()
            .name("glasshouse-gateway-accept".to_owned())
            .spawn({
                let token = Arc::clone(&token);
                let stop = Arc::clone(&stop);
                let upstream = Arc::clone(&upstream);
                let routing = Arc::clone(&routing);
                let quota_cache = quota_cache.clone();
                let evidence_ledger = evidence_ledger.clone();
                let health_cache = health_cache.clone();
                let degrade_sink = degrade_sink.clone();
                let prevention_sink = prevention_sink.clone();
                move || {
                    accept_loop(
                        listener,
                        stop,
                        token,
                        upstream,
                        routing,
                        quota_cache,
                        evidence_ledger,
                        health_cache,
                        degrade_sink,
                        prevention_sink,
                    )
                }
            })
            .context("could not start the local Glasshouse gateway's accept thread")?;

        Ok(Self {
            address,
            token,
            upstream,
            routing,
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

    /// Which backend is serving this session, and everything that has moved
    /// it — Phase 9H.
    ///
    /// The gateway holds this rather than owning any of the decisions in it:
    /// [`mod@crate::routing::interactive`] decides, [`session`] applies, and
    /// this is where a launch profile and a settings screen reach both.
    pub fn routing(&self) -> &SessionRouting {
        &self.routing
    }

    /// The most recent rate-limit headers a real forwarded response carried,
    /// and when they were observed — capability map line 1229's gateway
    /// half, the `ingress` module's own reading, passed through unread
    /// except for the allowlist [`crate::provider::telemetry`] already
    /// parses.
    ///
    /// `None` until this gateway has forwarded at least one request whose
    /// response carried a rate-limit header this reader understands. A
    /// passive reader: nothing here makes a request of its own, ever — see
    /// the module documentation's "the gateway forwards headers without
    /// reading them" history, now narrowed to the body.
    pub fn quota_headers(&self) -> Option<(crate::provider::telemetry::RateLimitHeaders, i64)> {
        self.routing.quota_headers()
    }

    /// The upstream this gateway forwards through, for a caller that needs to
    /// name one of its backends — a migration, or a settings screen listing
    /// what a session could move to.
    ///
    /// No credential comes out with it: [`Upstream`] has no accessor for one.
    pub fn upstream(&self) -> &Upstream {
        &self.upstream
    }

    /// The name of the provider this gateway is currently forwarding to —
    /// map line 1954's gateway shape: a launch backed by
    /// [`crate::profile::BackendResource::GlasshouseGateway`] does not know
    /// which entitlement it will charge until this gateway has resolved and
    /// started its upstream, and this is the one fact the launch path needs
    /// to ask `EffectiveConfig::entitlement_for_provider` the same question
    /// the direct-provider path asks before it ever exists.
    ///
    /// Delegates to `Upstream::serving`, which stays `pub(super)`: nothing
    /// else about the upstream — its routes, its credential — is exposed
    /// here or anywhere outside this module.
    pub fn serving_provider(&self) -> &str {
        self.upstream.serving().provider()
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
///
/// Nine parameters, all of them either identity (`listener`, `token`), the
/// two coordination handles (`stop`, `upstream`, `routing`) already threaded
/// through before this package, or one of four additive, independently
/// optional telemetry sinks. Grouping the sinks into a struct would trade one
/// clippy lint for an abstraction with a single call site and nothing else to
/// say about itself.
#[allow(clippy::too_many_arguments)]
fn accept_loop(
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    token: Arc<GatewayToken>,
    upstream: Arc<Upstream>,
    routing: Arc<SessionRouting>,
    quota_cache: Option<Arc<crate::provider::telemetry::GatewayQuotaCache>>,
    evidence_ledger: Option<Arc<crate::routing::evidence::EvidenceLedger>>,
    health_cache: Option<Arc<crate::provider::telemetry::GatewayHealthCache>>,
    degrade_sink: Option<DegradeSink>,
    prevention_sink: Option<session::FailoverPreventionSink>,
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
                let routing = Arc::clone(&routing);
                let quota_cache = quota_cache.clone();
                let evidence_ledger = evidence_ledger.clone();
                let health_cache = health_cache.clone();
                let degrade_sink = degrade_sink.clone();
                let prevention_sink = prevention_sink.clone();
                let spawned = std::thread::Builder::new()
                    .name("glasshouse-gateway-exchange".to_owned())
                    .spawn(move || {
                        // Phase 33A's own honest caveat, named where it is
                        // stamped rather than only in `routing::evidence`'s
                        // doc: this is the instant the connection was handed
                        // to `ingress::serve`, not the instant a request left
                        // for the provider — the true dispatch instant lives
                        // inside `ingress::forward`, outside this partition.
                        let dispatched_at = crate::provider::cache::now_unix_seconds();
                        // The assignment as of the same instant, so a bind or
                        // re-bind that lands while this exchange is in
                        // flight cannot be attributed to it — see
                        // `SessionRouting::record_routing_observation`'s own
                        // doc for the defect this snapshot closes.
                        let dispatched_assignment = routing.assignment();
                        // Capability map line 1368: consult the cooldown this
                        // very loop already recorded before spending an
                        // upstream request on a route whose declared cadence
                        // says the request will predictably fail.
                        // `observe_exchange` below only ever runs *after* an
                        // exchange completes, so without this check the
                        // resource it just cooled down stays cooled down in
                        // the pool while the very next connection dials it
                        // anyway.
                        if let Some(wait) =
                            paced_refusal(&routing, &upstream, dispatched_assignment.as_ref())
                        {
                            refuse_paced(stream, wait);
                            return;
                        }
                        let (exchange, quota) = ingress::serve(stream, &token, &upstream, &agent);
                        // The exchange is genuinely over here: every byte of
                        // the response has been relayed. Stamped before
                        // anything below it so nothing added later can push
                        // this reading later than the real completion.
                        let completed_at = crate::provider::cache::now_unix_seconds();
                        // Phase 9H and 9I's production feed. After the
                        // exchange, so the routing lock is never held across
                        // the provider hop, and before the log line, so a
                        // failover the exchange caused is already recorded
                        // when its own record is read. `evidence_ledger` and
                        // `completed_at` are the same values
                        // `record_routing_observation` below is given —
                        // Phase 9J and Phase 33A's one production consumer
                        // reads the very observations this loop's own writes
                        // produce.
                        let observed_at_instant = std::time::Instant::now();
                        // Capability map line 1319's missing wire. `quota` is
                        // this very response's own rate-limit headers, bound
                        // seventeen lines above and — before this — used only
                        // for capacity telemetry further down. A provider that
                        // answered `429` and said how long to wait has stated
                        // a temporary scheduling block, and
                        // `routing::free::ResourceHealth::fail` treats a
                        // stated wait as authoritative rather than as one more
                        // failure to count. `session::stated_retry_after`
                        // narrows the headers to that one duration; nothing
                        // else from them travels into a routing decision.
                        // What this exchange did to the assignment is kept
                        // for its own evidence row below — capability map
                        // line 1334's `failovers`, known here and nowhere
                        // else, because this is the thread that decided it.
                        let effect = routing.observe_exchange(
                            &upstream,
                            &exchange,
                            observed_at_instant,
                            evidence_ledger.as_deref(),
                            completed_at,
                            session::stated_retry_after(&quota),
                            // Capability map line 1851's write side. `None`
                            // reproduces this loop's behaviour exactly as it
                            // was before this package, the same additive
                            // shape every other sink here follows.
                            prevention_sink.as_ref(),
                        );
                        // Map line 1735: detect a gateway failure separately
                        // from a harness process failure. `session::classify`
                        // above already folded this exchange into routing
                        // health and failover; this is the same exchange
                        // asked the opposite-consequence question — not "does
                        // this session need to move", but "is the resource
                        // itself unhealthy" — and answered without touching
                        // any session's lifecycle, because nothing here calls
                        // anything that could.
                        if let Some(reason) = session::gateway_failure(&exchange)
                            && let Some(sink) = &degrade_sink
                        {
                            sink(&BackendResource::GlasshouseGateway.slug(), reason);
                        }
                        // Phase 33A's production producer — see
                        // `crate::gateway::session::SessionRouting::record_routing_observation`
                        // for exactly what this can and cannot supply.
                        //
                        // `quota` is borrowed here for line 1364/1365's
                        // throttle-versus-exhausted-quota reading and moved
                        // into `observe_quota_headers` below unchanged; see
                        // `session::ExchangeReading::quota` for why this
                        // borrow is a record and not a routing decision.
                        if let Some(ledger) = &evidence_ledger {
                            routing.record_routing_observation(
                                ledger,
                                &exchange,
                                session::ExchangeReading {
                                    quota: &quota,
                                    dispatched_at_unix: dispatched_at,
                                    completed_at_unix: completed_at,
                                    assignment: dispatched_assignment,
                                    effect,
                                },
                            );
                        }
                        // Capability map line 1229's gateway half — a passive
                        // reader, not a prober: this fires only when a real
                        // session actually forwards a request through this
                        // gateway, and `observe_quota_headers` itself is the
                        // one place `is_empty()` is checked, so an ordinary
                        // exchange that carried no rate-limit header is a
                        // silent no-op rather than a cleared reading.
                        let now = crate::provider::cache::now_unix_seconds();
                        // The durable half of the same reading — capability
                        // map lines 1217/1218/1229's bridge across the
                        // process boundary, see
                        // `GatewayQuotaCache::store`'s own doc. `exchange`
                        // already names the configured provider this
                        // response came from; nothing else in this crate
                        // knows that at the point a reading is captured.
                        if let Some(cache) = &quota_cache {
                            cache.store(&exchange.provider, &quota, now);
                        }
                        routing.observe_quota_headers(quota, now);
                        // Capability map lines 1311/1321/1322/1324's gateway
                        // half, symmetric with the quota write immediately
                        // above rather than folded into `observe_exchange`
                        // itself: the health this exchange just updated is
                        // read back out of `routing` (already mutated by
                        // `observe_exchange` above) and persisted for
                        // whichever provider this exchange named.
                        if let Some(cache) = &health_cache {
                            let readings = routing.health_readings_for(
                                &exchange.provider,
                                observed_at_instant,
                                now,
                            );
                            cache.store(&exchange.provider, &readings, now);
                        }
                        exchange.record();
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

/// The wait to refuse this connection with, if the resource the session is
/// currently assigned to is still inside a wait its provider itself declared
/// — capability map line 1368. `None` means the accept loop should serve
/// normally.
///
/// **Deliberately narrower than [`FreePool::is_available`].** That check
/// alone cannot be the guard here, because it folds two different kinds of
/// cooldown into one bool: a provider's own declared wait, which line 1319
/// makes authoritative, and a bounded cooldown Glasshouse *invents* after
/// ordinary repeated failures. Phase 9I line 534 and
/// [`routing::free`](crate::routing::free)'s own `MAX_COOLDOWN` doc make the
/// second kind deliberately still probed by real work — "the only way to
/// find out ... is to let real work try it" — and
/// `gateway::conformance::a_pinned_session_stays_on_its_failing_provider_and_never_reaches_the_other_one`
/// pins that: three ordinary `503`s must all still reach the provider. Only
/// the first kind is what line 1368 asks to stop retrying in place, so this
/// reads the most recent rate-limit headers this gateway observed —
/// [`SessionRouting::quota_headers`], already public for capability map line
/// 1229 — rather than trusting the pool's bool to say why it is `false`.
///
/// A sibling credential of the same provider is still offered the chance to
/// serve in its place first; deciding to actually rotate to it is
/// [`session::SessionRouting::observe_exchange`]'s own job, on the exchange
/// that runs, and this only asks whether one exists, so it never mutates the
/// assignment itself.
fn paced_refusal(
    routing: &SessionRouting,
    upstream: &Upstream,
    assignment: Option<&Assignment>,
) -> Option<Duration> {
    let assignment = assignment?;
    let resource = FreeResource::new(
        assignment.backend().credential().clone(),
        assignment.backend().model().label(),
    );
    let now = std::time::Instant::now();
    let pool = routing.free_pool();
    if pool.is_available(&resource, now) {
        return None;
    }
    let (headers, observed_at_unix) = routing.quota_headers()?;
    let declared_seconds = headers.retry_after_seconds()?;
    let now_unix = crate::provider::cache::now_unix_seconds();
    let remaining = observed_at_unix + declared_seconds - now_unix;
    if remaining <= 0 {
        // The declared wait has already elapsed; `is_available` will catch
        // up once a real exchange observes it, and nothing here should
        // refuse a request the provider never asked to wait on any more.
        return None;
    }
    let siblings = upstream.credentials_of(assignment.provider());
    if pool
        .rotate_from(resource.credential(), &siblings, resource.model(), now)
        .is_some()
    {
        return None;
    }
    Some(Duration::from_secs(remaining as u64))
}

/// Answer `429` on `stream` without dialling upstream at all — capability
/// map line 1368's refusal. `wait` is the provider-declared wait
/// [`paced_refusal`] read back from [`SessionRouting::quota_headers`],
/// carried back as this gateway's own `Retry-After` rather than a fabricated
/// header.
fn refuse_paced(mut stream: std::net::TcpStream, wait: Duration) {
    // Drained before responding, the same reason `ingress::settle` drains
    // before closing a refusal there: closing a socket with the client's
    // own bytes still unread resets the connection instead of ending it
    // cleanly, and the harness would see a network error instead of this
    // response's 429.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let mut drained = [0u8; 8192];
    let _ = stream.read(&mut drained);

    let headers = vec![
        ("connection".to_owned(), b"close".to_vec()),
        (
            "retry-after".to_owned(),
            wait.as_secs().to_string().into_bytes(),
        ),
    ];
    let _ = http::write_head(
        &mut stream,
        ureq::http::StatusCode::TOO_MANY_REQUESTS,
        &headers,
    );
    let _ = stream.shutdown(std::net::Shutdown::Both);
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

/// [`start_if_required`], with a
/// [`crate::provider::telemetry::GatewayQuotaCache`] a started gateway
/// persists every real forwarded exchange's rate-limit headers to.
///
/// **Not called from `crates/glasshouse/src/main.rs` today.** That file's
/// two launch paths (`launch_session` and the resume path,
/// `overlay_resolution`) both still call plain [`start_if_required`], which
/// this function reproduces exactly when `quota_cache` is `None`. Wiring a
/// real reading into `glasshouse resources` needs both of those call sites
/// changed to this function, with
/// `Some(crate::provider::telemetry::GatewayQuotaCache::new(runtime.paths()))`
/// — `runtime` is already in scope at both, since
/// `UserConfig::load(runtime.paths())` is the first line of
/// `main.rs::resources_report`'s own read of the same [`crate::paths::RuntimePaths`].
/// `crates/glasshouse/src/main.rs` is this package's `FORBIDDEN FILES`; see
/// the report.
pub fn start_if_required_with_quota_cache(
    profiles: &[LaunchProfile],
    upstream: impl FnOnce() -> Result<Upstream>,
    quota_cache: Option<crate::provider::telemetry::GatewayQuotaCache>,
) -> Result<Option<Gateway>> {
    if !gateway_is_required(profiles) {
        return Ok(None);
    }
    Gateway::start_with_quota_cache(upstream()?, quota_cache).map(Some)
}

/// [`start_if_required_with_quota_cache`], with a
/// [`crate::routing::evidence::EvidenceLedger`] a started gateway records
/// every bound, provider-reaching exchange to — Phase 33A — and a
/// [`crate::provider::telemetry::GatewayHealthCache`] it persists every
/// bound exchange's resource health to — capability map lines
/// 1311/1321/1322/1324.
///
/// `crates/glasshouse/src/main.rs` calls this at both of its gateway launch
/// sites (`launch_session` and the resume path, `overlay_resolution`),
/// passing a real [`crate::provider::telemetry::GatewayQuotaCache::new`] and
/// [`crate::provider::telemetry::GatewayHealthCache::new`] built from the
/// same [`crate::paths::RuntimePaths`] `UserConfig::load(runtime.paths())`
/// already resolves there.
pub fn start_if_required_with_telemetry(
    profiles: &[LaunchProfile],
    upstream: impl FnOnce() -> Result<Upstream>,
    quota_cache: Option<crate::provider::telemetry::GatewayQuotaCache>,
    evidence_ledger: Option<Arc<crate::routing::evidence::EvidenceLedger>>,
    health_cache: Option<crate::provider::telemetry::GatewayHealthCache>,
) -> Result<Option<Gateway>> {
    if !gateway_is_required(profiles) {
        return Ok(None);
    }
    Gateway::start_with_telemetry(upstream()?, quota_cache, evidence_ledger, health_cache).map(Some)
}

/// [`start_if_required_with_telemetry`], with a [`DegradeSink`] a started
/// gateway calls once per exchange whose outcome is a genuine gateway
/// failure — map line 1735, "detect gateway failure separately from harness
/// process failure."
///
/// `None` reproduces [`start_if_required_with_telemetry`] exactly, the same
/// additive guarantee every sink on this door already gives.
///
/// `crates/glasshouse/src/main.rs` calls this at **both** of its gateway
/// launch sites — `launch_session` and the resume path's
/// `resolve_resume_overlay` — and passes a real sink at each.
///
/// # The ownership answer, because the obvious one does not compile
///
/// A sink needs an `EventBus` and a session list, and neither exists when
/// either site starts its gateway: the launch path opens its `EventRecorder`
/// 184 lines later, and has no `SessionRecord` at all until the store has
/// created one. So the sink cannot close over them. `main.rs::DegradeRelay`
/// is what it closes over instead — a handle created before the gateway and
/// filled once both halves exist, which holds any failure that arrives in
/// between and replays it on installation. A failure in that window is
/// therefore neither a panic nor a silent loss, and nothing on this start
/// path waits for the recorder to be ready.
pub fn start_if_required_with_degrade_sink(
    profiles: &[LaunchProfile],
    upstream: impl FnOnce() -> Result<Upstream>,
    quota_cache: Option<crate::provider::telemetry::GatewayQuotaCache>,
    evidence_ledger: Option<Arc<crate::routing::evidence::EvidenceLedger>>,
    health_cache: Option<crate::provider::telemetry::GatewayHealthCache>,
    degrade_sink: Option<DegradeSink>,
    // Told what the failure-domain term did to each failover this gateway
    // takes — capability map line 1851. `None` reproduces the behaviour this
    // door had before that line's producer landed, exactly as `degrade_sink`
    // above does for line 1735.
    prevention_sink: Option<session::FailoverPreventionSink>,
) -> Result<Option<Gateway>> {
    if !gateway_is_required(profiles) {
        return Ok(None);
    }
    Gateway::start_with_degrade_sink(
        upstream()?,
        quota_cache,
        evidence_ledger,
        health_cache,
        degrade_sink,
        prevention_sink,
    )
    .map(Some)
}

#[cfg(test)]
mod conformance;
#[cfg(test)]
mod fixture;

#[cfg(test)]
mod tests;
