//! Phase 32B: `glasshouse resources` — what Glasshouse believes about every
//! model resource it can describe, and **where each belief came from**.
//!
//! This is modelled on `glasshouse pairing` and `glasshouse response` —
//! read-only, one screen, reports what Glasshouse believes rather than
//! deciding anything, and makes no network request unless asked.
//!
//! Capability map line 1240 asks that the telemetry source be surfaced, and
//! line 1234 that an inferred percentage never be labelled exact. Every
//! number printed here arrives as a [`Capacity`], whose
//! [`Capacity::telemetry_class_str`] answers
//! [`crate::provider::quota::UNKNOWN_TELEMETRY`] when nothing was read, and
//! every percentage arrives as a [`crate::provider::quota::Percentage`],
//! whose only rendering path marks an estimate as one. This module cannot
//! print an unlabelled figure because it has no access to one.
//!
//! Without `--probe` it makes **no network request**: it reads the user's
//! configuration and each installed harness's own status interface. With
//! `--probe <provider>` it makes exactly one request, to a provider the
//! user has configured, and folds in whatever rate-limit headers come back.
// History: design-decisions.md, "Trims: provider module docs", resources/mod.rs module doc.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::config::{EffectiveConfig, Layer, QuotaStaleAfterSeconds};
use crate::integrations::{IntegrationId, IntegrationKind};
use crate::provider::pricing::PriceTable;
use crate::provider::quota::{
    Capacity, CapacityBandThresholds, CapacityState, Freshness, NativeAmount, Pool, TelemetryClass,
    UNKNOWN_TELEMETRY,
};
use crate::provider::registry::{ResourceKind, registry};
use crate::provider::telemetry::{
    GatewayHealthCache, GatewayHealthReading, GatewayQuotaCache, HarnessTelemetry,
    RateLimitHeaders, apply_harness_report, apply_provider_headers, apply_user_configuration,
    budget_period_start, read_harness_plan,
};
use crate::routing::evidence::{
    CredentialCost, EvidenceLedger, FailureClass, FailureClassCounts, recent_credential_cost,
};

/// The status interface of a harness that has one, as a command line
/// Glasshouse constructs itself.
///
/// Only Claude Code exposes a stable, machine-readable status today:
/// `claude auth status --json`, whose `subscriptionType` names the plan.
/// Codex's `codex doctor --json` carries no usage, quota, limit, credit,
/// remaining, reset, plan, window or balance field, so it is deliberately
/// absent rather than listed-and-parsed-for-nothing. Antigravity and Cursor
/// CLI have no status or usage subcommand at all. What Claude Code exposes
/// is a **plan and not a usage figure** — capability map line 1231's *"or
/// status information"* clause, not its *"usage"* clause.
///
/// The arguments live here, not on the harness adapter: the name is not
/// duplicated — [`harness_status_command`] resolves it through
/// [`IntegrationId::executable_candidates`], so only the **arguments** are
/// here.
// History: design-decisions.md, "Trims: provider module docs", resources/mod.rs `HARNESS_STATUS_ARGS` doc.
const HARNESS_STATUS_ARGS: &[(IntegrationId, &[&str])] =
    &[(IntegrationId::ClaudeCode, &["auth", "status", "--json"])];

/// The arguments that ask `harness` for its own status, if it has a stable
/// machine-readable one.
///
/// Split out from [`harness_status_command`] so that the *declaration* — which
/// harnesses have a checked interface — can be asserted without resolving an
/// executable, which depends on what happens to be installed.
pub fn harness_status_args(harness: IntegrationId) -> Option<&'static [&'static str]> {
    HARNESS_STATUS_ARGS
        .iter()
        .find(|(id, _)| *id == harness)
        .map(|(_, args)| *args)
}

/// The command line that would read `harness`'s own status, if it has one and
/// it is installed — as the operating system can actually spawn it.
///
/// # Why this goes through `spawn_command` rather than the resolved path
///
/// The executable **name** comes from [`IntegrationId::executable_candidates`],
/// which is the adapter's own answer, so this module never spells a harness
/// binary's name. But a resolved path is not always the program to run:
/// [`crate::platform::exec::LaunchKind::WindowsScript`] is the case where it
/// is a `.cmd`/`.bat` that must go through the command interpreter, and
/// **the usual Windows install of a Node-packaged CLI is exactly that shim**.
/// Calling `Command::new(resolved.path())` on one fails to spawn, which this
/// module would report as "no plan" — a correct degradation, and a silently
/// absent feature on the one platform where the shim is normal.
///
/// [`crate::platform::exec::ResolvedExecutable::spawn_command`] is the
/// translation, and it validates every argument against `cmd.exe`
/// metacharacters before allowing one through. Its `Err` is treated as "no
/// status interface" rather than propagated, for this module's usual reason:
/// nothing in the telemetry path may hand a caller an error.
pub fn harness_status_command(
    harness: IntegrationId,
) -> Option<(std::path::PathBuf, Vec<std::ffi::OsString>)> {
    let args = harness_status_args(harness)?;
    let resolved = harness
        .executable_candidates()
        .iter()
        .find_map(|name| crate::platform::exec::resolve(name).ok())?;
    resolved.spawn_command(args.iter().copied()).ok()
}

/// A short, stable description of the interface a plan was read from, for a
/// [`crate::provider::quota::ReadingSource::HarnessReport`].
///
/// The **executable name and its arguments**, never the resolved path: a path
/// under a user's home directory names the user, and this string is printed
/// in a report they may share. `claude auth status --json` says everything a
/// reader needs in order to re-run it.
fn harness_interface_name(harness: IntegrationId, args: &[&str]) -> String {
    let program = harness
        .executable_candidates()
        .first()
        .copied()
        .unwrap_or(harness.slug());
    format!("{program} {}", args.join(" "))
}

/// Read one harness's own status — capability map lines 1231 and 1232.
///
/// # It cannot fail, and it cannot hang
///
/// A harness that is not installed, that exits non-zero, that prints
/// something other than JSON, or that prints JSON without a plan in it all
/// produce [`HarnessTelemetry::nothing`] — capability map line 1238. There is
/// no error to propagate, so no coding session can be stopped by a status
/// command having a bad day.
///
/// The child's output is captured rather than inherited, and only its
/// `stdout` is parsed. `stderr` is discarded unread: a harness's error output
/// is the same class of thing as a provider's error body, which
/// `design-decisions.md` requires be treated as sensitive by default.
pub fn read_harness_status(harness: IntegrationId, now_unix: i64) -> HarnessTelemetry {
    let Some(declared_args) = harness_status_args(harness) else {
        return HarnessTelemetry::nothing();
    };
    let Some((program, args)) = harness_status_command(harness) else {
        return HarnessTelemetry::nothing();
    };
    // The *declared* arguments, not the spawn-translated ones: on Windows the
    // latter begin `/D /C <script path>`, and a resolved path names the user.
    let interface = harness_interface_name(harness, declared_args);
    let Ok(output) = std::process::Command::new(&program)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return HarnessTelemetry::nothing();
    };
    if !output.status.success() {
        return HarnessTelemetry::nothing();
    }
    let Ok(body) = String::from_utf8(output.stdout) else {
        return HarnessTelemetry::nothing();
    };
    read_harness_plan(&body, now_unix, &interface)
}

