//! What the gateway forwards to, and the credential it attaches on the way —
//! the half of Phase 9G that the child harness must never be able to reach.
//!
//! # The credential lives here and nowhere else
//!
//! An [`Upstream`] holds a [`Secret`] resolved through
//! [`crate::secret::SecretStore`] inside the Glasshouse process. It is
//! attached to each forwarded request as an `authorization` header, and the
//! header value is marked sensitive so that even `http`'s own
//! [`Debug`](std::fmt::Debug) of a header map renders it as `Sensitive`
//! rather than as the key.
//!
//! Nothing hands this value to a child process, writes it to a file, or puts
//! it in a diagnostic. What the child gets instead is the gateway's own
//! per-instance token — see [`super::GatewayToken`] — which is worthless off
//! this machine and dies with the instance. That is the whole of "never
//! expose provider API keys to a child harness when the local gateway can
//! hold the credential itself".
//!
//! # One upstream, several protocols — and why not several upstreams
//!
//! The gateway serves more than one ingress protocol, and a provider
//! declares a **separate base URL for each one** — see
//! [`crate::provider::ProtocolSupport`], whose base URL is per protocol
//! precisely because a provider may serve them at different paths. So an
//! upstream is one provider, one credential, and a [`Route`] per protocol.
//!
//! The alternative shape — a set of `Upstream`s keyed by protocol — was
//! rejected on the one property this module exists for. Each of them would
//! need its own [`Secret`], and [`Secret`] is deliberately not `Clone` and
//! can only be minted inside [`mod@crate::secret`], so building that set
//! would mean either widening that module's API or resolving the same
//! credential once per protocol. Both turn "the credential lives here and
//! nowhere else" into "the credential lives in three places that happen to
//! agree". One owner, several destinations, keeps the sentence true.
//!
//! Several *providers* is a different question, and still refused: which
//! backend a session runs against is Phase 9H's sticky routing. See
//! [`crate::profile::gateway_upstream`].
//!
//! # Why `ureq`
//!
//! Glasshouse has no async runtime and this phase does not add one. `ureq`
//! is blocking, brings `rustls` rather than a system TLS stack, and — the
//! property that actually decided it — hands back a response body as a
//! [`Read`](std::io::Read). A body that is a reader is a body that can be
//! moved to the harness a piece at a time, which is what "preserve streaming
//! end-to-end" requires and what an implementation that returned `Vec<u8>`
//! could not offer at any price.
//!
//! Its default features are off: `gzip` would transparently decompress a
//! response and leave the `content-encoding` header describing something the
//! client is no longer being sent.

use ureq::Agent;
use ureq::config::AutoHeaderValue;
use ureq::http::{HeaderValue, Uri};

use crate::secret::Secret;

/// The scheme-and-host prefix an upstream base URL must carry.
///
/// Checked at construction rather than at the first request: a gateway that
/// bound a port and only then discovered it had nowhere to forward to would
/// have already told a harness it was ready.
const REQUIRED_SCHEMES: &[&str] = &["https", "http"];

/// The API-version segment a request target may or may not carry, and which
/// says nothing about **which** protocol the target belongs to.
///
/// Both harnesses that can back a gateway profile were run against a
/// listener that recorded the request line, pointed at a base URL with no
/// path — which is the only kind [`super::Gateway::base_url`] hands out:
///
/// - Claude Code 2.1.245, `ANTHROPIC_BASE_URL=http://127.0.0.1:<port>` →
///   `POST /v1/messages?beta=true`.
/// - Codex 0.149.1, `base_url = "http://127.0.0.1:<port>"` → `POST
///   /responses`. The same binary pointed at `.../v1` sends `POST
///   /v1/responses`.
///
/// So whether this segment is present depends on the harness's own idea of
/// where its base URL ends, not on the protocol. It is therefore stripped
/// before a target is **classified** — and never before it is
/// **forwarded**: [`Route::uri_for`] still appends the target byte for byte,
/// so the provider receives exactly the path the harness asked for.
const VERSION_SEGMENT: &str = "/v1";

