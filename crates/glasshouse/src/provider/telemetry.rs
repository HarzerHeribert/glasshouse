//! Phase 32B: the readers that turn something a provider or a harness
//! actually said into a [`Reading`] on a [`CapacityState`].
//!
//! [`mod@crate::provider::quota`] built the model and reads nothing; this
//! module is the half that reads, and it is deliberately the only place in
//! the crate that turns an outside string into a capacity number.
//!
//! # Two seams, kept apart on purpose — capability map line 1232
//!
//! [`RateLimitHeaders`] reads what an **API provider** sends back, and
//! [`HarnessTelemetry`] reads what a **harness** says about its own
//! first-party subscription. Line 1232 asks that harness adapters be able to
//! expose subscription-usage telemetry *independently from* API-provider
//! telemetry, and independence here is structural rather than promised:
//! neither type can write into the other's fields, each carries its own
//! [`ReadingSource`] variant, and [`apply_provider_headers`] and
//! [`apply_harness_report`] are separate functions that a caller may run in
//! either order, both, or neither. A harness that reports nothing cannot
//! blank a provider's headers, and a provider that answers no headers cannot
//! blank a harness's report — proven by
//! `tests::the_two_telemetry_seams_do_not_overwrite_each_other`.
//!
//! # Nothing here can fail a session — capability map line 1238
//!
//! **No function in this module returns a `Result`.** A header that is
//! missing, malformed, negative, or in a unit nobody recognises produces
//! [`Capacity::Unmeasured`] — the state that means "the provider publishes
//! this and nothing has read it", which is exactly true after a failed read.
//! A caller therefore cannot write an error path that stops a coding session
//! because a rate-limit header was a word instead of a number, because there
//! is no error to propagate. Falling back from authoritative telemetry to a
//! weaker source is [`Capacity::prefer`], which is likewise total.
//!
//! # What may become a source description, and what may never
//!
//! `design-decisions.md` records, measured against real hosts, that a
//! provider's error body may quote an **account identifier** (NVIDIA) or a
//! **masked tail of the submitted credential** (two others), and that such a
//! body "must be treated as sensitive by default: classified against, and
//! never copied whole into a log, a diagnostic, a session record, or anything
//! a user might share."
//!
//! A [`ReadingSource`] description is precisely such a diagnostic — it is
//! printed by `glasshouse resources`. So the rule is enforced here, at the
//! boundary, and it is narrower than "do not copy the body":
//!
//! - a header **name** may be recorded, because Glasshouse chose it from
//!   [`RATE_LIMIT_HEADERS`] and a name that is not on that list is never seen
//!   again;
//! - a header **value** may be *parsed into an integer* and never stored as
//!   text;
//! - a response **body** may not be recorded at all, in any form.
//!
//! `tests::a_source_description_is_built_only_from_names_glasshouse_chose`
//! is the standing guard: it feeds header values that are shaped like
//! credentials and account identifiers through the whole reader and asserts
//! none of them reaches any rendered string.
//!
//! # What was measured, and what was not
//!
//! **AnyRouter, 2026-08-27, unauthenticated `GET
//! https://anyrouter.dev/api/v1/models`** — the exact endpoint
//! [`crate::provider::discovery::model_catalogue`] already requests for that
//! template — answered `200` with:
//!
//! ```text
//! ratelimit-limit: 300
//! ratelimit-policy: 300;w=60
//! x-ratelimit-limit: 300
//! x-ratelimit-tier: ip
//! x-ratelimit-window: 60
//! access-control-expose-headers: …,X-RateLimit-Limit,X-RateLimit-Remaining,
//!   X-RateLimit-Reset,X-RateLimit-Tier,X-RateLimit-Window,RateLimit-Limit,
//!   RateLimit-Policy,RateLimit-Remaining,RateLimit-Reset,Retry-After
//! ```
//!
//! Two things follow and both are in [`RATE_LIMIT_HEADERS`]. The names this
//! parser knows are the ones **that host itself names** in its CORS
//! declaration plus the IETF `RateLimit-*` field names those follow; they are
//! not a guess at what providers generally send. And the *ceiling* is what
//! arrives here while the *remaining* count does not — asserted on a
//! deliberately cache-busted request as well as a cached one — which is why
//! [`RateLimitHeaders::apply_to`] fills a limit and leaves the matching
//! remaining count [`Capacity::Unmeasured`] rather than deriving one.
//!
//! Seven other hosts Glasshouse ships templates for — OpenRouter, UnoRouter,
//! Kilo, Nous, NVIDIA, opencode-zen and z.ai — sent **no** rate-limit header
//! of any name on the same route on the same day. That is recorded in the
//! evidence ledger as the reason line 1229 closes on one provider rather than
//! on a family of them.

use crate::provider::quota::{
    Capacity, CapacityState, KnownPlan, LongWindowRequests, NativeAmount, Pool, RateCeilings,
    Reading, ReadingSource,
};