/// Every reading a report has to work with, gathered before any of it is
/// rendered.
///
/// A value rather than a set of calls made mid-render, so that [`report`] is
/// a pure function of what was read. That is what lets a test drive the exact
/// production rendering path with readings it chose — including readings no
/// host would produce twice, like a header set captured from a real provider
/// on a specific day — without a network or a subprocess anywhere near it.
#[derive(Debug, Clone, Default)]
pub struct GatheredTelemetry {
    harness: BTreeMap<&'static str, HarnessTelemetry>,
    providers: BTreeMap<String, (RateLimitHeaders, i64)>,
    /// Capability map lines 1311/1321/1322/1324's own reading, kept separate
    /// from `providers` above rather than folded in: quota and health are two
    /// different facts about a resource, and line 1324 asks that a reader
    /// never be able to conflate them by construction.
    health: BTreeMap<String, Vec<GatewayHealthReading>>,
    /// Capability map lines 1316 and 1365: what the routing evidence ledger
    /// recorded about each provider's failures over
    /// [`FAILURE_CLASS_WINDOW_SECONDS`], by kind. A third map beside the two
    /// above for the same reason `health` is beside `providers`: a throttle
    /// count, a cooldown and a remaining-quota reading are three facts, and
    /// none may stand in for another.
    failure_classes: BTreeMap<String, FailureClassCounts>,
    /// Capability map line 1519's own reading: what the routing evidence
    /// ledger, priced through `pricing.toml`, counted as spend against each
    /// provider's configured money budget's own period. A fourth map beside
    /// the three above for the same reason: a failure count and a priced
    /// spend are two different facts, and neither may stand in for the
    /// other. Only providers with `[providers.<name>.quota] budget`
    /// configured are ever keys here — see [`Self::gather_budget_spend`].
    budget_spend: BTreeMap<String, CredentialCost>,
}

/// How far back `glasshouse resources` counts failures by class — capability
/// map line 1316's "recent". A day: long enough that a session's worth of
/// exchanges is in view, short enough that yesterday's outage does not
/// colour today's reading. Not the ledger's own routing window
/// (`crate::gateway::session::FAILOVER_EVIDENCE_WINDOW_SECONDS` is a
/// routing decision's horizon; this is a report's).
pub const FAILURE_CLASS_WINDOW_SECONDS: i64 = 24 * 60 * 60;

impl GatheredTelemetry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record what a harness said about itself — capability map line 1232's
    /// seam.
    pub fn with_harness(mut self, harness: IntegrationId, report: HarnessTelemetry) -> Self {
        self.harness.insert(harness.slug(), report);
        self
    }

    /// Record what a provider's response headers said — capability map
    /// line 1229's seam, independent of the one above.
    pub fn with_provider_headers(
        mut self,
        provider: impl Into<String>,
        headers: RateLimitHeaders,
        observed_at_unix: i64,
    ) -> Self {
        self.providers
            .insert(provider.into(), (headers, observed_at_unix));
        self
    }

    /// Read every installed harness's own status.
    ///
    /// Local process invocations only — no network, no credential, no quota
    /// spent. Runs for every [`IntegrationKind::Harness`] integration, so a
    /// harness that gains a status interface later is picked up by adding one
    /// line to this module's private `HARNESS_STATUS_ARGS` and nothing here.
    pub fn gather_harness_status(mut self, now_unix: i64) -> Self {
        for harness in IntegrationId::ALL
            .iter()
            .filter(|id| id.kind() == IntegrationKind::Harness)
        {
            let report = read_harness_status(*harness, now_unix);
            if report.known_plan().is_measured() {
                self.harness.insert(harness.slug(), report);
            }
        }
        self
    }

    /// Fold in whatever a local gateway has captured off real forwarded
    /// traffic and persisted to disk — capability map line 1229's gateway
    /// half, bridged across the process boundary between the `glasshouse
    /// run`/`glasshouse launch` that ran the gateway and this
    /// `glasshouse resources` invocation, which is never the same process.
    ///
    /// A read of [`GatewayQuotaCache::load_all`] — no network, no
    /// subprocess, no credential, exactly [`Self::gather_harness_status`]'s
    /// own cost. Providers this fills in are folded through
    /// [`Self::with_provider_headers`], the same seam `--probe` already
    /// uses, so `report`'s D3 staleness handling and D5's "prefer
    /// authoritative" ordering both apply to a gateway-sourced reading
    /// exactly as they do to a probed one.
    // History: design-decisions.md, "Trims: provider module docs", resources/mod.rs `gather_gateway_quota` doc.
    pub fn gather_gateway_quota(mut self, cache: &GatewayQuotaCache) -> Self {
        for (provider, headers, observed_at_unix) in cache.load_all() {
            self = self.with_provider_headers(provider, headers, observed_at_unix);
        }
        self
    }

    /// Record what a gateway has observed about a provider's resources'
    /// health — capability map lines 1311/1321/1322/1324's own seam,
    /// independent of every quota seam above: a resource's health and its
    /// quota are two different facts, and line 1324 asks that neither ever
    /// stand in for the other.
    pub fn with_provider_health(
        mut self,
        provider: impl Into<String>,
        readings: Vec<GatewayHealthReading>,
    ) -> Self {
        self.health.insert(provider.into(), readings);
        self
    }

    /// Fold in every resource's health a local gateway has observed and
    /// persisted to disk — capability map lines 1311/1321/1322/1324's bridge
    /// across the process boundary between the `glasshouse run`/`glasshouse
    /// launch` that ran the gateway and this `glasshouse resources`
    /// invocation, which is never the same process.
    ///
    /// A read of [`GatewayHealthCache::load_all`] — no network, no
    /// subprocess, no credential, exactly [`Self::gather_gateway_quota`]'s
    /// own cost and [`GatewayHealthCache`]'s own fail-soft contract: a cache
    /// with nothing in it, or a corrupt file, folds in nothing rather than
    /// producing an error.
    pub fn gather_gateway_health(mut self, cache: &GatewayHealthCache) -> Self {
        for (provider, readings) in cache.load_all() {
            self = self.with_provider_health(provider, readings);
        }
        self
    }

    /// Record what the routing evidence ledger counted about a provider's
    /// failures, by kind — capability map lines 1316 and 1365's own seam,
    /// independent of every seam above.
    pub fn with_provider_failure_classes(
        mut self,
        provider: impl Into<String>,
        counts: FailureClassCounts,
    ) -> Self {
        self.failure_classes.insert(provider.into(), counts);
        self
    }

    /// Fold in every provider's failure counts over the last
    /// [`FAILURE_CLASS_WINDOW_SECONDS`] from the project's routing evidence
    /// ledger — capability map lines 1316 and 1365's bridge from the gateway
    /// that recorded the rows to this `glasshouse resources` invocation.
    ///
    /// One `GROUP BY` over the ledger
    /// ([`EvidenceLedger::failure_classes_by_provider`]), no network, no
    /// subprocess. Fail-soft exactly as [`Self::gather_gateway_quota`] and
    /// [`Self::gather_gateway_health`] are: a ledger that cannot be read
    /// folds in nothing rather than failing the report, and the reason is
    /// logged at debug level.
    ///
    /// **Not yet called from `glasshouse resources`.** The caller this method
    /// exists for is `main.rs::resources_report`, which is outside this
    /// package's files; the call it needs is one line beside its
    /// `gather_gateway_health` call, with an [`EvidenceLedger::open`] on the
    /// `runtime` it already holds — see the package report. Tests exercise
    /// this method against a real ledger, which proves the gather and the
    /// rendering without claiming the production reach it does not yet have
    /// (practice §35).
    pub fn gather_failure_classes(mut self, ledger: &EvidenceLedger, now_unix: i64) -> Self {
        match ledger.failure_classes_by_provider(now_unix, FAILURE_CLASS_WINDOW_SECONDS) {
            Ok(by_provider) => {
                for (provider, counts) in by_provider {
                    self = self.with_provider_failure_classes(provider, counts);
                }
            }
            Err(err) => {
                tracing::debug!(
                    error = %err,
                    "could not count routing failures by class for the resources report"
                );
            }
        }
        self
    }

    /// Record what the routing evidence ledger counted as spend against one
    /// provider's own money budget, priced through `pricing.toml` — capability
    /// map line 1519's own seam, independent of every seam above.
    pub fn with_provider_budget_spend(
        mut self,
        provider: impl Into<String>,
        cost: CredentialCost,
    ) -> Self {
        self.budget_spend.insert(provider.into(), cost);
        self
    }

    /// Fold in, for every provider with a `[providers.<name>.quota] budget`
    /// configured, what the ledger counted as priced spend against that
    /// budget's own period — capability map line 1519's bridge from the
    /// ledger `main.rs::resources_report` already holds to this report, and
    /// the reading `observed_capacity` folds into the pool's remaining half.
    ///
    /// Fail-soft exactly as [`Self::gather_failure_classes`] and every other
    /// gather above: a provider whose window cannot be read folds in
    /// nothing rather than failing the whole report, and the reason is
    /// logged at debug level. A provider with no budget configured is never
    /// queried at all — there is nothing to count it against.
    ///
    /// **Not yet called from `glasshouse resources`.** See this package's
    /// report for the one call main.rs's `resources_report` and its
    /// telemetry builders need, exactly as
    /// [`Self::gather_failure_classes`]'s own doc records for its own call.
    pub fn gather_budget_spend(
        mut self,
        ledger: &EvidenceLedger,
        prices: &PriceTable,
        effective: &EffectiveConfig<'_>,
        now_unix: i64,
    ) -> Self {
        for provider in effective.provider_names() {
            let Some(budget) = effective.quota_override(&provider).value.budget() else {
                continue;
            };
            let since_unix = budget_period_start(budget.period(), now_unix);
            let window_seconds = (now_unix - since_unix).max(0);
            match ledger.observations_in_window(now_unix, window_seconds) {
                Ok(rows) => {
                    let cost = recent_credential_cost(&rows, &provider, None, prices, since_unix);
                    self = self.with_provider_budget_spend(provider, cost);
                }
                Err(err) => {
                    tracing::debug!(
                        provider = %provider,
                        error = %err,
                        "could not count budget spend for the resources report"
                    );
                }
            }
        }
        self
    }

    fn for_harness(&self, harness: IntegrationId) -> Option<&HarnessTelemetry> {
        self.harness.get(harness.slug())
    }

    fn for_provider(&self, provider: &str) -> Option<&(RateLimitHeaders, i64)> {
        self.providers.get(provider)
    }

    /// Capability map line 1366's *parse* half, for
    /// `main.rs::observed_provider_health`'s [`crate::routing::free::PoolReading`]:
    /// `provider`'s own `window_seconds` header, when it stated one — never
    /// derived, never guessed. `None` covers both "no reading gathered for
    /// this provider" and "a reading was gathered, and it said nothing about
    /// a window" alike, which is exactly what leaves the door open for a
    /// learned window to fill it in.
    pub fn stated_pool_window(&self, provider: &str) -> Option<crate::routing::free::Window> {
        let (headers, _) = self.for_provider(provider)?;
        let seconds = headers.window_seconds()?;
        Some(crate::routing::free::Window::Stated {
            seconds: u32::try_from(seconds).ok()?,
        })
    }

    /// Every resource's health gathered for `provider`, or an empty slice
    /// when nothing has been observed — capability map line 1324's "never
    /// invent a reading" half, at the one place a renderer can ask.
    fn for_provider_health(&self, provider: &str) -> &[GatewayHealthReading] {
        self.health.get(provider).map_or(&[], |readings| readings)
    }

    /// The failure counts gathered for `provider`, or `None` when the ledger
    /// had no exchange of it in the window — line 1316's reader, at the one
    /// place a renderer can ask.
    fn for_provider_failure_classes(&self, provider: &str) -> Option<&FailureClassCounts> {
        self.failure_classes.get(provider)
    }

    /// The priced spend gathered against `provider`'s own money budget, or
    /// `None` when nothing was gathered for it — either no budget is
    /// configured, or [`Self::gather_budget_spend`] was never called. Public,
    /// unlike its three siblings above: `main.rs`'s telemetry builders need
    /// this reading to decide whether a destination's provider budget is
    /// exhausted (map line 1519), one layer above what
    /// [`observed_capacity`] itself folds into a pool.
    pub fn provider_budget_spend(&self, provider: &str) -> Option<&CredentialCost> {
        self.budget_spend.get(provider)
    }

    /// Drop `provider`'s gathered budget spend, if any — for a caller that
    /// needs `observed_capacity`'s reading for a resource **as if no money
    /// budget applied to it**, map line 1519's own requirement that a
    /// free-tier candidate is never excluded by one.
    ///
    /// The need is structural, not incidental: `observed_capacity` folds a
    /// provider's money budget into the *same* `CapacityState` a metered and
    /// a free candidate of that provider both read `remaining_capacity_score`
    /// from, and `routing::disposable`'s existing line-1434 "known zero
    /// headroom" gate does not distinguish which dimension bound that score
    /// — a free candidate whose provider's budget reads exhausted would
    /// otherwise be excluded by a fact about money that never applies to it.
    /// `main.rs::disposable_candidates` is the one caller: it builds a free
    /// candidate's own capacity against a telemetry value this method has
    /// stripped, so the exclusion this method exists to prevent cannot
    /// happen upstream of `routing::disposable`, which this package leaves
    /// untouched.
    pub fn without_provider_budget_spend(mut self, provider: &str) -> Self {
        self.budget_spend.remove(provider);
        self
    }
}

