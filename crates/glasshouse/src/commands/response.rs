//! `commands::response` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use glasshouse::config::response::{ResponseProfileEntry, ResponseRequest};
use glasshouse::profile::response::{Dimension, Role as ResponseRole};

/// A [`ResponseRequest`] from the command line, refusing an unknown role by
/// name.
///
/// A role is refused rather than reported, because a role selects which
/// *defaults* apply: a mistyped `--response-role reviwer` that fell back to
/// `interactive` would silently give a session the wrong communication policy,
/// and the user would have no way to tell. An axis value is different — it is
/// carried through and reported by name if this build does not know it, which
/// is the visible-degradation rule the rest of the configuration follows.
///
/// History: design-decisions.md, "Trims: commands module docs", response_request.
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
