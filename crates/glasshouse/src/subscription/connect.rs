//! Connecting a subscription account, owned by Glasshouse end to end.
//!
//! The flow is the one the first-party CLIs use: an authorization code with
//! PKCE, redirected to a loopback listener this process opens. Glasshouse runs
//! the listener, performs the exchange, and writes the credential. What leaves
//! this module is **never a secret** — an authorization URL, a waiting state, a
//! success or a failure, and nothing else.
//!
//! That boundary is what makes the flow drivable from another program's
//! interface. The alternative Glasshouse used before — handing a child process
//! the terminal and looking away — cannot be rendered inside a harness's own
//! screen, because the child owns the screen. Progress reported as data can.
//!
//! The credential is written where the subscription broker already reads it,
//! so taking login in-house changes nothing about how a request is served. The
//! request path moves separately, and later.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// How long a person is given to finish in the browser before the listener
/// gives the port back.
pub const AUTHORIZE_TIMEOUT: Duration = Duration::from_secs(300);

/// One provider's OAuth client.
///
/// The redirect port is **fixed, not chosen**: it is registered with the
/// provider as part of the client, so a listener on any other port would be
/// refused at the redirect. That is why this is a table rather than a
/// negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OauthClient {
    pub provider: &'static str,
    /// The client's public identifier. It travels in every authorize URL and
    /// is not a secret — but it is the field a vendor rotates, so an operator
    /// can override it without waiting for a release. glasshouse:not-a-secret
    pub client_id: &'static str,
    pub authorize_url: &'static str,
    pub token_url: &'static str,
    pub redirect_port: u16,
    pub redirect_path: &'static str,
    pub scope: &'static str,
    /// Parameters this provider's client sends beyond the standard set,
    /// already encoded as `key=value` pairs.
    pub extra_params: &'static [&'static str],
}

/// The providers Glasshouse can connect itself.
///
/// Endpoints, redirect ports, scopes and client identifiers are the ones the
/// first-party clients use. A client id is public by construction — it is in
/// every authorize URL — so these are defaults rather than secrets, and
/// `GLASSHOUSE_OAUTH_CLIENT_ID_<PROVIDER>` overrides one the day a vendor
/// rotates it, without waiting for a release. glasshouse:not-a-secret
pub static CLIENTS: &[OauthClient] = &[
    OauthClient {
        provider: "anthropic",
        client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
        authorize_url: "https://claude.ai/oauth/authorize",
        token_url: "https://platform.claude.com/v1/oauth/token",
        redirect_port: 54545,
        redirect_path: "/callback",
        scope: "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload",
        extra_params: &[],
    },
    OauthClient {
        provider: "openai",
        client_id: "app_EMoamEEZ73f0CkXaXp7hrann",
        authorize_url: "https://auth.openai.com/oauth/authorize",
        token_url: "https://auth.openai.com/oauth/token",
        redirect_port: 1455,
        redirect_path: "/auth/callback",
        scope: "openid profile email offline_access",
        extra_params: &["codex_cli_simplified_flow=true", "originator=codex_cli_rs"],
    },
    OauthClient {
        provider: "google",
        client_id: "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com",
        authorize_url: "https://accounts.google.com/o/oauth2/v2/auth",
        token_url: "https://oauth2.googleapis.com/token",
        redirect_port: 8085,
        redirect_path: "/oauth2callback",
        scope: "https://www.googleapis.com/auth/cloud-platform openid email profile",
        extra_params: &["access_type=offline", "prompt=consent"],
    },
];

/// The client for `provider`, or `None`.
#[must_use]
pub fn client_for(provider: &str) -> Option<&'static OauthClient> {
    CLIENTS.iter().find(|client| client.provider == provider)
}

/// What a caller driving the flow is told.
///
/// Every variant is safe to render, log and send to another process. There is
/// deliberately no variant carrying a token, a code or a verifier: the type is
/// the boundary, so a future field cannot leak one by accident.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Progress {
    /// The URL to open. It carries a client id and a PKCE challenge, both
    /// public by construction.
    Opened {
        authorize_url: String,
    },
    /// The listener is up and nothing has arrived yet.
    Waiting {
        seconds_remaining: u64,
    },
    /// A credential was written. The account label is whatever the provider
    /// returned to identify it, never a token.
    Connected {
        account: Option<String>,
    },
    Failed {
        reason: String,
    },
}

