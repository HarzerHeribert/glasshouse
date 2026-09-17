//! The subscription-headroom estimate and its replay accounting — the one
//! join this file keeps once Glasshouse deletes its router
//! (design-decisions.md, 2026-09-16): the routing-consumption, effort-shadow
//! and responsiveness/separation readers it used to hold had no production
//! caller once the router that fed them was gone.

use super::*;

use crate::provider::quota::Confidence;

/// How recent a throttle must be to read as still-live pressure rather than
/// history the window happens to still hold, and how close a reset must sit
/// to count as imminent relief — map line 1245's "recency", one horizon for
/// both questions rather than a second invented number: an hour is the
/// shortest cadence window this project's own throttle producers actually
/// observe (`crate::gateway::session`'s own per-window limiters), so a
/// throttle or a reset outside it says nothing about the account's *current*
/// pressure.
pub const RECENT_SIGNAL_HORIZON_SECONDS: i64 = 3_600;

/// Map line 1249's second horizon — pressure that persists well past the
/// short window rather than a single accident. Three days, not a week or a
/// month: the one production caller queries rows only
/// [`CLASSIFICATION_EVIDENCE_WINDOW_SECONDS`] deep (seven days), and setting
/// this horizon at or past that bound would make
/// [`LongWindowPressure::NoPressure`] structurally unreachable — no query
/// could ever cover it, so the honest answer would always collapse to
/// [`LongWindowPressure::Undistinguished`]. Three days leaves the full
/// window room to actually prove an absence.
pub const LONG_SIGNAL_HORIZON_SECONDS: i64 = 3 * 24 * 3_600;

/// Map line 1248's anecdote guard: fewer than this many observed
/// throttle→success recoveries in window, and no reset window is learned at
/// all. Two is the floor at which a single unlucky pairing — a throttle
/// immediately followed, by coincidence, by an unrelated success — cannot be
/// the whole story behind the learned value.
pub const MIN_LEARNED_RESET_RECOVERIES: usize = 2;

