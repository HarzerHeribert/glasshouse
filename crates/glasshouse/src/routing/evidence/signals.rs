//! Classification over an already-fetched `&[RoutingObservation]` slice —
//! route correlation, throttle-scope classification, and credential
//! throttle/spend/cost readers. None of this touches SQL; it is a sibling of
//! `readers.rs`, split out because that file alone was over the Phase 59
//! 2,500-line ceiling.

use super::*;

use crate::provider::pricing::PriceTable;

/// One route as [`correlate_routes`] tells routes apart: the `provider` and
/// `model` already on every [`RoutingObservation`] — capability map line
/// 1373's "provider metadata", and nothing fetched from anywhere.
///
/// `model` is part of the identity because line 1373 asks for
/// *model-specific* 5xx events: two providers whose `claude-x` both fail at
/// once may share an upstream for that model and nothing else, and a
/// correlation keyed on provider alone would carry that pair's evidence to
/// models it was never observed on. The ledger's `route` column (the wire
/// protocol) is deliberately **not** part of it: the question is whether two
/// front doors lead to one room, and the protocol spoken at the door does
/// not change what is behind it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RouteIdentity {
    pub provider: String,
    pub model: String,
}

impl RouteIdentity {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }
}

impl std::fmt::Display for RouteIdentity {
    /// `provider/model` — what every explanation and report prints.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.provider, self.model)
    }
}

/// What [`RouteCorrelation::verdict`] answers — capability map line 1376.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CorrelationVerdict {
    /// Fewer than [`MIN_CORRELATION_SAMPLE`] informative events — line
    /// 1376's refusal, carrying the count so a reader prints *2 of 5* rather
    /// than *unknown*. **A consumer treats this exactly as no correlation.**
    InsufficientEvidence { sample_size: usize, required: usize },
    /// Enough events to say something, and what they say: the share of them
    /// in which the other route failed the same way at the same moment.
    Measured { confidence: f64, sample_size: usize },
}

/// What this project's ledger has observed about whether two routes fail
/// together — capability map lines 1370, 1373, 1374 and 1376, as one value.
///
/// An **informative failure event** is a correlatable failure
/// ([`FailureClass::is_correlatable`]) on one route during which the other
/// route was *observed at all* within
/// [`CORRELATION_OVERLAP_TOLERANCE_SECONDS`]. A failure while the other
/// route was idle is counted nowhere: line 1370's "measured, never assumed"
/// cuts both ways, and treating an unobserved route as having survived
/// would manufacture independence.
///
/// Of the informative events, `overlaps` are those where the other route
/// failed with the **same class** inside the tolerance, `lone` those where
/// it was observed and did not; each failure event is matched at most once.
///
/// [`Self::confidence`] is `overlaps / (overlaps + lone)`, recomputed from
/// the rows on every read and never persisted, because the rows are the
/// claim and the rows keep arriving.
// History: design-decisions.md, "Trims: routing module docs", routing/evidence/signals.rs `struct RouteCorrelation` doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteCorrelation {
    routes: (RouteIdentity, RouteIdentity),
    overlaps: usize,
    lone: usize,
}

impl RouteCorrelation {
    /// A pair nothing has been observed about — zero events, which
    /// [`Self::verdict`] reports as insufficient with a count of zero.
    pub fn unmeasured(a: RouteIdentity, b: RouteIdentity) -> Self {
        let routes = if a <= b { (a, b) } else { (b, a) };
        Self {
            routes,
            overlaps: 0,
            lone: 0,
        }
    }

    /// The two routes, in a fixed order so `(a, b)` and `(b, a)` are the
    /// same pair.
    pub fn routes(&self) -> (&RouteIdentity, &RouteIdentity) {
        (&self.routes.0, &self.routes.1)
    }

    /// Failure events the other route failed the same way during.
    pub fn overlaps(&self) -> usize {
        self.overlaps
    }

    /// Failure events the other route was observed during and did not
    /// fail the same way.
    pub fn lone(&self) -> usize {
        self.lone
    }

    /// Every informative failure event — the denominator, and the count
    /// line 1376 requires beside any confidence.
    pub fn sample_size(&self) -> usize {
        self.overlaps + self.lone
    }

    /// Line 1376: a confidence only once [`MIN_CORRELATION_SAMPLE`] events
    /// exist, and otherwise the count that fell short.
    pub fn verdict(&self) -> CorrelationVerdict {
        let sample_size = self.sample_size();
        if sample_size < MIN_CORRELATION_SAMPLE {
            return CorrelationVerdict::InsufficientEvidence {
                sample_size,
                required: MIN_CORRELATION_SAMPLE,
            };
        }
        CorrelationVerdict::Measured {
            confidence: self.overlaps as f64 / sample_size as f64,
            sample_size,
        }
    }