/// Map line 1519: whether `provider`'s own `[providers.<name>.quota] budget`
/// has been counted as exhausted, given what
/// [`GatheredTelemetry::gather_budget_spend`] counted against it. `None`
/// whenever either half is unestablished — no budget configured, or nothing
/// could be priced against it (an empty ledger, no `pricing.toml` entry,
/// every row relayed or unread) — never `Some` for a budget nobody could
/// count against, the same "nobody has said is not cannot" rule every other
/// entitlement gate in `routing::session` follows.
///
/// Lives here rather than in `main.rs`, where it was originally written, so
/// every caller that can reach this module can reach it too — `main.rs`'s
/// own `routing_entitlement` and `disposable_candidates`, and
/// `memory::rerank::resolve_rerank_model`, which is library code and cannot
/// call a binary-crate function.
pub fn budget_exhausted_for(
    provider: &str,
    effective: &EffectiveConfig<'_>,
    telemetry: &GatheredTelemetry,
) -> Option<crate::routing::BudgetExhaustion> {
    let budget = effective.quota_override(provider).value.budget()?;
    let spent_micro_usd = telemetry.provider_budget_spend(provider)?.micro_usd?;
    if spent_micro_usd < budget.amount_micro_usd() {
        return None;
    }
    Some(crate::routing::BudgetExhaustion {
        budget_micro_usd: budget.amount_micro_usd(),
        spent_micro_usd,
        period: budget.period().as_str(),
    })
}

/// The capacity Glasshouse believes `kind` has, after every reading that
/// applies to it has been folded in — the function every box in this phase
/// ultimately closes through.
///
/// Configuration first, then the harness's own report, then the provider's
/// own headers — weakest source applied first, strongest last, capability
/// map line 1228 — because [`Capacity::prefer`] resolves each collision in
/// favour of the more authoritative claim regardless of order.
///
/// Every step is total (line 1238). A missing reading, an unparseable one, a
/// harness that is not installed and a provider that answered no headers all
/// leave the state exactly as the previous step left it, and the worst case
/// is the state [`CapacityState::for_resource`] built with nothing read at
/// all — which is a complete, printable answer.
// History: design-decisions.md, "Trims: provider module docs", resources/mod.rs `observed_capacity` doc.
pub fn observed_capacity(
    kind: &ResourceKind,
    effective: &EffectiveConfig<'_>,
    telemetry: &GatheredTelemetry,
    now_unix: i64,
) -> CapacityState {
    let mut state = kind.capacity();

    if let ResourceKind::DirectProvider { provider, .. } = kind {
        let configured = effective.quota_override(provider);
        state = apply_user_configuration(
            state,
            configured.value.plan(),
            configured.value.budget().map(|b| b.amount_micro_usd()),
            telemetry.provider_budget_spend(provider),
            now_unix,
        );
    }

    if let ResourceKind::NativeSubscription { harness } = kind
        && let Some(report) = telemetry.for_harness(*harness)
    {
        state = apply_harness_report(state, report);
    }

    if let ResourceKind::DirectProvider { provider, .. } = kind
        && let Some((headers, observed_at_unix)) = telemetry.for_provider(provider)
    {
        state = apply_provider_headers(state, headers, *observed_at_unix);
    }

    state
}