/// Map line 1245's estimator, and lines 1244/1246/1250/1251/1254 with it —
/// see [`SubscriptionHeadroomEstimate`] and [`HeadroomBand`] for the type's
/// own honesty rules. No new table, no migration, no persisted estimator
/// state: every call re-derives the estimate from rows the caller already
/// holds.
///
/// Reads accepted-request counts and throttle events (narrowed to
/// `credential_label` only when **every** informative row names its
/// account; one contextless row widens to provider scope, map line 1246),
/// token usage (never turned into a figure, only recorded on
/// [`HeadroomBasis`], line 1251), reset behavior via `seconds_until_reset`
/// (line 1248: `None` falls back to a value learned from `scoped`'s own
/// throttle→success recoveries — see [`ResetBasis`] — never displacing a
/// real reading), and `recent_session_count` — none of them queried here,
/// all handed in by the caller.
///
/// `None` — unknown — when nothing at all is available: no informative row,
/// no session count, no reset reading. An account this genuinely unmeasured
/// is not "exhausted" and not "ample"; it is unmeasured, the 32B line-1239
/// discipline every other facet on `ResolvedEntitlement` already keeps.
// History: design-decisions.md, "Trims: routing module docs", routing/evidence/joins.rs `fn estimate_subscription_headroom`.
pub fn estimate_subscription_headroom(
    observations: &[RoutingObservation],
    provider: &str,
    credential_label: Option<&str>,
    now_unix: i64,
    seconds_until_reset: Option<i64>,
    recent_session_count: Option<usize>,
) -> Option<SubscriptionHeadroomEstimate> {
    let informative: Vec<&RoutingObservation> = observations
        .iter()
        .filter(|row| row.provider == provider)
        .filter(|row| row.outcome.is_some() && row.purpose.as_deref() != Some(CORRELATION_PURPOSE))
        .collect();

    let every_row_names_its_account =
        !informative.is_empty() && informative.iter().all(|row| row.quota_context.is_some());
    let account_narrowed = credential_label.is_some() && every_row_names_its_account;

    let scoped: Vec<&RoutingObservation> = if account_narrowed {
        informative
            .into_iter()
            .filter(|row| row.quota_context.as_deref() == credential_label)
            .collect()
    } else {
        informative
    };

    let accepted = scoped
        .iter()
        .filter(|row| row.outcome == Some(Outcome::Succeeded))
        .count();
    let most_recent_throttle_age = scoped
        .iter()
        .filter(|row| row.failure_class == Some(FailureClass::Throttle))
        .map(|row| now_unix.saturating_sub(row.observed_at_unix))
        .min();
    let carried_tokens = scoped
        .iter()
        .any(|row| row.input_tokens.is_some() || row.output_tokens.is_some());

    let session_count = recent_session_count.unwrap_or(0);

    if scoped.is_empty() && session_count == 0 && seconds_until_reset.is_none() {
        return None;
    }

    let recent_pressure =
        most_recent_throttle_age.is_some_and(|age| age <= RECENT_SIGNAL_HORIZON_SECONDS);
    let any_pressure = most_recent_throttle_age.is_some();
    let has_activity = accepted > 0 || session_count > 0;

    // Map line 1248: a stated reading is authoritative and is never
    // recomputed; only its absence opens the door to a learned fallback,
    // and even then only past the anecdote guard.
    let (effective_seconds_until_reset, reset_basis) = match seconds_until_reset {
        Some(seconds) => (Some(seconds), ResetBasis::Stated),
        None => match learn_reset_window_seconds(&scoped) {
            Some(window) => (Some(window), ResetBasis::Learned),
            None => (None, ResetBasis::Unknown),
        },
    };
    let reset_imminent = effective_seconds_until_reset
        .is_some_and(|seconds| (0..=RECENT_SIGNAL_HORIZON_SECONDS).contains(&seconds));

    // Map line 1249: positive evidence of long-window pressure needs no
    // full coverage of the horizon — one throttle out there is real
    // evidence regardless of how far back the rest of `scoped` reaches.
    // Its *absence* does, or the honest answer is "we did not look that
    // far", not "nothing happened".
    let long_window_pressure = {
        let present = scoped
            .iter()
            .filter(|row| row.failure_class == Some(FailureClass::Throttle))
            .map(|row| now_unix.saturating_sub(row.observed_at_unix))
            .any(|age| age > RECENT_SIGNAL_HORIZON_SECONDS && age <= LONG_SIGNAL_HORIZON_SECONDS);
        if present {
            LongWindowPressure::Present
        } else {
            let deepest_age = scoped
                .iter()
                .map(|row| now_unix.saturating_sub(row.observed_at_unix))
                .max();
            match deepest_age {
                Some(age) if age >= LONG_SIGNAL_HORIZON_SECONDS => LongWindowPressure::NoPressure,
                _ => LongWindowPressure::Undistinguished,
            }
        }
    };

    let band = match (recent_pressure, any_pressure, reset_imminent, has_activity) {
        (true, _, true, _) => HeadroomBand::Low,
        (true, _, false, _) => HeadroomBand::Exhausted,
        (false, true, _, _) => HeadroomBand::Low,
        (false, false, _, true) => HeadroomBand::Ample,
        (false, false, _, false) => HeadroomBand::Moderate,
    };

    Some(SubscriptionHeadroomEstimate {
        band,
        confidence: Confidence::Low,
        basis: if carried_tokens {
            HeadroomBasis::TokenUsage
        } else {
            HeadroomBasis::RequestActivity
        },
        account_narrowed,
        reset_basis,
        long_window_pressure,
        since_unix: None,
    })
}

/// Map line 1248's fallback window: the interval between a `Throttle` row
/// and the next `Succeeded` row after it in `scoped`, averaged across every
/// such recovery — `None` below [`MIN_LEARNED_RESET_RECOVERIES`] of them,
/// the anecdote rule stated in the packet this shipped from. Only ever
/// consulted by [`estimate_subscription_headroom`] when the caller supplied
/// no real `seconds_until_reset` at all.
fn learn_reset_window_seconds(scoped: &[&RoutingObservation]) -> Option<i64> {
    let mut ordered: Vec<&RoutingObservation> = scoped.to_vec();
    ordered.sort_by_key(|row| row.observed_at_unix);

    let mut recoveries = Vec::new();
    for (index, row) in ordered.iter().enumerate() {
        if row.failure_class != Some(FailureClass::Throttle) {
            continue;
        }
        if let Some(success) = ordered[index + 1..]
            .iter()
            .find(|later| later.outcome == Some(Outcome::Succeeded))
        {
            let recovery = success
                .observed_at_unix
                .saturating_sub(row.observed_at_unix);
            if recovery > 0 {
                recoveries.push(recovery);
            }
        }
    }

    if recoveries.len() < MIN_LEARNED_RESET_RECOVERIES {
        return None;
    }
    let sum: i64 = recoveries.iter().sum();
    Some(sum / recoveries.len() as i64)
}