/// One protocol the gateway serves: which request targets belong to it, and
/// where they go.
///
/// The protocol is carried as its **slug** rather than as a
/// [`crate::harness::WireProtocol`], because no file in this directory may
/// name `crate::harness` — see [`mod@super`]'s header and the scan that
/// enforces it. The slug is a name, of exactly the class this module already
/// puts in a diagnostic, and nothing here ever parses it back into anything.
#[derive(Debug)]
pub struct Route {
    /// The protocol's slug, from `WireProtocol::slug`. A name, never a
    /// credential.
    protocol: String,
    /// The version-independent path prefixes that belong to this protocol,
    /// composed by the caller that *can* see the protocol enum — see
    /// `crate::profile::ingress_targets`.
    targets: &'static [&'static str],
    /// The provider's declared base URL for this protocol, with any trailing
    /// slash removed so that appending a request target cannot produce `//`.
    base_url: String,
}

impl Route {
    /// Declare that `targets` belong to `protocol` and are forwarded to
    /// `base_url`.
    ///
    /// The base URL is only trimmed here; whether it is usable at all is
    /// checked by [`Upstream::new`], which is the layer that knows the
    /// provider's name and can therefore say whose base URL was wrong.
    pub fn new(protocol: String, targets: &'static [&'static str], base_url: &str) -> Self {
        Self {
            protocol,
            targets,
            base_url: base_url.trim_end_matches('/').to_owned(),
        }
    }

    /// The protocol's slug, for a diagnostic.
    pub(super) fn protocol(&self) -> &str {
        &self.protocol
    }

    /// The upstream host this route forwards to, for a diagnostic. A host,
    /// never a path and never a query.
    pub(super) fn host(&self) -> String {
        self.base_url
            .parse::<Uri>()
            .ok()
            .and_then(|uri| uri.host().map(str::to_owned))
            .unwrap_or_default()
    }

    /// Whether `target` belongs to this route's protocol.
    ///
    /// The query is dropped — it is part of the target and is forwarded, but
    /// it never decides where a request goes — the [`VERSION_SEGMENT`] is
    /// stripped, and what remains must match one of the declared prefixes
    /// **at a path-segment boundary**. That last part is the difference
    /// between `/messages/count_tokens` belonging to the Anthropic Messages
    /// route, which it does, and `/messagesomethingelse` belonging to it,
    /// which it must not.
    fn claims(&self, target: &str) -> bool {
        let path = path_of(target);
        let path = match path.strip_prefix(VERSION_SEGMENT) {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => rest,
            _ => path,
        };
        self.targets
            .iter()
            .any(|declared| is_segment_prefix(path, declared))
    }

    /// The request target appended to the declared base URL.
    ///
    /// This is one of the exactly three things the gateway rewrites, and it
    /// is a concatenation rather than a URL join: a join would normalise
    /// `..`, re-encode a query and resolve a relative reference, all of
    /// which change what the harness asked for.
    pub(super) fn uri_for(&self, target: &str) -> Option<Uri> {
        let separator = if target.starts_with('/') { "" } else { "/" };
        format!("{}{separator}{target}", self.base_url).parse().ok()
    }
}

/// A request target's path: everything before a query or a fragment.
fn path_of(target: &str) -> &str {
    let end = target.find(['?', '#']).unwrap_or(target.len());
    &target[..end]
}

/// Whether `path` is `prefix` or lies underneath it, on a segment boundary.
fn is_segment_prefix(path: &str, prefix: &str) -> bool {
    path == prefix || (path.starts_with(prefix) && path[prefix.len()..].starts_with('/'))
}

/// Where the gateway forwards, and the credential it forwards with.
///
/// Built once per Glasshouse instance and shared by every connection thread.
/// It is immutable: routing between several **providers** is Phase 9H's
/// sticky assignment, and a mutable pointer here would be that phase's
/// mechanism built early and without its evidence. Routing between the
/// protocols of the *one* provider is not that decision — it is decided by
/// the request target, which the harness already chose.
pub struct Upstream {
    /// The provider's configured name. A name, for diagnostics — never a
    /// credential, and the same class of value `BackendResource::slug`
    /// already puts in a session record.
    provider: String,
    /// One route per protocol this gateway serves. Never empty: an upstream
    /// with nowhere to forward to is refused at construction.
    routes: Vec<Route>,
    /// The provider credential, resolved in-process and never leaving it.
    credential: Secret,
}