/// How long `kind`'s telemetry stays current — capability map line 1237.
///
/// Per provider, resolved through [`EffectiveConfig::quota_stale_after`]. A
/// native subscription and the gateway are not keys in the provider table, so
/// they take the default; that is not a gap, it is that "provider-specific"
/// means specific to a *provider*, and neither of those is one.
fn stale_after(kind: &ResourceKind, effective: &EffectiveConfig<'_>) -> QuotaStaleAfterSeconds {
    match kind {
        ResourceKind::DirectProvider { provider, .. } => {
            effective.quota_stale_after(provider).value
        }
        _ => QuotaStaleAfterSeconds::DEFAULT,
    }
}

/// What to include in a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportOptions {
    /// Show every pool, window and rate ceiling rather than only the ones
    /// something is known about — capability map line 1761's debug view.
    pub verbose: bool,
    /// Unix seconds, supplied by the caller. This module has no clock, for
    /// [`mod@crate::provider::quota`]'s own reason.
    pub now_unix: i64,
}

/// Render what Glasshouse believes about every resource it can describe.
///
/// A pure function of `effective` and `telemetry`. Its output is the whole of
/// what `glasshouse resources` prints.
pub fn report(
    effective: &EffectiveConfig<'_>,
    telemetry: &GatheredTelemetry,
    options: ReportOptions,
) -> String {
    let mut out = String::new();
    out.push_str("RESOURCES\n\n");
    out.push_str(
        "Every model resource Glasshouse can describe, with what is known about its quota and\n\
         where that knowledge came from. A value marked `unknown` was never read; Glasshouse\n\
         does not fill one in.\n\n",
    );

    for kind in registry() {
        let state = observed_capacity(&kind, effective, telemetry, options.now_unix);
        let age_limit = stale_after(&kind, effective);
        render_resource(
            &mut out, &kind, &state, age_limit, effective, telemetry, options,
        );
        out.push('\n');
    }

    render_configuration_note(&mut out, effective, telemetry);
    out
}

/// What [`report`] prints, as structured data instead of text — capability
/// map line 1679: *"allow the API to retrieve current resource capacity and
/// quota telemetry."* The bin crate's control-API `resource_capacity`
/// request (`api::unix::resource_capacity`, declared from `main.rs`) is the
/// production caller; see `tests/capacity_api.rs` for the same guarantee
/// `tests/provider_discovery.rs` already holds [`report`] to, driven over
/// the real socket instead of a fixture.
///
/// A pure function of the same three inputs [`report`] takes, so the two
/// can never disagree about what Glasshouse believes: both fold the same
/// [`observed_capacity`] over the same [`registry`].
pub fn capacity_json(
    effective: &EffectiveConfig<'_>,
    telemetry: &GatheredTelemetry,
    now_unix: i64,
) -> serde_json::Value {
    let resources: Vec<serde_json::Value> = registry()
        .into_iter()
        .map(|kind| {
            let state = observed_capacity(&kind, effective, telemetry, now_unix);
            let thresholds = capacity_band_thresholds_for(&kind, effective);
            let capacity = state.remaining_capacity_score().map(|score| {
                let band = score.band(&thresholds);
                let reset_seconds = state.seconds_until_reset(now_unix);
                serde_json::json!({
                    "dimension": score.dimension(),
                    "percent": score.percent().render(),
                    "band": band.as_str(),
                    "score": score.fraction(),
                    "routing_score": score.routing_fraction(),
                    "effective": score.effective(reset_seconds),
                    "seconds_until_reset": reset_seconds,
                })
            });
            let health: Vec<serde_json::Value> = match &kind {
                ResourceKind::DirectProvider { provider, .. } => telemetry
                    .for_provider_health(provider)
                    .iter()
                    .map(|reading| {
                        serde_json::json!({
                            "model": reading.model,
                            "credential": reading.credential_label,
                            "consecutive_failures": reading.consecutive_failures,
                            "cooling_down_until_unix": reading.cooling_down_until_unix,
                            "credential_rejected": reading.credential_rejected,
                            "available": reading.is_available(now_unix),
                        })
                    })
                    .collect(),
                _ => Vec::new(),
            };
            serde_json::json!({
                "resource": kind.label(),
                "quota_shape": state.model().as_str(),
                "locality": state.locality().as_str(),
                "limited_by": describe_limits(&state),
                "telemetry_class": state.telemetry_class_str(),
                "last_observed_at_unix": state.last_observed_at_unix(),
                "capacity": capacity,
                // Capability map lines 1311/1321/1322/1324, kept separate
                // from `capacity` above rather than folded into it — a
                // resource's health and its quota are two different facts.
                // An empty list is honest: nothing has been observed, never
                // a fabricated healthy default.
                "health": health,
            })
        })
        .collect();
    serde_json::json!({ "resources": resources })
}

fn render_resource(
    out: &mut String,
    kind: &ResourceKind,
    state: &CapacityState,
    age_limit: QuotaStaleAfterSeconds,
    effective: &EffectiveConfig<'_>,
    telemetry: &GatheredTelemetry,
    options: ReportOptions,
) {
    let _ = writeln!(out, "{}", kind.label());
    let _ = writeln!(out, "  quota shape     {}", state.model().as_str());
    let _ = writeln!(out, "  locality        {}", state.locality().as_str());
    let _ = writeln!(out, "  limited by      {}", describe_limits(state));

    // Capability map lines 1227 and 1240: the class of claim this resource's
    // knowledge rests on, in one word, including `unknown`.
    let _ = writeln!(out, "  telemetry       {}", state.telemetry_class_str());

    // Capability map lines 1311/1321/1322/1324: this resource's observed
    // *health*, kept on its own lines rather than folded into anything
    // above — a resource cooling down after real failures is not the same
    // fact as a resource whose quota is low, and line 1324 asks that the two
    // never be scored as one.
    render_health(out, kind, telemetry, options);

    // Capability map lines 1316 and 1365: what kind of failures this
    // resource has produced recently, as three separate figures and a
    // per-class list — beside the health reading above, never folded into
    // it. A resource cooling down and a resource that answered `503` twice
    // are both "unhealthy" to a reader who only sees one line; the second
    // line is what tells them apart.
    render_failure_classes(out, kind, telemetry, options);

    // Capability map line 1236.
    match state.last_observed_at_unix() {
        Some(at) => {
            let freshness = Freshness::of(at, options.now_unix, age_limit.seconds());
            let _ = writeln!(
                out,
                "  last observed   unix {at} ({}; provider limit {}s)",
                freshness.describe(),
                age_limit.get()
            );
        }
        None => {
            let _ = writeln!(
                out,
                "  last observed   never — no quota observation has ever succeeded here"
            );
        }
    }

    // Capability map lines 1231 and 1233.
    let _ = writeln!(out, "  plan            {}", describe_plan(state));

    // Capability map line 1234, through the one rendering path a percentage
    // has. An estimate cannot print here as though it were exact.
    match state.normalized() {
        Some((pool, score)) => {
            let _ = writeln!(
                out,
                "  capacity        {} of {} ({}, {})",
                score.percent().render(),
                pool,
                score.native_unit(),
                score.remaining().source().describe()
            );
        }
        None => {
            let _ = writeln!(
                out,
                "  capacity        {UNKNOWN_TELEMETRY} — no pool was read on both halves in one unit"
            );
        }
    }

    // Phase 32D — capability map lines 1259-1268: the normalized 0.0..=1.0
    // score, the band a routing policy may read off it, and the effective
    // value a known reset adjusts. Never in place of the native-unit lines
    // above — line 1268 — only beside them.
    render_capacity_band(out, kind, state, effective, options);

    let mut shown = 0usize;
    for (label, pool) in state.pools() {
        if !options.verbose && !pool_has_a_reading(pool) {
            continue;
        }
        render_pool(out, label, pool, age_limit, options);
        shown += 1;
    }
    if shown == 0 {
        let _ = writeln!(
            out,
            "  pools           nothing read; re-run with --verbose to see every pool and its state"
        );
    }

    render_rate_ceilings(out, state, options);
    render_windows(out, state, age_limit, options);
}