/// Every response-header name this module will read, lowercased.
///
/// **An allowlist, and load-bearing for two separate reasons.**
///
/// The first is the one above: a name on this list was chosen by Glasshouse,
/// so recording it in a diagnostic reveals nothing a provider said. A
/// response header Glasshouse did not ask for never reaches a
/// [`ReadingSource`] — and never reaches memory either, which matters because
/// OpenRouter's `GET /api/v1/models` response carries a `set-cookie` header
/// (`__cf_bm`, measured 2026-08-27). A reader that captured "all the headers"
/// for a diagnostic would put a session cookie into a report a user is
/// invited to share.
///
/// The second is that a rate-limit header is not the only header whose name
/// contains `limit`: matching by substring would collect
/// `access-control-expose-headers`, whose *value* is a list of header names
/// and is exactly the kind of long attacker-influenced string this refuses to
/// hold.
pub const RATE_LIMIT_HEADERS: &[&str] = &[
    // IETF `RateLimit` fields, which AnyRouter sends and names in its own
    // `access-control-expose-headers`.
    "ratelimit-limit",
    "ratelimit-remaining",
    "ratelimit-reset",
    "ratelimit-policy",
    // The de-facto `X-`-prefixed spellings, likewise named by that host.
    "x-ratelimit-limit",
    "x-ratelimit-remaining",
    "x-ratelimit-reset",
    "x-ratelimit-window",
    // How long to wait, sent with a refusal rather than with a success.
    "retry-after",
];

/// Whether `name` is a header this module is willing to read.
///
/// Case-insensitive on the name only. HTTP field names are case-insensitive
/// by definition and `ureq` does not normalise them for us.
pub fn is_rate_limit_header(name: &str) -> bool {
    RATE_LIMIT_HEADERS
        .iter()
        .any(|known| name.eq_ignore_ascii_case(known))
}

/// Keep only the headers [`RATE_LIMIT_HEADERS`] names, with their names
/// lowercased.
///
/// The one funnel every captured header goes through. See
/// [`RATE_LIMIT_HEADERS`] for why this is an allowlist and not a filter for
/// things that look interesting.
pub fn retain_rate_limit_headers<'a>(
    headers: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Vec<(String, String)> {
    headers
        .into_iter()
        .filter(|(name, _)| is_rate_limit_header(name))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.to_owned()))
        .collect()
}

/// What a provider's rate-limit headers said — capability map line 1229.
///
/// Every field is an `Option<i64>` and never a string: a header value is
/// parsed into a number here or discarded here, and there is no field on this
/// type that could carry a provider's text onward. The `window_seconds` field
/// is what keeps a ceiling honest — a limit of `300` means nothing until you
/// know whether it is per minute or per day, and
/// [`RateLimitHeaders::apply_to`] files it into a different field depending
/// on the answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RateLimitHeaders {
    limit: Option<i64>,
    remaining: Option<i64>,
    reset: Option<i64>,
    window_seconds: Option<i64>,
    retry_after_seconds: Option<i64>,
    /// The names, from [`RATE_LIMIT_HEADERS`], that actually supplied the
    /// numbers above — never the values, never anything else.
    read_from: Vec<&'static str>,
}

/// One minute, in seconds — the window a requests-per-minute ceiling means.
const MINUTE_SECONDS: i64 = 60;

impl RateLimitHeaders {
    /// Read whichever of [`RATE_LIMIT_HEADERS`] are present.
    ///
    /// # Precedence, and why the IETF spelling wins
    ///
    /// A host may send both spellings of the same fact — AnyRouter sends
    /// `ratelimit-limit: 300` and `x-ratelimit-limit: 300` together. The
    /// unprefixed IETF field is preferred because it is the one with a
    /// specification behind it; the `x-` spelling fills in only when the
    /// standard one is absent. Where a host sends both and they disagree,
    /// that is a fact about the host and the specified field is the one to
    /// believe.
    ///
    /// # Nothing here can fail
    ///
    /// A value that is not an integer, or is negative, is dropped and the
    /// field stays `None` — capability map line 1238. A negative remaining
    /// count is not a number to record and clamp; it is a header this parser
    /// does not understand.
    pub fn read<'a>(headers: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let captured = retain_rate_limit_headers(headers);
        let mut out = Self::default();

