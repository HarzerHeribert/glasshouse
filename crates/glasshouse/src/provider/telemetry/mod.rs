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
//!
//! # A second seam, on a different route: the provider named its own units
//!
//! **Groq, `POST /chat/completions`, 2026-08-26** — a real (free-model,
//! one-token) inference response, the only kind of request that carries this
//! seam at all — answered `200` with both halves of *two* pools, not one:
//!
//! ```text
//! x-ratelimit-limit-requests: 7000
//! x-ratelimit-limit-tokens: 6000
//! x-ratelimit-remaining-requests: 6999
//! x-ratelimit-remaining-tokens: 5991
//! x-ratelimit-reset-requests: 12.342s
//! x-ratelimit-reset-tokens: 90ms
//! ```
//!
//! Two things distinguish this from AnyRouter's set. First, the header names
//! themselves say which resource they bound — `-requests` and `-tokens` are
//! separate suffixes rather than one ambiguous `x-ratelimit-limit` — so
//! [`RateLimitHeaders`] reads the `-requests` pair into the same fields
//! AnyRouter's unsuffixed spelling fills, and the `-tokens` pair into fields
//! of their own, landing in [`crate::provider::quota::TokenBudget::combined`]
//! rather than the request [`Pool`]. Second, the reset fields are not bare
//! integers: `12.342s` and `90ms` are a duration with its unit attached, which
//! this module's own duration parser reads apart from the plain-integer-seconds
//! [`RateLimitHeaders::reset`] AnyRouter's field uses.
//!
//! **This route is the gateway's own forwarding path**, and nowhere else:
//! `crate::provider::discovery` makes catalogue and base-URL reads only, on
//! purpose, because Glasshouse must not spend a token to check a quota — see
//! `crate::gateway::ingress`'s own header capture, which reads exactly this
//! allowlist from a response the gateway was already forwarding.
//!
//! # The gateway may read a response header now — this reverses a decision
//!
//! Phase 9I line 528 held that the gateway must not parse anything in a
//! response it exists to pass through, and an earlier packet for this phase
//! read that as forbidding the header block along with the body. **That
//! overreached.** The gateway already parses the status line and header block
//! in order to forward them; the body is what it streams untouched. Reading a
//! header is not reading the payload, so `crate::gateway::ingress` now reads
//! this module's allowlist — headers only, never a byte of the body — from
//! every response it forwards. See that module for where.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::config::BudgetPeriod;
use crate::provider::quota::{
    Capacity, CapacityState, KnownPlan, LimitingUnit, LongWindowRequests, NativeAmount, Pool,
    RateCeilings, Reading, ReadingSource,
};
use crate::routing::evidence::CredentialCost;

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
    // Groq's own spellings, which name the resource in the header itself
    // rather than leaving it to be inferred — see the module documentation's
    // "a second seam" entry. The `-requests` pair lands in the same fields as
    // the unsuffixed spellings above; the `-tokens` pair is the only header
    // seam observed anywhere that fills the token pool.
    "x-ratelimit-limit-requests",
    "x-ratelimit-remaining-requests",
    "x-ratelimit-reset-requests",
    "x-ratelimit-limit-tokens",
    "x-ratelimit-remaining-tokens",
    "x-ratelimit-reset-tokens",
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
///
/// `limit`/`remaining`/`reset`/`window_seconds` are the **request** pool —
/// AnyRouter's unsuffixed spelling and Groq's `-requests` spelling both land
/// here, because both name the same resource. `token_*` is a second, separate
/// pool that only Groq's `-tokens` spelling has ever been observed to fill —
/// see the module documentation's "a second seam" entry. There is
/// deliberately no `token_window_seconds`: no host observed anywhere states a
/// window for its token ceiling, and inventing one would be guessing at a
/// period nobody published.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RateLimitHeaders {
    limit: Option<i64>,
    remaining: Option<i64>,
    reset: Option<i64>,
    window_seconds: Option<i64>,
    retry_after_seconds: Option<i64>,
    token_limit: Option<i64>,
    token_remaining: Option<i64>,
    token_reset: Option<i64>,
    /// The names, from [`RATE_LIMIT_HEADERS`], that actually supplied the
    /// numbers above — never the values, never anything else.
    read_from: Vec<&'static str>,
}

/// One minute, in seconds — the window a requests-per-minute ceiling means.
///
/// `pub(crate)`, not private: `routing::disposable`'s automatic-classification
/// stickiness window ties itself to this same figure rather than inventing a
/// second one — see its own doc comment.
pub(crate) const MINUTE_SECONDS: i64 = 60;