    /// [`Self::verdict`]'s confidence, or `None` below the minimum — the
    /// shape a consumer composes with, where absent contributes nothing.
    pub fn confidence(&self) -> Option<f64> {
        match self.verdict() {
            CorrelationVerdict::Measured { confidence, .. } => Some(confidence),
            CorrelationVerdict::InsufficientEvidence { .. } => None,
        }
    }
}

/// Every pair of routes [`correlate_routes`] found anything about, looked
/// up by either ordering of the pair. [`Default`] is the empty set — every
/// pair unmeasured — which is what a caller with no ledger passes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteCorrelations {
    pairs: std::collections::BTreeMap<(RouteIdentity, RouteIdentity), RouteCorrelation>,
}

impl RouteCorrelations {
    /// What is known about `a` and `b` failing together — never `None`: a
    /// pair with no rows is [`RouteCorrelation::unmeasured`], so "nothing
    /// observed" and "too little observed" reach a consumer as the same
    /// verdict rather than as two shapes to handle.
    pub fn between(&self, a: &RouteIdentity, b: &RouteIdentity) -> RouteCorrelation {
        let key = if a <= b {
            (a.clone(), b.clone())
        } else {
            (b.clone(), a.clone())
        };
        self.pairs
            .get(&key)
            .cloned()
            .unwrap_or_else(|| RouteCorrelation::unmeasured(key.0, key.1))
    }

    /// Every pair with at least one informative event, in route order.
    pub fn iter(&self) -> impl Iterator<Item = &RouteCorrelation> {
        self.pairs.values()
    }

    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }
}

/// Whether two windows touch, or come within `tolerance` seconds of it.
fn overlaps_within(a: (i64, i64), b: (i64, i64), tolerance: i64) -> bool {
    a.0 <= b.1.saturating_add(tolerance) && b.0 <= a.1.saturating_add(tolerance)
}

/// Fold every correlatable failure in `failing` into `into`, judged against
/// what `other` was doing at the time — see [`RouteCorrelation`] for the
/// three outcomes an event can have.
fn count_failures_against(
    failing: &[&RoutingObservation],
    other: &[&RoutingObservation],
    into: &mut RouteCorrelation,
) {
    for failure in failing {
        let Some(class) = failure
            .failure_class
            .filter(|class| class.is_correlatable())
        else {
            continue;
        };
        let window = failure.window();
        let mut observed = false;
        let mut matched = false;
        for row in other {
            if !overlaps_within(window, row.window(), CORRELATION_OVERLAP_TOLERANCE_SECONDS) {
                continue;
            }
            observed = true;
            if row.failure_class == Some(class) {
                matched = true;
                break;
            }
        }
        match (observed, matched) {
            (false, _) => {}
            (true, true) => into.overlaps += 1,
            (true, false) => into.lone += 1,
        }
    }
}

/// Capability map lines 1370, 1373, 1374 and 1376 as one pure function over
/// raw rows, so every decision in it — the tolerance, the class match, the
/// route identity, the minimum — is reachable by a test with no database.
/// [`EvidenceLedger::route_correlations`] is the one door that feeds it.
///
/// Rows with no recorded outcome never inform a pair (an exchange nobody
/// judged is not evidence the route was up), and rows written under
/// [`CORRELATION_PURPOSE`] are this function's own consequence and are never
/// read back as its cause.
pub fn correlate_routes(observations: &[RoutingObservation]) -> RouteCorrelations {
    let mut by_route: std::collections::BTreeMap<RouteIdentity, Vec<&RoutingObservation>> =
        Default::default();
    for row in observations {
        if row.outcome.is_none() || row.purpose.as_deref() == Some(CORRELATION_PURPOSE) {
            continue;
        }
        by_route
            .entry(RouteIdentity::new(&row.provider, &row.model))
            .or_default()
            .push(row);
    }
    let routes: Vec<&RouteIdentity> = by_route.keys().collect();
    let mut pairs = std::collections::BTreeMap::new();
    for (index, a) in routes.iter().enumerate() {
        for b in &routes[index + 1..] {
            let mut correlation = RouteCorrelation::unmeasured((*a).clone(), (*b).clone());
            count_failures_against(&by_route[*a], &by_route[*b], &mut correlation);
            count_failures_against(&by_route[*b], &by_route[*a], &mut correlation);
            if correlation.sample_size() > 0 {
                pairs.insert(((*a).clone(), (*b).clone()), correlation);
            }
        }
    }
    RouteCorrelations { pairs }
}