        for known in RATE_LIMIT_HEADERS {
            let Some((_, value)) = captured.iter().find(|(name, _)| name == known) else {
                continue;
            };
            let parsed = match *known {
                "ratelimit-policy" => parse_policy_window(value),
                _ => parse_count(value),
            };
            let Some(parsed) = parsed else { continue };

            let slot = match *known {
                "ratelimit-limit" | "x-ratelimit-limit" => &mut out.limit,
                "ratelimit-remaining" | "x-ratelimit-remaining" => &mut out.remaining,
                "ratelimit-reset" | "x-ratelimit-reset" => &mut out.reset,
                "ratelimit-policy" | "x-ratelimit-window" => &mut out.window_seconds,
                "retry-after" => &mut out.retry_after_seconds,
                // Unreachable: `known` is an element of `RATE_LIMIT_HEADERS`
                // and every one is matched above. A new entry added there
                // without a home here is caught by
                // `every_known_header_has_a_field_to_land_in`.
                _ => continue,
            };
            // First writer wins, and `RATE_LIMIT_HEADERS` lists the IETF
            // spelling before the `x-` one for exactly that reason.
            if slot.is_none() {
                *slot = Some(parsed);
                out.read_from.push(known);
            }
        }
        out
    }

    /// The ceiling the provider stated, if it stated one.
    pub fn limit(&self) -> Option<i64> {
        self.limit
    }

    /// What the provider said is left in the current window, if it said.
    pub fn remaining(&self) -> Option<i64> {
        self.remaining
    }

    /// The provider's own reset field, if it sent one.
    ///
    /// Deliberately **not** interpreted as a unix second here: the IETF field
    /// is a delta in seconds and several hosts send an absolute timestamp
    /// under the `x-` spelling instead, and nothing Glasshouse has measured
    /// distinguishes the two on the wire. See
    /// [`RateLimitHeaders::resets_at_unix`], which requires the caller to say
    /// which it is holding.
    pub fn reset(&self) -> Option<i64> {
        self.reset
    }

    /// How long the ceiling's window is, in seconds, if the provider said.
    pub fn window_seconds(&self) -> Option<i64> {
        self.window_seconds
    }

    /// How long the provider asked the caller to wait, if it refused.
    pub fn retry_after_seconds(&self) -> Option<i64> {
        self.retry_after_seconds
    }

    /// Which of [`RATE_LIMIT_HEADERS`] supplied a number. Names only.
    pub fn read_from(&self) -> &[&'static str] {
        &self.read_from
    }

    /// Whether any header at all was understood.
    pub fn is_empty(&self) -> bool {
        self.read_from.is_empty()
    }

    /// When the window resets, as a unix second, given the time the response
    /// was observed — capability map line 1211.
    ///
    /// `None` unless a reset field was sent. The field is read as a **delta**,
    /// which is what the IETF field specifies, and a value already larger than
    /// `observed_at_unix` is taken as an absolute timestamp instead: a
    /// "seconds from now" of more than the observation's own unix second would
    /// be a window over fifty-five years long, so the two are separable in
    /// fact even though they are not separable by type.
    pub fn resets_at_unix(&self, observed_at_unix: i64) -> Option<i64> {
        self.reset.map(|reset| {
            if reset >= observed_at_unix {
                reset
            } else {
                observed_at_unix.saturating_add(reset)
            }
        })
    }

    /// Fold what these headers said into `state` — capability map line 1229.
    ///
    /// # What each header becomes, and what it deliberately does not
    ///
    /// - a limit whose window is a minute or shorter becomes
    ///   [`RateCeilings::requests_per_minute`];
    /// - a limit over a longer window becomes
    ///   [`RateCeilings::long_window_requests`], which carries its own
    ///   `window_seconds`, so a per-hour or per-day pool needs no new variant
    ///   (capability map line 1216);
    /// - a limit with **no** stated window becomes neither. `300` with no
    ///   period is not a rate and filing it as one would be inventing the
    ///   period;
    /// - a remaining count becomes the request pool's remaining half, and the
    ///   limit becomes its limit half — so that [`Pool::normalized`] can
    ///   produce a percentage only when the provider supplied both, which is
    ///   the case that lets it be [`crate::provider::quota::Percentage::Exact`];
    /// - a reset field becomes the rolling window's reset time.
    ///
    /// Every quantity the headers did not carry is left exactly as it was.
    /// This function never downgrades a pool: a state whose credits were
    /// already measured keeps them, because nothing here writes to credits.
    ///
    /// # It refuses to fill in what the provider does not publish
    ///
    /// If `state`'s request pool is [`Capacity::ProviderOpaque`] — a
    /// first-party subscription — the pool is left alone however many headers
    /// arrived. That is [`Capacity::is_readable`]'s contract, which Phase 32A
    /// called its best property, and this is the first reader with the
    /// opportunity to break it.
    pub fn apply_to(&self, state: CapacityState, observed_at_unix: i64) -> CapacityState {
        if self.is_empty() {
            return state;
        }

        let source = |name: &'static str| ReadingSource::ResponseHeader(name.to_owned());
        let requests_source = source(self.name_for(&["ratelimit-limit", "x-ratelimit-limit"]));

        let mut requests = state.requests().clone();
        if requests.limit().is_readable()
            && let Some(limit) = self.limit
        {
            requests = requests.with_limit(Capacity::Measured(Reading::new(
                NativeAmount::whole(limit, "requests"),
                observed_at_unix,
                requests_source.clone(),
            )));
        }
        if requests.remaining().is_readable()
            && let Some(remaining) = self.remaining
        {
            requests = requests.with_remaining(Capacity::Measured(Reading::new(
                NativeAmount::whole(remaining, "requests"),
                observed_at_unix,
                source(self.name_for(&["ratelimit-remaining", "x-ratelimit-remaining"])),
            )));
        }

        let mut rates = state.rate_ceilings().clone();
        if let (Some(limit), Some(window)) = (self.limit, self.window_seconds)
            && window > 0
        {
            let amount = NativeAmount::whole(limit, "requests");
            let reading_source = source(self.name_for(&["ratelimit-policy", "x-ratelimit-window"]));
            rates = if window <= MINUTE_SECONDS {
                rates.with_requests_per_minute(Capacity::Measured(Reading::new(
                    amount,
                    observed_at_unix,
                    reading_source,
                )))
            } else {
                rates.with_long_window_requests(Capacity::Measured(Reading::new(
                    LongWindowRequests::new(amount, window),
                    observed_at_unix,
                    reading_source,
                )))
            };
        }

        let mut windows = state.windows().clone();
        if windows.rolling().resets_at_unix().is_readable()
            && let Some(resets_at) = self.resets_at_unix(observed_at_unix)
        {
            let rolling =
                windows
                    .rolling()
                    .clone()
                    .with_resets_at(Capacity::Measured(Reading::new(
                        resets_at,
                        observed_at_unix,
                        source(self.name_for(&["ratelimit-reset", "x-ratelimit-reset"])),
                    )));
            windows = windows.with_rolling(rolling);
        }

        state
            .with_requests(requests)
            .with_rate_ceilings(rates)
            .with_windows(windows)
    }

    /// Which of `candidates` actually supplied a number, for naming a
    /// [`ReadingSource`]. Falls back to the first candidate so a source is
    /// never empty; `candidates` is always a non-empty slice of
    /// [`RATE_LIMIT_HEADERS`] entries, so the fallback names a real header
    /// either way.
    fn name_for(&self, candidates: &[&'static str]) -> &'static str {
        candidates
            .iter()
            .find(|name| self.read_from.contains(name))
            .copied()
            .unwrap_or(candidates[0])
    }
}

/// A non-negative integer, or nothing.
///
/// Trims surrounding whitespace, which `Retry-After` in particular arrives
/// with. Refuses a fractional value rather than truncating it: a rate limit
/// stated in fractions is a header this parser does not understand, and
/// guessing at the rounding would be inventing a number.
fn parse_count(value: &str) -> Option<i64> {
    let parsed: i64 = value.trim().parse().ok()?;
    (parsed >= 0).then_some(parsed)
}

/// The window out of an IETF `RateLimit-Policy` value — `"300;w=60"` is a
/// limit of 300 over 60 seconds, and 60 is what this returns.
///
/// Only the `w=` parameter is read. The quota figure at the front is the same
/// number `RateLimit-Limit` carries, and reading it twice from two fields
/// would be two chances to disagree.
fn parse_policy_window(value: &str) -> Option<i64> {
    value
        .split(';')
        .skip(1)
        .filter_map(|part| part.trim().strip_prefix("w="))
        .find_map(parse_count)
}

/// What a harness said about its own first-party subscription — capability
/// map lines 1231 and 1232.
///
/// # Why this is a plan and not a percentage
///
/// The hypothesis this package was given was that a harness exposes
/// machine-readable *usage*. Checked against the binaries installed on this
/// machine on 2026-08-27, that is **false and the weaker statement is true**:
/// `codex doctor --json` emits a `schemaVersion`-stamped report containing no
/// usage, quota, limit, credit, remaining or reset field of any kind, and
/// `claude auth status --json` — whose `--json` is the documented default —
/// emits a small object whose only capacity-adjacent field is the
/// subscription tier. Neither reports how much of a window is left.
///
/// So what a harness can be read for today is line 1231's *status*
/// information, and the honest shape for it is a [`KnownPlan`]: the same fact
/// line 1233 lets a user type, arriving from the account holder instead of
/// from memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessTelemetry {
    plan: Capacity<KnownPlan>,
}

