//! Map line 1212 — *"track rolling-window capacity separately from
//! fixed-calendar-window capacity."*
//!
//! Two real producers write these windows and they are genuinely different
//! call sites: [`RateLimitHeaders::apply_to`] fills the **rolling** window
//! from a response header (`ratelimit-reset` and friends), and
//! [`ProviderUsage::apply_to`] fills the **calendar** window from a usage
//! endpoint's JSON body (`data.limit_reset`). `provider/telemetry.rs`'s own
//! unit tests prove the header path lands on the rolling window and not the
//! calendar one; this file adds the half that was missing — the calendar
//! producer on its own, and the case that actually decides line 1212: both
//! readings arriving for the same resource, each landing in its own window
//! without disturbing the other.
//!
//! `provider/resources.rs:885-886` (`render_windows`) is the consumer,
//! displaying "rolling window" and "calendar window" as separate rows —
//! covered by inspection in the audit report and not re-derived here, since
//! it is a straight read of the same two fields these tests populate.

use glasshouse::provider::quota::WindowShape;
use glasshouse::provider::registry::ResourceKind;
use glasshouse::provider::telemetry::{ProviderUsage, RateLimitHeaders};

const OBSERVED: i64 = 1_800_000_000;

fn fresh_state() -> glasshouse::provider::quota::CapacityState {
    ResourceKind::from_direct_provider("quota-windows-test").capacity()
}

// ---------------------------------------------------------------------------
// Each producer alone.
// ---------------------------------------------------------------------------

/// A response header naming a rolling reset lands in the rolling window and
/// leaves the calendar window untouched — the same claim
/// `provider/telemetry.rs`'s own `a_reset_field_reaches_the_rolling_window_and_not_the_calendar_one`
/// makes, restated here as this line's own evidence rather than borrowed.
#[test]
fn a_rolling_reset_header_populates_only_the_rolling_window() {
    let state =
        RateLimitHeaders::read(vec![("ratelimit-reset", "30")]).apply_to(fresh_state(), OBSERVED);

    assert_eq!(state.windows().rolling().shape(), WindowShape::Rolling);
    assert_eq!(
        state.windows().rolling().resets_at_unix().value(),
        Some(&(OBSERVED + 30)),
        "the rolling window must carry the reset the header named"
    );
    assert!(
        !state.windows().calendar().resets_at_unix().is_measured(),
        "a header that named only a rolling reset must not populate the calendar window"
    );
}

/// A usage-endpoint body naming a calendar reset lands in the calendar
/// window and leaves the rolling window untouched — the discriminating half
/// the header-only unit test does not cover.
#[test]
fn a_calendar_reset_body_populates_only_the_calendar_window() {
    let body = serde_json::json!({
        "data": { "limit_reset": 3600 }
    })
    .to_string();
    let state = ProviderUsage::read(&body).apply_to(fresh_state(), OBSERVED);

    assert_eq!(state.windows().calendar().shape(), WindowShape::Calendar);
    assert_eq!(
        state.windows().calendar().resets_at_unix().value(),
        Some(&(OBSERVED + 3600)),
        "the calendar window must carry the reset the usage body named"
    );
    assert!(
        !state.windows().rolling().resets_at_unix().is_measured(),
        "a usage body that named only a calendar reset must not populate the rolling window"
    );
}

// ---------------------------------------------------------------------------
// Line 1212 itself: both readings on the same resource, neither overwriting
// the other.
// ---------------------------------------------------------------------------

/// **Line 1212.** A provider that reports a rolling reset over headers and a
/// distinct calendar reset over its usage endpoint ends up with two
/// different values in `CapacityState`, each in the window it belongs to.
/// If the two producers shared one field, the second write would clobber
/// the first; they do not, in either application order.
#[test]
fn a_rolling_and_a_calendar_reset_are_tracked_as_two_distinct_values() {
    let rolling_reset = OBSERVED + 30;
    let calendar_reset = OBSERVED + 86_400;

    let usage_body = serde_json::json!({
        "data": { "limit_reset": calendar_reset }
    })
    .to_string();

    let state =
        RateLimitHeaders::read(vec![("ratelimit-reset", "30")]).apply_to(fresh_state(), OBSERVED);
    let state = ProviderUsage::read(&usage_body).apply_to(state, OBSERVED);

    assert_eq!(
        state.windows().rolling().resets_at_unix().value(),
        Some(&rolling_reset),
        "the calendar producer must not have overwritten the rolling window's reading"
    );
    assert_eq!(
        state.windows().calendar().resets_at_unix().value(),
        Some(&calendar_reset),
        "the calendar window must carry its own reading, not the rolling window's"
    );
    assert_ne!(
        rolling_reset, calendar_reset,
        "the two readings must differ for this test to prove anything about keeping them apart"
    );

    // The reverse application order, proving neither producer depends on
    // running first.
    let reordered = ProviderUsage::read(&usage_body).apply_to(fresh_state(), OBSERVED);
    let reordered =
        RateLimitHeaders::read(vec![("ratelimit-reset", "30")]).apply_to(reordered, OBSERVED);
    assert_eq!(
        reordered.windows().rolling().resets_at_unix().value(),
        Some(&rolling_reset)
    );
    assert_eq!(
        reordered.windows().calendar().resets_at_unix().value(),
        Some(&calendar_reset)
    );
}
