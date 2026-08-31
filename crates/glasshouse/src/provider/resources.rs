//! Phase 32B: `glasshouse resources` — what Glasshouse believes about every
//! model resource it can describe, and **where each belief came from**.
//!
//! # Why this command exists at all
//!
//! Phase 32 built [`mod@crate::provider::registry`] and recorded, in its own
//! evidence ledger, that `registry()` had no production caller: *"Nothing in
//! the shipped binary currently prints 'here is everything Glasshouse can
//! describe' to a user."* Phase 32A built [`mod@crate::provider::quota`] and
//! recorded the same limit one layer down — the launch path reads exactly one
//! projection out of the capacity model, its quota *shape*, and every pool,
//! window and rate ceiling below that was proven only by tests.
//!
//! Both were right to say so, and both were pointing at the same missing
//! thing: a surface that reads the model. This is it, and it is modelled on
//! `glasshouse pairing` and `glasshouse response` — read-only, one screen,
//! reports what Glasshouse believes rather than deciding anything, and makes
//! no network request unless asked.
//!
//! # The one rule this whole surface exists to obey
//!
//! Capability map line 1240 asks that the telemetry source be surfaced, and
//! line 1234 that an inferred percentage never be labelled exact. Neither is
//! enforced by this module's care. Every number printed here arrives as a
//! [`Capacity`], whose [`Capacity::telemetry_class_str`] answers
//! [`crate::provider::quota::UNKNOWN_TELEMETRY`] when nothing was read, and
//! every percentage arrives as a [`crate::provider::quota::Percentage`],
//! whose only rendering path marks an estimate as one. This module cannot
//! print an unlabelled figure because it has no access to one.
//!
//! # What it reads, and what it costs
//!
//! Without `--probe` it makes **no network request**: it reads the user's
//! configuration and — because it is a local process invocation costing about
//! a quarter of a second and no quota — each installed harness's own status
//! interface. With `--probe <provider>` it makes exactly one request, to a
//! provider the user has configured, and folds in whatever rate-limit headers
//! come back.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::config::{EffectiveConfig, Layer, QuotaStaleAfterSeconds};
use crate::integrations::{IntegrationId, IntegrationKind};
use crate::provider::quota::{
    Capacity, CapacityBandThresholds, CapacityState, Freshness, NativeAmount, Pool, TelemetryClass,
    UNKNOWN_TELEMETRY,
};
use crate::provider::registry::{ResourceKind, registry};
use crate::provider::telemetry::{
    GatewayHealthCache, GatewayHealthReading, GatewayQuotaCache, HarnessTelemetry,
    RateLimitHeaders, apply_harness_report, apply_provider_headers, apply_user_configuration,
    read_harness_plan,
};
use crate::routing::evidence::{EvidenceLedger, FailureClass, FailureClassCounts};

/// The status interface of a harness that has one, as a command line
/// Glasshouse constructs itself.
///
/// # This list is what the installed binaries actually offer, checked
///
/// Practice §5's rule — *check a declaration against the use, not the claim*
/// — and the reason this project has been wrong about a harness's declared
/// surface five times. Checked on 2026-08-27 against the binaries installed
/// on this machine:
///
/// - **Claude Code** — `claude auth status --json`. `--json` is listed in
///   `claude auth status --help` as the **default** output, which is as
///   stable a declaration as a CLI gives. It emits a small object whose
///   `subscriptionType` names the plan.
/// - **Codex** — `codex doctor --json` exists and is stamped
///   `"schemaVersion": 1`, so it is genuinely stable and machine-readable.
///   It carries **no** usage, quota, limit, credit, remaining, reset, plan,
///   window or balance field: twenty-three checks about installation, auth
///   configuration, network reachability and disk. It is not a usage
///   interface and is deliberately absent from this list rather than
///   listed-and-parsed-for-nothing.
/// - **Antigravity** — the `agy` binary's `--help` lists no status or usage
///   subcommand at all.
/// - **Cursor CLI** — likewise.
///
/// So one harness of four exposes machine-readable status, and what it
/// exposes is a **plan and not a usage figure**. That is capability map line
/// 1231's *"or status information"* clause and not its *"usage"* clause, and
/// the evidence ledger says so rather than letting the list imply more.
///
/// # Why the arguments live here and not on the harness adapter
///
/// They should live on the adapter. [`IntegrationId::executable_candidates`]
/// argues exactly this about the executable *name* — *"keeping a second copy
/// here would be a second place for it to be wrong, and the two would
/// drift"* — and a status command is the same kind of fact. The name is not
/// duplicated: [`harness_status_command`] resolves it through
/// [`IntegrationId::executable_candidates`], so only the **arguments** are
/// here. `crates/glasshouse/src/harness/**` is outside this package's
/// partition; see the report for the two-line trait method this wants to be.
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
    /// exactly as they do to a probed one — there is no second code path for
    /// this source to disagree with the first through.
    ///
    /// **Not yet called from `glasshouse resources`.** The caller this
    /// method exists for is `main.rs::resources_report`, which this
    /// package's `FORBIDDEN FILES` does not let it reach — see the report
    /// for the one line that call site needs. Tests exercise this method
    /// directly, which is what proves the model side of the bridge without
    /// claiming the production reach it does not yet have (practice §35).
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

    fn for_harness(&self, harness: IntegrationId) -> Option<&HarnessTelemetry> {
        self.harness.get(harness.slug())
    }

    fn for_provider(&self, provider: &str) -> Option<&(RateLimitHeaders, i64)> {
        self.providers.get(provider)
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
}

