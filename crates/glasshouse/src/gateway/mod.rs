//! The local Glasshouse gateway: the process, not yet its protocol (Phase 9G,
//! first slice).
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
//! That rule is **structural here rather than promised**. This module imports
//! none of `crate::session`, `crate::shell`, `crate::tui` or
//! `crate::harness`, and
//! `tests::the_gateway_imports_none_of_the_modules_that_would_make_it_a_harness`
//! scans this file's own production source to keep it that way. A module that cannot see the session model
//! cannot own a session, and a reviewer can check that with a source scan
//! instead of reading for intent — the same move
//! `harness::no_adapter_depends_on_the_session_model` already makes for the
//! adapters.
//!
//! # What this slice owns: existence and lifetime
//!
//! A listener, an address, a token, and the moment each of them stops
//! existing. **No ingress.** There is no HTTP parser here, no framework, no
//! request handling, no streaming, no tool-call payload and no error mapping;
//! those need the protocol design, which is a later slice. Nothing accepts a
//! connection — see [`Gateway`] for exactly what a connection therefore does.
//!
//! # Loopback, and an ephemeral port
//!
//! The listener binds `127.0.0.1:0`. Loopback is not a default waiting to be
//! overridden: there is no configuration in this slice that could bind
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

use std::fmt;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};

use anyhow::{Context, Result};

use crate::profile::{BackendResource, LaunchProfile};
use crate::secret::REDACTED;

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
    /// of a child harness, and essentially nothing else.
    ///
    /// Every call site is a place the token leaves this module, so each one
    /// should be short-lived, obvious, and easy to count. There are none yet:
    /// handing the token to a harness belongs to the ingress slice, which is
    /// where a caller will first appear.
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

/// A running local gateway: a bound loopback listener and the token that
/// authenticates against it.
///
/// # Nothing accepts, and what that means for a connection
///
/// This slice owns the listener's existence, not its traffic, so no thread
/// calls `accept`. That is not the same as refusing a connection: the
/// kernel completes the TCP handshake into the listen backlog by itself, so
/// a client's `connect` **succeeds** and then sits there. What the client
/// never gets is a byte — the gateway writes nothing, because there is
/// nothing here to write anything — and when the instance exits the socket
/// closes underneath it and the connection is reset. Both halves of that are
/// asserted below rather than asserted about.
///
/// # Shutdown
///
/// The listener is dropped when the owning instance exits, which is all
/// "shut down the gateway when the owning instance exits" needs to mean
/// here. The rest of that line — "and no detached sessions depend on it" —
/// is **trivially true in this slice**, because nothing can yet depend on
/// the gateway: there is no ingress, so no session has ever reached it. A
/// dependency count that could only ever answer zero would be a mechanism
/// with no real input, which is worse than this sentence.
///
/// Teardown rides [`crate::shutdown`]'s existing model rather than adding a
/// second one, and lands on the RAII half of it: this value is a guard, and
/// dropping it closes the socket, which covers a normal return and an
/// unwinding panic alike. It deliberately registers **no**
/// [`crate::shutdown::on_forced_exit`] cleanup, for two reasons that both
/// matter. First, that hook exists for resources which *survive*
/// [`std::process::exit`] — a harness left running in its own session with
/// nothing to hang it up. A listening socket is not one of those: it is a
/// descriptor owned by this process, and process exit closes it and releases
/// the port on every platform Glasshouse supports. Second, that registry
/// holds exactly one callback, so registering here would silently displace
/// the one an attached session installs to kill its harness — trading a
/// cleanup that is unnecessary for one that is not.
#[derive(Debug)]
pub struct Gateway {
    /// Held for its lifetime, not for its traffic. Dropping this closes the
    /// socket and releases the port.
    _listener: TcpListener,
    address: SocketAddr,
    token: GatewayToken,
}