/// Capability map line 1317: whether a throttle on one route reads as this
/// provider's own cadence limiter firing everywhere, or as one model's own
/// limit — computed, never stored, from the same rows and the same overlap
/// [`correlate_routes`] measures, restricted to [`FailureClass::Throttle`]
/// and to one provider's own models rather than every route in the ledger.
///
/// Line 1317 names four scopes: provider-wide, model-specific,
/// account-specific, request-pool-specific. **Account-specific** gained its
/// key with Phase 56A — every gateway exchange row carries the serving
/// credential's label in [`RoutingObservation::quota_context`] — emitted
/// only when the evidence permits: rows without a `quota_context`
/// contribute nothing to it. **Request-pool-specific** still has neither a
/// producer nor a consumer: `routing::free::is_request_pool` has no
/// production caller, and the one production allowance read asks only
/// `is_exhausted`, which a pooled and a token-priced credential both answer
/// the same way (refusal register, row 531).
// History: design-decisions.md, "Trims: routing module docs", routing/evidence/signals.rs `enum ThrottleScope` doc.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ThrottleScope {
    /// A throttle on this route overlapped, within
    /// [`CORRELATION_OVERLAP_TOLERANCE_SECONDS`], a throttle on another model
    /// of the same provider — direct evidence the limiter reached more than
    /// one model, and outweighs any number of windows where it did not.
    ProviderWide,
    /// Every informative throttle on this route overlapped a sibling model
    /// of the same provider recording a **non-throttle** outcome — evidence
    /// the limiter is scoped to this model alone.
    ModelSpecific,
    /// A throttle on this route overlapped sibling-model throttles of the
    /// **same account** while a **different account** of the same provider
    /// (another [`RoutingObservation::quota_context`]) recorded a
    /// non-throttle outcome in the same window — the limiter reached more
    /// than one of this account's models, and another account kept serving
    /// through it, which refutes provider-wide without claiming
    /// model-specific. Never emitted from rows that carry no
    /// `quota_context`: with no account key the sibling-model overlap still
    /// reads [`ThrottleScope::ProviderWide`], exactly as before the key
    /// existed.
    AccountSpecific,
    /// Fewer than [`MIN_CORRELATION_SAMPLE`] informative throttle events for
    /// this route — line 1376's own refusal shape, reused rather than given
    /// a second minimum: this ledger keeps one answer to *how many
    /// observations before a figure is trusted*.
    Unknown { sample_size: usize, required: usize },
}

/// [`classify_throttle_scope`]'s per-event judgement: whether a throttle on
/// `route` was, within [`CORRELATION_OVERLAP_TOLERANCE_SECONDS`], observed
/// against a sibling model of the same provider, and whether that sibling
/// was throttled too — the same three-way outcome
/// [`count_failures_against`] folds into a [`RouteCorrelation`], specialised
/// to one provider's own models and to [`FailureClass::Throttle`] alone.
fn count_throttles_against_siblings(
    failing: &[&RoutingObservation],
    siblings: &[&RoutingObservation],
) -> (usize, usize) {
    let mut overlaps = 0usize;
    let mut lone = 0usize;
    for failure in failing {
        if failure.failure_class != Some(FailureClass::Throttle) {
            continue;
        }
        let window = failure.window();
        let mut observed = false;
        let mut matched = false;
        for row in siblings {
            if !overlaps_within(window, row.window(), CORRELATION_OVERLAP_TOLERANCE_SECONDS) {
                continue;
            }
            observed = true;
            if row.failure_class == Some(FailureClass::Throttle) {
                matched = true;
                break;
            }
        }
        match (observed, matched) {
            (false, _) => {}
            (true, true) => overlaps += 1,
            (true, false) => lone += 1,
        }
    }
    (overlaps, lone)
}