/// `current` if it is no longer readable (a subscription's opaque pool, say)
/// or `value` was not carried by this header set; otherwise `value` wrapped
/// as a fresh [`Capacity::Measured`] reading. The one "did this header
/// actually supply something, and is the pool still willing to accept it"
/// check [`RateLimitHeaders::apply_to`] repeated four times inline.
fn fill_measured(
    current: Capacity<NativeAmount>,
    value: Option<i64>,
    unit: &str,
    observed_at_unix: i64,
    source: ReadingSource,
) -> Capacity<NativeAmount> {
    match (current.is_readable(), value) {
        (true, Some(value)) => Capacity::Measured(Reading::new(
            NativeAmount::whole(value, unit),
            observed_at_unix,
            source,
        )),
        _ => current,
    }
}

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
                // Groq's reset fields carry a unit suffix (`"12.342s"`,
                // `"90ms"`) rather than the bare integer seconds AnyRouter's
                // `ratelimit-reset` uses; `parse_reset_seconds` reads both.
                "ratelimit-reset"
                | "x-ratelimit-reset"
                | "x-ratelimit-reset-requests"
                | "x-ratelimit-reset-tokens" => parse_reset_seconds(value),
                _ => parse_count(value),
            };
            let Some(parsed) = parsed else { continue };

            let slot = match *known {
                "ratelimit-limit" | "x-ratelimit-limit" | "x-ratelimit-limit-requests" => {
                    &mut out.limit
                }
                "ratelimit-remaining"
                | "x-ratelimit-remaining"
                | "x-ratelimit-remaining-requests" => &mut out.remaining,
                "ratelimit-reset" | "x-ratelimit-reset" | "x-ratelimit-reset-requests" => {
                    &mut out.reset
                }
                "ratelimit-policy" | "x-ratelimit-window" => &mut out.window_seconds,
                "retry-after" => &mut out.retry_after_seconds,
                "x-ratelimit-limit-tokens" => &mut out.token_limit,
                "x-ratelimit-remaining-tokens" => &mut out.token_remaining,
                "x-ratelimit-reset-tokens" => &mut out.token_reset,
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

    /// The token pool's ceiling, if the provider stated one — Groq's
    /// `x-ratelimit-limit-tokens`, the only header seam observed anywhere
    /// that names a token limit.
    pub fn token_limit(&self) -> Option<i64> {
        self.token_limit
    }

    /// What the provider said is left of the token pool, if it said.
    pub fn token_remaining(&self) -> Option<i64> {
        self.token_remaining
    }

    /// The token pool's own reset field, in seconds, if the provider sent
    /// one. Read but — see [`RateLimitHeaders::apply_to`] — not folded into
    /// [`CapacityState`], because [`crate::provider::quota::Windows`] holds
    /// one rolling window per *resource*, not one per pool, and Groq's
    /// request and token pools reset on different schedules.
    pub fn token_reset_seconds(&self) -> Option<i64> {
        self.token_reset
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
        let requests_source = source(self.name_for(&[
            "ratelimit-limit",
            "x-ratelimit-limit",
            "x-ratelimit-limit-requests",
        ]));

        let mut requests = state.requests().clone();
        let new_limit = fill_measured(
            requests.limit().clone(),
            self.limit,
            "requests",
            observed_at_unix,
            requests_source.clone(),
        );
        requests = requests.with_limit(new_limit);
        let new_remaining = fill_measured(
            requests.remaining().clone(),
            self.remaining,
            "requests",
            observed_at_unix,
            source(self.name_for(&[
                "ratelimit-remaining",
                "x-ratelimit-remaining",
                "x-ratelimit-remaining-requests",
            ])),
        );
        requests = requests.with_remaining(new_remaining);

        // The token pool — capability map line 1199. Only ever filled by
        // Groq's `-tokens` spelling; every other host measured here sends
        // nothing that names a token ceiling at all, so this is a no-op for
        // them rather than a guess dressed as a reading.
        let mut tokens = state.tokens().clone();
        let mut combined = tokens.combined().clone();
        let new_combined_limit = fill_measured(
            combined.limit().clone(),
            self.token_limit,
            "tokens",
            observed_at_unix,
            source(self.name_for(&["x-ratelimit-limit-tokens"])),
        );
        combined = combined.with_limit(new_combined_limit);
        let new_combined_remaining = fill_measured(
            combined.remaining().clone(),
            self.token_remaining,
            "tokens",
            observed_at_unix,
            source(self.name_for(&["x-ratelimit-remaining-tokens"])),
        );
        combined = combined.with_remaining(new_combined_remaining);
        tokens = tokens.with_combined(combined);

        // Capability map lines 1199 and 1200: a resource that has just been
        // seen to publish a request or a token ceiling is evidenced to be
        // limited by that unit, whether or not it was already known to be.
        // `with_evidenced` is a no-op for `LimitingUnits::None` and
        // `::Delegated`, so this cannot turn local inference or the gateway
        // into something they are not.
        let mut limits = state.limiting_units().clone();
        if requests.limit().is_measured() || requests.remaining().is_measured() {
            limits = limits.with_evidenced(LimitingUnit::Requests);
        }
        if tokens.combined().limit().is_measured() || tokens.combined().remaining().is_measured() {
            limits = limits.with_evidenced(LimitingUnit::Tokens);
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
                        source(self.name_for(&[
                            "ratelimit-reset",
                            "x-ratelimit-reset",
                            "x-ratelimit-reset-requests",
                        ])),
                    )));
            windows = windows.with_rolling(rolling);
        }

        state
            .with_requests(requests)
            .with_tokens(tokens)
            .with_rate_ceilings(rates)
            .with_windows(windows)
            .limited_by(limits)
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

    /// The primitive fields underneath this value, in the shape
    /// [`PersistedGatewayReading`] stores them.
    ///
    /// `read_from` is names only, exactly as [`RateLimitHeaders::read_from`]
    /// already guarantees — nothing new crosses the boundary here.
    ///
    /// `regime_changed_at_unix` is left `None` here: this method only knows
    /// the headers it was called on, never the reading they are replacing,
    /// so [`GatewayQuotaCache::try_store`] is the one place that fills it in
    /// after comparing against what was on disk.
    fn to_persisted(&self) -> PersistedGatewayReadingFields {
        PersistedGatewayReadingFields {
            limit: self.limit,
            remaining: self.remaining,
            reset: self.reset,
            window_seconds: self.window_seconds,
            retry_after_seconds: self.retry_after_seconds,
            token_limit: self.token_limit,
            token_remaining: self.token_remaining,
            token_reset: self.token_reset,
            read_from: self
                .read_from
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
            regime_changed_at_unix: None,
        }
    }

    /// The inverse of [`Self::to_persisted`].
    ///
    /// `read_from` is matched back against [`RATE_LIMIT_HEADERS`] rather than
    /// trusted as written: a name a hand-edited or corrupted cache file
    /// carries that is not on the allowlist is dropped rather than believed,
    /// the same refusal [`RateLimitHeaders::read`] already applies to a wire
    /// header this parser does not recognise.
    fn from_persisted(fields: &PersistedGatewayReadingFields) -> Self {
        Self {
            limit: fields.limit,
            remaining: fields.remaining,
            reset: fields.reset,
            window_seconds: fields.window_seconds,
            retry_after_seconds: fields.retry_after_seconds,
            token_limit: fields.token_limit,
            token_remaining: fields.token_remaining,
            token_reset: fields.token_reset,
            read_from: fields
                .read_from
                .iter()
                .filter_map(|name| {
                    RATE_LIMIT_HEADERS
                        .iter()
                        .find(|known| *known == name)
                        .copied()
                })
                .collect(),
        }
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

/// A reset delta in seconds, in either shape Glasshouse has observed on the
/// wire.
///
/// AnyRouter's `ratelimit-reset` is a bare integer — [`parse_count`] alone
/// would have been enough for it. Groq's `x-ratelimit-reset-requests` and
/// `x-ratelimit-reset-tokens` carry a unit suffix instead — `"12.342s"`,
/// `"90ms"` — so this tries the bare-integer reading first and falls back to
/// a `s`/`ms`-suffixed decimal. A sub-second delta rounds to the nearest
/// whole second rather than truncating to zero: `CapacityState` has nowhere
/// to keep sub-second precision, and "resets in under a second" is a
/// different fact from "has already reset".
fn parse_reset_seconds(value: &str) -> Option<i64> {
    let trimmed = value.trim();
    if let Some(count) = parse_count(trimmed) {
        return Some(count);
    }
    // Checked before `s`, because `"90ms"` also ends in `s`.
    let (number, seconds_per_unit) = if let Some(prefix) = trimmed.strip_suffix("ms") {
        (prefix, 0.001_f64)
    } else {
        let prefix = trimmed.strip_suffix('s')?;
        (prefix, 1.0_f64)
    };
    let magnitude: f64 = number.trim().parse().ok()?;
    if !magnitude.is_finite() || magnitude < 0.0 {
        return None;
    }
    Some((magnitude * seconds_per_unit).round() as i64)
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

/// One of a provider usage endpoint's own nullable fields — design decision
/// D3.
///
/// Three states because the wire distinguishes three facts: the field is
/// **absent** (nobody has read this provider's opinion, or this build of
/// Glasshouse does not know the field), **present and `null`** (the provider
/// has an opinion and it is "no ceiling"), and **present and a number** (a
/// measurement). Collapsing `null` and absent into one `Option` — which is
/// what a derived deserializer would do — would lose exactly the distinction
/// OpenRouter's own account needed: `GET /api/v1/key`'s `data.limit` was
/// read `null`, authenticated, 2026-08-27, and that is a real answer, not a
/// field nobody sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum UsageField {
    #[default]
    Absent,
    Null,
    Number(i64),
}

impl UsageField {
    /// Read `key` out of a JSON object the way every reader in this module
    /// does: a value that is not a non-negative integer is treated the same
    /// as one that was never sent, rather than as an error — capability map
    /// line 1238.
    fn of(object: &serde_json::Map<String, serde_json::Value>, key: &str) -> Self {
        match object.get(key) {
            None => UsageField::Absent,
            Some(serde_json::Value::Null) => UsageField::Null,
            Some(value) => value
                .as_i64()
                .filter(|number| *number >= 0)
                .map_or(UsageField::Absent, UsageField::Number),
        }
    }
}

/// What a provider's own usage endpoint answered — capability map line 1230.
///
/// # Established for exactly one provider, and one route
///
/// `crate::provider::usage_endpoint` names which providers this can even be
/// asked of. Today that is OpenRouter's `GET /api/v1/key` alone — the route
/// [`crate::provider::discovery::read_response_body`] fetches, behind
/// `--probe`, never on a path that runs without one.
///
/// # What this reader folds into [`CapacityState`], and what it deliberately
/// does not
///
/// `limit`, `limit_remaining` and `limit_reset` map onto the **credits**
/// pool's limit, the credits pool's remaining half, and the **calendar**
/// window's reset time respectively — the calendar window, not the rolling
/// one [`RateLimitHeaders`] fills, because an account-level usage ceiling
/// resets on the provider's own billing cycle rather than a short rolling
/// window. A field present and `null` becomes [`Capacity::Inapplicable`] and
/// a field present and a number becomes [`Capacity::Measured`] — D3's own
/// rule in code.
///
/// The response also carries `usage`, `usage_daily`, `usage_weekly`,
/// `usage_monthly` and `rate_limit.{requests,interval}`, none of which this
/// reader applies to [`CapacityState`]. `usage*` is a **cumulative all-time
/// spend counter**, a different quantity from "how much of a ceiling
/// remains" — the only shape [`Pool::remaining`] has — and folding one into
/// the other would assert a relationship the endpoint never stated,
/// especially on an account whose `limit` is `null`. `rate_limit.interval`'s
/// format was recorded only as a type (`str`), never a real value, so
/// parsing it would be guessing at units nobody confirmed. Both are a
/// decision for whoever holds a live account and a real response body to
/// make, not this package's to invent — see the report's `PROBES I NEED RUN`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderUsage {
    limit: UsageField,
    limit_remaining: UsageField,
    limit_reset: UsageField,
}

impl ProviderUsage {
    /// Read `body` the way [`read_harness_plan`] reads a harness status body:
    /// a shape this parser does not recognise produces the same
    /// [`ProviderUsage::default`] an absent field would, never an error.
    ///
    /// Only `data.limit`, `data.limit_remaining` and `data.limit_reset` are
    /// read — see the type documentation for why the rest of the documented
    /// shape is deliberately left unread.
    pub fn read(body: &str) -> Self {
        let Ok(serde_json::Value::Object(root)) = serde_json::from_str::<serde_json::Value>(body)
        else {
            return Self::default();
        };
        let Some(serde_json::Value::Object(data)) = root.get("data") else {
            return Self::default();
        };
        Self {
            limit: UsageField::of(data, "limit"),
            limit_remaining: UsageField::of(data, "limit_remaining"),
            limit_reset: UsageField::of(data, "limit_reset"),
        }
    }

    /// Whether any of the three fields this reader understands was present
    /// at all, `null` or a number.
    pub fn is_empty(&self) -> bool {
        matches!(self.limit, UsageField::Absent)
            && matches!(self.limit_remaining, UsageField::Absent)
            && matches!(self.limit_reset, UsageField::Absent)
    }

