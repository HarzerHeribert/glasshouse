//! `commands::response` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use glasshouse::config::response::{ResponseProfileEntry, ResponseRequest};
use glasshouse::profile::response::{Dimension, Role as ResponseRole};

/// Open a harness session attached to this terminal.
///
/// This is the production consumer of the sanctioned launch path: the harness
/// is chosen and its executable resolved from configuration (project level
/// overriding user level), the requested launch profile is resolved against
/// its adapter (Phase 9A/9F — see [`glasshouse::profile`]), and only then is
/// anything started through [`HarnessLaunch`] — the only route that exists,
/// and the one that derives the child's working directory from the active
/// project rather than from whatever directory Glasshouse happened to be run
/// in.
///
/// Setup is deliberately not triggered here. A user who has named a harness
/// has already said what they want; interrupting that with a first-run wizard
/// would be answering a question they did not ask.
/// A [`ResponseRequest`] from the command line, refusing an unknown role by
/// name.
///
/// A role is refused rather than reported, because a role selects which
/// *defaults* apply: a mistyped `--response-role reviwer` that fell back to
/// `interactive` would silently give a session the wrong communication policy,
/// and the user would have no way to tell. An axis value is different — it is
/// carried through and reported by name if this build does not know it, which
/// is the visible-degradation rule the rest of the configuration follows.
pub(crate) fn response_request(
    role: Option<&str>,
    session_preset: Option<String>,
    axes: impl IntoIterator<Item = (Dimension, Option<String>)>,
) -> anyhow::Result<ResponseRequest> {
    let role = match role {
        Some(slug) => Some(ResponseRole::from_slug(slug).ok_or_else(|| {
            anyhow::anyhow!(
                "`{slug}` is not a role Glasshouse knows; the roles are: {}",
                ResponseRole::names()
            )
        })?),
        None => None,
    };
    let mut task = ResponseProfileEntry::default();
    for (dimension, value) in axes {
        task.set_axis(dimension, value);
    }
    Ok(ResponseRequest {
        role,
        session_preset,
        task,
    })
}
