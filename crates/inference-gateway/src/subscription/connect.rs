//! What signing a subscription account in reports.
//!
//! The broker's own login does the signing in
//! (`gateway::subscription_broker::login`), so the credential is written in
//! the format the broker serves with. What leaves that login is **never a
//! secret**: a sign-in link, a device code a person types, a success or a
//! failure. That boundary is what lets the flow be rendered inside another
//! program's screen instead of handing the terminal to a child.

use serde::Serialize;

/// What a caller driving a sign-in is told.
///
/// Every variant is safe to render, log and send to another process. There is
/// deliberately no variant carrying a token, an authorization code or a
/// verifier: the type is the boundary, so a future field cannot leak one by
/// accident.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Progress {
    /// The link to open. It carries a client id and a PKCE challenge, both
    /// public by construction.
    Opened {
        authorize_url: String,
        /// Whether the gateway opened it in this machine's default browser.
        #[serde(default)]
        browser_opened: bool,
    },
    /// A code to enter at `verification_url` on any device. Not a credential:
    /// it is shown so a person can type it, and it authorises only this login.
    DeviceCode {
        verification_url: String,
        user_code: String,
    },
    /// Nothing has arrived yet.
    Waiting {
        seconds_remaining: u64,
    },
    /// A credential was written. The account label is the saved credential's
    /// name, never a token.
    Connected {
        account: Option<String>,
    },
    Failed {
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything a caller is told is safe to render and to send to another
    /// process. The type is the boundary.
    #[test]
    fn no_progress_variant_can_carry_a_secret() {
        let opened = Progress::Opened {
            authorize_url: "https://example.invalid/x".into(),
            browser_opened: true,
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
}