/// Why an [`Upstream`] could not be built.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UpstreamError {
    #[error(
        "the provider `{provider}` serves none of the protocols the Glasshouse gateway's \
         ingress offers, so the gateway would have nowhere to forward to"
    )]
    NoProtocolServed { provider: String },
    #[error(
        "the provider `{provider}` declares a base URL for {protocol} that is not an absolute \
         http(s) URL, so the Glasshouse gateway has nowhere to forward to"
    )]
    BaseUrlNotAbsolute { provider: String, protocol: String },
    #[error(
        "the credential for the provider `{provider}` cannot be attached to a request; it \
         contains a character that is not allowed in an HTTP header value"
    )]
    CredentialNotHeaderSafe { provider: String },
}

impl Upstream {
    /// Build an upstream from a provider's name, one [`Route`] per protocol
    /// it serves, and its resolved credential.
    ///
    /// The credential is moved in and never comes back out: there is no
    /// accessor for it, only the crate-private `authorization`, which produces
    /// the header the gateway attaches. That is deliberate — a getter returning
    /// the value would be a second door into the one thing this module
    /// exists to keep behind one.
    pub fn new(
        provider: String,
        routes: Vec<Route>,
        credential: Secret,
    ) -> Result<Self, UpstreamError> {
        if routes.is_empty() {
            return Err(UpstreamError::NoProtocolServed { provider });
        }
        for route in &routes {
            let uri: Uri =
                route
                    .base_url
                    .parse()
                    .map_err(|_| UpstreamError::BaseUrlNotAbsolute {
                        provider: provider.clone(),
                        protocol: route.protocol.clone(),
                    })?;
            let scheme_is_http = uri
                .scheme_str()
                .is_some_and(|scheme| REQUIRED_SCHEMES.contains(&scheme));
            if !scheme_is_http || uri.host().is_none() {
                return Err(UpstreamError::BaseUrlNotAbsolute {
                    provider,
                    protocol: route.protocol.clone(),
                });
            }
        }
        // Checked once, here, so that a credential carrying a newline is a
        // refusal to start rather than a header-injection attempt on every
        // forwarded request.
        if HeaderValue::from_str(&bearer(&credential)).is_err() {
            return Err(UpstreamError::CredentialNotHeaderSafe { provider });
        }

        Ok(Self {
            provider,
            routes,
            credential,
        })
    }

    /// The provider's name, for a diagnostic.
    pub(super) fn provider(&self) -> &str {
        &self.provider
    }

    /// The slug of every protocol this upstream can carry, in the order the
    /// routes were declared.
    ///
    /// Names only, and the caller that reads them — `crate::profile` — is
    /// the one that knows what a slug means. This is how a gateway-backed
    /// launch profile finds out what the running gateway can actually do
    /// without this module learning what a wire protocol is.
    pub fn served_protocols(&self) -> Vec<&str> {
        self.routes.iter().map(Route::protocol).collect()
    }

    /// The route a request target belongs to, or `None`.
    ///
    /// `None` is a refusal, never an invitation to pick the first route: a
    /// target the gateway cannot place is one it would otherwise append to
    /// whichever base URL happened to be declared first, which is a request
    /// sent somewhere nobody asked for it to go.
    pub(super) fn route_for(&self, target: &str) -> Option<&Route> {
        self.routes.iter().find(|route| route.claims(target))
    }

    /// The `authorization` header the gateway attaches, replacing whatever
    /// the child sent.
    ///
    /// Marked sensitive, so `http`'s own rendering of a header map prints
    /// `Sensitive` in its place. That is belt over braces — nothing here
    /// renders a request's headers — but it costs one call and removes a
    /// whole class of future accident.
    pub(super) fn authorization(&self) -> HeaderValue {
        let mut value = HeaderValue::from_str(&bearer(&self.credential))
            .expect("checked when the upstream was built");
        value.set_sensitive(true);
        value
    }
}