impl HarnessTelemetry {
    /// Nothing was read.
    pub fn nothing() -> Self {
        Self {
            plan: Capacity::Unmeasured,
        }
    }

    /// A plan a harness stated, naming the interface it was read from.
    ///
    /// `interface` is a command line Glasshouse itself constructed — see
    /// [`ReadingSource::HarnessReport`] — never anything the harness printed.
    pub fn plan(name: impl Into<String>, observed_at_unix: i64, interface: &str) -> Self {
        Self {
            plan: Capacity::Measured(Reading::new(
                KnownPlan::new(name),
                observed_at_unix,
                ReadingSource::HarnessReport(interface.to_owned()),
            )),
        }
    }

    pub fn known_plan(&self) -> &Capacity<KnownPlan> {
        &self.plan
    }

    /// Fold this report into `state` — capability map line 1232's *and*
    /// line 1228's halves at once.
    ///
    /// [`Capacity::prefer`] decides: a harness report is authoritative and
    /// beats a plan the user configured, and a state whose plan is already
    /// [`Capacity::Inapplicable`] or [`Capacity::DelegatedUpstream`] is
    /// unaffected, because a local server has no plan and the gateway's plan
    /// is not the gateway's.
    pub fn apply_to(&self, state: CapacityState) -> CapacityState {
        if !state.plan().is_readable() {
            return state;
        }
        let merged = state.plan().clone().prefer(self.plan.clone());
        state.with_plan(merged)
    }
}

/// Read a subscription tier out of the JSON object a harness status command
/// prints — the parser half of [`HarnessTelemetry`].
///
/// # It reads exactly one field, and that is a security property
///
/// `claude auth status --json` was measured on 2026-08-27 emitting eight
/// keys, of which **three identify the account holder** — an email address,
/// an organisation id and an organisation name. `design-decisions.md`'s rule
/// that a provider's response body may name the account, and must never be
/// copied whole into anything a user might share, applies with more force to
/// a harness's own account than to a provider's error text.
///
/// So this function reads `subscriptionType` and returns a
/// [`HarnessTelemetry`] carrying nothing else. Not a filtered map, not a
/// struct with the other fields left unread — one string. There is no
/// representation of this body inside Glasshouse for a later change to start
/// printing, which is the difference between a rule and a shape.
/// `tests::a_harness_report_carries_nothing_but_the_plan` is the guard.
///
/// Returns [`HarnessTelemetry::nothing`] for any body that is not an object,
/// has no `subscriptionType`, or whose `subscriptionType` is not a
/// non-empty string — capability map line 1238 again: an unreadable status
/// report leaves the plan unmeasured and stops nothing.
pub fn read_harness_plan(body: &str, observed_at_unix: i64, interface: &str) -> HarnessTelemetry {
    let Ok(serde_json::Value::Object(object)) = serde_json::from_str::<serde_json::Value>(body)
    else {
        return HarnessTelemetry::nothing();
    };
    let Some(plan) = object
        .get("subscriptionType")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|plan| !plan.is_empty())
    else {
        return HarnessTelemetry::nothing();
    };
    HarnessTelemetry::plan(plan, observed_at_unix, interface)
}

