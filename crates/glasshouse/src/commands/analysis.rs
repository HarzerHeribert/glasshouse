//! `glasshouse analysis` — the published model measurements, as a catalogue.
//!
//! **The first production caller `routing::analysis` has ever had.** The
//! module fetched, cached and normalised an Artificial Analysis catalogue and
//! nothing read it, which is exactly the shape `scripts/cluster-b.py` looks
//! for. Its consumer is `pane`'s model picker, ordering a 469-entry list by
//! something better than the alphabet.
//!
//! `pane` links against no part of this crate and reaches Glasshouse only as
//! a binary, so the catalogue crosses the same seam every other fact does:
//! this command's stdout.

use std::time::Duration;

use glasshouse::routing::analysis;

/// A day.
///
/// The index moves when a model is re-evaluated or a new one is listed, which
/// is not an hourly event. A shorter window would spend a free-tier budget on
/// answers that had not changed.
const TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Prints the catalogue, refreshing first when asked.
///
/// **A missing catalogue is an empty one, not a failure.** The key is
/// optional, the service is remote, and the only caller orders a list with
/// it: every one of those degrades to "no scores", and a non-zero exit would
/// turn an ordering preference into a broken model picker.
pub(crate) fn run(paths: &glasshouse::paths::RuntimePaths, refresh: bool) -> String {
    if refresh {
        // Reported on stderr so stdout stays exactly one JSON document for
        // the machine consumer.
        let said = match analysis::ensure(paths.data_dir(), TTL) {
            analysis::Refresh::NotConfigured => {
                format!("{} is not set; using any cached catalogue", analysis::KEY_VARIABLE)
            }
            analysis::Refresh::Fresh => "cache is fresh".to_string(),
            analysis::Refresh::BudgetLow { remaining } => {
                format!("budget low ({remaining} left); using the cached catalogue")
            }
            analysis::Refresh::Fetched { models } => format!("fetched {models} models"),
            analysis::Refresh::Failed(why) => format!("refresh failed: {why}"),
        };
        eprintln!("analysis: {said}");
    }
    match analysis::cached(paths.data_dir()) {
        Some(catalogue) => serde_json::to_string(&catalogue)
            .unwrap_or_else(|_| r#"{"fetched_at":0,"models":{}}"#.to_string()),
        None => r#"{"fetched_at":0,"models":{}}"#.to_string(),
    }
}