    fn reading(
        &self,
        field: UsageField,
        unit: &str,
        observed_at_unix: i64,
    ) -> Option<Capacity<NativeAmount>> {
        match field {
            UsageField::Absent => None,
            UsageField::Null => Some(Capacity::Inapplicable),
            UsageField::Number(amount) => Some(Capacity::Measured(Reading::new(
                NativeAmount::whole(amount, unit),
                observed_at_unix,
                ReadingSource::ProviderEndpoint("GET /key".to_owned()),
            ))),
        }
    }

    /// Fold this reading into `state` — capability map line 1230's seam.
    ///
    /// Guarded by [`Capacity::is_readable`] exactly like
    /// [`RateLimitHeaders::apply_to`]: a subscription's opaque pools and a
    /// gateway's delegated ones are left alone however this endpoint
    /// answered.
    pub fn apply_to(&self, state: CapacityState, observed_at_unix: i64) -> CapacityState {
        if self.is_empty() {
            return state;
        }

        let mut credits = state.credits().clone();
        if credits.limit().is_readable()
            && let Some(reading) = self.reading(self.limit, "USD", observed_at_unix)
        {
            credits = credits.with_limit(reading);
        }
        if credits.remaining().is_readable()
            && let Some(reading) = self.reading(self.limit_remaining, "USD", observed_at_unix)
        {
            credits = credits.with_remaining(reading);
        }

        let mut windows = state.windows().clone();
        if windows.calendar().resets_at_unix().is_readable() {
            let resets_at = match self.limit_reset {
                UsageField::Absent => None,
                UsageField::Null => Some(Capacity::Inapplicable),
                UsageField::Number(value) => {
                    let at = if value >= observed_at_unix {
                        value
                    } else {
                        observed_at_unix.saturating_add(value)
                    };
                    Some(Capacity::Measured(Reading::new(
                        at,
                        observed_at_unix,
                        ReadingSource::ProviderEndpoint("GET /key".to_owned()),
                    )))
                }
            };
            if let Some(resets_at) = resets_at {
                let calendar = windows.calendar().clone().with_resets_at(resets_at);
                windows = windows.with_calendar(calendar);
            }
        }

        state.with_credits(credits).with_windows(windows)
    }
}

/// Fold a provider's usage-endpoint response into a resource's capacity —
/// the public name of line 1230's seam, beside [`apply_provider_headers`]
/// and [`apply_harness_report`] for the same reason those two sit together.
pub fn apply_provider_usage(
    state: CapacityState,
    usage: &ProviderUsage,
    observed_at_unix: i64,
) -> CapacityState {
    usage.apply_to(state, observed_at_unix)
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

/// The start of `period`'s current window, given `now_unix` — a pure
/// function of the clock the caller supplies rather than one that reads the
/// wall clock itself, the same discipline `provider::resources` documents
/// for its own reports (`ReportOptions::now_unix`'s own doc: *"this module
/// has no clock"*).
///
/// [`BudgetPeriod::RollingThirtyDays`] needs no zone at all: it is thirty
/// days of absolute seconds back from `now_unix`.
/// [`BudgetPeriod::CalendarMonth`] is the first instant of the *local*
/// calendar month — what a person means by "this month" — read through the
/// platform's own `localtime_r` (POSIX) / `localtime_s` (the Windows CRT)
/// and re-normalised with `mktime`. That is the OS's own notion of the local
/// zone and its DST rules rather than a hand-rolled one: this crate
/// deliberately carries no date-library dependency for a single conversion
/// (see `shell::view::format_unix_utc`'s own comment on the same refusal for
/// UTC rendering), and the OS is the only source of "local" this binary has.
/// `tm_isdst` is set to `-1` before the `mktime` call so it is re-derived for
/// the *target* date rather than carried over from `now_unix`'s own DST
/// state — the one case that could otherwise put the boundary an hour off,
/// at a DST transition itself. Fails soft to `now_unix` on any libc error,
/// which makes a budget period start no earlier than "right now" rather than
/// panicking a report.
///
/// **Recorded limit:** a test can only assert this function's *invariants*
/// (the result is the first of some month at local midnight, at or before
/// `now_unix`) rather than a fixed absolute timestamp, because the correct
/// answer depends on the machine's own configured zone.
pub fn budget_period_start(period: BudgetPeriod, now_unix: i64) -> i64 {
    match period {
        BudgetPeriod::RollingThirtyDays => now_unix - 30 * 24 * 60 * 60,
        BudgetPeriod::CalendarMonth => calendar_month_start_local(now_unix),
    }
}

#[cfg(unix)]
fn calendar_month_start_local(now_unix: i64) -> i64 {
    // SAFETY: `time` and `broken_down` are local values this function alone
    // writes; `localtime_r` and `mktime` are POSIX functions taking a valid
    // pointer to each, which these are.
    unsafe {
        let time = now_unix as libc::time_t;
        let mut broken_down: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&time, &mut broken_down).is_null() {
            return now_unix;
        }
        broken_down.tm_mday = 1;
        broken_down.tm_hour = 0;
        broken_down.tm_min = 0;
        broken_down.tm_sec = 0;
        broken_down.tm_isdst = -1;
        let start = libc::mktime(&mut broken_down);
        if start == -1 { now_unix } else { start as i64 }
    }
}

#[cfg(windows)]
fn calendar_month_start_local(now_unix: i64) -> i64 {
    // The UCRT exports no plain `mktime`: its header maps `mktime` onto
    // `_mktime64`, and the `libc` crate binds `localtime_s` and `time` for
    // Windows but no `mktime` at all — so the 64-bit symbol is declared here
    // by its real name. Type-checked for `aarch64-pc-windows-msvc` against
    // `libc 0.2.189` at integration; it resolves through the same CRT import
    // library `localtime_s` above does.
    unsafe extern "C" {
        fn _mktime64(broken_down: *mut libc::tm) -> i64;
    }
    // SAFETY: `time` and `broken_down` are local values this function alone
    // writes; `localtime_s` and `_mktime64` are CRT functions taking a valid
    // pointer to each, which these are.
    unsafe {
        let time = now_unix as libc::time_t;
        let mut broken_down: libc::tm = std::mem::zeroed();
        if libc::localtime_s(&mut broken_down, &time) != 0 {
            return now_unix;
        }
        broken_down.tm_mday = 1;
        broken_down.tm_hour = 0;
        broken_down.tm_min = 0;
        broken_down.tm_sec = 0;
        broken_down.tm_isdst = -1;
        let start = _mktime64(&mut broken_down);
        if start == -1 { now_unix } else { start }
    }
}