/// A PKCE verifier and the challenge derived from it.
///
/// The verifier never leaves this process and never reaches [`Progress`]; the
/// challenge is in the URL and is meant to be seen.
pub struct Pkce {
    verifier: String,
    pub challenge: String,
}

impl Pkce {
    /// A fresh pair, from cryptographic randomness.
    pub fn generate() -> Result<Self> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).context("could not read randomness for a PKCE verifier")?;
        let verifier = base64_url(&bytes);
        let challenge = base64_url(&Sha256::digest(verifier.as_bytes()));
        Ok(Self {
            verifier,
            challenge,
        })
    }

    /// The verifier, for the token exchange only.
    #[must_use]
    pub fn verifier(&self) -> &str {
        &self.verifier
    }
}

impl std::fmt::Debug for Pkce {
    /// Never prints the verifier: a debug line is the easiest way for a secret
    /// to reach a log.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Pkce")
            .field("verifier", &"<redacted>")
            .field("challenge", &self.challenge)
            .finish()
    }
}

/// Base64url without padding, as the PKCE specification requires.
fn base64_url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let indices = [n >> 18 & 63, n >> 12 & 63, n >> 6 & 63, n & 63];
        for (position, index) in indices.iter().enumerate() {
            if position <= chunk.len() {
                out.push(char::from(ALPHABET[*index as usize]));
            }
        }
    }
    out
}

/// The authorization URL a person opens.
///
/// Built here rather than by a caller so that every parameter this flow
/// depends on — the challenge method, the redirect, the state — is present or
/// the URL is not produced at all.
#[must_use]
pub fn authorize_url(
    client: &OauthClient,
    client_id: &str,
    challenge: &str,
    state: &str,
) -> String {
    let redirect = format!(
        "http://localhost:{}{}",
        client.redirect_port, client.redirect_path
    );
    let mut url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={challenge}\
         &code_challenge_method=S256&state={state}",
        client.authorize_url,
        percent_encode(client_id),
        percent_encode(&redirect),
        percent_encode(client.scope),
    );
    for extra in client.extra_params {
        url.push('&');
        url.push_str(extra);
    }
    url
}

/// Minimal percent-encoding for a query value.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// What the redirect carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Callback {
    pub code: String,
    pub state: String,
}

/// Reads `code` and `state` out of a redirect request line.
///
/// Returns `None` for anything that is not the expected callback, including a
/// browser's favicon request — a listener that treated the first connection as
/// the answer would fail on every browser that asks for one.
#[must_use]
pub fn parse_callback(request_line: &str, expected_path: &str) -> Option<Callback> {
    let target = request_line.split_whitespace().nth(1)?;
    let (path, query) = target.split_once('?')?;
    if path != expected_path {
        return None;
    }
    let mut code = None;
    let mut state = None;
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some(("code", value)) => code = Some(percent_decode(value)),
            Some(("state", value)) => state = Some(percent_decode(value)),
            _ => {}
        }
    }
    Some(Callback {
        code: code?,
        state: state?,
    })
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The page the browser lands on. Plain, local, and mentioning nothing.
const DONE_PAGE: &str = "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\nconnection: close\r\n\r\n\
<!doctype html><meta charset=utf-8><title>Connected</title>\
<body style=\"font:16px system-ui;margin:4rem\">\
<p>This account is connected. You can close this tab and return to your session.</p>";

