//! What the context meter knows: how much of the window a task has used, how
//! much the window is, and how much the screen may claim about that figure.
//!
//! Moved out of `session.rs` whole on 2026-09-17 when the window's provenance
//! arrived and pushed that file past the size ratchet -- the user's rule for
//! that wall is that it is a signal to split rather than to shorten. Nothing
//! here is new: these are the three functions that were already the meter's,
//! and they are the meter's together.
//!
//! The one invariant worth stating in one place: **a figure and where it came
//! from travel together.** A cap kept without its source would let the status
//! line draw a percentage against a number nobody measured, which is the one
//! claim `crate::models::WindowSource` exists to prevent.

use super::*;

pub(super) fn record_request(notebook: &mut Notebook, measurement: RequestMeasurement) {
    output::parent_response(&measurement);
    if let Some(used) = measurement.context_tokens() {
        let cap = notebook.context.and_then(|context| context.cap);
        // The cap and where it came from travel together: a figure kept
        // without its provenance would let the meter claim a percentage it
        // has not earned.
        let cap_source = notebook
            .context
            .map(|context| context.cap_source)
            .unwrap_or_default();
        notebook.context = Some(ContextTokens {
            used,
            cap,
            cap_source,
            counted: Counted::Gateway,
        });
    }
    if notebook.requests.len() >= REQUEST_MEASUREMENT_CAP {
        let remove = notebook.requests.len() + 1 - REQUEST_MEASUREMENT_CAP;
        notebook.requests.drain(..remove);
    }
    notebook.requests.push(measurement);
}

pub(super) fn estimate_context(
    notebook: &mut Notebook,
    estimate: u64,
    cap: Option<u64>,
    cap_source: crate::models::WindowSource,
) {
    notebook.context = Some(ContextTokens {
        used: estimate,
        cap,
        cap_source,
        counted: Counted::Estimated,
    });
}

pub(super) fn context_cap(
    session: &Session<'_>,
    model: &str,
) -> (Option<u64>, crate::models::WindowSource) {
    let configured = session
        .context_window
        .as_ref()
        .and_then(|(configured_model, cap)| (configured_model == model).then_some(*cap));
    // `--context-window-tokens` first, then a window this route was watched
    // enforcing, then whatever a catalogue published for the model; an
    // unknown window stays unknown and the meter says so. The source travels
    // with the figure because the meter may only draw a percentage against a
    // measured one.
    crate::models::window_source_for(model, configured)
}