#[cfg(not(any(unix, windows)))]
fn calendar_month_start_local(now_unix: i64) -> i64 {
    now_unix
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
///
/// `spend` is capability map line 1519's own reading — `Some` only when a
/// caller actually counted spend against the budget's period
/// (`provider::resources::GatheredTelemetry::gather_budget_spend`) and that
/// count priced at least one row. Nothing in this crate counts spend against
/// a budget nobody could count against; see [`CredentialCost::micro_usd`].
pub fn apply_user_configuration(
    state: CapacityState,
    plan: Option<&str>,
    monthly_budget_micro_usd: Option<u64>,
    spend: Option<&CredentialCost>,
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
        // The ceiling is known; whether the remaining half moves depends on
        // whether anything below actually priced spend against it.
        let ceiling = Capacity::Measured(Reading::new(
            NativeAmount::millionths(budget as i64, "USD"),
            observed_at_unix,
            ReadingSource::UserConfiguration,
        ));
        let merged = state.user_budget().limit().clone().prefer(ceiling);
        let pool = state.user_budget().clone().with_limit(merged);
        state = state.with_user_budget(pool);

        // Capability map line 1519's own half: the remaining amount moves
        // only when a caller counted priced spend against this budget's
        // period. A budget with no counted spend — no ledger, no
        // `pricing.toml` entry, every row relayed or unread — leaves the
        // remaining half exactly as it was, unmeasured, so a resource view
        // can still say honestly "you set a ceiling and Glasshouse could not
        // count anything against it" rather than implying a balance.
        if let Some(spent_micro_usd) = spend.and_then(|spend| spend.micro_usd)
            && state.user_budget().remaining().is_readable()
        {
            let remaining_micro_usd = budget.saturating_sub(spent_micro_usd);
            let remaining = Capacity::Measured(Reading::new(
                NativeAmount::millionths(remaining_micro_usd as i64, "USD"),
                observed_at_unix,
                ReadingSource::LocalObservation(
                    "priced spend against the configured budget".to_owned(),
                ),
            ));
            let merged = state.user_budget().remaining().clone().prefer(remaining);
            let pool = state.user_budget().clone().with_remaining(merged);
            state = state.with_user_budget(pool);
        }
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

// --- a gateway-captured reading, surviving its own process -----------------
//
// Capability map line 1229's gateway half has a reader
// (`RateLimitHeaders::apply_to`, above) and a writer
// (`crate::gateway::ingress`'s capture), and both only ever run inside a
// `glasshouse run`/`glasshouse launch` process that is blocked on the
// harness it started. `glasshouse resources` — the one caller that turns a
// reading into a rendered line — is a separate invocation of the binary, and
// nothing in memory connects the two. This is that connection: the gateway
// process writes what it captured, and a later `glasshouse resources`
// process reads it back. See this package's report for why this is a
// durable cache rather than a shared-process design, and for the one line
// each side still needs before a real reading reaches the report.

/// The on-disk format's version — [`crate::provider::cache::ModelCatalogue`]'s
/// own pattern, for the same reason: a shape change should be a cache miss,
/// never a misread.
const GATEWAY_QUOTA_FORMAT_VERSION: u32 = 1;

/// [`RateLimitHeaders`]'s private fields, in the shape that survives a JSON
/// round trip. A separate type rather than `#[derive(Serialize,
/// Deserialize)]` directly on [`RateLimitHeaders`], because `read_from` is
/// `Vec<&'static str>` there — borrowed from [`RATE_LIMIT_HEADERS`] — and a
/// deserializer cannot hand back a `'static` reference from bytes it just
/// read; [`RateLimitHeaders::from_persisted`] is where an owned name is
/// matched back against that list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct PersistedGatewayReadingFields {
    limit: Option<i64>,
    remaining: Option<i64>,
    reset: Option<i64>,
    window_seconds: Option<i64>,
    retry_after_seconds: Option<i64>,
    token_limit: Option<i64>,
    token_remaining: Option<i64>,
    token_reset: Option<i64>,
    read_from: Vec<String>,
    /// Capability map line 1247's reachable half: the instant
    /// [`GatewayQuotaCache::try_store`] last detected a **regime change** —
    /// a difference in a *stated ceiling* (`limit`, `window_seconds` or
    /// `token_limit`) between this reading and the one it replaced. `None`
    /// on a first reading, carried forward unchanged on a reading whose
    /// ceiling did not move, and never cleared once set.
    ///
    /// `#[serde(default)]` rather than a new format version: a file written
    /// before this field existed has no evidence a change was ever detected,
    /// and reading it as `None` — "no change recorded" — is the accurate
    /// answer, not a cache miss.
    #[serde(default)]
    regime_changed_at_unix: Option<i64>,
}

/// One provider's file: the fields above, plus what the file itself needs to
/// say about itself — [`crate::provider::cache::ModelCatalogue`]'s own three
/// reasons, unchanged here: a format version to reject rather than misread,
/// the provider name to catch a file moved or hand-edited into disagreeing
/// with its own name, and the observation time D3 requires so a stale
/// reading can be told from a fresh one after it crosses a process boundary.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct PersistedGatewayReading {
    version: u32,
    provider: String,
    observed_at_unix: i64,
    #[serde(flatten)]
    fields: PersistedGatewayReadingFields,
}

/// Where a gateway-captured rate-limit reading is kept between processes —
/// capability map line 1229's gateway half, bridged to line 1217/1218's
/// caller.
///
/// Exactly [`crate::provider::cache::ModelCache`]'s own shape: [`Self::at`]
/// for tests, [`Self::new`] for production, one JSON file per provider named
/// by `crate::provider::cache::file_stem`, written to a temporary file and
/// renamed into place so a crash mid-write cannot leave [`Self::load`] a
/// half-written file to trip over. User-scoped rather than project-scoped
/// for the reason [`crate::paths::RuntimePaths::provider_cache_dir`]
/// gives for the model catalogue: a provider's rate-limit window belongs to
/// the account a credential names, not to whichever project a gateway
/// happened to be started for.
///
/// # Never resolved automatically — deliberately
///
/// This type never calls [`crate::paths::RuntimePaths::resolve`] itself.
/// `crate::gateway` has never had a project or a data directory in scope —
/// [`crate::gateway::start_if_required`] takes only launch profiles and an
/// upstream closure — and every other cache in this crate
/// ([`crate::provider::cache::ModelCache`] included) is handed an
/// already-resolved [`crate::paths::RuntimePaths`] by whatever constructed
/// [`crate::Runtime`] rather than resolving one of its own. A gateway that
/// resolved its own OS-standard data directory would also fire inside every
/// existing conformance test that runs a real accept loop, writing into
/// whichever machine happens to run `cargo test` — which is exactly why
/// `crate::gateway::Gateway::start` keeps taking no cache at all, and
/// `crate::gateway::Gateway::start_with_quota_cache` takes one only when a
/// caller explicitly supplies it. See this package's report for the caller
/// neither of those is yet: wiring a real [`crate::paths::RuntimePaths`] into
/// [`crate::gateway::start_if_required`]'s two call sites is
/// `crates/glasshouse/src/main.rs`, which this package may not edit.
#[derive(Debug, Clone)]
pub struct GatewayQuotaCache {
    root: PathBuf,
}

impl GatewayQuotaCache {
    /// The cache under this installation's data directory — the production
    /// constructor, for a caller that already resolved
    /// [`crate::paths::RuntimePaths`].
    pub fn new(paths: &crate::paths::RuntimePaths) -> Self {
        Self {
            root: paths.data_dir().join("gateway-quota"),
        }
    }

    /// A cache rooted at an explicit directory. For tests, exactly like
    /// [`crate::provider::cache::ModelCache::at`].
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path_for(&self, provider: &str) -> PathBuf {
        crate::provider::cache::provider_json_path(&self.root, provider)
    }

    /// The provider's persisted reading, whole — absent, unreadable,
    /// truncated, another format version, or a provider name the file
    /// disagrees with are all `None`, [`Self::load`]'s own contract, one
    /// level down. The one place both [`Self::load`] and
    /// [`Self::try_store`]'s regime-change comparison read a file from.
    fn load_raw(&self, provider: &str) -> Option<PersistedGatewayReading> {
        let path = self.path_for(provider);
        let bytes = std::fs::read(&path).ok()?;
        let stored: PersistedGatewayReading = serde_json::from_slice(&bytes).ok()?;
        if stored.version != GATEWAY_QUOTA_FORMAT_VERSION || stored.provider != provider {
            return None;
        }
        Some(stored)
    }

    /// The most recent gateway-captured reading for `provider`, if the
    /// gateway has ever forwarded a response for it that carried one.
    ///
    /// **Returns no error, ever, and reads no network.**
    /// [`crate::provider::cache::ModelCache::load`]'s own contract, for the
    /// same reason: every way this read can fail — absent, unreadable,
    /// truncated, another format version, a provider name the file
    /// disagrees with — means the same thing to a caller, which is "no
    /// reading here", never a reason to fail `glasshouse resources`.
    pub fn load(&self, provider: &str) -> Option<(RateLimitHeaders, i64)> {
        let stored = self.load_raw(provider)?;
        Some((
            RateLimitHeaders::from_persisted(&stored.fields),
            stored.observed_at_unix,
        ))
    }

    /// The instant `try_store` last detected a **regime change** for
    /// `provider` — capability map line 1247's reachable half. `None` when
    /// no change has ever been recorded: no reading at all, a first reading,
    /// a reading whose stated ceiling has never moved, or a file written
    /// before this field existed (`#[serde(default)]` reads that as "no
    /// change recorded", never a cache miss).
    ///
    /// A sibling accessor beside [`Self::load`] and [`Self::load_all`]
    /// rather than a widening of either's return shape: both already have
    /// production callers this package does not own
    /// (`provider::resources::GatheredTelemetry::gather_gateway_quota`,
    /// `shell::mod`'s route-health table), and the one caller this instant
    /// is for (`config::ResolvedEntitlement::populate_provider_facets`)
    /// already has `provider` in hand from the same [`Self::load`] call it
    /// makes today.
    pub fn regime_changed_at(&self, provider: &str) -> Option<i64> {
        self.load_raw(provider)?.fields.regime_changed_at_unix
    }