/// This resource's [`CapacityBandThresholds`] — the shared defaults, or a
/// user's own overrides, with a `DirectProvider`'s own protected reserve
/// percentage folded in where it has one — capability map line 1288, design
/// decision #6.
fn capacity_band_thresholds_for(
    kind: &ResourceKind,
    effective: &EffectiveConfig<'_>,
) -> CapacityBandThresholds {
    let thresholds = effective.capacity_band_thresholds().value;
    match kind {
        ResourceKind::DirectProvider { provider, .. } => {
            thresholds.with_resource_reserve(effective.reserve_percent(provider).value.get())
        }
        _ => thresholds,
    }
}

/// Capability map lines 1259-1268: the normalized score, the band a routing
/// policy may read off it, and the reset-adjusted effective value — never
/// replacing the native-unit lines [`render_resource`] already printed,
/// only joining them.
fn render_capacity_band(
    out: &mut String,
    kind: &ResourceKind,
    state: &CapacityState,
    effective: &EffectiveConfig<'_>,
    options: ReportOptions,
) {
    let Some(score) = state.remaining_capacity_score() else {
        let _ = writeln!(
            out,
            "  band            {UNKNOWN_TELEMETRY} — no dimension normalizes to a score yet"
        );
        return;
    };
    let thresholds = capacity_band_thresholds_for(kind, effective);
    let band = score.band(&thresholds);
    let reset_seconds = state.seconds_until_reset(options.now_unix);
    let effective_value = score.effective(reset_seconds);
    let reset_note = match reset_seconds {
        Some(seconds) => format!(", reset in {seconds}s"),
        None => String::new(),
    };
    let _ = writeln!(
        out,
        "  band            {band} (score {:.2}, routing {:.2}, effective {:.2}{reset_note}; \
         bound by {})",
        score.fraction(),
        score.routing_fraction(),
        effective_value,
        score.dimension()
    );
}

/// Capability map lines 1311, 1321, 1322 and 1324: what a local gateway has
/// observed about this resource's health, distinctly from its capacity.
///
/// Only [`ResourceKind::DirectProvider`] can have a reading at all — health is
/// learned per credential and per model behind the Glasshouse gateway
/// (`crate::routing::free::FreeResource`), keyed by the same provider name
/// this function is given, exactly [`observed_capacity`]'s own
/// `telemetry.for_provider(provider)` pattern for quota. A native subscription
/// or the gateway resource itself never has one, so both print `unknown`
/// unconditionally, the same as [`describe_plan`] does for a state nothing
/// filled in.
///
/// Never prints anything but `unknown` for a resource nothing has been
/// observed about — capability map line 1324's second half, "never invent a
/// reading" — and a cooling-down resource is printed as **paced**, never as
/// broken: pacing is a scheduling fact, not a verdict on the resource.
fn render_health(
    out: &mut String,
    kind: &ResourceKind,
    telemetry: &GatheredTelemetry,
    options: ReportOptions,
) {
    let readings: &[GatewayHealthReading] = match kind {
        ResourceKind::DirectProvider { provider, .. } => telemetry.for_provider_health(provider),
        _ => &[],
    };
    if readings.is_empty() {
        let _ = writeln!(
            out,
            "  health          {UNKNOWN_TELEMETRY} — no gateway exchange has been observed for \
             this resource"
        );
        return;
    }
    for reading in readings {
        let status = if reading.credential_rejected {
            "credential rejected".to_owned()
        } else {
            match reading.cooling_down_until_unix {
                Some(until) if until > options.now_unix => format!(
                    "paced, cooling down until unix {until} ({}s)",
                    until - options.now_unix
                ),
                _ => "available".to_owned(),
            }
        };
        let _ = writeln!(
            out,
            "  health          {} ({}): {status}, {} consecutive failure(s)",
            reading.model, reading.credential_label, reading.consecutive_failures
        );
    }
}

/// Capability map lines 1316 and 1365: what the routing evidence ledger has
/// recorded about this resource's failures over the last
/// [`FAILURE_CLASS_WINDOW_SECONDS`], by kind.
///
/// **Three figures, never one.** Cadence throttling, an exhausted quota and
/// provider ill-health have three different remedies — wait a minute; wait
/// for the window or pay; route elsewhere — and line 1365 asks that they
/// never be summed into a single "failures" number a reader would have to
/// take apart again. [`FailureClassCounts`] has no total on purpose, and
/// this renderer prints none. Every figure is printed beside the number of
/// exchanges it is out of, so "throttled 3" of 4 and "throttled 3" of 400
/// read as the different facts they are.
///
/// Only a [`ResourceKind::DirectProvider`] can have a count: the ledger keys
/// rows by the provider name the gateway forwarded to, exactly the key
/// [`render_health`] uses. A native subscription and the gateway resource
/// itself print `unknown`, as they do for health. And a provider nothing
/// has been recorded for prints `unknown` too — line 1324's "never invent a
/// reading", one line down.
fn render_failure_classes(
    out: &mut String,
    kind: &ResourceKind,
    telemetry: &GatheredTelemetry,
    options: ReportOptions,
) {
    let counts = match kind {
        ResourceKind::DirectProvider { provider, .. } => {
            telemetry.for_provider_failure_classes(provider)
        }
        _ => None,
    };
    let Some(counts) = counts.filter(|counts| !counts.is_empty()) else {
        let _ = writeln!(
            out,
            "  failures 24h    {UNKNOWN_TELEMETRY} — no routing observation has been recorded \
             for this resource"
        );
        return;
    };
    let _ = writeln!(
        out,
        "  failures 24h    cadence throttled {}, quota exhausted {}, provider unhealthy {} — \
         of {} exchange(s), {} served",
        counts.cadence_throttled(),
        counts.exhausted_quota(),
        counts.provider_health_failures(),
        counts.observed(),
        counts.served(),
    );
    let mut by_class: Vec<String> = FailureClass::ALL
        .into_iter()
        .filter(|class| options.verbose || counts.count(*class) > 0)
        .map(|class| format!("{} {}", describe_failure_class(class), counts.count(class)))
        .collect();
    if counts.unclassified() > 0 {
        by_class.push(format!("unclassified {}", counts.unclassified()));
    }
    if !by_class.is_empty() {
        let _ = writeln!(out, "  by class        {}", by_class.join(", "));
    }
}

/// A [`FailureClass`] as a reader sees it: the stored name with its
/// underscores opened out, so the report and the ledger spell one vocabulary.
fn describe_failure_class(class: FailureClass) -> String {
    class.as_str().replace('_', " ")
}

/// Capability map lines 1210 and 1211: when this resource's quota window
/// started and when it resets, for the rolling window headers fill and the
/// calendar window a usage endpoint fills.
///
/// This is the caller those two lines were missing even after Phase 32B
/// built a working reader for the reset half: `WindowCapacity::started_at_unix`
/// and `::resets_at_unix` were tracked and tested but never rendered, so a
/// reading landing there would have had nowhere to show. Skipped in the quiet
/// view when nothing is known, the same rule every other line here follows.
fn render_windows(
    out: &mut String,
    state: &CapacityState,
    age_limit: QuotaStaleAfterSeconds,
    options: ReportOptions,
) {
    for (label, window) in [
        ("rolling window", state.windows().rolling()),
        ("calendar window", state.windows().calendar()),
    ] {
        let started = window.started_at_unix();
        let resets = window.resets_at_unix();
        if !options.verbose && !started.is_measured() && !resets.is_measured() {
            continue;
        }
        let _ = writeln!(
            out,
            "  {label:<15} starts {}, resets {}",
            describe_timestamp(started, age_limit, options),
            describe_timestamp(resets, age_limit, options)
        );
    }
}