/// The capacity Glasshouse believes `kind` has, after every reading that
/// applies to it has been folded in — the function every box in this phase
/// ultimately closes through.
///
/// # The order is capability map line 1228, and it is not cosmetic
///
/// Configuration first, then the harness's own report, then the provider's
/// own headers — weakest source applied first, strongest last — because
/// [`Capacity::prefer`] resolves each collision in favour of the more
/// authoritative claim regardless of order, and applying them in this order
/// means the *stale* case behaves the same way: a fresh manual entry never
/// displaces a provider's own word, only fills a gap it left.
///
/// # Line 1238, structurally
///
/// Every step is total. A missing reading, an unparseable one, a harness that
/// is not installed and a provider that answered no headers all leave the
/// state exactly as the previous step left it, and the worst case is the
/// state [`CapacityState::for_resource`] built with nothing read at all —
/// which is a complete, printable answer. There is no path through this
/// function that yields an error for a caller to fail a session on.
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

    render_configuration_note(&mut out, effective);
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
fn render_configuration_note(out: &mut String, effective: &EffectiveConfig<'_>) {
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
            let _ = writeln!(
                out,
                "  {name}: budget {}.{:06} USD per {} ({layer}) — Glasshouse does not count \
                 spend against this",
                budget.amount_micro_usd() / 1_000_000,
                budget.amount_micro_usd() % 1_000_000,
                budget.period().as_str()
            );
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
mod tests {
    use super::*;
    use crate::config::{
        BudgetPeriod, MonetaryBudget, PremiumReservePercent, ProjectConfig, ProviderConfig,
        QuotaOverride, UserConfig,
    };
    use crate::provider::quota::{Capacity, NativeAmount, RateCeilings, Reading, ReadingSource};
    use crate::provider::telemetry::{GatewayQuotaCache, RateLimitHeaders};

    const OBSERVED: i64 = 1_787_800_000;
    const NOW: i64 = OBSERVED + 30;

    /// The header set `https://anyrouter.dev/api/v1/models` really answered
    /// with on 2026-08-27 — see `provider::telemetry`'s own fixture note.
    fn anyrouter_headers() -> RateLimitHeaders {
        RateLimitHeaders::read(vec![
            ("ratelimit-limit", "300"),
            ("ratelimit-policy", "300;w=60"),
            ("x-ratelimit-limit", "300"),
            ("x-ratelimit-tier", "ip"),
            ("x-ratelimit-window", "60"),
        ])
    }

    fn user_with_anyrouter(quota: QuotaOverride) -> UserConfig {
        let mut user = UserConfig::default();
        let mut provider = ProviderConfig::new("anyrouter");
        provider.set_quota(Some(quota));
        user.providers_mut().set("anyrouter", provider);
        user
    }

    fn options() -> ReportOptions {
        ReportOptions {
            verbose: false,
            now_unix: NOW,
        }
    }

    fn anyrouter_row(rendered: &str) -> String {
        rendered
            .split("\n\n")
            .find(|block| block.starts_with("anyrouter"))
            .unwrap_or_else(|| panic!("no anyrouter block in:\n{rendered}"))
            .to_owned()
    }

    // --- line 1229, through the function `main.rs` calls -------------------

    #[test]
    fn a_providers_own_rate_limit_header_reaches_the_report_as_authoritative() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let telemetry = GatheredTelemetry::new().with_provider_headers(
            "anyrouter",
            anyrouter_headers(),
            OBSERVED,
        );

        let rendered = report(&effective, &telemetry, options());
        let row = anyrouter_row(&rendered);

        assert!(row.contains("300 requests"), "{row}");
        assert!(row.contains("authoritative"), "{row}");
        assert!(row.contains("`ratelimit-limit` response header"), "{row}");
        // Line 1236: the observation is dated.
        assert!(row.contains(&format!("unix {OBSERVED}")), "{row}");
    }

    /// Capability map line 1200, at the report level: AnyRouter's own real
    /// header — measured, not invented — evidences that this resource really
    /// is request-limited, and `describe_limits`' "limited by" line now says
    /// so instead of only naming the shape's default (credits).
    #[test]
    fn a_request_ceiling_a_reader_measured_reaches_the_limited_by_line() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let before = anyrouter_row(&report(&effective, &GatheredTelemetry::new(), options()));
        assert_eq!(before.lines().nth(3), Some("  limited by      credits"));

        let telemetry = GatheredTelemetry::new().with_provider_headers(
            "anyrouter",
            anyrouter_headers(),
            OBSERVED,
        );
        let after = anyrouter_row(&report(&effective, &telemetry, options()));
        assert_eq!(
            after.lines().nth(3),
            Some("  limited by      requests, credits")
        );
    }

    /// The half of line 1229 that is about restraint. AnyRouter advertises
    /// `RateLimit-Remaining` and does not send it; a report that showed a
    /// remaining count anyway would be inventing one.
    #[test]
    fn a_count_the_provider_did_not_send_stays_unknown_in_the_report() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let telemetry = GatheredTelemetry::new().with_provider_headers(
            "anyrouter",
            anyrouter_headers(),
            OBSERVED,
        );
        let row = anyrouter_row(&report(&effective, &telemetry, options()));

        assert!(row.contains("remaining unmeasured (unknown)"), "{row}");
        // And with no both-halves reading there is no percentage at all.
        assert!(row.contains("capacity        unknown"), "{row}");
    }

    /// Capability map lines 1210 and 1211: the caller `render_windows` gives
    /// them. A reset field reads into `windows().rolling()` — proven at the
    /// model level by `provider::telemetry`'s own tests — and this is the
    /// production rendering path finally reading that field back out, which
    /// nothing did before this package.
    #[test]
    fn a_window_reset_a_reader_supplied_reaches_the_report() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let headers = RateLimitHeaders::read(vec![("ratelimit-reset", "30")]);
        let telemetry =
            GatheredTelemetry::new().with_provider_headers("anyrouter", headers, OBSERVED);
        let row = anyrouter_row(&report(&effective, &telemetry, options()));

        assert!(row.contains("rolling window"), "{row}");
        assert!(
            row.contains(&format!("resets unix {}", OBSERVED + 30)),
            "{row}"
        );
        // No window *start* has ever been observed anywhere — line 1210
        // stays open on its own honest terms, and the report says so rather
        // than inventing one.
        assert!(row.contains("starts unmeasured (unknown)"), "{row}");
    }

    // --- BRIDGE-QUOTA: a gateway-captured reading, folded in ---------------

    /// Capability map lines 1217/1218's antecedent, reached at the actual
    /// rendering function `main.rs::resources_report` calls, for the first
    /// time in this phase's history.
    ///
    /// **The numbers are synthetic and say so.** No live host `--probe` can
    /// reach has ever sent both halves of a pool in one unit — AnyRouter's
    /// own real header carries a limit and no remaining count at all (see
    /// `anyrouter_headers`, above); Groq's real inference response does
    /// carry both, proven at the model level by `provider::telemetry`'s
    /// `groqs_reading_produces_a_real_exact_percentage_from_the_model_alone`,
    /// but Groq is not a provider this build ships a registry template for,
    /// so it cannot appear in `report`'s own registry loop. This test uses
    /// AnyRouter's real provider slug with an invented remaining count, to
    /// isolate what this package can actually prove: that a reading reaching
    /// [`GatewayQuotaCache`] reaches this unmodified rendering function and
    /// produces a real, correctly-labelled percentage — not that a live host
    /// has ever supplied one over this seam.
    ///
    /// This is [`GatheredTelemetry::gather_gateway_quota`]'s only production-
    /// shaped proof: nothing in the shipped binary calls it yet (see the
    /// package report for the one line `main.rs::resources_report` needs),
    /// so this cannot claim §35's production-caller mutation the way
    /// `a_providers_own_rate_limit_header_reaches_the_report_as_authoritative`
    /// can for `--probe`. What it proves is narrower and still real: *if*
    /// that line existed, this is exactly what a user would see.
    #[test]
    fn a_persisted_gateway_reading_reaches_the_rendered_report() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayQuotaCache::at(dir.path());
        cache.store(
            "anyrouter",
            &RateLimitHeaders::read(vec![
                ("ratelimit-limit", "300"),
                ("ratelimit-remaining", "297"),
            ]),
            OBSERVED,
        );

        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let telemetry = GatheredTelemetry::new().gather_gateway_quota(&cache);
        let row = anyrouter_row(&report(&effective, &telemetry, options()));

        assert!(
            row.contains("capacity        99%"),
            "a reading with both halves must render a real, unlabelled-as-estimate \
             percentage: {row}"
        );
        assert!(
            !row.contains("capacity        unknown"),
            "the gathered reading must have reached the report: {row}"
        );
    }

    // --- Phase 32D/32F, through the same production rendering function ----

    /// Capability map lines 1259-1268, at the actual rendering function
    /// `main.rs::resources_report` calls: a real gateway-captured reading
    /// produces a real band line, not just a percentage.
    #[test]
    fn a_persisted_gateway_reading_reaches_the_rendered_band_line() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayQuotaCache::at(dir.path());
        cache.store(
            "anyrouter",
            &RateLimitHeaders::read(vec![
                ("ratelimit-limit", "300"),
                ("ratelimit-remaining", "297"),
            ]),
            OBSERVED,
        );

        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let telemetry = GatheredTelemetry::new().gather_gateway_quota(&cache);
        let row = anyrouter_row(&report(&effective, &telemetry, options()));

        assert!(
            row.contains("band            plenty"),
            "297/300 = 99% must fall in the default Plenty band: {row}"
        );
        assert!(row.contains("bound by requests"), "{row}");
    }

    /// §35: mutate the call, not the callee. Deleting `render_capacity_band`'s
    /// call inside `render_resource` must make a named test go red — proven
    /// here by asserting the line's own presence, so the mutation described
    /// in the package report (removing that one call) fails exactly this
    /// test rather than only a lower-level unit test that enters below the
    /// production rendering path.
    #[test]
    fn the_band_line_is_present_for_every_registry_resource_even_with_no_score() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let rendered = report(&effective, &GatheredTelemetry::new(), options());
        let resource_blocks = rendered.split("\n\n").filter(|block| {
            block
                .lines()
                .any(|line| line.trim_start().starts_with("quota shape"))
        });
        let mut checked = 0usize;
        for block in resource_blocks {
            assert!(
                block
                    .lines()
                    .any(|line| line.trim_start().starts_with("band")),
                "every resource block must print a band line, even `unknown`: {block}"
            );
            checked += 1;
        }
        assert!(
            checked > 0,
            "expected at least one resource block in:\n{rendered}"
        );
    }

    /// Capability map line 1288: a provider's own protected reserve
    /// percentage — configured, not hardcoded — narrows where the Reserve
    /// band begins for that resource specifically, reached through the same
    /// `EffectiveConfig` every other quota field in this file already goes
    /// through.
    #[test]
    fn a_providers_own_reserve_percentage_narrows_its_reserve_band() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayQuotaCache::at(dir.path());
        // 40% remaining: Plenty by the global defaults (>= 70% is Plenty,
        // but 40% falls in Healthy — see CapacityBandThresholds::DEFAULT),
        // and Reserve once this provider protects 50% of its own capacity.
        cache.store(
            "anyrouter",
            &RateLimitHeaders::read(vec![
                ("ratelimit-limit", "100"),
                ("ratelimit-remaining", "40"),
            ]),
            OBSERVED,
        );
        let telemetry = GatheredTelemetry::new().gather_gateway_quota(&cache);

        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let without_override = anyrouter_row(&report(&effective, &telemetry, options()));
        assert!(
            without_override.contains("band            healthy"),
            "{without_override}"
        );

        let mut quota = QuotaOverride::default();
        quota.set_reserve_percent(Some(PremiumReservePercent::try_from(50u16).unwrap()));
        let user = user_with_anyrouter(quota);
        let effective = EffectiveConfig::new(&user, None);
        let with_override = anyrouter_row(&report(&effective, &telemetry, options()));
        assert!(
            with_override.contains("band            reserve"),
            "a 50% protected reserve must move a 40%-remaining resource into Reserve: \
             {with_override}"
        );
    }

    /// The negative half: a cache with nothing in it changes nothing, so
    /// `gather_gateway_quota` is safe to fold in unconditionally rather than
    /// only when a gateway happens to have run.
    #[test]
    fn an_empty_gateway_quota_cache_leaves_the_report_exactly_as_before() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayQuotaCache::at(dir.path());

        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let without = report(&effective, &GatheredTelemetry::new(), options());
        let with = report(
            &effective,
            &GatheredTelemetry::new().gather_gateway_quota(&cache),
            options(),
        );
        assert_eq!(without, with);
    }

    // --- capability map lines 1311/1321/1322/1324: resource health --------

    fn health_reading(
        model: &str,
        consecutive_failures: u32,
        cooling_down_until_unix: Option<i64>,
        credential_rejected: bool,
    ) -> crate::provider::telemetry::GatewayHealthReading {
        crate::provider::telemetry::GatewayHealthReading {
            credential_label: "anyrouter/ANYROUTER_API_KEY".to_owned(),
            model: model.to_owned(),
            consecutive_failures,
            cooling_down_until_unix,
            credential_rejected,
        }
    }

    /// Capability map line 1324's "never invent a reading" half: a resource
    /// nothing has been observed about reports `unknown`, never a number and
    /// never a fabricated "available".
    #[test]
    fn a_resource_with_no_health_observation_reports_unknown() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let rendered = report(&effective, &GatheredTelemetry::new(), options());
        let row = anyrouter_row(&rendered);
        assert!(
            row.contains(&format!("health          {UNKNOWN_TELEMETRY}")),
            "{row}"
        );
        assert!(!row.contains("paced"), "{row}");
        assert!(!row.contains("consecutive failure"), "{row}");
    }

    /// Capability map line 1324's own point: a resource cooling down after
    /// real failures is **paced**, not broken — the property a test that
    /// cannot tell "unknown" from "cooling" from "available" would miss
    /// entirely.
    #[test]
    fn a_cooling_down_resource_is_shown_as_paced_not_broken() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayHealthCache::at(dir.path());
        cache.store(
            "anyrouter",
            &[health_reading(
                "anyrouter/free-model",
                3,
                Some(NOW + 120),
                false,
            )],
            OBSERVED,
        );
        let telemetry = GatheredTelemetry::new().gather_gateway_health(&cache);

        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let row = anyrouter_row(&report(&effective, &telemetry, options()));

        assert!(
            row.contains("health          anyrouter/free-model")
                && row.contains("paced, cooling down until unix"),
            "a resource still cooling down must render as paced: {row}"
        );
        assert!(
            row.contains("3 consecutive failure(s)"),
            "the observed failure count must reach the report: {row}"
        );
        assert!(
            !row.contains(&format!("health          {UNKNOWN_TELEMETRY}")),
            "a real observation must not render as unknown: {row}"
        );
        assert!(
            !row.contains("credential rejected"),
            "a cooldown is not a credential rejection: {row}"
        );
    }

    /// The other side of the same property: once a cooldown has elapsed by
    /// the report's own `now_unix`, the resource reads as available again —
    /// without a fresh observation, exactly
    /// [`crate::provider::telemetry::GatewayHealthReading::is_available`]'s
    /// own contract.
    #[test]
    fn an_elapsed_cooldown_reads_as_available_again() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayHealthCache::at(dir.path());
        cache.store(
            "anyrouter",
            &[health_reading(
                "anyrouter/free-model",
                2,
                Some(NOW - 1),
                false,
            )],
            OBSERVED,
        );
        let telemetry = GatheredTelemetry::new().gather_gateway_health(&cache);

        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let row = anyrouter_row(&report(&effective, &telemetry, options()));

        assert!(
            row.contains(
                "health          anyrouter/free-model (anyrouter/ANYROUTER_API_KEY): \
                          available"
            ),
            "{row}"
        );
        assert!(!row.contains("paced"), "{row}");
    }

    /// A credential Glasshouse's own gateway had to reject reports that fact
    /// distinctly from a cooldown — waiting does not fix a revoked key.
    #[test]
    fn a_rejected_credential_is_shown_as_rejected_not_paced() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayHealthCache::at(dir.path());
        cache.store(
            "anyrouter",
            &[health_reading("anyrouter/free-model", 1, None, true)],
            OBSERVED,
        );
        let telemetry = GatheredTelemetry::new().gather_gateway_health(&cache);

        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let row = anyrouter_row(&report(&effective, &telemetry, options()));

        assert!(row.contains("credential rejected"), "{row}");
        assert!(!row.contains("paced"), "{row}");
    }

    /// A corrupt cache file must leave `glasshouse resources` working and
    /// simply carry no health — [`GatewayHealthCache::load`]'s own fail-soft
    /// contract, proven at the rendering function it feeds.
    #[test]
    fn a_corrupt_health_cache_file_leaves_the_report_working_with_no_health() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayHealthCache::at(dir.path());
        cache.store(
            "anyrouter",
            &[health_reading("anyrouter/free-model", 1, None, false)],
            OBSERVED,
        );
        let path = dir.path().join(format!(
            "{}.json",
            crate::provider::cache::file_stem("anyrouter")
        ));
        std::fs::write(&path, b"not json").expect("overwritten with garbage");

        let telemetry = GatheredTelemetry::new().gather_gateway_health(&cache);
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let row = anyrouter_row(&report(&effective, &telemetry, options()));

        assert!(
            row.contains(&format!("health          {UNKNOWN_TELEMETRY}")),
            "a corrupt cache file must read as no health, not fail the report: {row}"
        );
    }

    /// [`capacity_json`]'s own twin: health rides beside capacity as a
    /// separate field, never merged into it, and an unobserved resource's
    /// list is empty rather than absent or fabricated.
    #[test]
    fn capacity_json_carries_health_separately_from_capacity() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayHealthCache::at(dir.path());
        cache.store(
            "anyrouter",
            &[health_reading(
                "anyrouter/free-model",
                2,
                Some(NOW + 60),
                false,
            )],
            OBSERVED,
        );
        let telemetry = GatheredTelemetry::new().gather_gateway_health(&cache);

        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let json = capacity_json(&effective, &telemetry, NOW);
        let anyrouter = json["resources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["resource"].as_str().unwrap().starts_with("anyrouter"))
            .expect("an anyrouter entry");

        let health = anyrouter["health"].as_array().expect("a health array");
        assert_eq!(health.len(), 1);
        assert_eq!(health[0]["model"], "anyrouter/free-model");
        assert_eq!(health[0]["consecutive_failures"], 2);
        assert_eq!(health[0]["available"], false);

        let gateway_kind = json["resources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["resource"] == "glasshouse gateway")
            .expect("the glasshouse gateway entry");
        assert_eq!(
            gateway_kind["health"].as_array().expect("a health array"),
            &Vec::<serde_json::Value>::new(),
            "a resource kind health cannot apply to must carry an empty list, never null"
        );
    }

    /// An explicit `--probe`'s own reading, folded in after
    /// `gather_gateway_quota`, still wins — a live probe the user just ran is
    /// never staled out by whatever a gateway happened to persist earlier,
    /// mirroring `Capacity::prefer`'s own freshness rule at the seam these
    /// two sources actually meet in `main.rs::resources_report`.
    #[test]
    fn an_explicit_probe_reading_overrides_a_persisted_gateway_one_for_the_same_provider() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = GatewayQuotaCache::at(dir.path());
        cache.store(
            "anyrouter",
            &RateLimitHeaders::read(vec![("ratelimit-limit", "300")]),
            OBSERVED,
        );

        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let telemetry = GatheredTelemetry::new()
            .gather_gateway_quota(&cache)
            .with_provider_headers(
                "anyrouter",
                RateLimitHeaders::read(vec![("ratelimit-limit", "150")]),
                OBSERVED + 60,
            );
        let row = anyrouter_row(&report(&effective, &telemetry, options()));
        assert!(row.contains("150 requests"), "{row}");
        assert!(!row.contains("300 requests"), "{row}");
    }

    /// The quiet view's own rule, extended to windows: nothing measured means
    /// nothing shown, and `--verbose` is what surfaces the unmeasured rows.
    #[test]
    fn a_resource_with_no_window_reading_shows_no_window_row_unless_verbose() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let quiet = anyrouter_row(&report(&effective, &GatheredTelemetry::new(), options()));
        assert!(!quiet.contains("rolling window"), "{quiet}");
        assert!(!quiet.contains("calendar window"), "{quiet}");

        let verbose = anyrouter_row(&report(
            &effective,
            &GatheredTelemetry::new(),
            ReportOptions {
                verbose: true,
                now_unix: NOW,
            },
        ));
        assert!(verbose.contains("rolling window"), "{verbose}");
        assert!(verbose.contains("calendar window"), "{verbose}");
    }

    // --- lines 1233 and 1203: what the user entered ------------------------

    #[test]
    fn a_configured_plan_and_budget_reach_the_report_marked_manual() {
        let mut quota = QuotaOverride::default();
        quota.set_plan(Some("free-tier".to_owned()));
        quota.set_budget(Some(
            MonetaryBudget::new(10_000_000, BudgetPeriod::CalendarMonth).unwrap(),
        ));
        let user = user_with_anyrouter(quota);
        let effective = EffectiveConfig::new(&user, None);
        let rendered = report(&effective, &GatheredTelemetry::new(), options());

        let row = anyrouter_row(&rendered);
        assert!(row.contains("free-tier [manual]"), "{row}");
        assert!(row.contains("the user's own configuration"), "{row}");

        // And the note that tells a user this is a thing they can do.
        assert!(
            rendered.contains("anyrouter: plan `free-tier` (user)"),
            "{rendered}"
        );
        assert!(
            rendered.contains("10.000000 USD per calendar month"),
            "{rendered}"
        );
        // Stated rather than implied: the ceiling is known, the spend is not.
        assert!(
            rendered.contains("does not count \n                 spend against this")
                || rendered.contains("does not count"),
            "{rendered}"
        );
    }

    #[test]
    fn a_project_layer_quota_table_replaces_the_users_and_says_which_layer_won() {
        let mut user_quota = QuotaOverride::default();
        user_quota.set_plan(Some("user-plan".to_owned()));
        let user = user_with_anyrouter(user_quota);

        let mut project = ProjectConfig::default();
        let mut project_provider = ProviderConfig::new("anyrouter");
        let mut project_quota = QuotaOverride::default();
        project_quota.set_plan(Some("project-plan".to_owned()));
        project_provider.set_quota(Some(project_quota));
        project.providers_mut().set("anyrouter", project_provider);

        let effective = EffectiveConfig::new(&user, Some(&project));
        let rendered = report(&effective, &GatheredTelemetry::new(), options());
        assert!(
            rendered.contains("anyrouter: plan `project-plan` (project)"),
            "{rendered}"
        );
        assert!(!rendered.contains("user-plan"), "{rendered}");
    }

    // --- line 1237: provider-specific staleness ---------------------------

    /// Two providers, two configured ages, one reading each of the same age:
    /// one is stale and one is not. A single global age could not produce
    /// this, which is what makes it a test of "provider-specific" rather
    /// than of staleness in general.
    #[test]
    fn two_providers_with_different_configured_ages_disagree_about_the_same_reading() {
        let mut user = UserConfig::default();
        for (name, seconds) in [("anyrouter", 10_u32), ("openrouter", 3_600)] {
            let mut provider = ProviderConfig::new(name);
            let mut quota = QuotaOverride::default();
            quota.set_stale_after(Some(seconds.try_into().unwrap()));
            provider.set_quota(Some(quota));
            user.providers_mut().set(name, provider);
        }
        let effective = EffectiveConfig::new(&user, None);
        let telemetry = GatheredTelemetry::new()
            .with_provider_headers("anyrouter", anyrouter_headers(), OBSERVED)
            .with_provider_headers("openrouter", anyrouter_headers(), OBSERVED);

        let rendered = report(&effective, &telemetry, options());
        let anyrouter = anyrouter_row(&rendered);
        let openrouter = rendered
            .split("\n\n")
            .find(|block| block.starts_with("openrouter"))
            .expect("an openrouter block")
            .to_owned();

        // 30 seconds old, against a ten-second limit and an hour's.
        assert!(anyrouter.contains("stale:"), "{anyrouter}");
        assert!(!openrouter.contains("stale:"), "{openrouter}");
    }

    #[test]
    fn a_stale_reading_is_still_reported_rather_than_discarded() {
        let mut user = UserConfig::default();
        let mut provider = ProviderConfig::new("anyrouter");
        let mut quota = QuotaOverride::default();
        quota.set_stale_after(Some(10_u32.try_into().unwrap()));
        provider.set_quota(Some(quota));
        user.providers_mut().set("anyrouter", provider);

        let effective = EffectiveConfig::new(&user, None);
        let telemetry = GatheredTelemetry::new().with_provider_headers(
            "anyrouter",
            anyrouter_headers(),
            OBSERVED,
        );
        let state = observed_capacity(
            &ResourceKind::from_direct_provider("anyrouter"),
            &effective,
            &telemetry,
            NOW,
        );

        assert!(is_entirely_stale(
            &state,
            NOW,
            effective.quota_stale_after("anyrouter").value
        ));
        // Line 1238: stale is not gone. The number is still there.
        assert_eq!(
            state.requests().limit().value().map(NativeAmount::value),
            Some(300)
        );
        let row = anyrouter_row(&report(&effective, &telemetry, options()));
        assert!(row.contains("300 requests"), "{row}");
    }

    // --- line 1228, at the report level -----------------------------------

    #[test]
    fn a_providers_own_header_outranks_the_plan_a_user_typed_for_the_same_provider() {
        let mut quota = QuotaOverride::default();
        quota.set_plan(Some("free-tier".to_owned()));
        let user = user_with_anyrouter(quota);
        let effective = EffectiveConfig::new(&user, None);

        let manual_only = strongest_class(&effective, &GatheredTelemetry::new(), NOW);
        assert_eq!(manual_only, Some(TelemetryClass::Manual));

        let with_header = strongest_class(
            &effective,
            &GatheredTelemetry::new().with_provider_headers(
                "anyrouter",
                anyrouter_headers(),
                OBSERVED,
            ),
            NOW,
        );
        assert_eq!(with_header, Some(TelemetryClass::Authoritative));
    }

    // --- lines 1231 and 1232, at the report level --------------------------

    #[test]
    fn a_harness_report_reaches_the_subscription_row_and_no_provider_row() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let telemetry = GatheredTelemetry::new().with_harness(
            IntegrationId::ClaudeCode,
            crate::provider::telemetry::HarnessTelemetry::plan(
                "max",
                OBSERVED,
                "claude auth status --json",
            ),
        );
        let rendered = report(&effective, &telemetry, options());

        let claude = rendered
            .split("\n\n")
            .find(|block| block.starts_with("Claude Code subscription"))
            .expect("a Claude Code block")
            .to_owned();
        assert!(claude.contains("max [authoritative]"), "{claude}");
        assert!(claude.contains("claude auth status --json"), "{claude}");

        // Independence, line 1232: no other harness and no provider learned
        // anything from it.
        let codex = rendered
            .split("\n\n")
            .find(|block| block.starts_with("Codex subscription"))
            .expect("a Codex block");
        assert!(
            codex.contains("plan            unmeasured (unknown)"),
            "{codex}"
        );
    }

    // --- line 1240 / map line 1761: the debug view -------------------------

    /// Every resource names the class of claim its knowledge rests on,
    /// including the ones where the answer is `unknown`. Driven over the
    /// whole registry rather than a sample, so a resource kind added later
    /// fails here instead of silently rendering without a source.
    #[test]
    fn every_resource_in_the_report_names_its_telemetry_class() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let rendered = report(&effective, &GatheredTelemetry::new(), options());

        for kind in registry() {
            let block = rendered
                .split("\n\n")
                .find(|block| block.starts_with(&kind.label()))
                .unwrap_or_else(|| panic!("no block for `{}`:\n{rendered}", kind.label()));
            assert!(
                block.contains("telemetry       "),
                "`{}` names no telemetry class",
                kind.label()
            );
        }
        // With nothing read at all, every one of them says so.
        assert_eq!(
            rendered.matches("telemetry       unknown").count(),
            registry().len()
        );
    }

    #[test]
    fn the_verbose_view_shows_every_pool_including_the_ones_nothing_is_known_about() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let quiet = report(&effective, &GatheredTelemetry::new(), options());
        let verbose = report(
            &effective,
            &GatheredTelemetry::new(),
            ReportOptions {
                verbose: true,
                now_unix: NOW,
            },
        );

        assert!(quiet.len() < verbose.len());
        assert!(!quiet.contains("cached input tokens"), "{quiet}");
        assert!(verbose.contains("cached input tokens"), "{verbose}");
        assert!(verbose.contains("calendar window"), "{verbose}");
    }

    /// The characteristic failure of this whole phase, guarded at the surface
    /// a person reads: with nothing measured, no number appears anywhere.
    #[test]
    fn a_report_with_no_telemetry_shows_no_capacity_figure_at_all() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let rendered = report(
            &effective,
            &GatheredTelemetry::new(),
            ReportOptions {
                verbose: true,
                now_unix: NOW,
            },
        );
        assert!(
            !rendered.contains('%'),
            "a percentage was printed: {rendered}"
        );
        assert!(
            !rendered.contains("last observed   unix"),
            "an observation was dated: {rendered}"
        );
        assert_eq!(
            rendered.matches("last observed   never").count(),
            registry().len()
        );
    }

    // --- line 1238, at the report level ------------------------------------

    /// Every reader is total, so the report is producible from any
    /// combination of nothing. This drives the exact function `main.rs`
    /// calls, with the readings a completely failed telemetry pass leaves
    /// behind.
    #[test]
    fn a_report_is_still_produced_when_every_telemetry_read_yielded_nothing() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let nothing = GatheredTelemetry::new()
            .with_harness(
                IntegrationId::ClaudeCode,
                crate::provider::telemetry::HarnessTelemetry::nothing(),
            )
            .with_provider_headers("anyrouter", RateLimitHeaders::read(Vec::new()), OBSERVED);

        let rendered = report(&effective, &nothing, options());
        assert!(rendered.starts_with("RESOURCES"));
        for kind in registry() {
            assert!(rendered.contains(&kind.label()), "{}", kind.label());
        }
    }

    // --- the provider-native unit survives rendering -----------------------

    /// Capability map line 1217, at the surface: a millionths-scaled amount
    /// prints as the provider's own money in the provider's own unit, never
    /// as a bare integer of microdollars and never converted.
    #[test]
    fn a_rendered_amount_keeps_the_providers_own_unit_and_scale() {
        assert_eq!(
            render_amount(&NativeAmount::whole(300, "requests")),
            "300 requests"
        );
        assert_eq!(
            render_amount(&NativeAmount::millionths(2_500_000, "USD")),
            "2.500000 USD"
        );
        assert_eq!(
            render_amount(&NativeAmount::millionths(1_200_000, "credits")),
            "1.200000 credits"
        );
    }

    /// The estimate path, at the surface. A percentage this report can print
    /// at all must go through `Percentage::render`, so an estimate is marked
    /// wherever it appears.
    #[test]
    fn an_estimated_percentage_is_marked_as_one_in_the_report() {
        let mut quota = QuotaOverride::default();
        quota.set_budget(Some(
            MonetaryBudget::new(10_000_000, BudgetPeriod::RollingThirtyDays).unwrap(),
        ));
        let user = user_with_anyrouter(quota);
        let effective = EffectiveConfig::new(&user, None);

        let state = observed_capacity(
            &ResourceKind::from_direct_provider("anyrouter"),
            &effective,
            &GatheredTelemetry::new(),
            NOW,
        );
        // Glasshouse counts no spend, so supply the observed half the way a
        // future local counter would, and check how it renders.
        let pool = state
            .user_budget()
            .clone()
            .with_remaining(Capacity::Measured(Reading::new(
                NativeAmount::millionths(2_500_000, "USD"),
                OBSERVED,
                ReadingSource::LocalObservation("this session's own spend".to_owned()),
            )));
        let state = state.with_user_budget(pool);
        let (label, score) = state.normalized().expect("the budget pool has both halves");
        assert_eq!(label, "user budget");

        let rendered = score.percent().render();
        assert!(rendered.starts_with("~25%"), "{rendered}");
        assert!(rendered.contains("estimated"), "{rendered}");
        assert_eq!(score.percent().exact(), None);
    }

    // --- line 1230: a provider's own usage endpoint -------------------------

    /// No credential in memory and none resolved — every `resolve` and
    /// `is_present` answers empty. Deliberately not
    /// `crate::secret::EnvironmentSecretStore`: a test process's own
    /// environment must never leak into what a probe request is built with.
    struct NoSecrets;
    impl crate::secret::SecretStore for NoSecrets {
        fn resolve(&self, _reference: &crate::secret::SecretRef) -> Option<crate::secret::Secret> {
            None
        }
        fn is_present(&self, _reference: &crate::secret::SecretRef) -> bool {
            false
        }
        fn describe(&self) -> &'static str {
            "no-secrets (test)"
        }
    }

    #[test]
    fn usage_probe_builds_a_request_against_the_declared_path_for_openrouter() {
        let openrouter = crate::provider::template("openrouter").expect("a built-in template");
        let request = usage_probe(&openrouter, &NoSecrets).expect("openrouter declares one");
        assert_eq!(request.url(), "https://openrouter.ai/api/v1/key");
    }

    /// Every other built-in template declares no usage endpoint, so
    /// `--probe`ing one makes exactly the one request it always did.
    #[test]
    fn a_provider_with_no_declared_usage_endpoint_is_not_probed_a_second_time() {
        let anyrouter = crate::provider::template("anyrouter").expect("a built-in template");
        assert!(usage_probe(&anyrouter, &NoSecrets).is_none());
    }

    /// `render_probe`, fed a reading exactly as `probe_provider` would build
    /// one from OpenRouter's real recorded response — the production path a
    /// `--probe openrouter` run actually renders through, driven here without
    /// a socket the way `a_providers_own_rate_limit_header_reaches_the_report_as_authoritative`
    /// drives `report` without one.
    #[test]
    fn a_null_usage_endpoint_limit_renders_as_not_applicable_and_no_percentage() {
        let reading = ProbeReading::Answered {
            outcome: crate::provider::discovery::ProbeOutcome::Reached { status: 200 },
            headers: RateLimitHeaders::read(Vec::new()),
            observed_at_unix: OBSERVED,
            usage: Some(Box::new(crate::provider::telemetry::ProviderUsage::read(
                r#"{"data": {"limit": null, "limit_remaining": null, "limit_reset": null}}"#,
            ))),
        };
        let mut rendered = String::new();
        render_probe(&mut rendered, "openrouter", &reading);
        assert!(
            rendered.contains("usage endpoint: limit not applicable"),
            "{rendered}"
        );
        assert!(
            !rendered.contains("usage endpoint capacity:"),
            "no percentage is computable from a null limit: {rendered}"
        );
    }

    /// The positive half: a hypothetical numeric reading — no live account
    /// has ever answered one — renders a real percentage line, proving the
    /// caller `main.rs` unconditionally invokes actually asks the question
    /// capability map lines 1217/1218 need answered.
    #[test]
    fn a_numeric_usage_endpoint_reading_renders_a_real_percentage() {
        let reading = ProbeReading::Answered {
            outcome: crate::provider::discovery::ProbeOutcome::Reached { status: 200 },
            headers: RateLimitHeaders::read(Vec::new()),
            observed_at_unix: OBSERVED,
            usage: Some(Box::new(crate::provider::telemetry::ProviderUsage::read(
                r#"{"data": {"limit": 20, "limit_remaining": 5}}"#,
            ))),
        };
        let mut rendered = String::new();
        render_probe(&mut rendered, "openrouter", &reading);
        assert!(
            rendered.contains("usage endpoint capacity: 25%"),
            "{rendered}"
        );
        assert!(!rendered.contains("estimated"), "{rendered}");
    }

    /// The two tests above drive `render_probe` with a hand-built
    /// [`ProbeReading`], which proves the rendering path but not that
    /// `probe_provider` — the function `main.rs` actually calls — makes the
    /// second request at all. Practice §35: a caller every test bypasses is
    /// not a caller. This one goes through a real socket and a real
    /// `EffectiveConfig`, over both requests `--probe openrouter` makes.
    #[test]
    fn probe_provider_makes_both_the_connectivity_request_and_the_usage_request() {
        let fixture = crate::provider::fixture::FixtureProvider::answering(
            "HTTP/1.1 200 OK",
            "content-type: application/json\r\nratelimit-limit: 300\r\n",
            r#"{"data":{"limit":null,"limit_remaining":null,"limit_reset":null}}"#,
        );
        let mut user = UserConfig::default();
        let mut provider = ProviderConfig::new("openrouter");
        provider.set_base_url(Some(fixture.base_url()));
        user.providers_mut().set("openrouter", provider);
        let effective = EffectiveConfig::new(&user, None);
        let secrets = NoSecrets;

        let reading = probe_provider(&effective, &secrets, "openrouter", OBSERVED);
        match reading {
            ProbeReading::Answered { headers, usage, .. } => {
                assert_eq!(
                    headers.limit(),
                    Some(300),
                    "the connectivity request's own headers did not reach the reading"
                );
                let usage = usage.expect(
                    "openrouter declares a usage endpoint, so probe_provider must have queried it",
                );
                assert!(
                    !usage.is_empty(),
                    "the usage endpoint's real body was not read"
                );
            }
            other => panic!("expected an answered probe, got {other:?}"),
        }
        // Two requests: the model-list connectivity probe and the usage
        // endpoint, both against the same fixture.
        assert_eq!(fixture.connections(), 2, "expected exactly two requests");
    }

    // --- the harness status command ----------------------------------------

    /// Practice §5: check a declaration against the *use*. Only harnesses
    /// with an interface checked on a real binary are listed, and the
    /// executable name comes from the adapter rather than from a second copy
    /// here.
    #[test]
    fn only_harnesses_with_a_checked_status_interface_are_listed() {
        for (id, args) in HARNESS_STATUS_ARGS {
            assert_eq!(id.kind(), IntegrationKind::Harness);
            assert!(!args.is_empty());
            // The name is the adapter's, never this module's.
            assert!(!id.executable_candidates().is_empty(), "{id:?}");
            assert_eq!(harness_status_args(*id), Some(*args));
        }
        // A harness with no checked interface is asked for nothing.
        assert_eq!(harness_status_args(IntegrationId::Cursor), None);
        // Codex is deliberately absent: `codex doctor --json` is stable and
        // machine-readable and carries no usage field at all.
        assert!(
            !HARNESS_STATUS_ARGS
                .iter()
                .any(|(id, _)| *id == IntegrationId::Codex)
        );
    }

    /// The interface string is printed in a report a user may share, so it
    /// must be the command and never the resolved path — a path under a home
    /// directory names the user.
    #[test]
    fn a_harness_interface_name_is_a_command_and_never_a_resolved_path() {
        let name = harness_interface_name(IntegrationId::ClaudeCode, &["auth", "status", "--json"]);
        assert_eq!(name, "claude auth status --json");
        assert!(!name.contains('/'));
    }

    // --- all four rate ceilings, not only the two something already reads --

    #[test]
    fn every_rate_ceiling_appears_in_the_verbose_view_including_the_unread_ones() {
        let state = CapacityState::metered_balance().with_rate_ceilings(RateCeilings::uniform(
            Capacity::Unmeasured,
            Capacity::Unmeasured,
        ));
        let mut out = String::new();
        render_rate_ceilings(
            &mut out,
            &state,
            ReportOptions {
                verbose: true,
                now_unix: NOW,
            },
        );

        // Premise first (practice §17): the two ceilings that already
        // rendered must still be here, so this could not pass because the
        // whole function was deleted.
        assert!(out.contains("requests/minute"), "{out}");
        assert!(out.contains("long window"), "{out}");
        assert!(out.contains("tokens/minute"), "{out}");
        assert!(out.contains("max concurrent"), "{out}");
    }

    #[test]
    fn a_measured_tokens_per_minute_ceiling_reaches_the_report() {
        let rates = RateCeilings::uniform(Capacity::Unmeasured, Capacity::Unmeasured)
            .with_tokens_per_minute(Capacity::Measured(Reading::new(
                NativeAmount::whole(40_000, "tokens"),
                OBSERVED,
                ReadingSource::LocalObservation("this session's own reading".to_owned()),
            )));
        let state = CapacityState::metered_balance().with_rate_ceilings(rates);
        let mut out = String::new();
        render_rate_ceilings(&mut out, &state, options());

        assert!(out.contains("tokens/minute"), "{out}");
        assert!(out.contains("40000 tokens"), "{out}");
    }

    #[test]
    fn an_unread_rate_ceiling_is_absent_from_the_non_verbose_view() {
        let state = CapacityState::metered_balance().with_rate_ceilings(RateCeilings::uniform(
            Capacity::Unmeasured,
            Capacity::Unmeasured,
        ));
        let mut out = String::new();
        render_rate_ceilings(&mut out, &state, options());

        assert!(!out.contains("tokens/minute"), "{out}");
    }

    // --- lines 1316 and 1365, through the same production rendering function ----

    use crate::routing::evidence::Outcome as RoutingOutcome;

    fn counts(
        throttle: usize,
        exhausted: usize,
        upstream: usize,
        served: usize,
    ) -> FailureClassCounts {
        let mut counts = FailureClassCounts::default();
        for _ in 0..throttle {
            counts.record(Some(RoutingOutcome::Failed), Some(FailureClass::Throttle));
        }
        for _ in 0..exhausted {
            counts.record(
                Some(RoutingOutcome::Failed),
                Some(FailureClass::ExhaustedQuota),
            );
        }
        for _ in 0..upstream {
            counts.record(
                Some(RoutingOutcome::Failed),
                Some(FailureClass::Upstream5xx),
            );
        }
        for _ in 0..served {
            counts.record(Some(RoutingOutcome::Succeeded), None);
        }
        counts
    }

    /// Line 1365 at the rendering function `main.rs::resources_report`
    /// calls: three figures, each with the denominator, and no line that
    /// adds them up.
    #[test]
    fn three_failure_figures_render_separately_with_their_denominator() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let telemetry = GatheredTelemetry::new()
            .with_provider_failure_classes("anyrouter", counts(3, 1, 2, 12));
        let row = anyrouter_row(&report(&effective, &telemetry, options()));

        assert!(
            row.contains(
                "failures 24h    cadence throttled 3, quota exhausted 1, provider unhealthy 2 — \
                 of 18 exchange(s), 12 served"
            ),
            "{row}"
        );
        assert!(
            row.contains("by class        throttle 3, exhausted quota 1, upstream 5xx 2"),
            "{row}"
        );
        // Nothing sums the three: neither the total failures (6) nor the
        // total with served (18) appears as a failures figure.
        assert!(!row.contains("failures 24h    6"), "{row}");
        assert!(!row.contains("6 failure"), "{row}");
    }

    /// Line 1316 at the same function: rate-limit responses counted apart
    /// from transport and model failures — a provider that was only ever
    /// throttled shows zero under provider health, and the other way round.
    #[test]
    fn rate_limit_responses_are_counted_apart_from_provider_failures() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);

        let throttled_only =
            GatheredTelemetry::new().with_provider_failure_classes("anyrouter", counts(4, 0, 0, 1));
        let row = anyrouter_row(&report(&effective, &throttled_only, options()));
        assert!(
            row.contains("cadence throttled 4, quota exhausted 0, provider unhealthy 0"),
            "{row}"
        );

        let failing_only =
            GatheredTelemetry::new().with_provider_failure_classes("anyrouter", counts(0, 0, 4, 1));
        let row = anyrouter_row(&report(&effective, &failing_only, options()));
        assert!(
            row.contains("cadence throttled 0, quota exhausted 0, provider unhealthy 4"),
            "{row}"
        );
    }

    /// Line 1324's rule applied to this line: a resource nothing has been
    /// recorded for says `unknown`, never zero.
    #[test]
    fn a_resource_with_no_recorded_exchange_prints_unknown_failures_not_zero() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let row = anyrouter_row(&report(&effective, &GatheredTelemetry::new(), options()));
        assert!(
            row.contains("failures 24h    unknown — no routing observation has been recorded"),
            "{row}"
        );
        assert!(!row.contains("cadence throttled 0"), "{row}");
    }

    /// The verbose view lists every class, zero or not — line 1761's debug
    /// view applied here; the quiet view lists only what happened.
    #[test]
    fn the_verbose_view_lists_every_class_and_the_quiet_view_only_the_nonzero_ones() {
        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let telemetry =
            GatheredTelemetry::new().with_provider_failure_classes("anyrouter", counts(1, 0, 0, 0));

        let quiet = anyrouter_row(&report(&effective, &telemetry, options()));
        assert!(quiet.contains("by class        throttle 1\n"), "{quiet}");

        let verbose = anyrouter_row(&report(
            &effective,
            &telemetry,
            ReportOptions {
                verbose: true,
                ..options()
            },
        ));
        for class in FailureClass::ALL {
            assert!(
                verbose.contains(&format!(
                    "{} {}",
                    describe_failure_class(class),
                    telemetry_count(&telemetry, class)
                )),
                "verbose must list `{class}`: {verbose}"
            );
        }
    }

    fn telemetry_count(telemetry: &GatheredTelemetry, class: FailureClass) -> usize {
        telemetry
            .for_provider_failure_classes("anyrouter")
            .map_or(0, |counts| counts.count(class))
    }

    /// The gather path against a real ledger, so the bridge from the rows
    /// the gateway writes to the line `glasshouse resources` prints is
    /// proven end to end minus the one `main.rs` call the report names.
    #[test]
    fn gather_failure_classes_reads_a_real_ledger_into_the_rendered_report() {
        use crate::routing::evidence::NewObservation;
        use crate::{Cli, Runtime};
        use clap::Parser;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("workspace").join("proj");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            tmp.path().join("data").to_str().unwrap(),
            "--config-dir",
            tmp.path().join("config").to_str().unwrap(),
        ])
        .unwrap();
        let runtime: Runtime = crate::bootstrap(&cli, &root).unwrap();
        let ledger = EvidenceLedger::open(&runtime).unwrap();

        for (class, outcome) in [
            (Some(FailureClass::Throttle), RoutingOutcome::Failed),
            (Some(FailureClass::Throttle), RoutingOutcome::Failed),
            (Some(FailureClass::Upstream5xx), RoutingOutcome::Failed),
            (None, RoutingOutcome::Succeeded),
        ] {
            ledger
                .record(
                    NewObservation::new("anyrouter", "some-model")
                        .with_outcome(outcome)
                        .with_failure_class(class),
                    NOW - 60,
                )
                .unwrap();
        }
        // Outside the window: yesterday's outage must not colour today's
        // reading.
        ledger
            .record(
                NewObservation::new("anyrouter", "some-model")
                    .with_outcome(RoutingOutcome::Failed)
                    .with_failure_class(Some(FailureClass::ExhaustedQuota)),
                NOW - FAILURE_CLASS_WINDOW_SECONDS - 1,
            )
            .unwrap();

        let user = UserConfig::default();
        let effective = EffectiveConfig::new(&user, None);
        let telemetry = GatheredTelemetry::new().gather_failure_classes(&ledger, NOW);
        let row = anyrouter_row(&report(&effective, &telemetry, options()));
        assert!(
            row.contains(
                "cadence throttled 2, quota exhausted 0, provider unhealthy 1 — of 4 \
                 exchange(s), 1 served"
            ),
            "{row}"
        );
    }
}