    /// Every provider this cache currently holds a reading for.
    ///
    /// What [`crate::provider::resources::GatheredTelemetry::gather_gateway_quota`]
    /// folds in without being told which providers to ask about — a gateway
    /// may have forwarded for any of them, and this is the one place that
    /// knows which files exist rather than guessing from the registry.
    pub fn load_all(&self) -> Vec<(String, RateLimitHeaders, i64)> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            // Skip a `<stem>.<pid>-<n>.writing` temporary from a write that
            // crashed before its rename — its extension is never `json`, and
            // a concurrent writer's in-progress content must never surface
            // as a second reading for a provider that already has one.
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            let Ok(stored) = serde_json::from_slice::<PersistedGatewayReading>(&bytes) else {
                continue;
            };
            if stored.version != GATEWAY_QUOTA_FORMAT_VERSION {
                continue;
            }
            out.push((
                stored.provider.clone(),
                RateLimitHeaders::from_persisted(&stored.fields),
                stored.observed_at_unix,
            ));
        }
        out
    }

    /// Persist `headers` for `provider`, replacing whatever it had before —
    /// the gateway's own half of capability map line 1229.
    ///
    /// A no-op when `headers` is empty, mirroring
    /// `crate::gateway::session::SessionRouting::observe_quota_headers`'s
    /// own guard: an ordinary exchange that carried no rate-limit header must
    /// not overwrite a real reading a previous one left on disk, any more
    /// than it may in memory.
    ///
    /// Best-effort on a write failure — logged, not propagated. The accept
    /// loop this is called from cannot fail a real session's exchange over a
    /// full disk or a permissions problem; see
    /// `crate::gateway::Gateway::start_with_quota_cache`'s own doc for the
    /// call site.
    pub fn store(&self, provider: &str, headers: &RateLimitHeaders, observed_at_unix: i64) {
        if headers.is_empty() {
            return;
        }
        if let Err(err) = self.try_store(provider, headers, observed_at_unix) {
            tracing::debug!(
                provider,
                error = %err,
                "could not persist a gateway-captured quota reading"
            );
        }
    }

    fn try_store(
        &self,
        provider: &str,
        headers: &RateLimitHeaders,
        observed_at_unix: i64,
    ) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        // Capability map line 1247's reachable half: the store is the one
        // place the earlier and the later reading meet, so it is where the
        // comparison happens. `load_raw` tolerates absence and a malformed
        // file the same way — both read as "no previous reading", never an
        // error this write must propagate.
        let previous = self.load_raw(provider);
        let regime_changed_at_unix = match &previous {
            Some(previous) if stated_ceiling_changed(&previous.fields, headers) => {
                Some(observed_at_unix)
            }
            Some(previous) => previous.fields.regime_changed_at_unix,
            None => None,
        };
        let stored = PersistedGatewayReading {
            version: GATEWAY_QUOTA_FORMAT_VERSION,
            provider: provider.to_owned(),
            observed_at_unix,
            fields: PersistedGatewayReadingFields {
                regime_changed_at_unix,
                ..headers.to_persisted()
            },
        };
        let encoded = serde_json::to_vec_pretty(&stored)
            .map_err(|err| std::io::Error::other(err.to_string()))?;
        crate::provider::cache::write_json_atomically(&self.path_for(provider), &encoded)
    }
}

/// Whether `headers` states a different ceiling than `previous` did —
/// capability map line 1247's own definition of a **regime change**: a
/// difference in `limit`, `window_seconds` or `token_limit`, each counted
/// only when **both** readings state a value for it. `remaining`, `reset`
/// and `retry_after` never enter this comparison at all — they are the pool
/// being spent, not the ceiling changing — and a field either reading left
/// unstated is not evidence of anything, so it is skipped rather than
/// treated as a mismatch.
fn stated_ceiling_changed(
    previous: &PersistedGatewayReadingFields,
    headers: &RateLimitHeaders,
) -> bool {
    fn differs(previous: Option<i64>, current: Option<i64>) -> bool {
        matches!((previous, current), (Some(previous), Some(current)) if previous != current)
    }

    differs(previous.limit, headers.limit())
        || differs(previous.window_seconds, headers.window_seconds())
        || differs(previous.token_limit, headers.token_limit())
}

// --- automatic classification's retained pick, surviving its own process --
//
// Capability map lines 1441 and 1442. `glasshouse classify` is a fresh
// process every time (`routing::disposable`'s own module doc: the disposable
// policy "re-decides every time"), so keeping a recent pick to avoid
// unnecessary provider churn needs the same kind of cross-process record
// [`GatewayQuotaCache`] already is — same shape, one real difference. See
// [`RoutingStickyCache`]'s own doc for why it is project-scoped and
// [`GatewayQuotaCache`] deliberately is not.

/// The on-disk format's version for a retained automatic-classification
/// pick — [`GATEWAY_QUOTA_FORMAT_VERSION`]'s own pattern: a shape change is a
/// cache miss, never a misread.
const ROUTING_STICKY_FORMAT_VERSION: u32 = 1;

/// One resource `glasshouse classify`'s automatic mode chose, and when —
/// capability map lines 1441 and 1442's own state.
///
/// Exactly three fields, and REQUIRED BEHAVIOR item 4 names them: a provider
/// name, a model name, a time. No credential, key or URL — this record is not
/// a secret, but it is project state, so it holds only what a later process
/// needs to decide whether to reuse the pick.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RetainedPick {
    pub provider: String,
    pub model: String,
    pub chosen_at_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct PersistedRoutingPick {
    version: u32,
    #[serde(flatten)]
    pick: RetainedPick,
}

/// Where automatic classification's most recently chosen resource is kept
/// between `glasshouse classify` processes — capability map lines 1441 and
/// 1442.
///
/// **Project-scoped**, unlike [`GatewayQuotaCache`] a few lines above: a rate
/// limit belongs to the account a credential names, but which resource
/// automatic mode last picked is a property of *this project's* own recent
/// activity — REQUIRED BEHAVIOR item 1 says a pick must never leak between
/// projects. So this is rooted at
/// [`crate::paths::RuntimePaths::project_state_dir`], not
/// [`crate::paths::RuntimePaths::data_dir`], the one difference from
/// [`GatewayQuotaCache`]'s own placement.
///
/// Everything else is [`GatewayQuotaCache`]'s shape, deliberately: a single
/// JSON file, write-to-a-temporary-file-then-rename so a crash mid-write
/// cannot leave [`Self::load`] a half-written file to trip over, and every
/// read failure — absent, unreadable, truncated, wrong version — answers
/// `None` rather than an error, so a caller never fails a classification over
/// a missing or corrupt cache (REQUIRED BEHAVIOR item 2).
#[derive(Debug, Clone)]
pub struct RoutingStickyCache {
    path: PathBuf,
}

impl RoutingStickyCache {
    /// The cache for one project, under this installation's data directory —
    /// the production constructor, for a caller that already resolved
    /// [`crate::paths::RuntimePaths`] and a project identifier.
    pub fn new(paths: &crate::paths::RuntimePaths, project_id: &str) -> Self {
        Self {
            path: paths
                .project_state_dir(project_id)
                .join("routing-sticky.json"),
        }
    }

    /// A cache rooted at an explicit file. For tests, exactly like
    /// [`GatewayQuotaCache::at`].
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The retained pick, if this cache holds one this process can trust the
    /// shape of.
    ///
    /// **Returns no error, ever, and reads no network** — [`GatewayQuotaCache::load`]'s
    /// own contract, for the same reason.
    pub fn load(&self) -> Option<RetainedPick> {
        let bytes = std::fs::read(&self.path).ok()?;
        let stored: PersistedRoutingPick = serde_json::from_slice(&bytes).ok()?;
        if stored.version != ROUTING_STICKY_FORMAT_VERSION {
            return None;
        }
        Some(stored.pick)
    }

    /// Persist `pick`, replacing whatever this cache had before.
    ///
    /// Best-effort on a write failure — logged, not propagated
    /// (REQUIRED BEHAVIOR item 2): the caller this is built for must not fail
    /// a classification over a full disk or a permissions problem, any more
    /// than [`GatewayQuotaCache::store`]'s own caller may.
    pub fn store(&self, pick: &RetainedPick) {
        if let Err(err) = self.try_store(pick) {
            tracing::debug!(
                error = %err,
                "could not persist automatic classification's retained pick"
            );
        }
    }

    fn try_store(&self, pick: &RetainedPick) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let stored = PersistedRoutingPick {
            version: ROUTING_STICKY_FORMAT_VERSION,
            pick: pick.clone(),
        };
        let encoded = serde_json::to_vec_pretty(&stored)
            .map_err(|err| std::io::Error::other(err.to_string()))?;
        crate::provider::cache::write_json_atomically(&self.path, &encoded)
    }
}

// --- a gateway-observed resource's health, surviving its own process ------
//
// Capability map lines 1311, 1321, 1322 and 1324 have a producer
// (`crate::routing::free::ResourceHealth`, real and complete) and a writer
// (`crate::gateway::session::SessionRouting::observe_exchange`, folding in
// every exchange, paid included), and both only ever run inside the
// `glasshouse run`/`glasshouse launch` process that started the gateway.
// `glasshouse resources` is a separate invocation with nothing in memory
// connecting the two — [`GatewayQuotaCache`]'s own gap, one seam over. This
// is that same connection, built the identical way and for the identical
// reason: the gateway process writes what it observed, and a later
// `glasshouse resources` process reads it back.

/// The on-disk format's version — [`GATEWAY_QUOTA_FORMAT_VERSION`]'s own
/// pattern and the identical reason: a shape change is a cache miss, never a
/// misread.
const GATEWAY_HEALTH_FORMAT_VERSION: u32 = 1;