/// The account axis of [`classify_throttle_scope`]: for each informative
/// throttle on the route that carries a [`RoutingObservation::quota_context`],
/// whether a row of a **different** account of the same provider (any model,
/// a different `quota_context`) was observed within
/// [`CORRELATION_OVERLAP_TOLERANCE_SECONDS`] — and whether that other
/// account was throttled too. Rows without a context contribute nothing on
/// either side: an account this column cannot name is not evidence about
/// accounts.
///
/// Returns `(cross_throttle, cross_served)`: throttles during which another
/// account was also throttled, and throttles during which another account
/// recorded a non-throttle outcome.
fn count_throttles_against_other_accounts(
    failing: &[&RoutingObservation],
    provider_rows: &[&RoutingObservation],
) -> (usize, usize) {
    let mut cross_throttle = 0usize;
    let mut cross_served = 0usize;
    for failure in failing {
        if failure.failure_class != Some(FailureClass::Throttle) {
            continue;
        }
        let Some(account) = failure.quota_context.as_deref() else {
            continue;
        };
        let window = failure.window();
        let mut served = false;
        let mut throttled = false;
        for row in provider_rows {
            let Some(other) = row.quota_context.as_deref() else {
                continue;
            };
            if other == account {
                continue;
            }
            if !overlaps_within(window, row.window(), CORRELATION_OVERLAP_TOLERANCE_SECONDS) {
                continue;
            }
            if row.failure_class == Some(FailureClass::Throttle) {
                throttled = true;
                break;
            }
            served = true;
        }
        if throttled {
            cross_throttle += 1;
        } else if served {
            cross_served += 1;
        }
    }
    (cross_throttle, cross_served)
}

/// Line 1317, as a pure function over raw rows — the same shape
/// [`correlate_routes`] takes, restricted to `route`'s own provider's other
/// models rather than every other route in the ledger: line 1317 asks
/// whether a throttle is provider-wide **within one provider**, not whether
/// it correlates with an unrelated one.
///
/// An informative event is a throttle on `route` during which a sibling
/// model of the same provider was observed at all, within
/// [`CORRELATION_OVERLAP_TOLERANCE_SECONDS`] — rows with no recorded outcome
/// and this reader's own [`CORRELATION_PURPOSE`] rows are excluded on both
/// sides, the same rule [`correlate_routes`] applies. Below
/// [`MIN_CORRELATION_SAMPLE`] informative events, [`ThrottleScope::Unknown`]
/// with the count, exactly line 1376's shape. At or above it,
/// [`ThrottleScope::ProviderWide`] if any sibling model was throttled at the
/// same moment, else [`ThrottleScope::ModelSpecific`].
pub fn classify_throttle_scope(
    observations: &[RoutingObservation],
    route: &RouteIdentity,
) -> ThrottleScope {
    let informative = |row: &&RoutingObservation| {
        row.outcome.is_some() && row.purpose.as_deref() != Some(CORRELATION_PURPOSE)
    };
    let failing: Vec<&RoutingObservation> = observations
        .iter()
        .filter(|row| row.provider == route.provider && row.model == route.model)
        .filter(informative)
        .collect();
    let siblings: Vec<&RoutingObservation> = observations
        .iter()
        .filter(|row| row.provider == route.provider && row.model != route.model)
        .filter(informative)
        .collect();
    // The account axis reads every informative row of the provider, the
    // failing route's own model included: another account running the *same*
    // model is still another account.
    let provider_rows: Vec<&RoutingObservation> = observations
        .iter()
        .filter(|row| row.provider == route.provider)
        .filter(informative)
        .collect();

    let (overlaps, lone) = count_throttles_against_siblings(&failing, &siblings);
    let (cross_throttle, cross_served) =
        count_throttles_against_other_accounts(&failing, &provider_rows);
    let sample_size = overlaps + lone;
    if sample_size < MIN_CORRELATION_SAMPLE {
        return ThrottleScope::Unknown {
            sample_size,
            required: MIN_CORRELATION_SAMPLE,
        };
    }
    if cross_throttle > 0 {
        // Two accounts throttled in one window: the limiter provably
        // reached past any single account, whatever the models said.
        ThrottleScope::ProviderWide
    } else if overlaps > 0 {
        if cross_served > 0 {
            // This account's sibling models throttled together while a
            // different account kept serving — see the variant's own doc.
            ThrottleScope::AccountSpecific
        } else {
            ThrottleScope::ProviderWide
        }
    } else {
        ThrottleScope::ModelSpecific
    }
}

/// Every route [`classify_throttle_scope`] has anything to say about — at
/// least one throttle, in the window queried — looked up by route. The same
/// relationship [`RouteCorrelations`] has to a single pair: one query builds
/// every entry at once, and a caller with one route in mind still asks this
/// type rather than the database again.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThrottleScopes {
    routes: std::collections::BTreeMap<RouteIdentity, ThrottleScope>,
}

