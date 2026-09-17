//! Credential throttle/spend/cost readers over an already-fetched
//! `&[RoutingObservation]` slice — entitlement telemetry's own readers,
//! kept once the router's route-correlation and throttle-scope
//! classification (both 2026-09-16, Glasshouse never decides which model is
//! used) had no caller left. None of this touches SQL; it is a sibling of
//! `readers.rs`.

use super::*;

use crate::provider::pricing::PriceTable;

/// Map line 1965's recent-throttling facet, counted from raw rows: how many
/// informative throttles the window's observations record against
/// `provider`, and whether that count could honestly be narrowed to one
/// account.
///
/// `account_narrowed` is `true` only when **every** throttle row of the
/// provider carries a [`RoutingObservation::quota_context`] and a
/// `credential_label` was given to narrow by — then `throttled` counts that
/// account's own rows alone. Any context-less throttle row makes the whole
/// reading provider-wide instead: a throttle no row attributes to an account
/// cannot be subtracted from one, so the honest count is the provider's
/// total, shared by every entitlement of that provider. Zero rows are a
/// provider-wide zero for the same reason — "none observed" is an
/// observation about the provider's rows, not about one account's.
///
/// The same informative-row rule every throttle-scope classifier applies:
/// rows with no recorded outcome and the correlation reader's own
/// [`CORRELATION_PURPOSE`] rows are not evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredentialThrottles {
    /// Informative throttles counted — the account's own when
    /// `account_narrowed`, the provider's total otherwise.
    pub throttled: usize,
    /// Whether `throttled` is the named credential's own count rather than
    /// the provider-wide total.
    pub account_narrowed: bool,
}

/// See [`CredentialThrottles`]. `credential_label` is the
/// [`crate::routing::CredentialId::label`] shape the gateway stamps into
/// [`RoutingObservation::quota_context`]; `None` — an entitlement with no
/// credential of its own — always yields the provider-wide count.
pub fn recent_credential_throttles(
    observations: &[RoutingObservation],
    provider: &str,
    credential_label: Option<&str>,
) -> CredentialThrottles {
    let throttles: Vec<&RoutingObservation> = observations
        .iter()
        .filter(|row| row.provider == provider)
        .filter(|row| row.failure_class == Some(FailureClass::Throttle))
        .filter(|row| row.outcome.is_some() && row.purpose.as_deref() != Some(CORRELATION_PURPOSE))
        .collect();
    let every_row_names_its_account =
        !throttles.is_empty() && throttles.iter().all(|row| row.quota_context.is_some());
    match credential_label {
        Some(label) if every_row_names_its_account => CredentialThrottles {
            throttled: throttles
                .iter()
                .filter(|row| row.quota_context.as_deref() == Some(label))
                .count(),
            account_narrowed: true,
        },
        _ => CredentialThrottles {
            throttled: throttles.len(),
            account_narrowed: false,
        },
    }
}

/// Token spend recorded against one account inside a queried window — map
/// line 1971's *"spend ceilings"* half, read from the rows this ledger
/// actually holds.
///
/// `routing_observations.cost_micro_usd` has one producer (map line 1307),
/// and it writes only on an entitlement-fallback event, at
/// [`CostConfidence::Estimated`] — so a reader that answered in money would
/// answer `None` for nearly every window, and a ceiling almost never
/// reached is a rule almost never enforced. Map line 1465's reader settled
/// the same question the same way, in [`RoutingOverhead`]'s own words:
/// *"'Spend' is tokens... because that is the only currency this ledger
/// holds."* This reader is that sentence applied per account. Cached input
/// tokens are excluded for the same reason: providers disagree on whether
/// they are already inside `input_tokens`.
// History: design-decisions.md, "Trims: routing module docs", routing/evidence/signals.rs `struct CredentialSpend` doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredentialSpend {
    /// Input plus output tokens summed over the rows that carried a count —
    /// the account's own when `account_narrowed`, the provider's total
    /// otherwise. `None` when **no** row carried a count at all, which is
    /// *unknown* and is not `Some(0)`: the columns are nullable so those two
    /// facts stay apart, and a spend ceiling may only be judged reached
    /// against a reading that exists.
    pub tokens: Option<u64>,
    /// Whether `tokens` is the named credential's own sum rather than the
    /// provider-wide total.
    pub account_narrowed: bool,
    /// How many rows contributed a count to `tokens`. `0` exactly when
    /// `tokens` is `None`.
    pub sample_count: usize,
}