/// One free resource's health, in the shape that crosses the process
/// boundary — the health twin of `PersistedGatewayReadingFields`.
///
/// `cooling_down_until_unix` is a wall-clock deadline, never the
/// process-local [`std::time::Instant`] `routing::free::ResourceHealth` holds
/// in memory: an `Instant` has no fixed epoch and cannot be compared across
/// two processes, so the write side
/// (`crate::gateway::session::SessionRouting::health_readings_for`)
/// converts the in-memory remaining duration into an absolute unix second
/// before this type is ever built, and every reader compares that against
/// its own `now_unix` instead.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GatewayHealthReading {
    /// [`crate::routing::CredentialId::label`] — safe to persist and render
    /// for the reason that method's own doc gives: a provider and a
    /// reference name, never a secret.
    pub credential_label: String,
    pub model: String,
    pub consecutive_failures: u32,
    /// `None` means not cooling down as of `observed_at_unix`. Capability
    /// map line 1324: a resource cooling down is paced, not broken, and this
    /// field is what lets a reader tell the two apart without inventing a
    /// verdict.
    pub cooling_down_until_unix: Option<i64>,
    /// Which kind of cooldown `cooling_down_until_unix` is, or `None` when
    /// there is no cooldown, or the file predates this field — capability
    /// map line 1546's bridge. `#[serde(default)]` so a cache file written
    /// by an older build, which has no such key, still deserializes as
    /// cause-unknown rather than failing.
    #[serde(default)]
    pub cooldown_cause: Option<crate::routing::free::CooldownCause>,
    pub credential_rejected: bool,
}

impl GatewayHealthReading {
    /// Whether this reading says the resource may be scheduled at `now_unix`
    /// — capability map line 1311, from a reading that has already crossed
    /// the process boundary. A cooldown that has already elapsed by
    /// `now_unix` reads as available again without needing a fresh
    /// observation, exactly as [`crate::routing::free::ResourceHealth::is_available`]
    /// treats an elapsed in-memory cooldown.
    pub fn is_available(&self, now_unix: i64) -> bool {
        if self.credential_rejected {
            return false;
        }
        match self.cooling_down_until_unix {
            Some(until) => until <= now_unix,
            None => true,
        }
    }

    /// This reading's cooldown deadline placed on **the reader's own
    /// monotonic clock**, or `None` when there is no cooldown or it has
    /// already elapsed.
    ///
    /// Capability map line 1599's second hazard, answered in one place.
    /// [`crate::routing::free::ResourceHealth::cooling_down_until`] is an
    /// [`Instant`], which has no epoch and cannot be compared across two
    /// processes; this reading carries the absolute unix second the write
    /// side converted it to. Going back requires **both clocks read at the
    /// same moment**, which is why they are two parameters rather than
    /// something read in here: a caller bridging a whole cache must place
    /// every reading against one pair, not against a clock that moved
    /// between them.
    ///
    /// Three cases and no fourth:
    ///
    /// - **no deadline** — `None`, and the resource is not cooling down;
    /// - **already elapsed** (`until <= now_unix`) — also `None`, matching
    ///   [`Self::is_available`]'s own reading of the same field and
    ///   [`crate::routing::free::ResourceHealth::is_available`]'s treatment
    ///   of an elapsed in-memory cooldown. **Never an `Instant` in the
    ///   past**: `Instant` arithmetic backwards from now is not guaranteed
    ///   to be representable, and a deadline that has passed is not a
    ///   cooldown to express at all.
    /// - **still in the future** — `now` plus the remaining seconds.
    ///
    /// A remaining span too large to place on this clock answers `None`
    /// rather than saturating. It cannot arise from
    /// `crate::gateway::session::SessionRouting::health_readings_for`, whose
    /// deadlines are bounded by `routing::free`'s own `MAX_COOLDOWN`, so the
    /// only way to reach it is a file that says something this program never
    /// wrote — and inventing a centuries-long cooldown from one is worse
    /// than reading no cooldown at all.
    pub fn cooling_down_until(&self, now: Instant, now_unix: i64) -> Option<Instant> {
        let remaining = self.cooling_down_until_unix?.checked_sub(now_unix)?;
        let remaining = u64::try_from(remaining).ok().filter(|left| *left > 0)?;
        now.checked_add(Duration::from_secs(remaining))
    }
}

/// One provider's file: every resource's health this gateway has observed for
/// it, plus what the file itself needs to say about itself —
/// [`PersistedGatewayReading`]'s own three reasons, unchanged here.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct PersistedGatewayHealth {
    version: u32,
    provider: String,
    observed_at_unix: i64,
    entries: Vec<GatewayHealthReading>,
}

/// Where a gateway-observed resource's health is kept between processes —
/// [`GatewayQuotaCache`]'s own shape, a second per-provider directory under
/// the same [`crate::paths::RuntimePaths::data_dir`]. `provider/cache.rs`'s
/// own `crate::provider::cache::file_stem` doc already names this
/// convention and expects a third user of it; this is that third user.
///
/// Never resolved automatically, for the identical reason
/// [`GatewayQuotaCache`]'s own doc gives: `crate::gateway` has never had a
/// data directory in scope, and a caller that wants persistence resolves its
/// own [`crate::paths::RuntimePaths`] and hands this a [`Self::new`] built
/// from it.
#[derive(Debug, Clone)]
pub struct GatewayHealthCache {
    root: PathBuf,
}

impl GatewayHealthCache {
    /// The cache under this installation's data directory — the production
    /// constructor, exactly [`GatewayQuotaCache::new`]'s own shape.
    pub fn new(paths: &crate::paths::RuntimePaths) -> Self {
        Self {
            root: paths.data_dir().join("gateway-health"),
        }
    }

    /// A cache rooted at an explicit directory. For tests, exactly like
    /// [`GatewayQuotaCache::at`].
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path_for(&self, provider: &str) -> PathBuf {
        crate::provider::cache::provider_json_path(&self.root, provider)
    }

    /// Every resource's health this cache holds for `provider`, if the
    /// gateway has ever forwarded an exchange bound to an assignment on it.
    ///
    /// **Returns no error, ever, and reads no network.**
    /// [`GatewayQuotaCache::load`]'s own contract, for the same reason: every
    /// way this read can fail — absent, unreadable, truncated, another format
    /// version, a provider name the file disagrees with — means the same
    /// thing to a caller, an empty list, never a reason to fail `glasshouse
    /// resources` and never a reading this cache did not actually observe.
    pub fn load(&self, provider: &str) -> Vec<GatewayHealthReading> {
        let path = self.path_for(provider);
        let Ok(bytes) = std::fs::read(&path) else {
            return Vec::new();
        };
        let Ok(stored) = serde_json::from_slice::<PersistedGatewayHealth>(&bytes) else {
            return Vec::new();
        };
        if stored.version != GATEWAY_HEALTH_FORMAT_VERSION || stored.provider != provider {
            return Vec::new();
        }
        stored.entries
    }

    /// Every provider this cache currently holds health for —
    /// [`GatewayQuotaCache::load_all`]'s own shape, for the identical
    /// consumer: [`crate::provider::resources::GatheredTelemetry::gather_gateway_health`]
    /// folds these in without being told which providers to ask about.
    pub fn load_all(&self) -> Vec<(String, Vec<GatewayHealthReading>)> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            // Skip a `<stem>.<pid>-<n>.writing` temporary from a write that
            // crashed before its rename — its extension is never `json`. The
            // health cache now has two producers in separate processes
            // (the gateway and `main.rs::persist_support_work_health`), so a
            // stale temporary here would otherwise surface as a second,
            // contradictory reading for a provider that already has one.
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            let Ok(stored) = serde_json::from_slice::<PersistedGatewayHealth>(&bytes) else {
                continue;
            };
            if stored.version != GATEWAY_HEALTH_FORMAT_VERSION {
                continue;
            }
            out.push((stored.provider.clone(), stored.entries));
        }
        out
    }

    /// [`Self::load_all`], with **the unix second each provider's file was
    /// written** beside its entries — map line 1854's *stale* half, which
    /// this cache has always held and never handed out.
    ///
    /// # Why the date is per file and not per reading
    ///
    /// [`Self::store`] replaces a provider's whole file in one write, and its
    /// one production caller builds that vector in one pass
    /// (`crate::gateway::session::SessionRouting::health_readings_for` maps
    /// the free pool at a single instant). So every entry in one file was
    /// observed at the file's own `observed_at_unix`, and a per-entry column
    /// would be that number copied N times — a second source of truth for a
    /// fact the file already carries, which is the duplication
    /// `crate::evaluation`'s own module header refuses one seam over.
    ///
    /// # A file that cannot be dated is not returned at all
    ///
    /// `observed_at_unix` is a required field of the stored document, so a
    /// file without one fails to deserialize and is skipped by exactly the
    /// same guard that skips a truncated one — [`Self::load_all`]'s own
    /// fail-soft contract. A caller therefore never sees an undated reading,
    /// and cannot mistake one for a fresh reading.
    ///
    /// [`Self::load_all`] is deliberately left as it is rather than widened:
    /// its two other callers
    /// (`crate::provider::resources::GatheredTelemetry::gather_gateway_health`
    /// and the shell's own reader) render health, not its age.
    pub fn load_all_dated(&self) -> Vec<(String, i64, Vec<GatewayHealthReading>)> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            // Skip a `<stem>.<pid>-<n>.writing` temporary — see
            // `Self::load_all`'s identical guard just above.
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            let Ok(stored) = serde_json::from_slice::<PersistedGatewayHealth>(&bytes) else {
                continue;
            };
            if stored.version != GATEWAY_HEALTH_FORMAT_VERSION {
                continue;
            }
            out.push((
                stored.provider.clone(),
                stored.observed_at_unix,
                stored.entries,
            ));
        }
        out
    }

    /// Persist `entries` for `provider`, replacing whatever it had before —
    /// the gateway's own half of capability map lines 1311/1321/1322/1324.
    ///
    /// A no-op when `entries` is empty, mirroring [`GatewayQuotaCache::store`]'s
    /// own guard: an exchange with nothing bound to an assignment yet (so
    /// `crate::gateway::session::SessionRouting::health_readings_for` has
    /// nothing to report for this provider) must not overwrite a real reading
    /// a previous exchange left on disk.
    ///
    /// Best-effort on a write failure — logged, not propagated, for the
    /// identical reason [`GatewayQuotaCache::store`] gives: the accept loop
    /// cannot fail a real session's exchange over a full disk.
    pub fn store(&self, provider: &str, entries: &[GatewayHealthReading], observed_at_unix: i64) {
        if entries.is_empty() {
            return;
        }
        if let Err(err) = self.try_store(provider, entries, observed_at_unix) {
            tracing::debug!(
                provider,
                error = %err,
                "could not persist a gateway-observed health reading"
            );
        }
    }

    fn try_store(
        &self,
        provider: &str,
        entries: &[GatewayHealthReading],
        observed_at_unix: i64,
    ) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let stored = PersistedGatewayHealth {
            version: GATEWAY_HEALTH_FORMAT_VERSION,
            provider: provider.to_owned(),
            observed_at_unix,
            entries: entries.to_vec(),
        };
        let encoded = serde_json::to_vec_pretty(&stored)
            .map_err(|err| std::io::Error::other(err.to_string()))?;
        crate::provider::cache::write_json_atomically(&self.path_for(provider), &encoded)
    }
}

