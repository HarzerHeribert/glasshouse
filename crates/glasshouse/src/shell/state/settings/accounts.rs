//! The Settings overlay's Subscriptions section: the accounts a user connects
//! their existing Claude, ChatGPT or Gemini plan through.
//!
//! **The invariant: this section never runs the login.** `glasshouse
//! subscriptions login` spawns the CLIProxyAPI broker with inherited stdio and
//! blocks up to fifteen minutes on a browser OAuth flow; the shell is holding
//! the terminal in raw mode with an alternate screen up, so hosting that flow
//! inside an overlay would hand the browser's console output to a screen that
//! is not scrolling and take the keyboard away from a user who has no way to
//! give it back. What the section does instead is **name the exact command**,
//! with the account under the cursor substituted into it, and let the user
//! copy it into another terminal. A user told precisely what to type is
//! helped; a user shown nothing is not — which was the state before this
//! section existed, subscriptions having had no TUI surface at all.
//!
//! The second invariant is [`crate::subscription`]'s and is repeated here
//! because this is where the words reach a user: the section reports
//! **presence**, never validity. `credential present` is what a directory
//! entry proves.

use crate::subscription::Account;

/// One row of the Settings "Subscriptions" section.
///
/// A projection of [`crate::subscription::Account`] rather than the account
/// itself, matching every other row type in this module: `SettingsState` holds
/// display-ready values and never reaches back into the filesystem, which is
/// what keeps `shell/state` free of I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionRow {
    /// The `[entitlements.<name>]` table this account is.
    pub entitlement: String,
    /// The vendor word the `login` subcommand takes — `anthropic`, `openai`,
    /// `google`.
    pub provider: String,
    /// Whether the account's auth directory holds a file. Not whether that
    /// file still authenticates anything.
    pub credential_present: bool,
    /// The literal command that connects this account.
    pub connect_command: String,
    /// The literal command that disconnects it.
    pub disconnect_command: String,
}

impl SubscriptionRow {
    /// Build a row from what the run loop read off disk.
    pub fn from_account(account: &Account) -> Self {
        Self {
            entitlement: account.entitlement.clone(),
            provider: account.provider.as_str().to_owned(),
            credential_present: account.credential_present,
            connect_command: account.connect_command(),
            disconnect_command: account.disconnect_command(),
        }
    }

    /// The status word, in the measured-not-judged vocabulary.
    pub fn status(&self) -> &'static str {
        if self.credential_present {
            "credential present"
        } else {
            "not connected"
        }
    }

    /// The command the section offers for this row: disconnect once a
    /// credential is present, connect while it is not.
    ///
    /// One accessor rather than two call sites choosing, so the pill's label
    /// and the command spelled out beneath it cannot name different acts.
    pub fn primary_command(&self) -> &str {
        if self.credential_present {
            &self.disconnect_command
        } else {
            &self.connect_command
        }
    }

    /// What the pill for [`Self::primary_command`] is called.
    pub fn primary_action(&self) -> &'static str {
        if self.credential_present {
            "disconnect"
        } else {
            "connect"
        }
    }
}

/// Which of an account's commands the section was asked to spell out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountCommand {
    /// Whatever the row's own state calls for — connect, or disconnect once a
    /// credential is present.
    Primary,
    Connect,
    Disconnect,
    /// The step before any of them: adopting a broker binary.
    AdoptBroker,
}

/// Whether a CLIProxyAPI binary is adopted, which gates every `login`.
///
/// Carried beside the rows rather than derived from them: an empty account
/// list and an unadopted broker are different problems with different first
/// steps, and a section that showed one sentence for both would send a user
/// to the wrong one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BrokerState {
    pub adopted: bool,
}

/// What the section shows above its rows, chosen by the state it is actually
/// in — the "tips and tricks" the user asked for, as one line that changes
/// rather than a help page nobody opens.
///
/// The order is the order the steps must happen in: a broker binary, then an
/// entitlement, then a login. Naming the second step while the first is
/// missing sends the user down a path that will refuse them.
pub fn subscription_tip(rows: &[SubscriptionRow], broker: BrokerState) -> String {
    if !broker.adopted {
        return format!(
            "Step 1 of 3 — Glasshouse ships no broker and downloads none. Adopt a \
             CLIProxyAPI binary you verified yourself: {}",
            crate::subscription::ADOPT_BINARY_COMMAND
        );
    }
    if rows.is_empty() {
        return format!("Step 2 of 3 — {}", crate::subscription::NO_ACCOUNTS_HINT);
    }
    if rows.iter().all(|row| row.credential_present) {
        return "Every configured account has a credential on disk. Presence is not \
                validity: an expired token is still a file, so a routing failure is \
                how an expired account shows up."
            .to_owned();
    }
    "Step 3 of 3 — connect an account with the command shown below it. The login \
     opens your browser and Glasshouse never sees the token."
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::SubscriptionProvider;

    fn account(entitlement: &str, present: bool) -> Account {
        Account {
            entitlement: entitlement.to_owned(),
            provider: SubscriptionProvider::Anthropic,
            credential_present: present,
        }
    }

    #[test]
    fn a_disconnected_row_offers_the_login_command_for_its_own_account() {
        let row = SubscriptionRow::from_account(&account("claude-max", false));
        assert_eq!(row.primary_action(), "connect");
        assert_eq!(
            row.primary_command(),
            "glasshouse subscriptions login anthropic --entitlement claude-max"
        );
        assert_eq!(row.status(), "not connected");
    }

    #[test]
    fn a_connected_row_offers_the_logout_command_instead() {
        let row = SubscriptionRow::from_account(&account("claude-max", true));
        assert_eq!(row.primary_action(), "disconnect");
        assert_eq!(
            row.primary_command(),
            "glasshouse subscriptions logout anthropic --entitlement claude-max"
        );
        assert_eq!(row.status(), "credential present");
    }

    /// The tip names the step that is actually next, not the step the section
    /// is about.
    #[test]
    fn the_tip_names_the_first_missing_step_and_not_a_later_one() {
        let unadopted = subscription_tip(&[], BrokerState { adopted: false });
        assert!(
            unadopted.contains("adopt-binary"),
            "an unadopted broker must be named before any login: `{unadopted}`"
        );
        assert!(
            !unadopted.contains("subscriptions login"),
            "naming the login while it cannot run sends the user to a refusal"
        );

        let no_accounts = subscription_tip(&[], BrokerState { adopted: true });
        assert!(
            no_accounts.contains("entitlements."),
            "with the broker adopted the next step is an entitlement: `{no_accounts}`"
        );

        let disconnected = subscription_tip(
            &[SubscriptionRow::from_account(&account("claude-max", false))],
            BrokerState { adopted: true },
        );
        assert!(
            disconnected.contains("connect an account"),
            "`{disconnected}`"
        );
    }

    #[test]
    fn the_all_connected_tip_refuses_to_call_a_present_credential_valid() {
        let tip = subscription_tip(
            &[SubscriptionRow::from_account(&account("claude-max", true))],
            BrokerState { adopted: true },
        );
        assert!(
            tip.contains("Presence is not"),
            "the section must say what presence does not prove: `{tip}`"
        );
    }
}