/// See [`CredentialSpend`]. `credential_label` is the
/// [`crate::routing::CredentialId::label`] shape the gateway stamps into
/// [`RoutingObservation::quota_context`]; `None` — an entitlement with no
/// credential of its own — always yields the provider-wide sum.
///
/// The narrowing rule is [`recent_credential_throttles`]'s, deliberately
/// verbatim: the reading is the account's own only when **every** counted
/// row of that provider names an account, because one contextless row means
/// the ledger holds spend nobody can attribute, and a sum that quietly
/// dropped it would under-report the very number a ceiling is checked
/// against. Under-reporting is the direction that lets a ceiling be
/// exceeded, so this reader widens rather than narrows when it is unsure.
///
/// [`CORRELATION_PURPOSE`] rows are excluded for the reason that constant
/// gives — they are this ledger's own bookkeeping and not exchanges — and
/// rows with no outcome are excluded because an exchange that never
/// completed reported no usage to sum.
pub fn recent_credential_spend(
    observations: &[RoutingObservation],
    provider: &str,
    credential_label: Option<&str>,
) -> CredentialSpend {
    let counted: Vec<&RoutingObservation> = observations
        .iter()
        .filter(|row| row.provider == provider)
        .filter(|row| row.outcome.is_some() && row.purpose.as_deref() != Some(CORRELATION_PURPOSE))
        .filter(|row| row.input_tokens.is_some() || row.output_tokens.is_some())
        .collect();
    let every_row_names_its_account =
        !counted.is_empty() && counted.iter().all(|row| row.quota_context.is_some());
    let account_narrowed = match credential_label {
        Some(_) => every_row_names_its_account,
        None => false,
    };
    let rows: Vec<&&RoutingObservation> = match (account_narrowed, credential_label) {
        (true, Some(label)) => counted
            .iter()
            .filter(|row| row.quota_context.as_deref() == Some(label))
            .collect(),
        _ => counted.iter().collect(),
    };
    let sample_count = rows.len();
    let tokens = if sample_count == 0 {
        None
    } else {
        Some(rows.iter().fold(0u64, |sum, row| {
            let input = row.input_tokens.unwrap_or(0).max(0) as u64;
            let output = row.output_tokens.unwrap_or(0).max(0) as u64;
            sum.saturating_add(input).saturating_add(output)
        }))
    };
    CredentialSpend {
        tokens,
        account_narrowed,
        sample_count,
    }
}

/// See [`CredentialCost`]. `credential_label` and the narrowing rule are
/// [`recent_credential_spend`]'s, deliberately verbatim — see that
/// function's own doc for why. `since_unix` bounds the window this reader
/// counts, in addition to whatever window the caller already fetched
/// `observations` over: a caller that fetched a wider window than one
/// budget's own period (two providers with different `BudgetPeriod`s sharing
/// one query, say) still gets this budget's own start honoured here rather
/// than the caller's.
pub fn recent_credential_cost(
    observations: &[RoutingObservation],
    provider: &str,
    credential_label: Option<&str>,
    prices: &PriceTable,
    since_unix: i64,
) -> CredentialCost {
    let counted: Vec<&RoutingObservation> = observations
        .iter()
        .filter(|row| row.provider == provider)
        .filter(|row| row.observed_at_unix >= since_unix)
        .filter(|row| row.outcome.is_some() && row.purpose.as_deref() != Some(CORRELATION_PURPOSE))
        .collect();
    let every_row_names_its_account =
        !counted.is_empty() && counted.iter().all(|row| row.quota_context.is_some());
    let account_narrowed = match credential_label {
        Some(_) => every_row_names_its_account,
        None => false,
    };
    let rows: Vec<&&RoutingObservation> = match (account_narrowed, credential_label) {
        (true, Some(label)) => counted
            .iter()
            .filter(|row| row.quota_context.as_deref() == Some(label))
            .collect(),
        _ => counted.iter().collect(),
    };

    let mut unread_rows = 0usize;
    let mut unpriced_rows = 0usize;
    let mut priced_rows = 0usize;
    let mut micro_usd_sum: u64 = 0;

    for row in &rows {
        if row.input_tokens.is_none() && row.output_tokens.is_none() {
            unread_rows += 1;
            continue;
        }
        let Some(price) = prices.price_for(provider, &row.model) else {
            unpriced_rows += 1;
            continue;
        };
        priced_rows += 1;
        let input = row.input_tokens.unwrap_or(0).max(0) as f64;
        let output = row.output_tokens.unwrap_or(0).max(0) as f64;
        let cost_micro_usd = (input * price.input_per_million_usd
            + output * price.output_per_million_usd)
            .max(0.0)
            .round() as u64;
        micro_usd_sum = micro_usd_sum.saturating_add(cost_micro_usd);
    }

    CredentialCost {
        micro_usd: if priced_rows == 0 {
            None
        } else {
            Some(micro_usd_sum)
        },
        priced_rows,
        unread_rows,
        unpriced_rows,
        account_narrowed,
    }
}