// --- a dispatch's in-flight claim on a paced pool -------------------------
//
// Capability map line 1367. [`GatewayQuotaCache`] and [`GatewayHealthCache`]
// above carry what a request **already spent** across a process boundary;
// this carries what a request is **about to spend**, which is the fact two
// concurrent short-lived dispatchers need from each other and the one no
// cache in this build held. `glasshouse hook` and `glasshouse memory commit`
// are separate processes that overlap in supported use, and until this
// existed each read the same remaining-request count off disk and each spent
// it.

/// The on-disk format's version — [`GATEWAY_QUOTA_FORMAT_VERSION`]'s own
/// pattern and the identical reason: a shape change is a cache miss, never a
/// misread.
const DISPATCH_RESERVATION_FORMAT_VERSION: u32 = 1;

/// How long a reservation stands before a reader is entitled to ignore it.
///
/// # Why an expiry exists at all
///
/// The reserving process may not be alive to release. A `glasshouse hook`
/// runs inside the user's turn and can be killed with the harness at any
/// moment, and a row with no deadline left behind by one would take a
/// request out of the pool for ever. So the record's authority is bounded by
/// its own field, and a reader compares that field against its own
/// wall-clock second — never against a process id, which recycles and whose
/// liveness has no portable answer.
///
/// # Why ten seconds
///
/// Twice `main.rs`'s `EXTRACTION_BOUND`, which is the bound on the work a
/// reservation covers: the extraction thread is abandoned five seconds after
/// it starts, so no dispatch this record protects can still be spending
/// after that. Doubling it is the margin for the two things either side of
/// the call — resolving the credential before it and writing the health back
/// after — so a live dispatch is never evicted while its request is
/// genuinely in flight, and a killed one frees the slot within seconds
/// rather than within a rate-limit window.
///
/// `main.rs`'s `the_reservation_lease_outlives_the_extraction_it_covers`
/// pins the relationship between the two constants so they cannot drift
/// apart in separate edits.
pub const DISPATCH_RESERVATION_LEASE: Duration = Duration::from_secs(10);

/// How many concurrent reservations one credential's pool is tracked at.
///
/// A dispatcher claims the first free slot below this, so the walk costs one
/// `open` per live reservation and the number only matters when that many
/// support jobs are in flight against one credential at once. Sixty-four is
/// far past anything this build can produce — a hook and a commit is two —
/// and a pool that genuinely has more than sixty-four requests left is not
/// the scarce thing capability map line 1367 is about.
const MAX_TRACKED_RESERVATIONS: u32 = 64;

/// One dispatch's claim on one request of a credential's paced pool.
///
/// # Names, never a value
///
/// `credential_label` is [`crate::routing::CredentialId::label`] — a
/// provider and a variable *name* — for the reason that method's own doc
/// gives, and it is the same field [`GatewayHealthReading`] persists beside
/// it. Nothing here resolves a secret and nothing here has one to write.
///
/// # `process_id` is a diagnostic, not a liveness test
///
/// It says who wrote the row, which is what a person debugging a pool that
/// will not free wants to know. It is deliberately **not** consulted when
/// deciding whether a row still counts: pids recycle, and asking the
/// operating system whether one is alive has no answer that is the same on
/// Unix and on Windows. [`Self::is_live`] reads `expires_at_unix` and
/// nothing else.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DispatchReservation {
    pub credential_label: String,
    pub model: String,
    /// One request. A job needing several would claim several slots; memory
    /// extraction is one `ExtractionModel::complete` per dispatch
    /// (`main.rs::run_extraction` calls the model once, inside one bound),
    /// so today this is always one and the field is what says so.
    pub requests: u32,
    pub process_id: u32,
    pub reserved_at_unix: i64,
    pub expires_at_unix: i64,
}

impl DispatchReservation {
    /// Whether this row still speaks for a request that may yet be spent.
    ///
    /// The whole of the expiry rule, in one place, so the reader and the
    /// slot-takeover path cannot disagree about it.
    pub fn is_live(&self, now_unix: i64) -> bool {
        self.expires_at_unix > now_unix
    }
}

/// The row as it survives a round trip, with the version every other cache
/// in this module carries and for the same reason.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct PersistedDispatchReservation {
    version: u32,
    #[serde(flatten)]
    reservation: DispatchReservation,
}

/// A claim held by this process, released when the work it covers is done.
///
/// Deliberately not `Drop`: the release has to happen at a point the caller
/// chooses — after the exchange, whether it succeeded or failed — and a type
/// that released itself when it went out of scope would release at whichever
/// frame happened to own it last. [`crate::memory::RoutedModel`] is what
/// holds the release for the extraction path, and that type *does* have the
/// drop guard, because there the last frame is exactly the right moment.
#[derive(Debug, Clone)]
pub struct DispatchReservationLease {
    path: PathBuf,
}

impl DispatchReservationLease {
    /// Give the request back to the pool.
    ///
    /// Best-effort and idempotent: a file already gone — released twice, or
    /// taken over by a dispatcher that judged it expired — is the same
    /// outcome as one removed here, and neither is worth a diagnostic on a
    /// path that runs inside somebody's coding session.
    pub fn release(&self) {
        if let Err(err) = std::fs::remove_file(&self.path)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::debug!(
                error = %err,
                "could not release a dispatch reservation; it expires on its own"
            );
        }
    }

    /// Where the row is, for a diagnostic and for this module's own tests.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Which requests of a paced pool are already spoken for by dispatches that
/// have not finished — capability map line 1367's cross-process channel.
///
/// # Why this one is not read-modify-write
///
/// [`GatewayHealthCache`] has one file per provider, and a writer reads it,
/// replaces one entry and writes the whole file back. Two writers racing
/// there lose an entry, which is a recorded limit of that cache and
/// tolerable because the entry is *history*: the next observation restores
/// it.
///
/// A reservation cannot tolerate it. The lost update **is** the double spend
/// the line is about — two dispatchers that each read "nothing reserved" and
/// each write their own row over the other's have exactly reproduced the
/// defect. So a reservation is not an entry in a shared file. It is a file
/// of its own, at a path derived from the credential and a slot number,
/// claimed with `create_new`: one `O_EXCL`/`CREATE_NEW` open, atomic on
/// every platform this ships to, which exactly one of two racing processes
/// can win.
///
/// # The slot's key is the credential, and the row names the model
///
/// [`crate::routing::free::Allowance`] is *"what a provider is limiting, for
/// one credential"* and [`crate::routing::free::FreePool`] holds one
/// allowance per [`crate::routing::CredentialId`], so what two dispatches
/// contend for is a credential's pool, not a model's. Two models behind one
/// key draw down the same requests, and giving each its own slots would let
/// two dispatches spend one remaining request between them — the same defect
/// wearing a different key. The model is therefore a *field* of the row
/// rather than part of its path: it says what the reserved request is for,
/// so a person can see which model is holding a pool open.
#[derive(Debug, Clone)]
pub struct DispatchReservationCache {
    root: PathBuf,
}