/// `Bearer <credential>`, the one place the resolved value is read.
fn bearer(credential: &Secret) -> String {
    format!("Bearer {}", credential.expose())
}

/// Prints the provider and its routes, and the credential's own redaction
/// marker.
///
/// Manual rather than derived for the same reason
/// [`crate::profile::LaunchOverlay`]'s is: the field this type exists to
/// hold must not be renderable, and a derive is one added field away from
/// making it so. [`Secret`]'s own rendering would already print the marker;
/// this makes that independent of it.
impl std::fmt::Debug for Upstream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Upstream")
            .field("provider", &self.provider)
            .field("routes", &self.routes)
            .field("credential", &crate::secret::REDACTED)
            .finish()
    }
}

/// The one HTTP client the gateway uses, configured for pass-through.
///
/// Every setting here exists to stop `ureq` from being helpful:
///
/// - `http_status_as_error(false)` — a `429` is a response to forward, not an
///   error to swallow. With the default, the provider's own error body would
///   never reach the harness.
/// - `max_redirects(0)` — a redirect is a response the harness is entitled to
///   see and decide about. Following one here would also mean deciding
///   whether to re-attach the credential to a host the provider named at
///   runtime.
/// - `user_agent`, `accept` and `accept_encoding` set to
///   [`AutoHeaderValue::None`] — the harness's own headers are forwarded, and
///   a gateway that added its own would be visible to the provider as a
///   client the harness is not.
/// - `allow_non_standard_methods(true)` — the method is forwarded, not
///   vetted.
///
/// Timeouts are left at `ureq`'s defaults, which are unset. A streaming
/// response may legitimately go minutes between events, and a receive
/// timeout here would cut a long generation off mid-stream.
pub(super) fn agent() -> Agent {
    Agent::new_with_config(
        Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .user_agent(AutoHeaderValue::None)
            .accept(AutoHeaderValue::None)
            .accept_encoding(AutoHeaderValue::None)
            .allow_non_standard_methods(true)
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three protocols the ingress serves, spelled here as the slugs and
    /// target prefixes `crate::profile` composes them from.
    ///
    /// Duplicated deliberately rather than imported: this module cannot name
    /// `crate::harness`, and `crate::profile`'s own
    /// `the_gateway_routes_every_protocol_its_ingress_declares` is what
    /// checks that these two spellings have not drifted apart.
    const ANTHROPIC: (&str, &[&str]) = ("anthropic-messages", &["/messages"]);
    const RESPONSES: (&str, &[&str]) = ("openai-responses", &["/responses"]);
    const CHAT: (&str, &[&str]) = ("openai-chat", &["/chat/completions"]);

    fn route((protocol, targets): (&str, &'static [&'static str]), base_url: &str) -> Route {
        Route::new(protocol.to_owned(), targets, base_url)
    }

    fn upstream_with(routes: Vec<Route>) -> Result<Upstream, UpstreamError> {
        Upstream::new(
            "test-provider".to_owned(),
            routes,
            Secret::mint_for_test("sk-test-credential-value"),
        )
    }

    fn upstream_at(base_url: &str) -> Result<Upstream, UpstreamError> {
        upstream_with(vec![route(ANTHROPIC, base_url)])
    }

    /// The upstream a multi-protocol test forwards through: one provider,
    /// three protocols, three visibly different base URLs.
    fn three_protocol_upstream() -> Upstream {
        upstream_with(vec![
            route(ANTHROPIC, "https://provider.example/anthropic"),
            route(RESPONSES, "https://provider.example/openai/v1"),
            route(CHAT, "https://provider.example/chat/v1"),
        ])
        .expect("three absolute https URLs")
    }

    fn uri_for(upstream: &Upstream, target: &str) -> Option<String> {
        let route = upstream.route_for(target)?;
        Some(route.uri_for(target)?.to_string())
    }

    #[test]
    fn a_request_target_is_appended_to_the_declared_base_url_verbatim() {
        let upstream = upstream_at("https://openrouter.ai/api").expect("an absolute https URL");
        assert_eq!(
            uri_for(&upstream, "/v1/messages?beta=true").unwrap(),
            "https://openrouter.ai/api/v1/messages?beta=true"
        );
    }

    #[test]
    fn a_trailing_slash_on_the_base_url_does_not_double_up() {
        let upstream = upstream_at("https://openrouter.ai/api/").expect("an absolute https URL");
        assert_eq!(
            uri_for(&upstream, "/v1/messages").unwrap(),
            "https://openrouter.ai/api/v1/messages"
        );
    }

    /// The property line 1 and line 2 of Phase 9G are: a target reaches the
    /// base URL its **own** protocol declared, and not the one that happens
    /// to be first.
    #[test]
    fn each_protocols_target_reaches_that_protocols_own_base_url() {
        let upstream = three_protocol_upstream();
        for (target, expected) in [
            (
                "/v1/messages",
                "https://provider.example/anthropic/v1/messages",
            ),
            (
                "/v1/messages/count_tokens",
                "https://provider.example/anthropic/v1/messages/count_tokens",
            ),
            // Codex 0.149.1 pointed at a base URL with no path sends exactly
            // this, which is why the version segment cannot be required.
            ("/responses", "https://provider.example/openai/v1/responses"),
            (
                "/v1/responses",
                "https://provider.example/openai/v1/v1/responses",
            ),
            (
                "/chat/completions",
                "https://provider.example/chat/v1/chat/completions",
            ),
        ] {
            assert_eq!(
                uri_for(&upstream, target).as_deref(),
                Some(expected),
                "{target} was not routed to its own protocol's base URL"
            );
        }
    }

    /// The refusal half of the same property. A target that belongs to no
    /// served protocol must not be appended to whichever base URL came
    /// first — which is precisely what the single-upstream implementation
    /// this replaced would have done with every one of these.
    #[test]
    fn a_target_belonging_to_no_served_protocol_is_not_routed_anywhere() {
        let upstream = three_protocol_upstream();
        for target in [
            // Claude Code 2.1.245 really does send this, before its first
            // `/v1/messages` — observed against a recording listener.
            "/api/hello",
            "/v1/models",
            "/",
            "",
            // A prefix match that is not on a segment boundary.
            "/v1/messagesomethingelse",
            "/messagesomethingelse",
            // The version segment is stripped only when it *is* a segment.
            "/v1beta/messages",
            // Absolute-form targets are not origin-form and are not placed.
            "https://elsewhere.example/v1/messages",
        ] {
            assert!(
                upstream.route_for(target).is_none(),
                "{target:?} was routed somewhere"
            );
        }
    }

    /// A gateway that serves one protocol places that protocol's targets and
    /// refuses everything else — the same rule, not a special case.
    #[test]
    fn a_single_protocol_upstream_places_only_its_own_targets() {
        let upstream = upstream_at("https://openrouter.ai/api").expect("an absolute https URL");
        assert_eq!(upstream.served_protocols(), vec!["anthropic-messages"]);
        assert!(upstream.route_for("/v1/messages").is_some());
        assert!(upstream.route_for("/responses").is_none());
        assert!(upstream.route_for("/v1/chat/completions").is_none());
    }

    #[test]
    fn an_upstream_with_no_route_at_all_is_refused_at_construction() {
        assert_eq!(
            upstream_with(Vec::new()).err(),
            Some(UpstreamError::NoProtocolServed {
                provider: "test-provider".to_owned()
            })
        );
    }

    #[test]
    fn a_base_url_that_is_not_an_absolute_http_url_is_refused_at_construction() {
        for base_url in ["", "openrouter.ai/api", "/api", "ftp://openrouter.ai"] {
            assert_eq!(
                upstream_at(base_url).err(),
                Some(UpstreamError::BaseUrlNotAbsolute {
                    provider: "test-provider".to_owned(),
                    protocol: "anthropic-messages".to_owned(),
                }),
                "accepted {base_url:?}"
            );
        }
    }

    /// Every route is checked, not just the first — otherwise a provider
    /// with a good Anthropic URL and a broken Responses one would bind a
    /// port and fail at the first Codex request instead of at start.
    #[test]
    fn a_broken_base_url_on_a_later_route_is_refused_too() {
        let broken = upstream_with(vec![
            route(ANTHROPIC, "https://provider.example/anthropic"),
            route(RESPONSES, "not-a-url"),
        ]);
        assert_eq!(
            broken.err(),
            Some(UpstreamError::BaseUrlNotAbsolute {
                provider: "test-provider".to_owned(),
                protocol: "openai-responses".to_owned(),
            })
        );
    }

    #[test]
    fn a_credential_that_could_inject_a_header_is_refused_at_construction() {
        let injected = Upstream::new(
            "test-provider".to_owned(),
            vec![route(ANTHROPIC, "https://openrouter.ai/api")],
            Secret::mint_for_test("value\r\nx-injected: yes"),
        );
        assert_eq!(
            injected.err(),
            Some(UpstreamError::CredentialNotHeaderSafe {
                provider: "test-provider".to_owned()
            })
        );
    }

    /// The credential is reachable through the whole upstream, so the whole
    /// upstream has to be safe to render — and a `Debug` on the owner is
    /// exactly how a field gets printed by accident.
    #[test]
    fn debug_on_an_upstream_never_reaches_its_credential() {
        const VALUE: &str = "sk-planted-credential-qqqqwwwweeee";
        let upstream = Upstream::new(
            "test-provider".to_owned(),
            vec![route(ANTHROPIC, "https://openrouter.ai/api")],
            Secret::mint_for_test(VALUE),
        )
        .expect("an absolute https URL");

        let rendered = format!("{upstream:?}");
        assert!(
            !rendered.contains(VALUE),
            "the credential survived into {rendered:?}"
        );
        assert!(
            rendered.contains(crate::secret::REDACTED),
            "the redaction marker must be shown rather than the field omitted: {rendered:?}"
        );
        // ... and the parts that are not secret are still there, or the
        // diagnostic would be useless and would get switched off.
        assert!(rendered.contains("test-provider"));
        assert!(rendered.contains("anthropic-messages"));
    }

    /// `http`'s own header rendering is the other place a value can escape,
    /// and it is one this module does not control — so the header is marked
    /// sensitive rather than trusted to stay unprinted.
    #[test]
    fn the_attached_authorization_header_renders_as_sensitive() {
        const VALUE: &str = "sk-planted-credential-qqqqwwwweeee";
        let upstream = Upstream::new(
            "test-provider".to_owned(),
            vec![route(ANTHROPIC, "https://openrouter.ai/api")],
            Secret::mint_for_test(VALUE),
        )
        .expect("an absolute https URL");

        let header = upstream.authorization();
        assert!(header.is_sensitive());
        let rendered = format!("{header:?}");
        assert!(
            !rendered.contains(VALUE),
            "the credential survived into {rendered:?}"
        );

        // ... while the value itself really is the credential, or nothing
        // above would be protecting anything.
        assert_eq!(header.as_bytes(), format!("Bearer {VALUE}").as_bytes());
    }

    /// One credential for every protocol: the same header goes out whichever
    /// route carried the request, because there is only one credential to
    /// attach. This is the observable consequence of the shape chosen in
    /// this module's header.
    #[test]
    fn every_route_forwards_with_the_one_credential_the_upstream_holds() {
        let upstream = three_protocol_upstream();
        assert_eq!(upstream.served_protocols().len(), 3);
        let attached = upstream.authorization();
        assert!(attached.is_sensitive());
        assert_eq!(
            attached.as_bytes(),
            b"Bearer sk-test-credential-value".as_slice()
        );
    }

    #[test]
    fn the_upstream_host_is_a_host_and_never_a_path() {
        let upstream = upstream_at("https://openrouter.ai/api").expect("an absolute https URL");
        let route = upstream
            .route_for("/v1/messages")
            .expect("the anthropic route");
        assert_eq!(route.host(), "openrouter.ai");
        assert_eq!(route.protocol(), "anthropic-messages");
        assert_eq!(upstream.provider(), "test-provider");
    }
}
