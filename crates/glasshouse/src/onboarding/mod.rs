//! First-run onboarding wizard (Phase 2C).
//!
//! On first launch this detects supported harnesses and useful local tools,
//! shows what was found, and lets the user enable or ignore each before the
//! normal interface opens: a welcome, the integration list, an optional
//! bypass-acknowledgement step, an optional provider step, and an optional
//! routing-model step, then a summary.
//!
//! The routing-model step offers exactly Phase 2C's three choices —
//! Automatic, Choose model, Do later — and *records which one the user
//! picked*, as a reference (provider, model, never a credential; see
//! [`crate::config::RoutingModelChoice`]); it does not classify anything or
//! build a fallback chain (Phases 34B/34C). "Do later" leaves deterministic
//! routing heuristics in charge; the provider step configures from a
//! built-in template and stops there (the gateway is Phase 9D).
//!
//! The state machine ([`state::WizardState`]), the rendering, and the event
//! loop ([`run`]) are separate; only [`run`] touches a terminal. History:
//! design-decisions.md, "Trims: the remaining module docs, second packet",
//! onboarding/mod.rs module doc.

mod state;
mod view;

use anyhow::{Context, Result};

use crate::config::UserConfig;
use crate::integrations::Discovery;
use crate::tui::{DEFAULT_TICK, Event, EventSource, Screen};
use crate::{Runtime, VERSION};

pub use state::{Action, IntegrationDetection, PathInputView, RowView, Step, WizardState};

/// What happened when the wizard exited.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// The user finished the wizard. `config` is the same [`UserConfig`]
    /// [`run`] was given, with every recorded decision applied and
    /// onboarding marked completed — already persisted to disk, so the
    /// caller does not need to save it again.
    Completed(Box<UserConfig>),
    /// The user cancelled (Esc, Ctrl-C, or a shutdown signal). Nothing was
    /// written: onboarding is not marked completed, so it opens again next
    /// time (or the caller can retry `run` immediately for a "reconfigure"
    /// invocation the user backed out of).
    Cancelled,
}

/// Whether the first-run wizard needs to run before the normal interface
/// opens.
///
/// Trivial today (`!config.onboarding().completed()`), but kept as a named
/// function rather than an inline check at every call site: the condition is
/// a product decision (Phase 2C: "Detect whether the current user has
/// completed Glasshouse onboarding before opening the normal TUI for the
/// first time"), and callers should ask that question by name instead of
/// reaching into [`UserConfig`]'s shape themselves.
pub fn is_required(config: &UserConfig) -> bool {
    !config.onboarding().completed()
}

/// Map a completed discovery pass into the plain data [`WizardState::new`]
/// wants. See [`IntegrationDetection`]'s documentation for why the wizard
/// does not consume [`crate::integrations::DetectedIntegration`] directly.
fn detections_from(discovery: &Discovery) -> Vec<IntegrationDetection> {
    discovery
        .all()
        .iter()
        .map(|d| IntegrationDetection {
            id: d.id(),
            status: d.status(),
            executable: d.executable().map(|e| e.path().to_path_buf()),
            version: d.version().map(ToString::to_string),
        })
        .collect()
}

/// Run the first-run wizard, or a later "reconfigure" invocation.
///
/// `existing` seeds every screen with the user's current configuration —
/// on a genuine first run that is [`UserConfig::default`] (nothing decided
/// yet); on a reopen from settings it is whatever was loaded from disk, so
/// the wizard pre-selects the user's existing choices instead of starting
/// blank (Phase 2C: "Allow the onboarding wizard to be reopened later from
/// settings"). `discovery` is a result the caller already has — this
/// function never calls [`Discovery::run`] itself, so a reconfigure
/// invocation can hand it a fresh pass and a test can hand it a
/// deterministic one.
///
/// On [`Outcome::Completed`], the updated configuration has already been
/// saved via [`UserConfig::save`]. On [`Outcome::Cancelled`] — the user
/// pressed Esc/Ctrl-C, or a shutdown signal arrived — nothing is written at
/// all: a signal or an Esc press is not consent to persist whatever partial
/// state the wizard happened to be in, so this deliberately does not offer
/// an "are you sure" prompt either. Simply not saving is the whole
/// cancellation behavior.
pub fn run(runtime: &Runtime, discovery: &Discovery, existing: UserConfig) -> Result<Outcome> {
    let detected = detections_from(discovery);
    let mut state = WizardState::new(
        &detected,
        &existing,
        runtime.project().name(),
        runtime.project().display_root(),
        VERSION.to_owned(),
    );
    let mut config = existing;

    let mut screen = Screen::acquire()?;
    let events = EventSource::new(DEFAULT_TICK);

    screen.draw(|frame| view::render(&state, frame))?;

    loop {
        match events.next()? {
            Event::Key(key) => {
                // `handle_key` recognizes Esc/Ctrl-C itself (see `state`'s
                // module documentation for why cancellation lives there
                // rather than being special-cased in this loop) and answers
                // with `Action::Cancel`, handled in the same place as every
                // other outcome below.
                match state.handle_key(key) {
                    Action::None => {}
                    Action::Redraw => screen.draw(|frame| view::render(&state, frame))?,
                    Action::Cancel => return Ok(Outcome::Cancelled),
                    Action::Finish => {
                        state.apply_to(&mut config);
                        config
                            .save(runtime.paths())
                            .context("could not save onboarding choices")?;
                        return Ok(Outcome::Completed(Box::new(config)));
                    }
                }
            }
            Event::Resize(cols, rows) => {
                screen.on_resize(cols, rows)?;
                screen.draw(|frame| view::render(&state, frame))?;
            }
            // A signal is not consent: leave immediately without saving a
            // half-finished config, exactly like an explicit cancel.
            Event::Shutdown => return Ok(Outcome::Cancelled),
            // Nothing here reacts to these: a plain tick must not repaint an
            // idle wizard, and mouse/paste/app events carry nothing the
            // wizard understands.
            Event::Tick | Event::Mouse(_) | Event::Paste(_) | Event::App(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onboarding_is_required_until_completed() {
        let mut config = UserConfig::default();
        assert!(is_required(&config));

        config.onboarding_mut().mark_completed(VERSION.to_owned());
        assert!(!is_required(&config));
    }
}