impl DispatchReservationCache {
    /// The cache under this installation's data directory — exactly
    /// [`GatewayQuotaCache::new`]'s own shape, and user-scoped for the same
    /// reason: a credential's request pool belongs to the account the
    /// credential names, not to whichever project a dispatch happened to run
    /// in.
    pub fn new(paths: &crate::paths::RuntimePaths) -> Self {
        Self {
            root: paths.data_dir().join("dispatch-reservations"),
        }
    }

    /// A cache rooted at an explicit directory. For tests, exactly like
    /// [`GatewayQuotaCache::at`].
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The directory the rows live in, so a caller can say whether anything
    /// was ever reserved at all.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path_for(&self, credential_label: &str, slot: u32) -> PathBuf {
        self.root.join(format!(
            "{}.slot{slot}.json",
            crate::provider::cache::file_stem(credential_label)
        ))
    }

    /// How many of `credential_label`'s requests are currently spoken for.
    ///
    /// This is the number a dispatcher subtracts from what the quota cache
    /// says is left, before it asks the router to choose.
    ///
    /// **Returns no error, ever, and reads no network** —
    /// [`GatewayQuotaCache::load`]'s own contract. An absent directory, an
    /// unreadable file and another format version all mean "nothing reserved
    /// here", never a reason to fail a dispatch.
    ///
    /// The credential is matched by the row's **path**, not by the label
    /// inside it: a slot file that cannot be read still has to count against
    /// the credential whose slot it occupies, and its name is the only thing
    /// about it that is still legible.
    pub fn reserved(&self, credential_label: &str, now_unix: i64) -> u32 {
        (0..MAX_TRACKED_RESERVATIONS)
            .filter(|slot| self.is_held(&self.path_for(credential_label, *slot), now_unix))
            .count() as u32
    }

    /// Every reservation this cache holds and can read, live at `now_unix` —
    /// for a diagnostic and for the tests that assert what a dispatch wrote.
    pub fn live(&self, now_unix: i64) -> Vec<DispatchReservation> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            // Skip a `<stem>.slot<n>.<pid>-<n>.writing` temporary from a
            // write that crashed before its rename — [`GatewayQuotaCache`]'s
            // own guard, and this cache has as many concurrent writers as
            // there are dispatchers.
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            if let Some(reservation) = Self::read(&path).filter(|row| row.is_live(now_unix)) {
                out.push(reservation);
            }
        }
        out
    }

    /// Claim one of `capacity`'s requests for `credential_label`, or answer
    /// [`None`] because every one of them is already claimed.
    ///
    /// `capacity` is what the pool is known to hold — the remainder a real
    /// response stated, read back through
    /// [`crate::provider::resources::observed_capacity`]. A caller that does
    /// not know it must not call this at all: a claim against a ceiling
    /// nobody measured would refuse a dispatch on an invented number.
    ///
    /// # Why the claim is trustworthy under a race, and the read is not
    ///
    /// [`Self::reserved`] can be read by two processes before either writes,
    /// so netting alone is a check with no lock. This is the lock: the claim
    /// is an exclusive create, so two dispatchers that both believed a
    /// request was free find out here and exactly one of them is right. A
    /// caller whose claim is refused knows the pool is spoken for **now**,
    /// which is the moment that matters, and goes on to the next candidate.
    ///
    /// # An expired slot is taken over, not overwritten
    ///
    /// A row past its deadline is removed and the slot is then claimed the
    /// same exclusive way, so two dispatchers that both notice one dead row
    /// still produce exactly one holder rather than two that each renamed a
    /// file over the other's.
    ///
    /// Best-effort in the same sense every other writer in this module is:
    /// an I/O failure answers [`None`], and the caller's own documentation
    /// says what it does with that — never a failed dispatch over a full
    /// disk.
    pub fn claim(
        &self,
        credential_label: &str,
        model: &str,
        capacity: u32,
        now_unix: i64,
    ) -> Option<DispatchReservationLease> {
        if capacity == 0 || std::fs::create_dir_all(&self.root).is_err() {
            return None;
        }
        let reservation = DispatchReservation {
            credential_label: credential_label.to_owned(),
            model: model.to_owned(),
            requests: 1,
            process_id: std::process::id(),
            reserved_at_unix: now_unix,
            expires_at_unix: now_unix + DISPATCH_RESERVATION_LEASE.as_secs() as i64,
        };
        for slot in 0..capacity.min(MAX_TRACKED_RESERVATIONS) {
            let path = self.path_for(credential_label, slot);
            if !self.take(&path, &reservation, now_unix) {
                continue;
            }
            return Some(DispatchReservationLease { path });
        }
        None
    }

    /// Take `path` for `reservation`, or answer `false` because somebody
    /// else holds it.
    ///
    /// The exclusive create is the whole of the mutual exclusion; the write
    /// after it only fills in a file this process already owns.
    fn take(&self, path: &Path, reservation: &DispatchReservation, now_unix: i64) -> bool {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if self.is_held(path, now_unix) {
                    return false;
                }
                // Past its deadline: remove it and let a second exclusive
                // create decide who gets the empty slot, rather than
                // renaming over a file another dispatcher may be claiming at
                // this instant.
                let _ = std::fs::remove_file(path);
                if std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .is_err()
                {
                    return false;
                }
            }
            Err(err) => {
                tracing::debug!(
                    error = %err,
                    "could not claim a dispatch reservation; this dispatch reserves nothing"
                );
                return false;
            }
        }
        let Ok(encoded) = serde_json::to_vec_pretty(&PersistedDispatchReservation {
            version: DISPATCH_RESERVATION_FORMAT_VERSION,
            reservation: reservation.clone(),
        }) else {
            return true;
        };
        if let Err(err) = crate::provider::cache::write_json_atomically(path, &encoded) {
            // The claim stands — this process holds the slot and will
            // release it — but nobody else can read what it is for. The
            // empty file's own age is what bounds it, which is
            // [`Self::is_held`]'s second arm.
            tracing::debug!(
                error = %err,
                "a dispatch reservation was claimed but could not be described"
            );
        }
        true
    }

    /// Whether the slot at `path` is claimed by a dispatch that has not
    /// finished.
    ///
    /// # A row being written counts as held
    ///
    /// A slot file is created empty and filled a moment later, so a reader
    /// can catch one with no content in it. That file is a claim somebody
    /// currently holds, and treating it as free would hand out the very
    /// request it is claiming. It therefore counts, and its deadline is the
    /// file's own modification time plus [`DISPATCH_RESERVATION_LEASE`] —
    /// the same bound the row would have carried — so an unreadable claim
    /// cannot hold a pool open any longer than a readable one.
    ///
    /// A file whose modification time cannot be read at all is treated as
    /// **not** held. That is the one place where refusing to invent a
    /// deadline is the safer direction, because a row with no deadline is
    /// the single thing this mechanism must never produce.
    fn is_held(&self, path: &Path, now_unix: i64) -> bool {
        match Self::read(path) {
            Some(reservation) => reservation.is_live(now_unix),
            None => Self::written_at(path)
                .is_some_and(|written_at| written_at + Self::lease_seconds() > now_unix),
        }
    }

    /// The row at `path`, or [`None`] for every way a read can fail — which
    /// all mean the same thing to a caller and none of which is an error.
    fn read(path: &Path) -> Option<DispatchReservation> {
        let bytes = std::fs::read(path).ok()?;
        let stored: PersistedDispatchReservation = serde_json::from_slice(&bytes).ok()?;
        (stored.version == DISPATCH_RESERVATION_FORMAT_VERSION).then_some(stored.reservation)
    }

    /// When the file at `path` was last written, as a unix second, or
    /// [`None`] when it does not exist or the filesystem cannot say.
    fn written_at(path: &Path) -> Option<i64> {
        let modified = std::fs::metadata(path).ok()?.modified().ok()?;
        Some(
            modified
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .ok()?
                .as_secs() as i64,
        )
    }

    fn lease_seconds() -> i64 {
        DISPATCH_RESERVATION_LEASE.as_secs() as i64
    }

    /// Write `reservation` into `slot` regardless of who holds it.
    ///
    /// **Not the production path** — [`Self::claim`] is, and it is
    /// exclusive. This exists for a test that has to plant a row a live
    /// dispatcher would never write: one whose deadline has already passed,
    /// or one belonging to a process that is not running. Both are states
    /// the readers above have rules for, and neither can be produced by
    /// asking this cache for a claim.
    pub fn plant(&self, slot: u32, reservation: &DispatchReservation) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let encoded = serde_json::to_vec_pretty(&PersistedDispatchReservation {
            version: DISPATCH_RESERVATION_FORMAT_VERSION,
            reservation: reservation.clone(),
        })
        .map_err(|err| std::io::Error::other(err.to_string()))?;
        crate::provider::cache::write_json_atomically(
            &self.path_for(&reservation.credential_label, slot),
            &encoded,
        )
    }
}

#[cfg(test)]
mod dispatch_reservation_cache_tests;
#[cfg(test)]
mod gateway_health_cache_tests;
#[cfg(test)]
mod routing_sticky_cache_tests;
#[cfg(test)]
mod tests;