/// A unix-second quantity, rendered the way [`describe_amount`] renders a
/// [`NativeAmount`] — what kind of claim it is, where it came from, and how
/// old it is. Kept separate from [`describe_amount`] rather than made
/// generic over it: a window boundary is a point in time, not an amount in a
/// provider's own unit, and the two must never be interchangeable at the
/// type level either.
fn describe_timestamp(
    capacity: &Capacity<i64>,
    age_limit: QuotaStaleAfterSeconds,
    options: ReportOptions,
) -> String {
    let Some(reading) = capacity.reading() else {
        return format!("{} ({})", capacity.as_str(), capacity.telemetry_class_str());
    };
    let freshness = reading.freshness(options.now_unix, age_limit.seconds());
    let stale = if freshness.is_stale() {
        format!(", {}", freshness.describe())
    } else {
        String::new()
    };
    format!(
        "unix {} [{}] from {}{stale}",
        reading.value(),
        reading.class().as_str(),
        reading.source().describe()
    )
}

fn render_pool(
    out: &mut String,
    label: &str,
    pool: &Pool,
    age_limit: QuotaStaleAfterSeconds,
    options: ReportOptions,
) {
    let _ = writeln!(
        out,
        "  {label:<15} remaining {}, limit {}",
        describe_amount(pool.remaining(), age_limit, options),
        describe_amount(pool.limit(), age_limit, options)
    );
}

fn render_rate_ceilings(out: &mut String, state: &CapacityState, options: ReportOptions) {
    let rates = state.rate_ceilings();
    let per_minute = rates.requests_per_minute();
    if per_minute.is_measured() || options.verbose {
        let _ = writeln!(
            out,
            "  requests/minute {} [{}] {}",
            per_minute
                .value()
                .map_or_else(|| UNKNOWN_TELEMETRY.to_owned(), render_amount),
            per_minute.telemetry_class_str(),
            per_minute.describe_source()
        );
    }
    let long = rates.long_window_requests();
    if long.is_measured() || options.verbose {
        let described = long.value().map_or_else(
            || UNKNOWN_TELEMETRY.to_owned(),
            |window| {
                format!(
                    "{} per {}s",
                    render_amount(window.limit()),
                    window.window_seconds()
                )
            },
        );
        let _ = writeln!(
            out,
            "  long window     {described} [{}] {}",
            long.telemetry_class_str(),
            long.describe_source()
        );
    }
    let tokens_per_minute = rates.tokens_per_minute();
    if tokens_per_minute.is_measured() || options.verbose {
        let _ = writeln!(
            out,
            "  tokens/minute   {} [{}] {}",
            tokens_per_minute
                .value()
                .map_or_else(|| UNKNOWN_TELEMETRY.to_owned(), render_amount),
            tokens_per_minute.telemetry_class_str(),
            tokens_per_minute.describe_source()
        );
    }
    let max_concurrent = rates.max_concurrent_requests();
    if max_concurrent.is_measured() || options.verbose {
        let _ = writeln!(
            out,
            "  max concurrent  {} [{}] {}",
            max_concurrent
                .value()
                .map_or_else(|| UNKNOWN_TELEMETRY.to_owned(), render_amount),
            max_concurrent.telemetry_class_str(),
            max_concurrent.describe_source()
        );
    }
}

/// One quantity, with the two things capability map lines 1227, 1235, 1236
/// and 1237 require beside it: what kind of claim it is, where it came from,
/// and how old it is.
fn describe_amount(
    capacity: &Capacity<NativeAmount>,
    age_limit: QuotaStaleAfterSeconds,
    options: ReportOptions,
) -> String {
    let Some(reading) = capacity.reading() else {
        return format!("{} ({})", capacity.as_str(), capacity.telemetry_class_str());
    };
    let freshness = reading.freshness(options.now_unix, age_limit.seconds());
    let stale = if freshness.is_stale() {
        format!(", {}", freshness.describe())
    } else {
        String::new()
    };
    format!(
        "{} [{}] from {}{stale}",
        render_amount(reading.value()),
        reading.class().as_str(),
        reading.source().describe()
    )
}

/// A provider-native amount, in the provider's own unit — capability map
/// line 1217. Never converted, never rounded into a different unit.
fn render_amount(amount: &NativeAmount) -> String {
    match amount.scale() {
        crate::provider::quota::UnitScale::Whole => {
            format!("{} {}", amount.value(), amount.unit())
        }
        crate::provider::quota::UnitScale::Millionths => format!(
            "{}.{:06} {}",
            amount.value() / 1_000_000,
            (amount.value() % 1_000_000).abs(),
            amount.unit()
        ),
    }
}

fn describe_plan(state: &CapacityState) -> String {
    match state.plan().reading() {
        Some(reading) => format!(
            "{} [{}] from {}",
            reading.value().name(),
            reading.class().as_str(),
            reading.source().describe()
        ),
        None => format!(
            "{} ({})",
            state.plan().as_str(),
            state.plan().telemetry_class_str()
        ),
    }
}

fn describe_limits(state: &CapacityState) -> String {
    use crate::provider::quota::LimitingUnits;
    match state.limiting_units() {
        LimitingUnits::None => "nothing — this resource cannot be exhausted".to_owned(),
        LimitingUnits::Delegated => "whatever limits its assigned upstream".to_owned(),
        LimitingUnits::These(units) => units
            .iter()
            .map(|unit| unit.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    }
}

fn pool_has_a_reading(pool: &Pool) -> bool {
    pool.limit().is_measured() || pool.remaining().is_measured()
}

/// The half of this surface that is about what the *user* can do — capability
/// map lines 1233 and 1203.
///
/// A user whose provider publishes nothing needs to be told that entering a
/// plan or a budget is an option, at the moment they are looking at a screen
/// full of `unknown`. Naming the layer each configured value came from
/// matches what `glasshouse response` and `glasshouse pairing` already do.
fn render_configuration_note(
    out: &mut String,
    effective: &EffectiveConfig<'_>,
    telemetry: &GatheredTelemetry,
) {
    out.push_str("CONFIGURED QUOTA OVERRIDES\n");
    let mut any = false;
    for name in effective.provider_names() {
        let configured = effective.quota_override(&name);
        if configured.value.is_empty() {
            continue;
        }
        any = true;
        let layer = describe_layer(configured.layer);
        if let Some(plan) = configured.value.plan() {
            let _ = writeln!(out, "  {name}: plan `{plan}` ({layer})");
        }
        if let Some(budget) = configured.value.budget() {
            let cost = telemetry.provider_budget_spend(&name);
            let amount = budget.amount_micro_usd();
            let period = budget.period().as_str();
            match cost.and_then(|cost| cost.micro_usd.map(|spent| (cost, spent))) {
                Some((cost, spent)) => {
                    let remaining = amount.saturating_sub(spent);
                    let _ = writeln!(
                        out,
                        "  {name}: budget {}.{:06} USD per {period} ({layer}) — {}.{:06} USD \
                         counted spent over {} priced exchanges ({} unread, {} unpriced), \
                         {}.{:06} USD remaining",
                        amount / 1_000_000,
                        amount % 1_000_000,
                        spent / 1_000_000,
                        spent % 1_000_000,
                        cost.priced_rows,
                        cost.unread_rows,
                        cost.unpriced_rows,
                        remaining / 1_000_000,
                        remaining % 1_000_000,
                    );
                }
                None => {
                    let unread = cost.map_or(0, |cost| cost.unread_rows);
                    let unpriced = cost.map_or(0, |cost| cost.unpriced_rows);
                    let _ = writeln!(
                        out,
                        "  {name}: budget {}.{:06} USD per {period} ({layer}) — spend not \
                         counted ({} exchanges: {unread} unread, {unpriced} unpriced)",
                        amount / 1_000_000,
                        amount % 1_000_000,
                        unread + unpriced,
                    );
                }
            }
        }
        if let Some(age) = configured.value.stale_after() {
            let _ = writeln!(
                out,
                "  {name}: telemetry stale after {}s ({layer})",
                age.get()
            );
        }
    }
    if !any {
        out.push_str(
            "  none. Where a provider publishes no usable telemetry, record what you know with\n\
             \x20 a `[providers.<name>.quota]` table: `plan = \"max\"` for a known plan, and\n\
             \x20 `budget = { amount_micro_usd = 10000000, period = \"calendar-month\" }` for a\n\
             \x20 spending ceiling. Both are read as `manual`, never as measurements.\n",
        );
    }
}

fn describe_layer(layer: Layer) -> &'static str {
    match layer {
        Layer::Project => "project",
        Layer::User => "user",
        Layer::Default => "default",
    }
}