impl Gateway {
    /// Bind the listener and mint the token.
    ///
    /// Private on purpose. [`start_if_required`] is the only way in from
    /// outside this module, which is what stops a gateway from being started
    /// by a caller that simply decided to, rather than by a profile that
    /// asked for one.
    fn start() -> Result<Self> {
        let listener = TcpListener::bind((GATEWAY_INTERFACE, EPHEMERAL_PORT))
            .context("could not bind the local Glasshouse gateway to loopback")?;
        // Port 0 was a request, not an address. This is the answer, and it is
        // the only place the real port ever comes from.
        let address = listener
            .local_addr()
            .context("could not read the local Glasshouse gateway's bound address")?;
        let token = GatewayToken::generate()?;
        Ok(Self {
            _listener: listener,
            address,
            token,
        })
    }

    /// Where this gateway is listening: always loopback, always the port the
    /// operating system chose.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// This instance's authentication token.
    pub fn token(&self) -> &GatewayToken {
        &self.token
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
/// `Ok(None)` is the ordinary answer today and is not a failure: it means no
/// active profile asked for a gateway, and so **no listener was bound at
/// all**. That absence is the behaviour, not an optimisation of it.
///
/// Note honestly that [`crate::profile::resolve`] still refuses a
/// [`BackendResource::GlasshouseGateway`] profile — Phase 9F left it refused
/// and named 9G as the phase that lifts it — so while such a profile is
/// reachable from configuration and this predicate answers `true` for it, a
/// launch carrying one does not get far enough to run. This slice builds the
/// process; it does not open the path, and it does not touch that refusal.
pub fn start_if_required(profiles: &[LaunchProfile]) -> Result<Option<Gateway>> {
    if !gateway_is_required(profiles) {
        return Ok(None);
    }
    Gateway::start().map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Read;
    use std::net::TcpStream;
    use std::time::Duration;

    use crate::integrations::IntegrationId;

    /// This module's own source with its `#[cfg(test)]` block excluded and
    /// `//` comments stripped — the idiom `harness/mod.rs` introduced and
    /// that `main.rs`, `shim.rs`, `secret/mod.rs` and `session/lifecycle.rs`
    /// each keep their own copy of. This is the sixth: the copies are local
    /// on purpose, because each one scans the file it lives in.
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

    /// A profile with the given backend, for the start predicate.
    fn profile_backed_by(backend: BackendResource) -> LaunchProfile {
        let mut profile = LaunchProfile::native(IntegrationId::ClaudeCode);
        profile.backend = backend;
        profile
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
    /// how a redacted field gets printed anyway.
    #[test]
    fn debug_on_a_gateway_never_reaches_its_token() {
        let gateway = Gateway::start().expect("loopback is bindable");
        let rendered = format!("{gateway:?}");
        assert!(
            !rendered.contains(gateway.token().expose()),
            "the gateway's own Debug leaked its token"
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

    // --- line 4: the profiles decide, not a flag --------------------------

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
    #[test]
    fn no_profile_needing_a_gateway_binds_no_listener_at_all() {
        let profiles = [
            profile_backed_by(BackendResource::Native),
            profile_backed_by(BackendResource::DirectProvider {
                provider: "openrouter".to_owned(),
            }),
        ];
        let started = start_if_required(&profiles).expect("deciding not to start cannot fail");
        assert!(
            started.is_none(),
            "a gateway was bound for profiles that never asked for one"
        );
    }

    /// The other half of the same rule, and the one that keeps it from being
    /// satisfied by a function that simply never starts anything.
    #[test]
    fn a_profile_backed_by_the_gateway_binds_a_listener() {
        let profiles = [profile_backed_by(BackendResource::GlasshouseGateway)];
        let started = start_if_required(&profiles).expect("loopback is bindable");
        assert!(
            started.is_some(),
            "a gateway-backed profile did not produce a gateway"
        );
        // What that gateway's address must look like is asserted once, in
        // `the_gateway_binds_v4_loopback_on_a_port_the_operating_system_chose`.
        // Repeating it here would put the loopback contract in two places and
        // leave the next reader guessing which one is the statement of it.
    }

    // --- no ingress: the honest behaviour of a listener nothing accepts ---

    /// Nothing in this slice calls `accept`, and the point of this test is
    /// that "nothing accepts" is *not* the same as "the connection is
    /// refused": the kernel completes the handshake into the listen backlog
    /// by itself, so a client connects successfully. What must stay true is
    /// that the gateway never sends the client a byte — there is no ingress
    /// here to send one — and that the socket dies with the instance.
    ///
    /// Bytes written before a close survive in the receiving buffer and are
    /// still readable afterwards, so reading *after* the drop catches a
    /// gateway that greeted its client, without this test having to sleep to
    /// find out.
    #[test]
    fn a_connection_to_the_gateway_is_never_answered_and_dies_with_it() {
        let gateway = Gateway::start().expect("loopback is bindable");
        let address = gateway.address();

        let mut client = TcpStream::connect(address)
            .expect("the kernel completes the handshake even though nothing accepts");
        // A bound, generous ceiling rather than a wait: on every platform the
        // read below returns as soon as the listener goes away. The timeout
        // exists so that a platform which does neither resets nor closes
        // cannot hang the suite.
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("a non-zero read timeout is valid");

        drop(gateway);

        let mut buffer = [0u8; 64];
        let read = client.read(&mut buffer);
        assert!(
            !matches!(read, Ok(n) if n > 0),
            "the gateway sent bytes to a client; this slice has no ingress and must send none"
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
        let gateway = Gateway::start().expect("loopback is bindable");
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
    }

    /// "Multiple Glasshouse instances can coexist" is a claim about two
    /// listeners being alive *at the same time*, so both are held across the
    /// comparison. Drop the first before asking and the operating system is
    /// entitled to reissue its port to the second: the assertion would still
    /// pass and would have proved nothing.
    ///
    /// What this catches is a fixed port creeping in anywhere between
    /// [`EPHEMERAL_PORT`] and `local_addr` — after which a second instance
    /// would either refuse to start or quietly take the first one's address.
    #[test]
    fn two_gateways_in_one_process_bind_different_ports() {
        let first = Gateway::start().expect("loopback is bindable");
        let second = Gateway::start().expect("loopback is bindable");

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
        let first = Gateway::start().expect("loopback is bindable");
        let second = Gateway::start().expect("loopback is bindable");

        assert!(
            first.token().0 != second.token().0,
            "two gateways minted the same token"
        );
    }

    /// Nothing here calls a `close` or a `stop`: the port is released only
    /// because dropping the [`Gateway`] drops the listener inside it. Lose
    /// that and a process which started and finished with several gateways
    /// would hold every port it had ever bound until it exited — and the
    /// coexistence the ephemeral port buys would be spent on its own
    /// leftovers.
    ///
    /// Asserted as "the same address binds again", which is a direct
    /// statement that the descriptor is gone. The alternative — "connecting
    /// now fails" — depends on when the kernel gets around to refusing, and
    /// that is a wait this test would have to encode as a timeout.
    #[test]
    fn dropping_the_gateway_releases_its_port() {
        let gateway = Gateway::start().expect("loopback is bindable");
        let address = gateway.address();
        drop(gateway);

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
    /// Lose this and the four `use` lines that would undo Glasshouse's whole
    /// premise become an ordinary edit that no gate objects to.
    #[test]
    fn the_gateway_imports_none_of_the_modules_that_would_make_it_a_harness() {
        let code = production_code(include_str!("mod.rs"));
        for forbidden in [
            "crate::session",
            "crate::shell",
            "crate::tui",
            "crate::harness",
        ] {
            assert!(
                !code.contains(forbidden),
                "gateway/mod.rs names `{forbidden}` in production code: the gateway has become \
                 able to see the session model it must never own, and \"the harness stays the \
                 harness\" is back to being a promise rather than something this module is \
                 structurally unable to break"
            );
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
    }
}
