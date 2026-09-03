//! `commands::setup` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use std::io::IsTerminal;

use glasshouse::Runtime;
use glasshouse::config::UserConfig;
use glasshouse::integrations::Discovery;
use glasshouse::onboarding;

/// Why setup is being considered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupTrigger {
    /// Glasshouse is starting normally and setup has never been completed.
    FirstRun,
    /// The user asked for it with `glasshouse setup`.
    Requested,
}

/// Run the setup wizard when it is wanted and possible.
///
/// Returns whether setup ended up completed. A first run that cannot show a
/// wizard is not an error: Glasshouse still works, it just has not recorded
/// the user's harness choices yet.
pub(crate) fn setup(runtime: &Runtime, trigger: SetupTrigger) -> anyhow::Result<bool> {
    let config = UserConfig::load(runtime.paths())?;

    if trigger == SetupTrigger::FirstRun && !onboarding::is_required(&config) {
        return Ok(true);
    }

    // The wizard needs a terminal it can take over. Piped or redirected output
    // means Glasshouse is being scripted, and silently blocking on a full
    // screen interface nobody can see would be worse than skipping it.
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        match trigger {
            SetupTrigger::FirstRun => {
                eprintln!(
                    "glasshouse: setup has not been completed. Run `glasshouse setup` \
                     in an interactive terminal to choose which harnesses to use."
                );
                return Ok(false);
            }
            SetupTrigger::Requested => {
                anyhow::bail!("`glasshouse setup` needs an interactive terminal");
            }
        }
    }

    // Discovery probes each harness for its version, so it is done once, here,
    // rather than inside the wizard: the wizard is a state machine over an
    // already-known result, which is what makes it testable without a terminal.
    let discovery = Discovery::run(runtime.project());

    match onboarding::run(runtime, &discovery, config)? {
        onboarding::Outcome::Completed(_) => Ok(true),
        onboarding::Outcome::Cancelled => {
            eprintln!("glasshouse: setup cancelled; nothing was saved.");
            Ok(false)
        }
    }
}