/// A plan or budget the user entered — capability map line 1233.
///
/// The manual half of the same seam the two readers above cover, and the only
/// [`ReadingSource::UserConfiguration`] producer in the crate. It is
/// [`crate::config::QuotaOverride`] that holds what the user wrote; this turns
/// it into readings so that a configured value and a measured one are the same
/// kind of thing and [`Capacity::prefer`] can rank them.
///
/// `observed_at_unix` is when Glasshouse *read the configuration*, not when
/// the user wrote it. That is the honest stamp: a monetary ceiling in a file
/// is current as of the moment it was loaded, and dating it to the file's
/// mtime would make an unchanged budget look stale (capability map line 1237)
/// for no reason.
pub fn apply_user_configuration(
    state: CapacityState,
    plan: Option<&str>,
    monthly_budget_micro_usd: Option<u64>,
    observed_at_unix: i64,
) -> CapacityState {
    let mut state = state;

    if let Some(plan) = plan.map(str::trim).filter(|plan| !plan.is_empty())
        && state.plan().is_readable()
    {
        let configured = Capacity::Measured(Reading::new(
            KnownPlan::new(plan),
            observed_at_unix,
            ReadingSource::UserConfiguration,
        ));
        let merged = state.plan().clone().prefer(configured);
        state = state.with_plan(merged);
    }

    if let Some(budget) = monthly_budget_micro_usd
        && state.user_budget().limit().is_readable()
    {
        // The ceiling is known and the spend against it is not: nothing in
        // Glasshouse counts money spent, so the remaining half stays
        // whatever it was. Capability map line 1209 needs both, and this is
        // the half that exists.
        let ceiling = Capacity::Measured(Reading::new(
            NativeAmount::millionths(budget as i64, "USD"),
            observed_at_unix,
            ReadingSource::UserConfiguration,
        ));
        let merged = state.user_budget().limit().clone().prefer(ceiling);
        let pool = state.user_budget().clone().with_limit(merged);
        state = state.with_user_budget(pool);
    }

    state
}

/// Fold a provider's response headers into a resource's capacity — the
/// public name of line 1229's seam.
///
/// A free function beside [`apply_harness_report`] rather than a method,
/// because line 1232's independence is easier to read when the two entry
/// points sit next to each other with the same shape and no shared state.
pub fn apply_provider_headers(
    state: CapacityState,
    headers: &RateLimitHeaders,
    observed_at_unix: i64,
) -> CapacityState {
    headers.apply_to(state, observed_at_unix)
}

/// Fold a harness's own report into a resource's capacity — line 1232's
/// seam, independent of [`apply_provider_headers`].
pub fn apply_harness_report(state: CapacityState, report: &HarnessTelemetry) -> CapacityState {
    report.apply_to(state)
}

/// A `Pool` builder used by the tests below and by
/// [`crate::provider::resources`] to state what a reader would have produced.
///
/// Public because a caller outside this module needs to construct the same
/// shape to compare against, and re-deriving it there would be a second
/// definition of "a pool with a measured limit".
pub fn pool_with_measured_limit(
    pool: Pool,
    amount: NativeAmount,
    observed_at_unix: i64,
    source: ReadingSource,
) -> Pool {
    pool.with_limit(Capacity::Measured(Reading::new(
        amount,
        observed_at_unix,
        source,
    )))
}