/// Waits on the loopback redirect for one matching callback.
///
/// Binds the provider's registered port, because that is the redirect the
/// client was registered with — a port of our choosing would be rejected by
/// the provider before a person ever saw a prompt.
pub fn await_callback(
    client: &OauthClient,
    expected_state: &str,
    timeout: Duration,
    mut report: impl FnMut(Progress),
) -> Result<Callback> {
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, client.redirect_port);
    let listener = TcpListener::bind(address).with_context(|| {
        format!(
            "could not listen on {address} for the {} redirect; another login may be in progress",
            client.provider
        )
    })?;
    listener.set_nonblocking(true)?;

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false)?;
                let mut line = String::new();
                BufReader::new(stream.try_clone()?).read_line(&mut line)?;
                let Some(callback) = parse_callback(&line, client.redirect_path) else {
                    // A favicon or a stray probe. Answer and keep waiting.
                    let _ =
                        stream.write_all(b"HTTP/1.1 404 Not Found\r\nconnection: close\r\n\r\n");
                    continue;
                };
                if callback.state != expected_state {
                    let _ =
                        stream.write_all(b"HTTP/1.1 400 Bad Request\r\nconnection: close\r\n\r\n");
                    bail!("the redirect carried a state this flow did not issue");
                }
                let _ = stream.write_all(DONE_PAGE.as_bytes());
                return Ok(callback);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                let remaining = deadline.saturating_duration_since(Instant::now()).as_secs();
                report(Progress::Waiting {
                    seconds_remaining: remaining,
                });
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(error) => return Err(error.into()),
        }
    }
    bail!("nothing arrived on the redirect within {timeout:?}")
}

/// A fresh anti-forgery `state` value.
pub fn random_state() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).context("could not read randomness for an OAuth state")?;
    Ok(base64_url(&bytes))
}

