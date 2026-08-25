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

/// Where the gateway forwards, and the credential it forwards with.
///
/// Built once per Glasshouse instance and shared by every connection thread.
/// It is immutable: routing between several upstreams is Phase 9H's sticky
/// assignment, and a mutable pointer here would be that phase's mechanism
/// built early and without its evidence.
pub struct Upstream {
    /// The provider's configured name. A name, for diagnostics — never a
    /// credential, and the same class of value `BackendResource::slug`
    /// already puts in a session record.
    provider: String,
    /// The provider's declared base URL for this protocol, with any trailing
    /// slash removed so that appending a request target cannot produce `//`.
    base_url: String,
    /// The provider credential, resolved in-process and never leaving it.
    credential: Secret,
}

/// Why an [`Upstream`] could not be built.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UpstreamError {
    #[error(
        "the provider `{provider}` declares a base URL that is not an absolute http(s) URL, so \
         the Glasshouse gateway has nowhere to forward to"
    )]
    BaseUrlNotAbsolute { provider: String },
    #[error(
        "the credential for the provider `{provider}` cannot be attached to a request; it \
         contains a character that is not allowed in an HTTP header value"
    )]
    CredentialNotHeaderSafe { provider: String },
}

impl Upstream {
    /// Build an upstream from a provider's name, its declared base URL, and
    /// its resolved credential.
    ///
    /// The credential is moved in and never comes back out: there is no
    /// accessor for it, only the crate-private `authorization`, which produces
    /// the header the gateway attaches. That is deliberate — a getter returning
    /// the value would be a second door into the one thing this module
    /// exists to keep behind one.
    pub fn new(
        provider: String,
        base_url: &str,
        credential: Secret,
    ) -> Result<Self, UpstreamError> {
        let trimmed = base_url.trim_end_matches('/');
        let uri: Uri = trimmed
            .parse()
            .map_err(|_| UpstreamError::BaseUrlNotAbsolute {
                provider: provider.clone(),
            })?;
        let scheme_is_http = uri
            .scheme_str()
            .is_some_and(|scheme| REQUIRED_SCHEMES.contains(&scheme));
        if !scheme_is_http || uri.host().is_none() {
            return Err(UpstreamError::BaseUrlNotAbsolute { provider });
        }
        // Checked once, here, so that a credential carrying a newline is a
        // refusal to start rather than a header-injection attempt on every
        // forwarded request.
        if HeaderValue::from_str(&bearer(&credential)).is_err() {
            return Err(UpstreamError::CredentialNotHeaderSafe { provider });
        }

        Ok(Self {
            provider,
            base_url: trimmed.to_owned(),
            credential,
        })
    }

    /// The provider's name, for a diagnostic.
    pub(super) fn provider(&self) -> &str {
        &self.provider
    }

    /// The upstream host this gateway forwards to, for a diagnostic. A host,
    /// never a path and never a query.
    pub(super) fn host(&self) -> String {
        self.base_url
            .parse::<Uri>()
            .ok()
            .and_then(|uri| uri.host().map(str::to_owned))
            .unwrap_or_default()
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

/// Prints the provider and the base URL, and the credential's own redaction
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
            .field("base_url", &self.base_url)
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

    fn upstream_at(base_url: &str) -> Result<Upstream, UpstreamError> {
        Upstream::new(
            "test-provider".to_owned(),
            base_url,
            Secret::mint_for_test("sk-test-credential-value"),
        )
    }

    #[test]
    fn a_request_target_is_appended_to_the_declared_base_url_verbatim() {
        let upstream = upstream_at("https://openrouter.ai/api").expect("an absolute https URL");
        assert_eq!(
            upstream
                .uri_for("/v1/messages?beta=true")
                .unwrap()
                .to_string(),
            "https://openrouter.ai/api/v1/messages?beta=true"
        );
    }

    #[test]
    fn a_trailing_slash_on_the_base_url_does_not_double_up() {
        let upstream = upstream_at("https://openrouter.ai/api/").expect("an absolute https URL");
        assert_eq!(
            upstream.uri_for("/v1/messages").unwrap().to_string(),
            "https://openrouter.ai/api/v1/messages"
        );
    }

    #[test]
    fn a_base_url_that_is_not_an_absolute_http_url_is_refused_at_construction() {
        for base_url in ["", "openrouter.ai/api", "/api", "ftp://openrouter.ai"] {
            assert_eq!(
                upstream_at(base_url).err(),
                Some(UpstreamError::BaseUrlNotAbsolute {
                    provider: "test-provider".to_owned()
                }),
                "accepted {base_url:?}"
            );
        }
    }

    #[test]
    fn a_credential_that_could_inject_a_header_is_refused_at_construction() {
        let injected = Upstream::new(
            "test-provider".to_owned(),
            "https://openrouter.ai/api",
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
            "https://openrouter.ai/api",
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
    }

    /// `http`'s own header rendering is the other place a value can escape,
    /// and it is one this module does not control — so the header is marked
    /// sensitive rather than trusted to stay unprinted.
    #[test]
    fn the_attached_authorization_header_renders_as_sensitive() {
        const VALUE: &str = "sk-planted-credential-qqqqwwwweeee";
        let upstream = Upstream::new(
            "test-provider".to_owned(),
            "https://openrouter.ai/api",
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

    #[test]
    fn the_upstream_host_is_a_host_and_never_a_path() {
        let upstream = upstream_at("https://openrouter.ai/api").expect("an absolute https URL");
        assert_eq!(upstream.host(), "openrouter.ai");
        assert_eq!(upstream.provider(), "test-provider");
    }
}