/// The strongest kind of claim in a whole report, for a one-line summary.
///
/// Public because the acceptance tests assert on it directly rather than on
/// the rendered text: a class is a fact and a layout is a preference, and a
/// test that pinned the layout would fail on every wording change.
pub fn strongest_class(
    effective: &EffectiveConfig<'_>,
    telemetry: &GatheredTelemetry,
    now_unix: i64,
) -> Option<TelemetryClass> {
    registry()
        .iter()
        .filter_map(|kind| {
            observed_capacity(kind, effective, telemetry, now_unix).telemetry_class()
        })
        .min_by_key(|class| class.rank())
}

/// A stale reading is still a reading — capability map line 1238.
///
/// Answers whether every reading on `state` has aged past `age_limit`, which
/// is what a caller needs in order to say "this is what Glasshouse last saw,
/// and it is old" rather than discarding it. Nothing in Glasshouse deletes a
/// stale reading: falling back means preferring a fresher weaker source where
/// one exists, never blanking the strong one.
pub fn is_entirely_stale(
    state: &CapacityState,
    now_unix: i64,
    age_limit: QuotaStaleAfterSeconds,
) -> bool {
    match state.last_observed_at_unix() {
        Some(at) => Freshness::of(at, now_unix, age_limit.seconds()).is_stale(),
        None => false,
    }
}

/// Build the one request `--probe` makes for `provider` — capability map
/// line 1229's production path.
///
/// # It reuses the endpoint Glasshouse already calls
///
/// `ProbeTarget::ModelList` when the provider's model-list endpoint is
/// established and the base URL otherwise, which is exactly what
/// `crate::profile::capability_probe` chooses for the same provider. That is
/// deliberate and it is the whole reason this seam is affordable: the map's
/// line 1230 asks for telemetry that can be had *without excessive request
/// cost*, and a catalogue read Glasshouse already knows how to make costs one
/// request and no inference. It was measured returning usable rate-limit
/// headers on a real host — see [`mod@crate::provider::telemetry`].
///
/// `None` when the provider declares no protocol with a base URL, which is
/// the honest answer for the two generic templates before a user fills one
/// in. Not an error: there is nothing wrong, there is simply nowhere to send
/// a request.
pub fn telemetry_probe(
    provider: &crate::provider::Provider,
    secrets: &dyn crate::secret::SecretStore,
) -> Option<crate::provider::discovery::ProbeRequest> {
    let support = provider
        .protocols
        .iter()
        .find(|support| !support.base_url.trim().is_empty())?;
    let target = if provider.model_list_endpoint.is_known_present() {
        crate::provider::discovery::ProbeTarget::ModelList
    } else {
        crate::provider::discovery::ProbeTarget::BaseUrl
    };
    // The same search `profile::apply_direct_provider` performs: the first
    // declared credential variable that currently resolves. A probe with no
    // credential still answers a real question — AnyRouter's rate-limit
    // headers arrived on an unauthenticated request — so `None` is not
    // refused.
    let credential = provider
        .secret_refs()
        .iter()
        .find_map(|reference| secrets.resolve(reference));
    Some(crate::provider::discovery::ProbeRequest::new(
        provider.name.clone(),
        support.protocol,
        support.base_url.clone(),
        target,
        provider.headers.clone(),
        credential,
    ))
}

/// Build the one extra request `--probe` makes when `provider` declares a
/// usage endpoint — capability map line 1230's production path.
///
/// `crate::provider::usage_endpoint` is the lookup table naming which
/// providers have one; today that is OpenRouter alone. `None` here is the
/// ordinary answer for every other provider and is not an error — the same
/// shape [`telemetry_probe`] already uses for "nowhere to send a request".
pub fn usage_probe(
    provider: &crate::provider::Provider,
    secrets: &dyn crate::secret::SecretStore,
) -> Option<crate::provider::discovery::ProbeRequest> {
    let path = crate::provider::usage_endpoint(&provider.name)?;
    let support = provider
        .protocols
        .iter()
        .find(|support| !support.base_url.trim().is_empty())?;
    let url = format!("{}{}", support.base_url.trim_end_matches('/'), path);
    let credential = provider
        .secret_refs()
        .iter()
        .find_map(|reference| secrets.resolve(reference));
    Some(crate::provider::discovery::ProbeRequest::new(
        provider.name.clone(),
        support.protocol,
        url,
        crate::provider::discovery::ProbeTarget::BaseUrl,
        provider.headers.clone(),
        credential,
    ))
}

/// Capability map line 1369 — the fraction of a provider's remaining request
/// pool a `--probe` is allowed to spend before Glasshouse refuses on its own
/// initiative.
///
/// 10%: small enough that one probe never meaningfully dents a pool a user
/// may need for real work a minute later, and large enough that a pool
/// already down to single digits — the thinnest free tiers this project has
/// measured — is still refused rather than walked down to zero one `--probe`
/// at a time. [`ProbeBudget`]'s own materiality check also floors the
/// threshold at 2 requests, so a percentage this small never rounds down to
/// "anything at all is fine" against a pool of one or two.
pub const PROBE_BUDGET_FRACTION: f64 = 0.10;

/// What a `--probe` would cost, against what a provider's own pool has left.
///
/// Built only when [`authorize_probe`] has a **known** remainder to compare
/// against — an unknown, unmeasured, or non-request-pool resource (a
/// metered/token-priced provider, in this module's own vocabulary) never
/// produces one, which is what keeps [`ProbeAuthorization::Allowed`] the
/// answer for every one of those shapes without a special case for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeBudget {
    pub remaining: u32,
    pub cost: u32,
}

impl ProbeBudget {
    /// Whether spending `cost` out of `remaining` is a material fraction of
    /// what is left — capability map line 1369's own phrase.
    fn is_material(&self) -> bool {
        let threshold = (f64::from(self.remaining) * PROBE_BUDGET_FRACTION).ceil() as u32;
        self.cost >= threshold.max(2)
    }
}

/// What a `--probe <name>` costs: one request for the connectivity read
/// every provider gets, plus a second when `provider` declares a usage
/// endpoint — capability map line 1230's own extra request, and the same
/// fact [`usage_probe`] answers as "does this provider get a second
/// request".
pub fn probe_cost(provider: &crate::provider::Provider) -> u32 {
    1 + u32::from(crate::provider::usage_endpoint(&provider.name).is_some())
}

/// Capability map line 1369's decision: whether a `--probe <name>` should
/// fire, or whether it would spend a material fraction of what the
/// provider's own pool has left.
#[derive(Debug, Clone, Copy)]
pub enum ProbeAuthorization {
    /// Nothing stops the probe: the provider is unconfigured (`probe_provider`
    /// reports that on its own), its pool has never been measured, or it is
    /// not a request-pool resource at all (a metered/token-priced provider).
    Allowed,
    /// Refused: spending `budget.cost` requests would be a material fraction
    /// of the `budget.remaining` this provider's pool has left.
    Refused(ProbeBudget),
}