/// Every rate ceiling in the same unknown state — re-exported shape so a
/// caller can rebuild one without importing four types.
pub fn uniform_rate_ceilings(unknown: Capacity<NativeAmount>) -> RateCeilings {
    RateCeilings::uniform(unknown, Capacity::Unmeasured)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::quota::{Percentage, TelemetryClass, UnitScale};
    use crate::provider::registry::ResourceKind;

    const OBSERVED: i64 = 1_787_800_000;

    /// The exact header set `https://anyrouter.dev/api/v1/models` answered
    /// with, unauthenticated, on 2026-08-27 — copied field for field from the
    /// response, not composed.
    ///
    /// It is a fixture of a **measurement**, which is a different thing from
    /// an invented fixture: every name and every value here was observed, and
    /// the two facts this package leans on hardest are visible in it — the
    /// ceiling arrived and the remaining count did not.
    fn anyrouter_models_headers() -> Vec<(&'static str, &'static str)> {
        vec![
            ("date", "Thu, 27 Aug 2026 14:22:24 GMT"),
            ("content-type", "application/json"),
            ("content-length", "375402"),
            ("cf-ray", "a31bba6cb87a290f-DUS"),
            ("cf-cache-status", "HIT"),
            ("access-control-allow-origin", "*"),
            (
                "access-control-expose-headers",
                "X-Request-Id,X-AnyRouter-Trace-Id,X-AnyRouter-Handler,X-RateLimit-Limit,\
                 X-RateLimit-Remaining,X-RateLimit-Reset,X-RateLimit-Tier,X-RateLimit-Window,\
                 RateLimit-Limit,RateLimit-Policy,RateLimit-Remaining,RateLimit-Reset,Retry-After",
            ),
            ("ratelimit-limit", "300"),
            ("ratelimit-policy", "300;w=60"),
            ("x-ratelimit-limit", "300"),
            ("x-ratelimit-tier", "ip"),
            ("x-ratelimit-window", "60"),
            ("x-anyrouter-handler", "api"),
            ("server", "cloudflare"),
        ]
    }

    // --- line 1229: read rate-limit headers ------------------------------

    #[test]
    fn the_headers_a_real_provider_sent_are_read_into_a_ceiling_and_a_window() {
        let read = RateLimitHeaders::read(anyrouter_models_headers());
        assert_eq!(read.limit(), Some(300));
        assert_eq!(read.window_seconds(), Some(60));
        // The host advertises `RateLimit-Remaining` in its CORS list and did
        // not send it. Glasshouse does not fill one in.
        assert_eq!(read.remaining(), None);
        assert_eq!(read.reset(), None);
    }

    #[test]
    fn a_response_with_no_rate_limit_header_reads_as_nothing_rather_than_as_zero() {
        // OpenRouter's own `GET /api/v1/models` response, same day: no
        // rate-limit header of any name.
        let read = RateLimitHeaders::read(vec![
            ("date", "Thu, 27 Aug 2026 14:22:24 GMT"),
            ("content-type", "application/json"),
            ("cf-cache-status", "HIT"),
        ]);
        assert!(read.is_empty());
        assert_eq!(read.limit(), None);
        assert_eq!(read.remaining(), None);
    }

    #[test]
    fn a_ceiling_over_a_minute_becomes_a_requests_per_minute_limit() {
        let state = RateLimitHeaders::read(anyrouter_models_headers()).apply_to(
            ResourceKind::from_direct_provider("anyrouter").capacity(),
            OBSERVED,
        );
        let per_minute = state.rate_ceilings().requests_per_minute();
        let amount = per_minute.value().expect("a ceiling was read");
        assert_eq!(amount.value(), 300);
        assert_eq!(amount.unit(), "requests");
        // And it did not land in the long-window pool as well.
        assert!(!state.rate_ceilings().long_window_requests().is_measured());
    }

    /// Capability map line 1216 — "requests-per-day **or equivalent**".
    ///
    /// The same `300` over an hour is not a per-minute ceiling, and a parser
    /// that filed it as one would report a resource as fifty times more
    /// throttled than it is.
    #[test]
    fn a_ceiling_over_a_longer_window_becomes_a_long_window_pool_carrying_its_period() {
        let state = RateLimitHeaders::read(vec![
            ("ratelimit-limit", "300"),
            ("ratelimit-policy", "300;w=3600"),
        ])
        .apply_to(
            ResourceKind::from_direct_provider("anyrouter").capacity(),
            OBSERVED,
        );
        assert!(!state.rate_ceilings().requests_per_minute().is_measured());
        let long = state
            .rate_ceilings()
            .long_window_requests()
            .value()
            .expect("a long-window ceiling was read");
        assert_eq!(long.limit().value(), 300);
        assert_eq!(long.window_seconds(), 3600);
    }

    /// A limit with no stated period is not a rate, and inventing the period
    /// is the one thing this parser must not do.
    #[test]
    fn a_ceiling_with_no_stated_window_becomes_no_rate_at_all() {
        let read = RateLimitHeaders::read(vec![("ratelimit-limit", "300")]);
        assert_eq!(read.limit(), Some(300));
        assert_eq!(read.window_seconds(), None);
        let state = read.apply_to(
            ResourceKind::from_direct_provider("anyrouter").capacity(),
            OBSERVED,
        );
        assert!(!state.rate_ceilings().requests_per_minute().is_measured());
        assert!(!state.rate_ceilings().long_window_requests().is_measured());
        // The pool's ceiling is still recorded — a limit without a period is
        // a real fact about the pool, just not a rate.
        assert!(state.requests().limit().is_measured());
    }

    #[test]
    fn the_ietf_spelling_wins_over_the_x_prefixed_one_when_a_host_sends_both() {
        let read = RateLimitHeaders::read(vec![
            ("ratelimit-limit", "300"),
            ("x-ratelimit-limit", "999"),
        ]);
        assert_eq!(read.limit(), Some(300));
        assert!(read.read_from().contains(&"ratelimit-limit"));
        assert!(!read.read_from().contains(&"x-ratelimit-limit"));
    }

    #[test]
    fn the_x_prefixed_spelling_fills_in_when_the_standard_one_is_absent() {
        let read = RateLimitHeaders::read(vec![
            ("X-RateLimit-Remaining", "17"),
            ("X-RateLimit-Window", "60"),
        ]);
        assert_eq!(read.remaining(), Some(17));
        assert_eq!(read.window_seconds(), Some(60));
    }

    #[test]
    fn a_policy_value_yields_its_window_and_not_its_quota_figure() {
        assert_eq!(parse_policy_window("300;w=60"), Some(60));
        assert_eq!(parse_policy_window("100;w=3600;burst=10"), Some(3600));
        // No `w=` parameter: there is no window here to read.
        assert_eq!(parse_policy_window("300"), None);
    }

    // --- line 1238: nothing here can fail a session ----------------------

    #[test]
    fn an_unparseable_header_value_leaves_the_quantity_unmeasured_rather_than_erroring() {
        for value in ["", "unlimited", "-5", "12.5", "  ", "300; w=60"] {
            let read = RateLimitHeaders::read(vec![("ratelimit-limit", value)]);
            assert_eq!(read.limit(), None, "`{value}` must not parse as a ceiling");
            let state = read.apply_to(
                ResourceKind::from_direct_provider("anyrouter").capacity(),
                OBSERVED,
            );
            // And the state is still complete and printable.
            assert!(!state.requests().limit().is_measured());
        }
    }

    #[test]
    fn a_status_body_that_is_not_what_the_parser_expects_leaves_the_plan_unmeasured() {
        for body in [
            "",
            "not json",
            "[]",
            "{}",
            r#"{"subscriptionType": ""}"#,
            r#"{"subscriptionType": 7}"#,
            r#"{"subscription_type": "max"}"#,
        ] {
            let report = read_harness_plan(body, OBSERVED, "claude auth status --json");
            assert!(
                !report.known_plan().is_measured(),
                "`{body}` must not yield a plan"
            );
        }
    }

    // --- line 1232: the two seams are independent ------------------------

    /// The line's load-bearing word is *independently*. Applied in either
    /// order, in isolation or together, neither reader disturbs the other's
    /// fields.
    #[test]
    fn the_two_telemetry_seams_do_not_overwrite_each_other() {
        let headers = RateLimitHeaders::read(anyrouter_models_headers());
        let harness = HarnessTelemetry::plan("max", OBSERVED, "claude auth status --json");

        let provider_only =
            apply_provider_headers(CapacityState::opaque_subscription(), &headers, OBSERVED);
        let harness_only = apply_harness_report(CapacityState::opaque_subscription(), &harness);
        let both = apply_harness_report(provider_only.clone(), &harness);
        let both_reversed = apply_provider_headers(harness_only.clone(), &headers, OBSERVED);

        // A harness report leaves the rate ceilings exactly as they were...
        assert_eq!(
            harness_only.rate_ceilings(),
            CapacityState::opaque_subscription().rate_ceilings()
        );
        // ...and provider headers leave the plan exactly as it was.
        assert_eq!(
            provider_only.plan(),
            CapacityState::opaque_subscription().plan()
        );
        // Order does not matter, which is what independence means.
        assert_eq!(both, both_reversed);
    }

    /// A first-party subscription's pools are `ProviderOpaque`, and Phase 32A
    /// called `is_readable()` its best property. This is the first reader
    /// with the opportunity to break it.
    #[test]
    fn a_reader_cannot_fill_in_a_pool_the_provider_publishes_nothing_for() {
        let subscription = CapacityState::opaque_subscription();
        assert!(!subscription.requests().limit().is_readable());
        let after = RateLimitHeaders::read(vec![
            ("ratelimit-limit", "300"),
            ("ratelimit-remaining", "42"),
        ])
        .apply_to(subscription, OBSERVED);
        assert!(!after.requests().limit().is_measured());
        assert!(!after.requests().remaining().is_measured());
    }

    #[test]
    fn a_local_server_has_no_plan_for_a_harness_or_a_user_to_set() {
        let local = CapacityState::unmetered_local();
        let after = apply_harness_report(
            local,
            &HarnessTelemetry::plan("max", OBSERVED, "claude auth status --json"),
        );
        assert!(!after.plan().is_measured());
        let configured = apply_user_configuration(
            CapacityState::unmetered_local(),
            Some("pro"),
            None,
            OBSERVED,
        );
        assert!(!configured.plan().is_measured());
    }

    // --- line 1228: authoritative wins -----------------------------------

    #[test]
    fn a_harness_report_outranks_a_plan_the_user_typed_whichever_arrives_first() {
        let configured = apply_user_configuration(
            CapacityState::opaque_subscription(),
            Some("pro"),
            None,
            OBSERVED,
        );
        assert_eq!(
            configured.plan().telemetry_class(),
            Some(TelemetryClass::Manual)
        );

        let reported = HarnessTelemetry::plan("max", OBSERVED, "claude auth status --json");
        let harness_last = apply_harness_report(configured, &reported);
        assert_eq!(harness_last.plan().value().unwrap().name(), "max");
        assert_eq!(
            harness_last.plan().telemetry_class(),
            Some(TelemetryClass::Authoritative)
        );

        // And the other order: a manual entry never displaces the harness.
        let harness_first = apply_harness_report(CapacityState::opaque_subscription(), &reported);
        let then_user = apply_user_configuration(harness_first, Some("pro"), None, OBSERVED);
        assert_eq!(then_user.plan().value().unwrap().name(), "max");
        assert_eq!(
            then_user.plan().telemetry_class(),
            Some(TelemetryClass::Authoritative)
        );
    }

    // --- line 1233: what the user can enter ------------------------------

    #[test]
    fn a_configured_budget_becomes_a_ceiling_with_the_spend_against_it_left_unknown() {
        let state = apply_user_configuration(
            CapacityState::metered_balance(),
            None,
            Some(10_000_000),
            OBSERVED,
        );
        let limit = state.user_budget().limit().value().expect("a ceiling");
        assert_eq!(limit.value(), 10_000_000);
        assert_eq!(limit.unit(), "USD");
        assert_eq!(limit.scale(), UnitScale::Millionths);
        assert_eq!(
            state.user_budget().limit().telemetry_class(),
            Some(TelemetryClass::Manual)
        );
        // Nothing counts spend, so the remaining half stays unknown rather
        // than being set equal to the ceiling.
        assert!(!state.user_budget().remaining().is_measured());
    }

    /// Capability map line 1234, at the seam where it could actually go
    /// wrong: a percentage over a user-configured ceiling is an estimate, and
    /// there is no accessor that yields its digits without saying so.
    #[test]
    fn a_percentage_over_a_manually_configured_ceiling_is_never_exact() {
        let observed_remaining = Capacity::Measured(Reading::new(
            NativeAmount::millionths(2_500_000, "USD"),
            OBSERVED,
            ReadingSource::LocalObservation("this session's own spend".to_owned()),
        ));
        let state = apply_user_configuration(
            CapacityState::metered_balance(),
            None,
            Some(10_000_000),
            OBSERVED,
        );
        let pool = state
            .user_budget()
            .clone()
            .with_remaining(observed_remaining);
        let score = pool.normalized().expect("both halves were read");

        assert_eq!(score.percent().exact(), None);
        let percentage = score.percent();
        let (percent, confidence, source) = percentage.estimated().expect("this is an estimate");
        assert_eq!(percent, 25);
        assert_eq!(confidence, crate::provider::quota::Confidence::Medium);
        assert!(source.contains("configuration"));
        assert!(matches!(score.percent(), Percentage::Estimated { .. }));
        assert!(score.percent().render().starts_with('~'));
        assert!(score.percent().render().contains("estimated"));
    }

    // --- the security boundary -------------------------------------------

    /// `design-decisions.md`: a provider's response may quote an account
    /// identifier or a masked tail of the submitted credential, and must
    /// never be copied whole into anything a user might share.
    ///
    /// The values here are shaped like the real ones that rule was written
    /// from. None of them may survive into any string this module produces.
    #[test]
    fn a_source_description_is_built_only_from_names_glasshouse_chose() {
        const ACCOUNT: &str = "account-8f21c0de-4b77-11ee-be56-0242ac120002";
        const MASKED_KEY: &str = "sk-or-v1-****************************9f3c";
        let headers = vec![
            ("ratelimit-limit", "300"),
            ("ratelimit-policy", "300;w=60"),
            ("x-account-id", ACCOUNT),
            (
                "set-cookie",
                "__cf_bm=oGkHQJmsGX6wCH7Quh5JYzAK6KXu1icwUg5MExQ2LqQ",
            ),
            ("x-key-tail", MASKED_KEY),
            (
                "access-control-expose-headers",
                "X-RateLimit-Limit,RateLimit-Remaining",
            ),
        ];

        // Nothing but the allowlisted names survives the funnel at all.
        let kept = retain_rate_limit_headers(headers.clone());
        let kept_names: Vec<&str> = kept.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(kept_names, vec!["ratelimit-limit", "ratelimit-policy"]);

        let state = RateLimitHeaders::read(headers).apply_to(
            ResourceKind::from_direct_provider("anyrouter").capacity(),
            OBSERVED,
        );

        let mut rendered = String::new();
        for (_, pool) in state.pools() {
            rendered.push_str(&pool.limit().describe_source());
            rendered.push_str(&pool.remaining().describe_source());
        }
        rendered.push_str(
            &state
                .rate_ceilings()
                .requests_per_minute()
                .describe_source(),
        );
        rendered.push_str(
            &state
                .rate_ceilings()
                .long_window_requests()
                .describe_source(),
        );
        rendered.push_str(&format!("{state:?}"));

        for forbidden in [
            ACCOUNT,
            MASKED_KEY,
            "__cf_bm",
            "oGkHQJmsGX",
            "X-RateLimit-Limit,",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "`{forbidden}` reached a rendered surface"
            );
        }
        // And the thing it may say, it does say.
        assert!(rendered.contains("`ratelimit-limit` response header"));
    }

    /// The other half of the same rule, on the harness side: the status body
    /// measured on 2026-08-27 carried three fields identifying the account
    /// holder, and exactly one field about capacity.
    #[test]
    fn a_harness_report_carries_nothing_but_the_plan() {
        let body = r#"{
            "loggedIn": true,
            "authMethod": "claude.ai",
            "apiProvider": "firstParty",
            "analyticsDisabled": false,
            "email": "someone@example.com",
            "orgId": "5916b68d-0000-0000-0000-000000000000",
            "orgName": "someone@example.com's Organization",
            "subscriptionType": "max"
        }"#;
        let report = read_harness_plan(body, OBSERVED, "claude auth status --json");
        let state = apply_harness_report(CapacityState::opaque_subscription(), &report);

        assert_eq!(state.plan().value().unwrap().name(), "max");
        let rendered = format!("{report:?}{state:?}");
        for forbidden in [
            "someone@example.com",
            "5916b68d",
            "Organization",
            "firstParty",
            "loggedIn",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "`{forbidden}` reached a rendered surface"
            );
        }
    }

    // --- the allowlist itself --------------------------------------------

    #[test]
    fn every_known_header_has_a_field_to_land_in() {
        for name in RATE_LIMIT_HEADERS {
            let read = RateLimitHeaders::read(vec![(
                *name,
                if *name == "ratelimit-policy" {
                    "10;w=60"
                } else {
                    "10"
                },
            )]);
            assert!(
                !read.is_empty(),
                "`{name}` is on the allowlist but nothing reads it"
            );
        }
    }

    #[test]
    fn a_header_name_that_merely_contains_limit_is_not_a_rate_limit_header() {
        assert!(!is_rate_limit_header("access-control-expose-headers"));
        assert!(!is_rate_limit_header("x-ratelimit-tier"));
        assert!(!is_rate_limit_header("content-length"));
        assert!(is_rate_limit_header("RateLimit-Limit"));
        assert!(is_rate_limit_header("ratelimit-limit"));
    }

    // --- line 1211: a reset field -----------------------------------------

    #[test]
    fn a_reset_delta_and_a_reset_timestamp_both_become_the_same_unix_second() {
        let delta = RateLimitHeaders::read(vec![("ratelimit-reset", "30")]);
        assert_eq!(delta.resets_at_unix(OBSERVED), Some(OBSERVED + 30));

        let absolute =
            RateLimitHeaders::read(vec![("ratelimit-reset", &(OBSERVED + 30).to_string()[..])]);
        assert_eq!(absolute.resets_at_unix(OBSERVED), Some(OBSERVED + 30));
    }

    #[test]
    fn a_reset_field_reaches_the_rolling_window_and_not_the_calendar_one() {
        let state = RateLimitHeaders::read(vec![("ratelimit-reset", "30")]).apply_to(
            ResourceKind::from_direct_provider("anyrouter").capacity(),
            OBSERVED,
        );
        assert_eq!(
            state.windows().rolling().resets_at_unix().value(),
            Some(&(OBSERVED + 30))
        );
        assert!(!state.windows().calendar().resets_at_unix().is_measured());
    }
}