/// [`EvidenceLedger::headroom_replay`]'s result — map line 1836, replaying
/// [`estimate_subscription_headroom`] against every throttle or exhaustion a
/// provider recorded, using only the rows that preceded it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HeadroomReplayCounts {
    /// The replayed estimate's band was [`HeadroomBand::Low`] or
    /// [`HeadroomBand::Exhausted`] — the estimator would have warned.
    pub warned: usize,
    /// The replayed estimate's band was [`HeadroomBand::Moderate`] or
    /// [`HeadroomBand::Ample`] — the estimator would have missed it.
    pub missed: usize,
    /// [`estimate_subscription_headroom`] returned [`None`]: fewer rows
    /// came before this throttle than the estimator could read anything
    /// from at all.
    pub unestimable: usize,
    /// The median seconds from a throttle to this provider's first
    /// [`Outcome::Succeeded`] row after it — `None` when no throttle in the
    /// window was ever followed by one.
    pub observed_reset_lag_median_seconds: Option<i64>,
    /// How many throttles [`Self::observed_reset_lag_median_seconds`] is a
    /// median over.
    pub observed_reset_lag_sample_count: usize,
}

impl HeadroomReplayCounts {
    /// [`Self::warned`] + [`Self::missed`] + [`Self::unestimable`] — every
    /// throttle or exhaustion this replay scored, the denominator
    /// [`MIN_SAMPLE_FOR_SUMMARY`] gates the whole reading on.
    pub fn throttles(&self) -> usize {
        self.warned + self.missed + self.unestimable
    }
}
impl EvidenceLedger {
    /// **Map line 1836.** Replays [`estimate_subscription_headroom`] against
    /// every throttle or exhaustion this provider recorded in the window,
    /// using only the rows that came *before* it — never the estimator's
    /// live inputs. `credential_label`, `seconds_until_reset` and
    /// `recent_session_count` are always absent here: this replay has no
    /// account narrowing to apply and no gateway-quota-cache reading to
    /// hand in, and pretending otherwise would score the estimator against
    /// evidence it never actually had at that moment. `estimate_subscription_headroom`
    /// itself is not modified; this calls it once per throttle.
    ///
    /// Paired with the *observed reset lag*: this ledger records no
    /// provider-stated wait ([`RoutingObservation`] carries no
    /// `retry_after`/reset field — that reading lives only in the gateway's
    /// quota-cache file, a different store [`Self`] does not open), so the
    /// only honest reset figure is the one actually observed — from a
    /// throttle at `t` to this provider's first [`Outcome::Succeeded`] row
    /// after `t`, in the same window.
    pub fn headroom_replay(
        &self,
        provider: &str,
        now_unix: i64,
        window_seconds: i64,
    ) -> Result<HeadroomReplayCounts, EvidenceLedgerError> {
        let rows = self.observations_in_window(now_unix, window_seconds)?;
        let provider_rows: Vec<&RoutingObservation> =
            rows.iter().filter(|row| row.provider == provider).collect();

        let mut warned = 0usize;
        let mut missed = 0usize;
        let mut unestimable = 0usize;
        let mut reset_lags: Vec<i64> = Vec::new();

        for row in &provider_rows {
            if !matches!(
                row.failure_class,
                Some(FailureClass::Throttle) | Some(FailureClass::ExhaustedQuota)
            ) {
                continue;
            }
            let t = row.observed_at_unix;
            let prior: Vec<RoutingObservation> = provider_rows
                .iter()
                .filter(|candidate| candidate.observed_at_unix < t)
                .map(|candidate| (*candidate).clone())
                .collect();
            match estimate_subscription_headroom(&prior, provider, None, t, None, None) {
                Some(estimate) => match estimate.band {
                    HeadroomBand::Low | HeadroomBand::Exhausted => warned += 1,
                    HeadroomBand::Moderate | HeadroomBand::Ample => missed += 1,
                },
                None => unestimable += 1,
            }
            if let Some(recovery) = provider_rows
                .iter()
                .filter(|candidate| candidate.observed_at_unix > t)
                .find(|candidate| candidate.outcome == Some(Outcome::Succeeded))
            {
                reset_lags.push(recovery.observed_at_unix - t);
            }
        }

        let observed_reset_lag_sample_count = reset_lags.len();
        let observed_reset_lag_median_seconds = if reset_lags.is_empty() {
            None
        } else {
            Some(median(reset_lags))
        };

        Ok(HeadroomReplayCounts {
            warned,
            missed,
            unestimable,
            observed_reset_lag_median_seconds,
            observed_reset_lag_sample_count,
        })
    }
}