/// Whether `--probe <name>` should be allowed to fire — capability map
/// line 1369, run before [`probe_provider`] rather than inside it, so a
/// refusal never opens a socket.
///
/// This reads [`observed_capacity`] for `name`'s own
/// [`ResourceKind::DirectProvider`], whose `requests().remaining()` is
/// `Some` only when a real response's rate-limit headers stated one. No new
/// network path and no new credential resolution — this asks the same cache
/// the report already reads, keyed by provider name, not by credential, so a
/// request-pool reading here is provider-wide by construction — the same
/// granularity `--probe <name>` itself already probes at.
///
/// An unconfigured provider, a pool nothing has measured, and a
/// non-request-pool resource all answer [`ProbeAuthorization::Allowed`]:
/// there is no remainder to spend down in any of those three cases.
// History: design-decisions.md, "Trims: provider module docs", resources/mod.rs `authorize_probe` doc.
pub fn authorize_probe(
    effective: &EffectiveConfig<'_>,
    telemetry: &GatheredTelemetry,
    name: &str,
    now_unix: i64,
) -> ProbeAuthorization {
    let Ok(provider) = effective.configured_provider(name) else {
        return ProbeAuthorization::Allowed;
    };
    let kind = ResourceKind::from_direct_provider(provider.value.name.clone());
    let state = observed_capacity(&kind, effective, telemetry, now_unix);
    let Some(remaining) = state.requests().remaining().reading() else {
        return ProbeAuthorization::Allowed;
    };
    let Some(remaining) = u32::try_from(remaining.value().value()).ok() else {
        return ProbeAuthorization::Allowed;
    };
    let budget = ProbeBudget {
        remaining,
        cost: probe_cost(&provider.value),
    };
    if budget.is_material() {
        ProbeAuthorization::Refused(budget)
    } else {
        ProbeAuthorization::Allowed
    }
}

/// What one `--probe` produced, for a caller to report.
///
/// A named type rather than a tuple because the *absence* of headers is the
/// interesting answer — it was the answer for seven of eight hosts measured —
/// and a bare empty [`RateLimitHeaders`] beside a probe outcome does not say
/// whether the request even happened.
#[derive(Debug, Clone)]
pub enum ProbeReading {
    /// The provider is not configured, or declares nowhere to send a request.
    NotProbeable { reason: String },
    /// Capability map line 1369: [`authorize_probe`] refused to spend a
    /// material fraction of the provider's own remaining request pool.
    /// Carries no credential — only what [`ProbeBudget`] already stated.
    Refused { remaining: u32, cost: u32 },
    /// A request was made. `headers` is empty when it carried no rate-limit
    /// header this reader understands, which is a finding about the provider
    /// and not a failure.
    Answered {
        outcome: crate::provider::discovery::ProbeOutcome,
        headers: RateLimitHeaders,
        observed_at_unix: i64,
        /// A second, separate request to `provider`'s own usage endpoint —
        /// capability map line 1230 — when
        /// [`crate::provider::usage_endpoint`] names one for it. `None` for
        /// every other provider, and for one that declares an endpoint but
        /// answered something this reader could not read a body from.
        usage: Option<Box<crate::provider::telemetry::ProviderUsage>>,
    },
}

/// Probe one configured provider and read its rate-limit headers —
/// capability map line 1229 — and, when it declares one, its own usage
/// endpoint — capability map line 1230.
///
/// # It cannot fail the caller
///
/// Every failure is a [`ProbeReading`] variant, never an `Err`: an unknown
/// provider, an unreachable host, a timeout and a refusal all produce
/// something printable. Capability map line 1238 is a property of this
/// signature.
pub fn probe_provider(
    effective: &EffectiveConfig<'_>,
    secrets: &dyn crate::secret::SecretStore,
    name: &str,
    now_unix: i64,
) -> ProbeReading {
    let Ok(provider) = effective.configured_provider(name) else {
        return ProbeReading::NotProbeable {
            reason: format!(
                "`{name}` is not a configured provider; `glasshouse doctor` lists the ones that are"
            ),
        };
    };
    let Some(request) = telemetry_probe(&provider.value, secrets) else {
        return ProbeReading::NotProbeable {
            reason: format!("`{name}` declares no base URL to send a request to"),
        };
    };
    let response = crate::provider::discovery::connectivity_with_headers(
        &request,
        crate::provider::discovery::ProbeTimeouts::default(),
    );

    let usage = usage_probe(&provider.value, secrets).and_then(|request| {
        match crate::provider::discovery::read_response_body(
            &request,
            crate::provider::discovery::ProbeTimeouts::default(),
        ) {
            crate::provider::discovery::BodyFetch::Answered { body, .. } => Some(Box::new(
                crate::provider::telemetry::ProviderUsage::read(&body),
            )),
            // Refused, unreachable, timed out, or answered with a body this
            // reader could not get whole: a fact about this account or this
            // moment, not a reading to invent.
            crate::provider::discovery::BodyFetch::NotRead { .. }
            | crate::provider::discovery::BodyFetch::Probe(_) => None,
        }
    });

    ProbeReading::Answered {
        outcome: response.outcome().clone(),
        headers: response.rate_limits(),
        observed_at_unix: now_unix,
        usage,
    }
}

/// Render what a `--probe` found, above the report itself.
///
/// Names the URL that was asked and what came back, because "no rate-limit
/// headers" is only a useful answer if the reader can see which request it is
/// an answer about.
pub fn render_probe(out: &mut String, name: &str, reading: &ProbeReading) {
    match reading {
        ProbeReading::NotProbeable { reason } => {
            let _ = writeln!(out, "  {name}: not probed — {reason}");
        }
        ProbeReading::Refused { remaining, cost } => {
            let _ = writeln!(
                out,
                "  glasshouse: not probing {name}: {remaining} request(s) remain in its pool \
                 and this probe would spend {cost}; pass --force to spend them."
            );
        }
        ProbeReading::Answered {
            outcome,
            headers,
            observed_at_unix,
            usage,
        } => {
            let _ = writeln!(
                out,
                "  {name}: {}",
                crate::profile::describe_probe_outcome(outcome)
            );
            if headers.is_empty() {
                let _ = writeln!(
                    out,
                    "    no rate-limit header this reader understands. That is a fact about the \
                     provider, not a failure: seven of the eight hosts Glasshouse ships templates \
                     for sent none on this route."
                );
            } else {
                let _ = writeln!(
                    out,
                    "    read {} from: {}",
                    headers.read_from().len(),
                    headers.read_from().join(", ")
                );
            }
            if let Some(usage) = usage {
                render_usage_probe(out, usage, *observed_at_unix);
            }
        }
    }
}

/// The line `--force` prints when it overrides a refusal
/// [`authorize_probe`] would otherwise have made — capability map line
/// 1369's override, stated rather than spent silently.
pub fn render_forced_probe(out: &mut String, name: &str, budget: &ProbeBudget) {
    let _ = writeln!(
        out,
        "  glasshouse: probing {name} anyway: spending {} of {} request(s) left in its pool.",
        budget.cost, budget.remaining
    );
}

/// Capability map line 1230's own line of a `--probe` report.
fn render_usage_probe(
    out: &mut String,
    usage: &crate::provider::telemetry::ProviderUsage,
    observed_at_unix: i64,
) {
    if usage.is_empty() {
        let _ = writeln!(
            out,
            "    usage endpoint: answered, but this reader found none of `data.limit`, \
             `data.limit_remaining` or `data.limit_reset` in the body"
        );
        return;
    }
    let state = crate::provider::telemetry::apply_provider_usage(
        CapacityState::metered_balance(),
        usage,
        observed_at_unix,
    );
    let _ = writeln!(
        out,
        "    usage endpoint: limit {}, remaining {}",
        describe_amount(
            state.credits().limit(),
            QuotaStaleAfterSeconds::DEFAULT,
            ReportOptions {
                verbose: false,
                now_unix: observed_at_unix,
            }
        ),
        describe_amount(
            state.credits().remaining(),
            QuotaStaleAfterSeconds::DEFAULT,
            ReportOptions {
                verbose: false,
                now_unix: observed_at_unix,
            }
        )
    );
    // Line 1217/1218: if this account's own credits pool ever answers with
    // both halves — a real, non-null ceiling and a remaining figure, which
    // no account probed for this package has — this is what a live
    // `Percentage::Exact` from a usage endpoint looks like.
    if let Some(score) = state.credits().normalized() {
        let _ = writeln!(
            out,
            "    usage endpoint capacity: {}",
            score.percent().render()
        );
    }
}

#[cfg(test)]
mod tests;