/// Exchanges the code for tokens and writes the credential.
///
/// The tokens are written and never returned: the only thing a caller learns
/// is the account label the provider gave, which is what a person needs to see
/// to know which account they just connected.
pub fn exchange_and_store(
    client: &OauthClient,
    client_id: &str,
    pkce: &Pkce,
    code: &str,
    auth_dir: &Path,
) -> Result<Option<String>> {
    let redirect = format!(
        "http://localhost:{}{}",
        client.redirect_port, client.redirect_path
    );
    let form = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
        percent_encode(code),
        percent_encode(&redirect),
        percent_encode(client_id),
        percent_encode(pkce.verifier()),
    );
    let mut response = ureq::post(client.token_url)
        .header("content-type", "application/x-www-form-urlencoded")
        .send(&form)
        .map_err(|error| anyhow::anyhow!("the token exchange did not answer: {error}"))?;
    if !response.status().is_success() {
        // The provider's own body may echo the code back, so the status is
        // reported and the body is not.
        bail!(
            "the token exchange was refused with status {}",
            response.status()
        );
    }
    let text = response
        .body_mut()
        .read_to_string()
        .context("the token exchange response was unreadable")?;
    let parsed: serde_json::Value =
        serde_json::from_str(&text).context("the token exchange response was not JSON")?;

    let account = parsed
        .get("account_id")
        .or_else(|| parsed.get("email"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let credential = to_broker_credential(&parsed, client.provider, account.as_deref());

    std::fs::create_dir_all(auth_dir)
        .with_context(|| format!("could not create {}", auth_dir.display()))?;
    let path = credential_path(
        auth_dir,
        client.provider,
        account.as_deref().unwrap_or("account"),
    );
    write_private(&path, &serde_json::to_vec_pretty(&credential)?)?;
    Ok(account)
}

/// The credential in the shape the subscription broker already reads.
///
/// Written to the broker's own format on purpose: a login Glasshouse performed
/// must be indistinguishable to the request path from one the broker
/// performed, which is what lets login move first and the request path move
/// later.
fn to_broker_credential(
    tokens: &serde_json::Value,
    provider: &str,
    account: Option<&str>,
) -> serde_json::Value {
    let string = |key: &str| {
        tokens
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let expires_in = tokens
        .get("expires_in")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    let now = std::time::SystemTime::now();
    let expired = now + Duration::from_secs(expires_in.max(0) as u64);
    serde_json::json!({
        "type": provider,
        "access_token": string("access_token"),
        "refresh_token": string("refresh_token"),
        "id_token": string("id_token"),
        "account_id": account.unwrap_or_default(),
        "email": string("email"),
        "expired": rfc3339(expired),
        "last_refresh": rfc3339(now),
        "disabled": false,
    })
}

/// A timestamp in the shape the broker's own files use.
fn rfc3339(at: std::time::SystemTime) -> String {
    let seconds = at
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();
    // Civil time from a Unix second, without pulling in a date library for
    // one field the broker only ever reads back as an opaque marker.
    let days = seconds / 86_400;
    let rest = seconds % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// Howard Hinnant's `civil_from_days`, which is exact for every day this will
/// ever see and needs no table.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

/// Writes a credential so that only its owner can read it.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::fs::OpenOptions;
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("could not write {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("could not write {}", path.display()))?;
    Ok(())
}

/// Where a connected account's credential is written.
///
/// The subscription broker's own auth directory, in its own file naming, so
/// that a login Glasshouse performed is indistinguishable to the request path
/// from one the broker performed. That is what lets login move first and the
/// request path move later.
#[must_use]
pub fn credential_path(auth_dir: &Path, provider: &str, account: &str) -> std::path::PathBuf {
    // A dot survives because an account label is usually an email address.
    // A separator never does, and a leading dot never does: the first would
    // let a label choose a directory, the second would hide the file.
    let mut safe: String = account
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "@.-_".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    while safe.starts_with('.') {
        safe.replace_range(..1, "-");
    }
    auth_dir.join(format!("{provider}-{safe}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verifier_never_reaches_a_debug_line() {
        let pkce = Pkce::generate().unwrap();
        let printed = format!("{pkce:?}");
        assert!(!printed.contains(pkce.verifier()));
        assert!(printed.contains("<redacted>"));
        assert!(printed.contains(&pkce.challenge));
    }

    /// The challenge is the hash of the verifier and never the verifier, which
    /// is the whole of what PKCE protects.
    #[test]
    fn the_challenge_is_derived_and_differs_from_the_verifier() {
        let pkce = Pkce::generate().unwrap();
        assert_ne!(pkce.challenge, pkce.verifier());
        assert_eq!(
            pkce.challenge,
            base64_url(&Sha256::digest(pkce.verifier().as_bytes()))
        );
        // Base64url, unpadded, as the specification requires.
        assert!(!pkce.challenge.contains('='));
        assert!(!pkce.challenge.contains('+'));
        assert!(!pkce.challenge.contains('/'));
    }

    #[test]
    fn two_flows_never_share_a_verifier() {
        assert_ne!(
            Pkce::generate().unwrap().verifier(),
            Pkce::generate().unwrap().verifier()
        );
    }

    /// Everything a caller is told is safe to render and to send to another
    /// process. The type is the boundary.
    #[test]
    fn no_progress_variant_can_carry_a_secret() {
        let opened = Progress::Opened {
            authorize_url: "https://example.invalid/x".into(),
        };
        let json = serde_json::to_string(&opened).unwrap();
        assert!(json.contains("\"state\":\"opened\""));
        for progress in [
            Progress::Waiting {
                seconds_remaining: 30,
            },
            Progress::Connected {
                account: Some("someone@example.com".into()),
            },
            Progress::Failed {
                reason: "refused".into(),
            },
        ] {
            let json = serde_json::to_string(&progress).unwrap();
            for forbidden in ["token", "code", "verifier", "secret"] {
                assert!(!json.contains(forbidden), "{json} names {forbidden}");
            }
        }
    }

    #[test]
    fn the_authorize_url_carries_the_challenge_and_never_the_verifier() {
        let client = client_for("anthropic").unwrap();
        let pkce = Pkce::generate().unwrap();
        let url = authorize_url(client, client.client_id, &pkce.challenge, "state-abc");
        assert!(url.contains(&pkce.challenge));
        assert!(!url.contains(pkce.verifier()));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=state-abc"));
        assert!(url.contains("localhost%3A54545%2Fcallback"));
        assert!(url.contains(client.client_id));

        // A provider that needs extra parameters gets them, and one that does
        // not is not given any.
        let openai = client_for("openai").unwrap();
        let with_extras = authorize_url(openai, openai.client_id, "c", "s");
        assert!(with_extras.contains("originator=codex_cli_rs"));
        assert!(with_extras.contains("codex_cli_simplified_flow=true"));
    }

    /// The redirect port is registered with the provider, so it is part of the
    /// client and not something a listener may choose.
    #[test]
    fn every_client_declares_the_redirect_it_was_registered_with() {
        for client in CLIENTS {
            assert!(client.redirect_port > 1024, "{}", client.provider);
            assert!(client.redirect_path.starts_with('/'), "{}", client.provider);
            assert!(client.authorize_url.starts_with("https://"));
            assert!(client.token_url.starts_with("https://"));
        }
        assert_eq!(client_for("anthropic").unwrap().redirect_port, 54545);
        assert_eq!(client_for("openai").unwrap().redirect_port, 1455);
        assert!(client_for("nobody").is_none());

        // Every client carries the identifier it is registered under, so a
        // login works without configuration and configuration only overrides.
        for client in CLIENTS {
            assert!(!client.client_id.is_empty(), "{}", client.provider);
            assert!(!client.scope.is_empty(), "{}", client.provider);
        }
    }

    #[test]
    fn a_callback_is_read_out_of_the_request_line() {
        let parsed =
            parse_callback("GET /callback?code=abc123&state=xyz HTTP/1.1", "/callback").unwrap();
        assert_eq!(parsed.code, "abc123");
        assert_eq!(parsed.state, "xyz");
    }

    /// A browser asks for a favicon. A listener that took the first connection
    /// as the answer would fail on every browser that does.
    #[test]
    fn an_unrelated_request_is_not_mistaken_for_the_callback() {
        assert!(parse_callback("GET /favicon.ico HTTP/1.1", "/callback").is_none());
        assert!(parse_callback("GET /callback HTTP/1.1", "/callback").is_none());
        assert!(parse_callback("GET /other?code=a&state=b HTTP/1.1", "/callback").is_none());
        assert!(parse_callback("GET /callback?code=a HTTP/1.1", "/callback").is_none());
    }

    #[test]
    fn an_encoded_callback_value_is_decoded() {
        let parsed = parse_callback(
            "GET /callback?code=a%2Fb%2Bc&state=s%20t HTTP/1.1",
            "/callback",
        )
        .unwrap();
        assert_eq!(parsed.code, "a/b+c");
        assert_eq!(parsed.state, "s t");
    }

    /// The credential lands where the broker already reads it, so a login
    /// Glasshouse performed serves requests exactly as one the broker did.
    #[test]
    fn the_credential_is_written_where_the_broker_reads_it() {
        let path = credential_path(Path::new("/tmp/auth"), "claude", "someone@example.com");
        assert!(path.ends_with("claude-someone@example.com.json"));
        // A hostile account label cannot choose a directory or hide a file.
        // The property is the *parent*, not the spelling: a dot is legitimate
        // in a label and only a separator or a leading dot is dangerous.
        let hostile = credential_path(Path::new("/tmp/auth"), "claude", "../../etc/passwd");
        assert_eq!(hostile.parent(), Some(Path::new("/tmp/auth")));
        let name = hostile.file_name().unwrap().to_string_lossy().into_owned();
        assert!(!name.contains('/') && !name.contains('\\'), "{name}");
        assert!(name.starts_with("claude-"), "{name}");
        assert!(!name.starts_with('.'), "{name}");

        // And a label that is nothing but dots cannot become `.` or `..`.
        let dots = credential_path(Path::new("/tmp/auth"), "claude", "..");
        assert_eq!(dots.parent(), Some(Path::new("/tmp/auth")));
    }

    /// The unpadded base64url alphabet, checked against the specification's
    /// own vectors rather than against this implementation's output.
    #[test]
    fn base64_url_matches_the_specification_alphabet() {
        assert_eq!(base64_url(b""), "");
        assert_eq!(base64_url(b"f"), "Zg");
        assert_eq!(base64_url(b"fo"), "Zm8");
        assert_eq!(base64_url(b"foo"), "Zm9v");
        assert_eq!(base64_url(b"foob"), "Zm9vYg");
        assert_eq!(base64_url(b"fooba"), "Zm9vYmE");
        assert_eq!(base64_url(b"foobar"), "Zm9vYmFy");
        // The two characters standard base64 uses and base64url must not.
        assert_eq!(base64_url(&[251, 255]), "-_8");
    }
}