impl ThrottleScopes {
    /// What this reader knows about `route`'s own throttles — never a bare
    /// absence: a route with no recorded throttle is
    /// [`ThrottleScope::Unknown`] with a count of zero, the same "nothing
    /// observed and too little observed read as one verdict" rule
    /// [`RouteCorrelations::between`] keeps.
    pub fn for_route(&self, route: &RouteIdentity) -> ThrottleScope {
        self.routes
            .get(route)
            .copied()
            .unwrap_or(ThrottleScope::Unknown {
                sample_size: 0,
                required: MIN_CORRELATION_SAMPLE,
            })
    }

    /// Every route with at least one recorded throttle, in route order.
    pub fn iter(&self) -> impl Iterator<Item = (&RouteIdentity, &ThrottleScope)> {
        self.routes.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

/// [`classify_throttle_scope`] for every route that recorded a throttle in
/// `observations`, rather than one asked about by name.
pub fn classify_throttle_scopes(observations: &[RoutingObservation]) -> ThrottleScopes {
    let routes: std::collections::BTreeSet<RouteIdentity> = observations
        .iter()
        .filter(|row| row.failure_class == Some(FailureClass::Throttle))
        .map(|row| RouteIdentity::new(&row.provider, &row.model))
        .collect();
    let routes = routes
        .into_iter()
        .map(|route| {
            let scope = classify_throttle_scope(observations, &route);
            (route, scope)
        })
        .collect();
    ThrottleScopes { routes }
}

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
/// The same informative-row rule as [`classify_throttle_scope`]: rows with
/// no recorded outcome and the correlation reader's own
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

/// Map line 1519's own reader, beside [`recent_credential_spend`]: what a
/// **provider's own money budget** costs, in the currency it is actually
/// stated in, rather than in tokens.
///
/// # Why this reader may answer in money and [`recent_credential_spend`] may
/// not
///
/// [`recent_credential_spend`]'s own doc explains why a *ceiling* is stated
/// in tokens: `routing_observations.cost_micro_usd` has almost no producer,
/// so a reader keyed on that column would answer `None` for nearly every
/// window. This reader does not read that column at all — it multiplies the
/// same token counts by [`PriceTable::price_for`], the user's own
/// `pricing.toml`, exactly as `routing::session::expected_marginal_cost`
/// already does to price one decision. A row this table has no price for is
/// not silently zero; see [`CredentialCost::unpriced_rows`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredentialCost {
    /// The priced rows' cost, summed in micro-USD. `None` exactly when
    /// [`Self::priced_rows`] is `0` — *nothing could be priced*, which is not
    /// the same claim as *nothing was spent*, and a caller may judge a
    /// budget exhausted only against `Some`.
    pub micro_usd: Option<u64>,
    /// How many rows contributed to `micro_usd` — carried a token count
    /// **and** matched a `pricing.toml` entry.
    pub priced_rows: usize,
    /// How many rows carried no token count at all — a relayed exchange, or
    /// one written before token counts existed. Not priced, and not the same
    /// gap as [`Self::unpriced_rows`].
    pub unread_rows: usize,
    /// How many rows carried a token count with no matching `pricing.toml`
    /// entry — `PriceTable::price_for` answered `None`. Not priced, and not
    /// the same gap as [`Self::unread_rows`].
    pub unpriced_rows: usize,
    /// Whether the rows behind `micro_usd` are the named credential's own
    /// spend rather than the provider-wide total — [`recent_credential_spend`]'s
    /// own narrowing rule, applied verbatim.
    pub account_narrowed: bool,
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

/// Map line 1158's producer, beside [`recent_credential_spend`]: a session's
/// estimated context size, read off the **latest** row this session's own
/// gateway exchanges wrote, never guessed. See `design-decisions.md`,
/// *"Context size is read off the gateway's own exchange, never guessed"*,
/// for why this is a reading and not a model, and for the wire rule below:
/// Anthropic Messages bills `input_tokens` excluding what the cache served,
/// so the prompt size is their sum; every other known wire's own figure
/// already includes the cached subset, so it stands alone, and an unknown
/// wire takes the same conservative floor. `None` — never `Some(0)` — when
/// this session wrote no row with a known input-token count.
pub fn estimated_context_tokens(
    observations: &[RoutingObservation],
    session_id: &str,
) -> Option<i64> {
    let latest = observations
        .iter()
        .filter(|row| row.session_id.as_deref() == Some(session_id))
        .filter(|row| row.input_tokens.is_some())
        .max_by_key(|row| (row.observed_at_unix, row.seq))?;
    let input = latest.input_tokens?;
    Some(match latest.route.as_deref() {
        Some("anthropic-messages") => input + latest.cached_input_tokens.unwrap_or(0),
        _ => input,
    })
}
