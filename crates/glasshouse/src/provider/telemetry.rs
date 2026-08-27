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

use std::path::PathBuf;

use crate::provider::quota::{
    Capacity, CapacityState, KnownPlan, LimitingUnit, LongWindowRequests, NativeAmount, Pool,
    RateCeilings, Reading, ReadingSource,
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
                source(self.name_for(&[
                    "ratelimit-remaining",
                    "x-ratelimit-remaining",
                    "x-ratelimit-remaining-requests",
                ])),
            )));
        }

        // The token pool — capability map line 1199. Only ever filled by
        // Groq's `-tokens` spelling; every other host measured here sends
        // nothing that names a token ceiling at all, so this is a no-op for
        // them rather than a guess dressed as a reading.
        let mut tokens = state.tokens().clone();
        let mut combined = tokens.combined().clone();
        if combined.limit().is_readable()
            && let Some(limit) = self.token_limit
        {
            combined = combined.with_limit(Capacity::Measured(Reading::new(
                NativeAmount::whole(limit, "tokens"),
                observed_at_unix,
                source(self.name_for(&["x-ratelimit-limit-tokens"])),
            )));
        }
        if combined.remaining().is_readable()
            && let Some(remaining) = self.token_remaining
        {
            combined = combined.with_remaining(Capacity::Measured(Reading::new(
                NativeAmount::whole(remaining, "tokens"),
                observed_at_unix,
                source(self.name_for(&["x-ratelimit-remaining-tokens"])),
            )));
        }
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
        self.root.join(format!(
            "{}.json",
            crate::provider::cache::file_stem(provider)
        ))
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
        let path = self.path_for(provider);
        let bytes = std::fs::read(&path).ok()?;
        let stored: PersistedGatewayReading = serde_json::from_slice(&bytes).ok()?;
        if stored.version != GATEWAY_QUOTA_FORMAT_VERSION || stored.provider != provider {
            return None;
        }
        Some((
            RateLimitHeaders::from_persisted(&stored.fields),
            stored.observed_at_unix,
        ))
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
        let stored = PersistedGatewayReading {
            version: GATEWAY_QUOTA_FORMAT_VERSION,
            provider: provider.to_owned(),
            observed_at_unix,
            fields: headers.to_persisted(),
        };
        let encoded = serde_json::to_vec_pretty(&stored)
            .map_err(|err| std::io::Error::other(err.to_string()))?;
        let path = self.path_for(provider);
        let temporary = path.with_extension("json.writing");
        std::fs::write(&temporary, &encoded)?;
        std::fs::rename(&temporary, &path)?;
        Ok(())
    }
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

    // --- line 1230: a provider's own usage endpoint -----------------------

    /// The exact shape `GET https://openrouter.ai/api/v1/key` answered with,
    /// authenticated, 2026-08-27, field names and *types* recorded in
    /// `.agent-runtime/probe-quota-headers-2026-08-27.md` — never a value.
    /// `data.limit`, `data.limit_remaining` and `data.limit_reset` really
    /// were `null` on the probed account; `9` below stands in for `usage`'s
    /// real figure, which was never recorded, and is not asserted on for
    /// exactly that reason — this reader does not apply `usage` to anything.
    const OPENROUTER_KEY_BODY: &str = r#"{
        "data": {
            "limit": null,
            "limit_remaining": null,
            "limit_reset": null,
            "usage": 9,
            "usage_daily": 9,
            "usage_weekly": 9,
            "usage_monthly": 9,
            "is_free_tier": false,
            "include_byok_in_limit": false,
            "rate_limit": { "requests": 9, "interval": "10s" }
        }
    }"#;

    #[test]
    fn a_null_limit_is_read_as_present_and_inapplicable_not_as_absent() {
        let usage = ProviderUsage::read(OPENROUTER_KEY_BODY);
        assert!(
            !usage.is_empty(),
            "a body carrying three null fields is not nothing"
        );

        let state = usage.apply_to(CapacityState::metered_balance(), OBSERVED);
        assert_eq!(state.credits().limit(), &Capacity::Inapplicable);
        assert_eq!(state.credits().remaining(), &Capacity::Inapplicable);
        assert_eq!(
            state.windows().calendar().resets_at_unix(),
            &Capacity::Inapplicable
        );
        // And the rolling window — what `RateLimitHeaders` fills — is
        // untouched: this endpoint's reset is an account-level one, not a
        // short rolling ceiling's.
        assert!(!state.windows().rolling().resets_at_unix().is_measured());
    }

    /// D3's other half: an endpoint an account never answered at all reads
    /// as nothing, the same as `RateLimitHeaders` on a header-free response.
    #[test]
    fn a_body_with_no_data_object_reads_as_nothing() {
        for body in ["", "not json", "{}", r#"{"data": []}"#, r#"{"data": {}}"#] {
            let usage = ProviderUsage::read(body);
            assert!(usage.is_empty(), "`{body}` must not yield a reading");
            let state = usage.apply_to(CapacityState::metered_balance(), OBSERVED);
            assert!(!state.credits().limit().is_measured());
            assert_eq!(
                state.credits().limit(),
                CapacityState::metered_balance().credits().limit()
            );
        }
    }

    /// The numeric branch, exercised with a value shaped like the field's
    /// documented type rather than a live observation — **no authenticated
    /// account this project has read has ever answered a non-null `limit`**,
    /// so this proves the parser's arithmetic, not a provider's behaviour.
    #[test]
    fn a_numeric_limit_becomes_a_measured_credits_ceiling() {
        let usage = ProviderUsage::read(
            r#"{"data": {"limit": 25, "limit_remaining": 10, "limit_reset": 30}}"#,
        );
        let state = usage.apply_to(CapacityState::metered_balance(), OBSERVED);
        assert_eq!(
            state.credits().limit().value().map(NativeAmount::value),
            Some(25)
        );
        assert_eq!(
            state.credits().remaining().value().map(NativeAmount::value),
            Some(10)
        );
        assert_eq!(
            state.windows().calendar().resets_at_unix().value(),
            Some(&(OBSERVED + 30))
        );
        assert!(
            state
                .credits()
                .limit()
                .describe_source()
                .contains("GET /key")
        );
    }

    /// A subscription has no credit balance at all — `Pool::inapplicable`,
    /// per `CapacityState::opaque_subscription`'s own documentation — and
    /// `is_readable` refuses it exactly as it refuses a genuinely opaque
    /// pool. This reader must respect that exactly as `RateLimitHeaders`
    /// does.
    #[test]
    fn a_reader_cannot_fill_in_a_subscriptions_inapplicable_credits_pool() {
        let usage = ProviderUsage::read(r#"{"data": {"limit": 25, "limit_remaining": 10}}"#);
        let subscription = CapacityState::opaque_subscription();
        assert!(!subscription.credits().limit().is_readable());
        let after = usage.apply_to(subscription, OBSERVED);
        assert!(!after.credits().limit().is_measured());
        assert_eq!(after.credits().limit(), &Capacity::Inapplicable);
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

    // --- Groq's real inference-response headers, capability map lines
    // 1199, 1200, 1207, 1215, 1217 and 1218 -------------------------------

    /// The exact header set `POST /chat/completions` answered with, against a
    /// free model with `max_tokens: 1`, read 2026-08-26 and recorded in
    /// `.agent-runtime/probe-quota-headers-2026-08-27.md`. Field for field,
    /// not composed — the same discipline `anyrouter_models_headers` follows.
    fn groq_inference_headers() -> Vec<(&'static str, &'static str)> {
        vec![
            ("x-ratelimit-limit-requests", "7000"),
            ("x-ratelimit-limit-tokens", "6000"),
            ("x-ratelimit-remaining-requests", "6999"),
            ("x-ratelimit-remaining-tokens", "5991"),
            ("x-ratelimit-reset-requests", "12.342s"),
            ("x-ratelimit-reset-tokens", "90ms"),
        ]
    }

    #[test]
    fn groqs_named_headers_split_into_a_request_pool_and_a_token_pool() {
        let read = RateLimitHeaders::read(groq_inference_headers());
        // The `-requests` pair lands in the same fields AnyRouter's unsuffixed
        // spelling fills.
        assert_eq!(read.limit(), Some(7000));
        assert_eq!(read.remaining(), Some(6999));
        // The `-tokens` pair is a pool of its own.
        assert_eq!(read.token_limit(), Some(6000));
        assert_eq!(read.token_remaining(), Some(5991));
    }

    /// `"12.342s"` and `"90ms"` are not the bare integers every other host
    /// measured here sends — this is the duration-suffixed shape only Groq
    /// has been observed to use.
    #[test]
    fn a_duration_suffixed_reset_is_read_in_whole_seconds() {
        assert_eq!(parse_reset_seconds("12.342s"), Some(12));
        assert_eq!(parse_reset_seconds("90ms"), Some(0));
        assert_eq!(parse_reset_seconds("1500ms"), Some(2));
        // The plain-integer shape still works: this function replaces
        // `parse_count` for reset fields, not adds to it.
        assert_eq!(parse_reset_seconds("30"), Some(30));
        // Nonsense is nothing, not a panic and not a guess.
        for junk in ["", "s", "ms", "-3s", "abcs", "3.4.5s"] {
            assert_eq!(parse_reset_seconds(junk), None, "`{junk}`");
        }
    }

    #[test]
    fn groqs_headers_reach_the_token_pool_as_a_reading_never_read_from_anywhere_else() {
        let state = RateLimitHeaders::read(groq_inference_headers()).apply_to(
            ResourceKind::from_direct_provider("groq").capacity(),
            OBSERVED,
        );
        let tokens = state.tokens().combined();
        assert_eq!(tokens.limit().value().map(NativeAmount::value), Some(6000));
        assert_eq!(
            tokens.limit().value().map(NativeAmount::unit),
            Some("tokens")
        );
        assert_eq!(
            tokens.remaining().value().map(NativeAmount::value),
            Some(5991)
        );
        assert!(
            tokens
                .limit()
                .describe_source()
                .contains("x-ratelimit-limit-tokens")
        );

        // And the request pool independently, from the `-requests` spelling.
        assert_eq!(
            state.requests().limit().value().map(NativeAmount::value),
            Some(7000)
        );
    }

    /// Capability map lines 1199 and 1200: a resource that has just
    /// published both a request and a token ceiling is now evidenced to be
    /// limited by both units at once, not only by the shape's default
    /// (credits, for a metered account — Phase 32A's `metered_balance`).
    #[test]
    fn a_reading_of_both_pools_evidences_both_limiting_units_at_once() {
        let state = RateLimitHeaders::read(groq_inference_headers()).apply_to(
            ResourceKind::from_direct_provider("groq").capacity(),
            OBSERVED,
        );
        assert!(state.limiting_units().includes(LimitingUnit::Requests));
        assert!(state.limiting_units().includes(LimitingUnit::Tokens));
        // The shape's own default is not lost — a metered account is still
        // credit-limited even once its request and token pools are read.
        assert!(state.limiting_units().includes(LimitingUnit::Credits));
    }

    /// A resource this reader is not allowed to fill in stays that way: local
    /// inference cannot receive headers in the first place, but the guard is
    /// asserted directly against `LimitingUnits::None` and `::Delegated`
    /// rather than relied on implicitly.
    #[test]
    fn evidencing_a_unit_is_a_no_op_for_none_and_delegated() {
        use crate::provider::quota::LimitingUnits;
        assert_eq!(
            LimitingUnits::None.with_evidenced(LimitingUnit::Tokens),
            LimitingUnits::None
        );
        assert_eq!(
            LimitingUnits::Delegated.with_evidenced(LimitingUnit::Requests),
            LimitingUnits::Delegated
        );
    }

    /// Capability map lines 1217 and 1218, at the point this package can
    /// actually reach: Groq's headers are the first and only seam observed
    /// anywhere that gives both halves of a pool in one unit, so
    /// `Pool::normalized` produces a real `Percentage::Exact` from them — the
    /// structural guarantee Phase 32A built, exercised for the first time by
    /// a live reading rather than by hand-built test data.
    ///
    /// **This still does not close either line, and the reason has moved
    /// again.** BRIDGE-QUOTA built the persisted cache
    /// (`GatewayQuotaCache`, below) and the gateway-side write into it
    /// (`crate::gateway::Gateway::start_with_quota_cache`), so a reading
    /// this shaped can now survive the process boundary between the gateway
    /// and a `glasshouse resources` invocation — proven end to end at
    /// `resources::tests::a_persisted_gateway_reading_reaches_the_rendered_report`.
    /// What still does not exist is the one line in `main.rs` that would
    /// call either new entry point from the shipped binary; see this
    /// package's report for exactly which line, at which of two call sites.
    /// Recorded here as proof the model is ready the day that caller exists,
    /// per practice §36: a reading arriving is not the same question as
    /// something asking for a percentage.
    #[test]
    fn groqs_reading_produces_a_real_exact_percentage_from_the_model_alone() {
        let state = RateLimitHeaders::read(groq_inference_headers()).apply_to(
            ResourceKind::from_direct_provider("groq").capacity(),
            OBSERVED,
        );

        let requests_score = state
            .requests()
            .normalized()
            .expect("both halves of the request pool were read");
        assert_eq!(requests_score.percent().exact(), Some(99));

        let tokens_score = state
            .tokens()
            .combined()
            .normalized()
            .expect("both halves of the token pool were read");
        assert_eq!(tokens_score.percent().exact(), Some(99));
        assert!(!tokens_score.percent().render().contains("estimated"));
    }

    // --- GatewayQuotaCache: a reading surviving its own process ------------

    #[test]
    fn a_stored_reading_comes_back_with_every_field_and_its_timestamp() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayQuotaCache::at(dir.path());
        let written = RateLimitHeaders::read(groq_inference_headers());
        cache.store("groq", &written, OBSERVED);

        let (read, observed_at_unix) = cache.load("groq").expect("the reading is cached");
        assert_eq!(read.limit(), Some(7000));
        assert_eq!(read.remaining(), Some(6999));
        assert_eq!(read.token_limit(), Some(6000));
        assert_eq!(read.token_remaining(), Some(5991));
        assert_eq!(observed_at_unix, OBSERVED);
        // The round trip is exact enough to reproduce the same real
        // percentage the model-level test above computes directly — proof
        // that persisting and reading back changes nothing about what the
        // reading means.
        let state = read.apply_to(
            ResourceKind::from_direct_provider("groq").capacity(),
            OBSERVED,
        );
        assert_eq!(
            state
                .requests()
                .normalized()
                .and_then(|s| s.percent().exact()),
            Some(99)
        );
    }

    #[test]
    fn a_provider_with_no_persisted_reading_is_a_miss_rather_than_an_error() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        assert!(
            GatewayQuotaCache::at(dir.path())
                .load("never-forwarded")
                .is_none()
        );
    }

    #[test]
    fn storing_again_replaces_the_previous_reading_for_the_same_provider() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayQuotaCache::at(dir.path());
        cache.store(
            "anyrouter",
            &RateLimitHeaders::read(vec![("ratelimit-limit", "300")]),
            OBSERVED,
        );
        cache.store(
            "anyrouter",
            &RateLimitHeaders::read(vec![("ratelimit-limit", "150")]),
            OBSERVED + 60,
        );
        let (read, observed_at_unix) = cache.load("anyrouter").expect("cached");
        assert_eq!(read.limit(), Some(150), "the newer reading must win");
        assert_eq!(observed_at_unix, OBSERVED + 60);
    }

    #[test]
    fn an_empty_reading_is_never_written_at_all() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayQuotaCache::at(dir.path());
        cache.store("groq", &RateLimitHeaders::read(Vec::new()), OBSERVED);
        assert!(
            !dir.path().exists() || std::fs::read_dir(dir.path()).unwrap().next().is_none(),
            "an exchange that carried no rate-limit header must not create a cache file"
        );
    }

    #[test]
    fn a_reading_already_on_disk_is_not_erased_by_a_later_empty_one() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayQuotaCache::at(dir.path());
        cache.store(
            "groq",
            &RateLimitHeaders::read(vec![("x-ratelimit-limit-requests", "7000")]),
            OBSERVED,
        );
        cache.store("groq", &RateLimitHeaders::read(Vec::new()), OBSERVED + 60);
        assert_eq!(
            cache.load("groq").and_then(|(h, _)| h.limit()),
            Some(7000),
            "an empty reading must not overwrite a real one on disk, mirroring \
             SessionRouting::observe_quota_headers's own in-memory guard"
        );
    }

    #[test]
    fn load_all_finds_every_provider_a_gateway_has_ever_written_for() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayQuotaCache::at(dir.path());
        cache.store(
            "groq",
            &RateLimitHeaders::read(groq_inference_headers()),
            OBSERVED,
        );
        cache.store(
            "anyrouter",
            &RateLimitHeaders::read(vec![("ratelimit-limit", "300")]),
            OBSERVED + 10,
        );
        let mut found: Vec<String> = cache
            .load_all()
            .into_iter()
            .map(|(provider, _, _)| provider)
            .collect();
        found.sort();
        assert_eq!(found, vec!["anyrouter".to_owned(), "groq".to_owned()]);
    }

    #[test]
    fn a_provider_name_that_looks_like_a_path_cannot_escape_the_cache_directory() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayQuotaCache::at(dir.path());
        cache.store(
            "../../.ssh/authorized_keys",
            &RateLimitHeaders::read(vec![("ratelimit-limit", "1")]),
            OBSERVED,
        );
        assert!(
            !dir.path().parent().unwrap().join(".ssh").exists(),
            "a hostile provider name must never steer a write outside the cache directory"
        );
        assert_eq!(
            cache
                .load("../../.ssh/authorized_keys")
                .and_then(|(h, _)| h.limit()),
            Some(1),
            "the same hostile name must still round-trip through its own digested file"
        );
    }

    /// A cache file for one provider must never answer another provider's
    /// query, even if the file were somehow moved or hand-edited to a
    /// different provider name inside — [`ModelCache::load`]'s own guard,
    /// mirrored here.
    #[test]
    fn a_reading_stored_for_one_provider_is_never_returned_for_another() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayQuotaCache::at(dir.path());
        cache.store(
            "groq",
            &RateLimitHeaders::read(groq_inference_headers()),
            OBSERVED,
        );
        assert!(cache.load("anyrouter").is_none());
    }

    /// design-decisions.md's own rule, checked against the bytes actually
    /// written: header *values* Groq or AnyRouter sent become parsed
    /// integers or vanish, and only names Glasshouse chose from
    /// [`RATE_LIMIT_HEADERS`] ever reach the file — mirroring
    /// `discovery::tests::nothing_but_an_allowlisted_header_survives_the_capture`
    /// at the point this reading is written to disk rather than read off the
    /// wire.
    #[test]
    fn nothing_but_an_allowlisted_header_name_survives_into_the_persisted_file() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayQuotaCache::at(dir.path());
        cache.store(
            "groq",
            &RateLimitHeaders::read(vec![
                ("x-ratelimit-limit-requests", "7000"),
                ("set-cookie", "__cf_bm=a-planted-session-cookie-value"),
                ("authorization", "Bearer sk-planted-provider-credential"),
            ]),
            OBSERVED,
        );
        let bytes = std::fs::read(cache.path_for("groq")).expect("the file was written");
        let text = String::from_utf8(bytes).expect("the cache file is UTF-8 JSON");
        assert!(!text.contains("cf_bm"));
        assert!(!text.contains("planted-session-cookie"));
        assert!(!text.contains("planted-provider-credential"));
        assert!(!text.contains("authorization"));
        assert!(text.contains("x-ratelimit-limit-requests"));
    }

    /// A file written by a future format version is a miss, not a misread —
    /// [`ModelCache::load`]'s own contract, mirrored here.
    #[test]
    fn a_future_format_version_is_ignored_rather_than_misread() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayQuotaCache::at(dir.path());
        cache.store(
            "groq",
            &RateLimitHeaders::read(groq_inference_headers()),
            OBSERVED,
        );
        // Overwrite with a hand-bumped version, the same way a future build
        // that changed the shape would leave one behind for this build.
        // `serde_json::Value` rather than a string replace, so this does not
        // depend on `to_vec_pretty`'s exact spacing.
        let path = cache.path_for("groq");
        let bytes = std::fs::read(&path).expect("written above");
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid JSON");
        value["version"] = serde_json::json!(99);
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).expect("overwritten");
        assert!(cache.load("groq").is_none());
    }

    /// A corrupted or partially written file is a miss, not a panic — the
    /// same crash-mid-write case
    /// `crate::provider::cache::ModelCache::store`'s own doc names, proven
    /// here at the read end.
    #[test]
    fn a_truncated_file_is_a_miss_rather_than_a_panic() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayQuotaCache::at(dir.path());
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(cache.path_for("groq"), b"{\"version\":1,\"provider\":\"gr")
            .expect("a deliberately truncated file");
        assert!(cache.load("groq").is_none());
    }

    /// [`RateLimitHeaders::from_persisted`]'s own refusal: a name in a
    /// hand-edited file that is not on [`RATE_LIMIT_HEADERS`] must not
    /// survive into `read_from`, the same way a header off the wire that is
    /// not on the allowlist never does.
    #[test]
    fn a_hand_edited_read_from_name_off_the_allowlist_is_dropped_on_load() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayQuotaCache::at(dir.path());
        cache.store(
            "groq",
            &RateLimitHeaders::read(vec![("x-ratelimit-limit-requests", "7000")]),
            OBSERVED,
        );
        let path = cache.path_for("groq");
        let text = std::fs::read_to_string(&path).unwrap().replacen(
            "\"x-ratelimit-limit-requests\"",
            "\"x-a-name-nobody-chose\"",
            1,
        );
        std::fs::write(&path, text).unwrap();

        let (read, _) = cache
            .load("groq")
            .expect("the rest of the file is still valid");
        assert_eq!(
            read.read_from(),
            &[] as &[&str],
            "a name off the allowlist must not reach read_from even once the number beside it did"
        );
    }
}
