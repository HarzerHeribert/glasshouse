//! `commands::launch` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use std::ffi::OsString;
use std::process::ExitCode;
use std::sync::Arc;

use glasshouse::config::response::ResponseRequest;
use glasshouse::config::{self, EffectiveConfig, ProjectConfig, UserConfig};
use glasshouse::events::{LifecycleEvent, ProcessExit};
use glasshouse::guardrails::GuardrailOverride;
use glasshouse::integrations::cmux;
use glasshouse::launch::HarnessLaunch;
use glasshouse::session;
use glasshouse::session::{
    NewSession, ProjectSessions, SessionId, SessionLifecycle, SessionPresentation,
};
use glasshouse::{Cli, Runtime};

/// Phase 9J line 576: the native-pairing preference and corrections in
/// effect, resolved into the form `crate::profile`'s gateway path accepts —
/// see `glasshouse::profile::GatewayPairing`'s own doc comment for why that
/// module cannot resolve this itself. Both of `launch_session`'s and
/// `resolve_resume_overlay`'s gateway-backed launches call this, so a
/// configured preference reaches a resumed session exactly as it reaches a
/// fresh one.
pub(crate) fn resolved_gateway_pairing(
    effective: &EffectiveConfig<'_>,
) -> glasshouse::profile::GatewayPairing {
    let (preference, _source) = effective.native_pairing_preference();
    glasshouse::profile::GatewayPairing {
        preference_slug: preference.slug(),
        overrides: effective.pairing_overrides(),
    }
}

// ---------------------------------------------------------------------------
// Phase 37 — the session router's production callers, map lines 1592–1602.
//
// `glasshouse::routing::session` ranks *destinations*, and a destination is
// something only this file can assemble: it needs this project's session
// records, this user's launch profiles, the provider table and the quota
// cache, none of which that module is allowed to reach (its own
// `the_session_router_cannot_look_a_session_or_a_checkpoint_up` fails the
// build if it ever tries). So the five inputs are read here, once, and every
// caller below goes through the same two functions.
// ---------------------------------------------------------------------------

/// Everything a person typed about **where** this session goes and what it
/// boots from — the four arguments `launch_session` reads before it resolves
/// anything.
///
/// One type rather than four parameters because they are one statement, and
/// the router reads all four together: `to` and `fresh` are line 1602's
/// override outright, and `profile` and `from_checkpoint` are the two ways of
/// saying "a new session" without using that word (see the override built in
/// `launch_session`). Separating them would let a caller pass this decision's
/// profile with last decision's override, which is the same reason
/// `routing::session::RouterInputs` is one struct.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LaunchDestination<'a> {
    /// `--profile`: the launch profile a **new** session runs under.
    pub(crate) profile: Option<&'a str>,
    /// `--from-checkpoint`: the handoff a new session opens with.
    pub(crate) from_checkpoint: Option<&'a str>,
    /// `--to`: this destination, whatever the ranking says.
    pub(crate) to: Option<&'a str>,
    /// `--fresh`: a new session, whatever the ranking says.
    pub(crate) fresh: bool,
    /// `--task`: what the work is, which decides what the destination must
    /// be able to do (Phase 34D). `None` classifies nothing and reproduces
    /// the launch exactly as it was before classification existed.
    pub(crate) task: Option<&'a str>,
    /// `--no-routing`: take no routing decision for this launch at all —
    /// capability map line 1712.
    ///
    /// Not a fifth way of naming a destination, which is why it sits here
    /// beside them rather than being folded into one: the four fields above
    /// say *where*, and this one says *stop deciding*. With it set, the four
    /// above are still read and still obeyed — a person who says both "do
    /// not rank" and "go here" has said two compatible things.
    pub(crate) no_routing: bool,
    /// `--checkpoint-first`: check point the session this work is leaving
    /// before it moves — capability map line 1716.
    pub(crate) checkpoint_first: bool,
}

/// 56A line 1969's binding half, beside line 1973's child-env scrub: the
/// launch's secret store with every **foreign** entitlement's credential
/// reference refused. `profile::resolve` binds a direct-provider launch to
/// "the first credential reference that currently resolves" out of the
/// provider's declared pool — a rule written before the broker existed —
/// so with the pool brokered, the resolution the overlay sees must only be
/// able to answer with the serving account's own reference, or the process
/// would authenticate as whichever account is listed first while the
/// announcement names another. Same filter as
/// `EffectiveConfig::foreign_entitlement_credential_refs`, wrapped rather
/// than re-derived; everything that is not an entitlement credential
/// resolves exactly as before.
struct EntitlementScopedSecrets<'a> {
    inner: &'a dyn glasshouse::secret::SecretStore,
    foreign: Vec<glasshouse::secret::SecretRef>,
}

impl glasshouse::secret::SecretStore for EntitlementScopedSecrets<'_> {
    fn resolve(
        &self,
        reference: &glasshouse::secret::SecretRef,
    ) -> Option<glasshouse::secret::Secret> {
        if self.foreign.contains(reference) {
            return None;
        }
        self.inner.resolve(reference)
    }

    fn is_present(&self, reference: &glasshouse::secret::SecretRef) -> bool {
        // The same answer `resolve` gives, without producing a value: a
        // foreign account's credential is not present *to this launch*.
        !self.foreign.contains(reference) && self.inner.is_present(reference)
    }

    fn describe(&self) -> &'static str {
        // The underlying store's own label: this wrapper narrows which
        // references answer, not where values come from, and a diagnostic
        // naming a store the user has never heard of would mislead.
        self.inner.describe()
    }
}

/// The profile a `fresh:<harness>:<profile>` identifier names, when it names
/// one for `harness`.
///
/// `None` for a recorded session's identifier, and `None` for a fresh
/// identifier belonging to a different harness — which then reaches the router
/// as an override naming a destination that was not offered, and is refused
/// out loud rather than silently reinterpreted.
fn fresh_destination_profile(
    id: &str,
    harness: glasshouse::integrations::IntegrationId,
) -> Option<&str> {
    let profile = id
        .strip_prefix("fresh:")?
        .strip_prefix(harness.slug())?
        .strip_prefix(':')?;
    // 56A line 1953: a pool candidate's id carries its entitlement after an
    // `@` (`fresh:<harness>:<profile>@<entitlement>`); the profile is the
    // part before it. A profile whose own name contains `@` cannot be named
    // through such an identifier — recorded, not guessed around.
    Some(profile.split_once('@').map_or(profile, |(name, _)| name))
}

/// The cost class of the destination a launch actually routed to — map line
/// 1835's *"low-cost or free route"* versus *"the premium route it
/// displaced"*, as a fact rather than a guess.
///
/// # Why this is not `destination.backend().cost()`
///
/// [`destination_backend`] hardcodes `Cost::Metered` for every destination it
/// builds, and says so: the session router reads a backend's provider,
/// credential, model and tool semantics and never its cost, so the field is
/// the fail-closed constant rather than a measurement. Recording *that* as a
/// route's class would give line 1835 one bucket for ever and report a
/// tautology.
///
/// So the class is read where the fact actually lives:
/// [`ProviderConfig::cost_of`], the same one lookup `disposable_candidates`
/// and `gateway_upstream` use, applied to the destination's own provider and
/// model with the project layer winning over the user layer. `glasshouse::
/// profile` and `glasshouse::routing` may not import `glasshouse::config`, so
/// main.rs is where this can be answered at all.
///
/// # `None` is the third answer, and it is honest
///
/// A destination on a harness's own sign-in names no configured provider, and
/// a gateway-backed one assigns its model when the session starts. Neither
/// has a marked cost, and Glasshouse does not know what a subscription costs
/// at the margin. That is recorded as
/// [`glasshouse::evaluation::UNKNOWN_COST_CLASS`] and counted in its own
/// bucket — never folded into `metered`, which would be a number nobody
/// measured.
fn routed_cost_class(
    user: &UserConfig,
    project: Option<&ProjectConfig>,
    destination: &glasshouse::routing::session::Destination,
) -> Option<glasshouse::routing::Cost> {
    let model = destination.backend().model().name()?;
    let provider = destination.backend().provider();
    let config = project
        .and_then(|project| project.providers().get(provider))
        .or_else(|| user.providers().get(provider))?;
    Some(config.cost_of(model))
}

/// Whether the pool this launch handed the router held any observed health
/// reading for the destination it chose — map line 1854's *sparse* half.
///
/// The key is built exactly as [`observed_provider_health`] builds it, from
/// the destination's own credential and model label, so a hit here means the
/// same resource and not a resource that merely renders the same.
///
/// **Two of line 1854's three words now, not one.** `routing::evidence`'s
/// `Confidence` belongs to the gateway's aggregate ledger, which
/// `SessionRouter` never reads, and a
/// [`glasshouse::routing::free::FreePool`] health entry carries no
/// observation time — but the cache the pool was filled from does, per
/// provider file, and [`ObservedHealth`] carries it here. So *sparse* is
/// answered by whether the pool held this destination and *stale* by how old
/// the file that supplied it was, against
/// [`glasshouse::evaluation::HEALTH_EVIDENCE_HORIZON_SECONDS`].
///
/// *Incorrectly segmented*, line 1854's third, still has no producer
/// anywhere on this path and is not invented: nothing in this build compares
/// a health reading's segmentation against the resource it was attributed
/// to, and the line stays open on that word alone.
fn routing_evidence_for(
    health: &crate::commands::routing_destinations::ObservedHealth,
    destination: &glasshouse::routing::session::Destination,
    now_unix: i64,
) -> glasshouse::evaluation::RoutingEvidence {
    use glasshouse::routing::free::FreeResource;

    let chosen = FreeResource::new(
        destination.backend().credential().clone(),
        destination.backend().model().label(),
    );
    let held = health
        .pool()
        .observed()
        .iter()
        .any(|(resource, _)| *resource == chosen);
    // A pool hit whose date is somehow missing answers `absent`, not fresh —
    // `and_then` rather than `unwrap_or(now_unix)`, which is the one
    // substitution that would turn an unknown into a favourable fact.
    let observed_at = held.then(|| health.observed_at(&chosen)).flatten();
    glasshouse::evaluation::RoutingEvidence::from_observation(observed_at, now_unix)
}

/// Capability map line 1566: one ledger row per tier movement the launch
/// path acted on, under [`glasshouse::routing::evidence::TIER_ESCALATION_PURPOSE`]
/// or [`glasshouse::routing::evidence::TIER_DOWNGRADE_PURPOSE`], so a later
/// evaluation can count how often the router moved a tier and which way.
///
/// The same `glasshouse`/`session-router` identity and the same
/// open-write-drop shape as [`record_routing_latency`], for the same reasons
/// — and a `Held` movement writes nothing, because "the tier stood" is the
/// row's absence, exactly as a launch that classified nothing leaves no
/// latency row.
fn record_tier_movement(
    runtime: &Runtime,
    harness: glasshouse::integrations::IntegrationId,
    movement: &glasshouse::routing::session::TierMovement,
) {
    use glasshouse::routing::evidence::{
        EvidenceLedger, NewObservation, TIER_DOWNGRADE_PURPOSE, TIER_ESCALATION_PURPOSE,
    };
    use glasshouse::routing::session::TierMovement;

    let purpose = match movement {
        TierMovement::Escalated { .. } => TIER_ESCALATION_PURPOSE,
        TierMovement::Downgraded { .. } => TIER_DOWNGRADE_PURPOSE,
        TierMovement::Held { .. } => return,
    };
    let ledger = match EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "routing evidence ledger unavailable; the tier movement is not recorded"
            );
            return;
        }
    };
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let observation = NewObservation::new("glasshouse", "session-router")
        .with_harness(Some(harness.slug()))
        .with_purpose(Some(purpose))
        .with_timing(Some(now_unix), Some(now_unix));
    if let Err(err) = ledger.record(observation, now_unix) {
        tracing::warn!(error = %err, "could not record the tier movement");
    }
}

/// Capability map line 1970: one ledger row per pool fallback the launch
/// path acted on. The same open-write-drop shape as
/// [`record_tier_movement`], for the same reasons — and **a decision that
/// made no fallback writes nothing**, because "the broker stayed put" is
/// the row's absence, exactly as a held tier is.
///
/// The row carries the fallback whole **without a migration**: `purpose` is
/// the trigger, `quota_context` is the account the work LEFT (so the
/// entitlements view's own per-account reader finds it), and the account
/// the work went TO is the `sessions.entitlement` column migration 22
/// added, written by this same launch from this same decision. `provider`
/// and `model` are the chosen destination's.
///
/// Map line 1307's own producer: `cost`, when given, is
/// [`glasshouse::routing::session::Routed::cost`] — the value **that
/// decision itself computed**, carried in rather than recomputed here from a
/// `PriceTable` that may since have changed on disk. This is the only launch
/// writer with a `Destination` in scope
/// (`record_tier_movement`'s `TierMovement` carries none), so it is the only
/// production caller `cost_micro_usd` has today; most rows still leave it
/// `NULL`, on every decision that made no fallback at all.
fn record_entitlement_fallback(
    runtime: &Runtime,
    harness: glasshouse::integrations::IntegrationId,
    destination: &glasshouse::routing::session::Destination,
    fallback: &glasshouse::routing::session::EntitlementFallback,
    cost: Option<glasshouse::routing::evidence::ObservedCost>,
) {
    use glasshouse::routing::evidence::{
        ENTITLEMENT_FALLBACK_EXHAUSTED_PURPOSE, ENTITLEMENT_FALLBACK_THROTTLED_PURPOSE,
        EvidenceLedger, NewObservation,
    };
    use glasshouse::routing::session::FallbackReason;

    let fallback_purpose = match fallback.reason() {
        FallbackReason::Exhausted => ENTITLEMENT_FALLBACK_EXHAUSTED_PURPOSE,
        FallbackReason::Throttled => ENTITLEMENT_FALLBACK_THROTTLED_PURPOSE,
    };
    let ledger = match EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "routing evidence ledger unavailable; the entitlement fallback is not recorded"
            );
            return;
        }
    };
    let now_unix = glasshouse::provider::cache::now_unix_seconds();
    let observation = NewObservation::new(
        destination.backend().provider(),
        destination.backend().model().label(),
    )
    .with_harness(Some(harness.slug()))
    .with_purpose(Some(fallback_purpose))
    .with_quota_context(Some(fallback.from().to_owned()))
    .with_timing(Some(now_unix), Some(now_unix))
    .with_cost(cost);
    if let Err(err) = ledger.record(observation, now_unix) {
        tracing::warn!(error = %err, "could not record the entitlement fallback");
    }
}

/// The workload tier a launch's routing decision used, and whether line
/// 1459's conservative rule moved it — **capability map line 1834**'s
/// producer input, from the classification that decision actually acted on.
///
/// `None` — no `--task`, so nothing classified — is
/// [`glasshouse::evaluation::RoutingTier::Unclassified`], **its own bucket
/// and never nothing**: a launch that states no task still made a routing
/// decision, and omitting its row would make *"this project never states its
/// tasks"* read as *"this project never launches"*.
///
/// *Escalated* is whether the tier the decision used differs from the tier
/// the classifier stated, which is not the same as whether the conservative
/// rule fired — see [`glasshouse::evaluation::RoutingTier::Classified`]'s own
/// doc comment for the case at the top of the scale where the two part.
fn routed_tier(
    classified: Option<&crate::commands::routing_classification::ClassifiedRouting>,
) -> glasshouse::evaluation::RoutingTier {
    use glasshouse::evaluation::RoutingTier;

    let Some(classified) = classified else {
        return RoutingTier::Unclassified;
    };
    let answer = &classified.answer;
    RoutingTier::Classified {
        tier: answer.required_tier(),
        escalated: answer.required_tier() != answer.stated_tier(),
    }
}

/// Map line 1855's producer call, shared by both of `launch_session`'s
/// routed exits: writes the launch's own expected output-token size only
/// when its task class has a real median in [`comparable_output_tokens`]'s
/// own window — the same window and the same reader
/// [`session_router`] already consulted to rank this launch, read again
/// here rather than threaded through, for the reason [`comparable_output_tokens`]'s
/// own doc gives: this ledger read is fail-soft and costs this estimate
/// alone, never the launch.
///
/// A launch that stated no task, or whose task class has no comparable rows
/// in the window, writes nothing at all — never a fabricated zero.
fn record_consumption_estimate(
    runtime: &Runtime,
    session_id: &str,
    classified: Option<&crate::commands::routing_classification::ClassifiedRouting>,
    observed_at_unix: i64,
) {
    let Some(task_class) = classified.map(|classified| classified.answer.task_class()) else {
        return;
    };
    let Some(median_output_tokens) =
        crate::commands::routing_destinations::comparable_output_tokens(runtime)
            .into_iter()
            .find(|class_output| class_output.class == task_class)
            .and_then(|class_output| class_output.median_output_tokens)
    else {
        return;
    };
    glasshouse::evaluation::record_routing_consumption_estimate(
        runtime,
        session_id,
        task_class,
        median_output_tokens,
        observed_at_unix,
    );
}

/// Leave this decision's classification behind for the next one — line
/// 1467's write side, called once the session the work landed on has an
/// identifier. Only an answer a model actually gave is worth remembering:
/// heuristics are free to re-run, and a reused answer is already on disk.
fn remember_classification(
    cache: &crate::commands::routing_classification::ClassificationStickyCache,
    classified: Option<&crate::commands::routing_classification::ClassifiedRouting>,
    session: &str,
) {
    let Some(classified) = classified else {
        return;
    };
    if !classified.answer.provenance().asked_a_model() {
        return;
    }
    cache.store(&glasshouse::routing::request::StickyClassification::new(
        session,
        classified.fingerprint.clone(),
        classified.answer.classification(),
        glasshouse::provider::cache::now_unix_seconds(),
    ));
}

/// What `routing_observations.purpose` records for map line 1849's
/// measurement. Spelled once, like [`CLASSIFICATION_PURPOSE`], and now in
/// `routing::evidence` beside it, because `RoutingOverhead` reads this word
/// back and a second spelling would split the only producer from the only
/// reader.
const ROUTING_LATENCY_PURPOSE: &str = glasshouse::routing::evidence::ROUTING_LATENCY_PURPOSE;

/// Map line 1849: record what routing added to this launch, from the start
/// of the decision (`started`) to its end — the point after which profile
/// resolution, the gateway and the process spawn happen identically whether
/// or not a task was stated, and are therefore the launch's own cost rather
/// than routing's.
///
/// Called only when a classification ran, so a launch that states no task
/// opens no ledger (practice §65) and leaves no row: the row's absence is
/// the honest reading of "nothing was added". Opened, written and dropped
/// here, before any gateway holds its own handle.
///
/// The ledger's timing columns are unix **seconds** (migration 11), so a
/// sub-second decision reads back as `0` through `duration_ms()`; the
/// millisecond figure goes to the log beside it. A finer column is a schema
/// decision this package does not take.
///
/// **This row carries no session id** — `glasshouse::database` migration
/// 24's `session_id` stays `NULL` here, deliberately and permanently. The
/// decision this row measures is taken *before* `store.create` mints a
/// session, so there is no id to write; and the row is about the routing
/// decision rather than about an exchange some session was served, which is
/// the only thing that column is for. Filling it from a session recorded
/// later would make "the launch decided this before any session existed"
/// indistinguishable from "this exchange belonged to that session", which is
/// the distinction the nullable column exists to keep.
fn record_routing_latency(
    runtime: &Runtime,
    started: std::time::Instant,
    started_at_unix: i64,
    harness: glasshouse::integrations::IntegrationId,
    answer: &glasshouse::routing::request::RouterAnswer,
) {
    let elapsed = started.elapsed();
    let completed_at_unix = glasshouse::provider::cache::now_unix_seconds();
    tracing::info!(
        elapsed_ms = elapsed.as_millis() as u64,
        asked_a_model = answer.provenance().asked_a_model(),
        "routing decision latency before the harness starts"
    );
    let ledger = match glasshouse::routing::evidence::EvidenceLedger::open(runtime) {
        Ok(ledger) => ledger,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "routing evidence ledger unavailable; routing latency is not recorded"
            );
            return;
        }
    };
    let observation =
        glasshouse::routing::evidence::NewObservation::new("glasshouse", "session-router")
            .with_harness(Some(harness.slug()))
            .with_purpose(Some(ROUTING_LATENCY_PURPOSE))
            // Map line 1276's missing link, and the reason migration 23
            // exists: `answer` has carried a `TaskClass` since Phase 34C and
            // this row — the one row every routed request produces — has
            // never written it down. `glasshouse::routing::burn` reads it
            // back.
            .with_task_class(Some(answer.task_class()))
            .with_timing(Some(started_at_unix), Some(completed_at_unix));
    if let Err(err) = ledger.record(observation, completed_at_unix) {
        tracing::warn!(error = %err, "could not record routing latency");
    }
}

// Eight, and the eighth arrived at integration: `external` is Phase 17's and
// `guardrail` is Phase 21K's, written by two packages that never shared a
// tree. Neither belongs in `LaunchDestination` -- that bundle answers *where
// the work goes*, and one of these says where the session is *shown* while
// the other says how hard its premises are *gated*. Folding either in to
// satisfy a lint would put an unrelated fact in a named type.
#[allow(clippy::too_many_arguments)]
pub(crate) fn launch_session(
    runtime: &Runtime,
    harness: Option<&str>,
    destination: LaunchDestination<'_>,
    response: &ResponseRequest,
    headless: bool,
    no_memory: bool,
    external: ExternalPresentation,
    harness_args: &[String],
    guardrail: Option<GuardrailOverride>,
) -> anyhow::Result<ExitCode> {
    let LaunchDestination {
        profile: profile_name,
        from_checkpoint,
        to,
        fresh,
        task,
        no_routing,
        checkpoint_first,
    } = destination;
    // Map line 1849: the routing decision is timed from here. Whether the
    // figure is ever recorded is decided by whether a task was stated.
    let routing_started = std::time::Instant::now();
    let routing_started_at_unix = glasshouse::provider::cache::now_unix_seconds();
    let user = UserConfig::load(runtime.paths())?;
    let project = config::load_project_config(runtime.project())?;
    let effective = EffectiveConfig::new(&user, project.as_ref());
    let selection = session::select::select(harness, effective)?;
    // -----------------------------------------------------------------------
    // Phase 37 lines 1592, 1593 and 1595–1600: **where** this work goes is
    // decided here, at a session boundary, before a launch profile is
    // resolved — because the destination is what chooses the profile and not
    // the other way round.
    //
    // This is the production call the router was built for. Everything below
    // it already worked; what it did not do was ask whether this project
    // already had a session worth continuing, which is line 1593 in one
    // sentence. Deleting the `choose` call below must break
    // `tests/route_command.rs`, and that is the point (practice §35): the
    // router's own eleven mutations prove its scoring and none of them can
    // prove that anything calls it.
    // -----------------------------------------------------------------------
    // Which profile a *new* session would run under, from the same three
    // sources it has always come from — with `--to fresh:<harness>:<profile>`
    // added as a fourth, because an identifier a person pasted out of
    // `glasshouse route` has to mean the same thing here as it did there.
    let named_profile = to
        .and_then(|id| fresh_destination_profile(id, selection.id()))
        .or(profile_name);
    // Map line 372's remaining clause: with no profile named, the router is
    // asked to rank every *enabled* profile rather than the one implied
    // fallback below picks for it. `--to`, `--fresh` and `--from-checkpoint`
    // all leave `named_profile` unset too, and none of them names a profile
    // either — the ranking still gets to pick which one a fresh session
    // would run under; only `--to fresh:<harness>:<profile>` and `--profile`
    // count as "the user pinned one," because those are the only two that
    // said so by name.
    let profile_selection = named_profile.is_none();
    // A profile the user disabled is not a profile Glasshouse may start,
    // and being asked for it by name is the one case where saying nothing
    // would be worst: the routing filter above simply stops offering it, so
    // without this a `--profile` naming it would launch it anyway and
    // `enabled` would mean nothing on the path that actually starts a
    // session.
    //
    // Refused *here*, before `routing_destinations` and before any
    // pre-flight check, so a refusal costs nothing — no probe, no session
    // record, no process — matching the harness-not-installed refusal below
    // it in `session::select`.
    //
    // Only a name the person supplied is checked. `fresh_profile`'s fallback
    // is the implied Native profile, which nobody asked for and which
    // `profile_enabled` never reports as disabled anyway.
    if let Some(name) = named_profile {
        let enabled = effective.profile_enabled(name);
        if !enabled.value {
            eprintln!(
                "glasshouse: {}",
                config::ProfileDisabled::new(name, enabled.layer)
            );
            return Ok(ExitCode::FAILURE);
        }
    }
    // -----------------------------------------------------------------------
    // Phase 17 lines 754, 755, 757 and 761 — external presentation.
    //
    // Decided after the harness and the profile have been refused or
    // accepted, so a launch that would fail is refused *here*, in this
    // terminal, and never as a pane that opens and dies — and before the
    // router runs, because a launch that hands itself to a pane has not
    // routed anything: the launch inside the pane does all of that, once.
    //
    // Absence is a first-class path: every way cmux can be unavailable is a
    // reason printed and a session that runs embedded, byte for byte as it
    // would have without the flag.
    // -----------------------------------------------------------------------
    // "Here" is wherever this launch was going anyway: the flag asked for
    // a pane on top of that, and without one nothing else changes.
    let here = if headless { "headless" } else { "embedded" };
    let hosted_pane: Option<cmux::PaneRef> = match &external {
        ExternalPresentation::Embedded => None,
        ExternalPresentation::SpawnIn { pane_command } => match cmux::detect() {
            cmux::Availability::Available(control) => {
                return open_cmux_pane(runtime, &control, selection.id().slug(), pane_command);
            }
            cmux::Availability::Absent(reason) => {
                eprintln!("glasshouse: cmux is not available ({reason}); the session runs {here}");
                None
            }
        },
        // A reference given by hand is metadata the caller asserted;
        // recording it asks cmux nothing.
        ExternalPresentation::HostedBy(cmux::PaneRefRequest::Given(pane)) => Some(pane.clone()),
        ExternalPresentation::HostedBy(request @ cmux::PaneRefRequest::Caller) => {
            match cmux::resolve_pane_ref(request, &cmux::detect()) {
                Ok(pane) => Some(pane),
                Err(reason) => {
                    eprintln!("glasshouse: {reason}; the session runs {here}");
                    None
                }
            }
        }
    };
    let fresh_profile = named_profile.unwrap_or(glasshouse::profile::NATIVE_PROFILE_NAME);

    // -----------------------------------------------------------------------
    // Line 1712: the off switch, and it is taken **here** — before
    // `routing_destinations` opens this project's session store, its quota
    // cache and its health cache, and before anything classifies this
    // launch's task.
    //
    let sticky_cache = crate::commands::routing_classification::ClassificationStickyCache::new(
        runtime.paths(),
        runtime.project().id().as_str(),
    );
    let text_cache = crate::commands::routing_classification::ClassificationTextCache::new(
        runtime.paths(),
        runtime.project().id().as_str(),
    );
    // # Why routing off does not report what it would have done
    //
    // The obvious courtesy is to rank anyway and print *"routing is off; it
    // would have continued session X"*. That is exactly the work the person
    // turned off. The ranking's inputs are not free: three on-disk stores are
    // opened to build the destinations — practice §65 is this project's
    // record of what an unnecessary open handle costs on the platform it does
    // not develop on, where SQLite's locks are mandatory rather than advisory
    // — and the task classification on this path can reach a routing model.
    // Doing all of it to render one sentence would make "off" mean "the same
    // work, silently, and then a message about it".
    //
    // So the line says routing is off and where that was decided, and points
    // at `glasshouse route`, which answers *"what would have happened"* on
    // demand and starts nothing. Asking is a thing a person does
    // deliberately; being charged for the answer is not.
    // -----------------------------------------------------------------------
    let automatic = effective.automatic_routing();
    let routing_off = no_routing || !automatic.value;
    if routing_off {
        if no_routing {
            eprintln!(
                "glasshouse: automatic routing is off for this launch (--no-routing), so no \
                 ranking was taken. `glasshouse route` shows what it would have chosen, \
                 without starting anything."
            );
        } else {
            eprintln!(
                "glasshouse: automatic routing is off {}, so no ranking was taken. \
                 `glasshouse route` shows what it would have chosen, without starting \
                 anything.",
                automatic.layer.describe_source()
            );
        }
        // A `--to` naming a session this project already has is the one thing
        // that still moves work into an existing session with the ranking
        // off. It is not the ranking deciding — it is the person, and turning
        // the ranking off was never a statement about their own flags.
        //
        // A `fresh:<harness>:<profile>` identifier falls through instead: it
        // names a session that does not exist yet, and starting it is what
        // the rest of this function already does under `fresh_profile`, which
        // `named_profile` has already read that identifier's profile out of.
        if let Some(id) = to
            && fresh_destination_profile(id, selection.id()).is_none()
        {
            eprintln!(
                "glasshouse: continuing session `{id}` because you named it; with routing off, \
                 nothing else was considered."
            );
            if checkpoint_first {
                crate::commands::resume::checkpoint_before_moving(runtime, Some(id))?;
            }
            return crate::commands::resume::resume_session(
                runtime,
                id,
                harness_args,
                headless,
                crate::commands::resume::RouteOnResume::AlreadyRouted,
            );
        }
    }

    // Line 1712 again: with the ranking off, none of this runs at all —
    // not the three stores `routing_destinations` opens, not the health
    // bridge, not `choose`. `routed` is `None`, which the tail below already
    // handles as "there was no routing decision", and the fresh destination
    // is the profile this launch resolved on its own.
    let (routed, classified, health) = if routing_off {
        (
            None,
            None,
            crate::commands::routing_destinations::ObservedHealth {
                pool: glasshouse::routing::free::FreePool::new(),
                observed_at: Vec::new(),
            },
        )
    } else {
        // Map line 372: automatic routing is on here (`routing_off` is
        // false only when it is), so the fresh side of the candidate set
        // widens to every enabled profile exactly when the person did not
        // pin one — `Launchable` unchanged for a pin, `Launchable` unchanged
        // for automatic off (that branch never reaches this arm at all).
        let scope = if profile_selection {
            crate::commands::routing_destinations::DestinationScope::LaunchableAcrossProfiles
        } else {
            crate::commands::routing_destinations::DestinationScope::Launchable {
                profile: fresh_profile,
            }
        };
        let destinations = crate::commands::routing_destinations::routing_destinations(
            runtime,
            &effective,
            selection.id(),
            scope,
            task,
        )?;
        let overrides = effective.pairing_overrides();
        // **Map line 1599's bridge, on the path that acts.** The live pool a
        // gateway fills still does not exist here — that gateway is started
        // further down, and only for a profile that needs one — but what a
        // gateway *exports* does: `provider::telemetry::GatewayHealthReading`s,
        // persisted to `GatewayHealthCache` under this run's own data directory,
        // by whichever earlier `glasshouse run` or `glasshouse launch` served the
        // work. `observed_provider_health` reads them into the pool
        // `provider_health` looks in, and its own doc has the two hazards that
        // make it a design rather than a wiring — the rendered `credential_label`
        // against a `CredentialId`, and unix seconds against an epoch-less
        // `Instant`. Neither is guessed at; a reading that cannot be attributed
        // without guessing is not attributed, which leaves exactly the inert
        // `0.0` this line had before the bridge.
        //
        // The reading comes from a *previous* process. That is the whole point:
        // the health of a provider is not a fact this launch can observe about a
        // session it has not started yet.
        let health = crate::commands::routing_destinations::observed_provider_health(
            runtime,
            &effective,
            &destinations,
        );
        // Capability map line 1419: the destination this launch lands on
        // when classification does nothing — `fresh_profile`'s own fresh
        // destination among the ones just built (`fresh_profile` is always
        // concrete: it falls back to the implied Native profile above), read
        // through the same `pricing.toml` `session_router` prices
        // `expected_marginal_cost` from. A profile whose backend is a
        // harness's own sign-in, or that names no model, or that
        // `pricing.toml` does not price, leaves this `None` — inert, never
        // guessed (`design-decisions.md`, *"The premium capacity a
        // classifier protects"*).
        let protected_capacity_prices =
            glasshouse::provider::pricing::PriceTable::load_from_dir(runtime.paths().config_dir());
        let protected_capacity_price = destinations
            .iter()
            .find(|destination| {
                destination.is_fresh() && destination.launch_profile() == fresh_profile
            })
            .and_then(|destination| {
                destination
                    .backend()
                    .model()
                    .name()
                    .map(|model| (destination.backend().provider(), model))
            })
            .and_then(|(provider, model)| protected_capacity_prices.price_for(provider, model));
        // Phase 34D, on the path that acts: what the work *is* decides what the
        // destination must be able to do. `None` — no `--task` — hands the
        // router `TaskRequirements::default()` and asks nothing, which is this
        // launch exactly as it was before classification existed.
        let classified = crate::commands::routing_classification::classify_for_routing(
            runtime,
            &effective,
            crate::commands::routing_classification::RoutingClassificationSite {
                task,
                moment: glasshouse::routing::session::RoutingMoment::SessionStart,
                harness: Some(selection.id()),
                harness_named: harness.is_some(),
                to,
                fresh,
                destinations: &destinations,
                health: health.pool(),
                sticky: Some(&sticky_cache),
                text_cache: Some(&text_cache),
                protected_capacity_price,
            },
        );
        let inputs = glasshouse::routing::session::RouterInputs {
            overrides: &overrides,
            health: health.pool(),
            now: std::time::Instant::now(),
            requirements: classified
                .as_ref()
                .map(|classified| classified.answer.requirements())
                .unwrap_or_default(),
        };
        // Line 1602 on the path that acts, not only on the one that reports.
        //
        // Two of these are the user's flags. The other two are statements they
        // already made by typing something else, and reading them as anything but
        // "this launch is a fresh one" would be a router overruling a person:
        // `--profile` names the profile a *new* session should run under, and
        // `--from-checkpoint` hands a new session its opening prompt. Neither is
        // a thing to do to a session that is already going.
        let user_override = if to.is_some() || fresh {
            crate::commands::routing_destinations::routing_override(to, fresh)
        } else if from_checkpoint.is_some() {
            glasshouse::routing::session::RoutingOverride::fresh()
        } else if let Some(name) = profile_name {
            glasshouse::routing::session::RoutingOverride::to(
                crate::commands::routing_destinations::fresh_destination_id(selection.id(), name),
            )
        } else {
            glasshouse::routing::session::RoutingOverride::none()
        };
        let router = crate::commands::routing_destinations::session_router(
            runtime,
            &effective,
            user_override,
        );
        let routed = router.choose(
            glasshouse::routing::session::RoutingMoment::SessionStart,
            None,
            &destinations,
            &inputs,
        );
        // Phase 56 line 1954, the half a ranking cannot express. `choose`
        // answers `None` when every destination failed a hard constraint and
        // there is no current session to hold — and until now this launch read
        // that as "nowhere to go" and started `fresh_profile` anyway, the
        // silence Phase 35D's decision 3 recorded. A destination the user's
        // own entitlement rule refused must not be started by that fallback,
        // so the same gate is asked what it refused, and the launch stops by
        // name. Only the entitlement constraint is read here: a protocol or
        // tool-semantics refusal of the sole destination keeps the behaviour it
        // had, which `profile::resolve` already refuses on its own terms.
        let nowhere_to_go = routed.is_none();
        if nowhere_to_go
            && let Some(refused) = router.refused(&destinations, &inputs).into_iter().find(
                |(destination, constraint)| {
                    destination.is_fresh()
                        && destination.launch_profile() == fresh_profile
                        && matches!(
                            constraint,
                            glasshouse::routing::HardConstraint::Entitlement { .. }
                        )
                },
            )
        {
            let (_, constraint) = refused;
            let name = match &constraint {
                glasshouse::routing::HardConstraint::Entitlement { entitlement, .. } => {
                    entitlement.clone()
                }
                _ => unreachable!("filtered to the entitlement constraint above"),
            };
            eprintln!(
                "glasshouse: not starting this session — {}, and launch profile `{fresh_profile}` \
                 would charge it. Change the rule under `[entitlements.{name}]`, or launch \
                 under a profile whose entitlement serves this work.",
                constraint.reason().unwrap_or_default()
            );
            return Ok(ExitCode::FAILURE);
        }
        if let Some(classified) = &classified {
            // The classification the decision just acted on, in the same words
            // `glasshouse route --task` prints — including whether line 1459's
            // conservative rules fired. And the end of what routing added to
            // this launch (line 1849), recorded before anything below opens a
            // ledger handle of its own.
            eprintln!("glasshouse: {}", classified.answer.explain());
            // Lines 1565 and 1566, on the path that acts: a moved tier is
            // said before the destination it produced is announced below,
            // and recorded so it can be counted. `glasshouse route` renders
            // the same movement in its report and records nothing.
            if let Some(routed) = &routed
                && let Some(movement) = routed.movement().filter(|movement| movement.fired())
            {
                eprintln!(
                    "glasshouse: tier {}. `glasshouse route --task ...` says why; `--to <id>` \
                     overrules it.",
                    movement.describe()
                );
                record_tier_movement(runtime, selection.id(), movement);
            }
            record_routing_latency(
                runtime,
                routing_started,
                routing_started_at_unix,
                selection.id(),
                &classified.answer,
            );
        }
        // Line 1970, on the path that acts — and OUTSIDE the classified
        // guard, because a fallback is not a classification and a launch
        // that states no task can still make one. The account the broker
        // left is said before the destination it produced is announced
        // below, and recorded so it can be counted. `glasshouse route`
        // renders the same fallback in its report and records nothing.
        if let Some(routed) = &routed
            && let Some(fallback) = routed.fallback()
        {
            eprintln!(
                "glasshouse: {}. `glasshouse route` says why.",
                fallback.describe()
            );
            record_entitlement_fallback(
                runtime,
                selection.id(),
                routed.chosen(),
                fallback,
                routed.cost(),
            );
        }
        (routed, classified, health)
    };

    // A destination the router chose is announced before anything happens,
    // never after: a person who did not want their previous session continued
    // needs to read that on the way in, while `--fresh` is still an answer.
    if let Some(routed) = &routed {
        // Map lines 1829 and 1830: this is the one moment both facts are
        // known, and the one the two `eprintln!`s below already render for a
        // person without either being counted anywhere. `glasshouse route`
        // (main.rs:1462) reaches the same router but never this branch, so
        // it never reaches this call either — it reports without acting.
        glasshouse::evaluation::record_routing_decision(
            runtime,
            routed.chosen().id(),
            routed.chosen().is_fresh(),
            routed.overrode(),
            glasshouse::evaluation::now_unix(),
        );
        // -------------------------------------------------------------------
        // Line 1720: *"surface automation decisions instead of silently
        // moving work between sessions."* Every automated outcome this
        // function can reach says so here, before it happens — an override
        // that was refused, an override that was honoured, a continuation, or
        // a fresh session the ranking chose over destinations it could have
        // continued. The one case with nothing to announce is a project with
        // no alternative: a ranking of one destination moved nothing.
        // -------------------------------------------------------------------
        if let Some(refusal) = routed.override_refused() {
            eprintln!(
                "glasshouse: your routing override was not applied — {refusal}. The ranking's \
                 own choice was used instead."
            );
        }
        if let Some(automatic) = routed.overrode() {
            eprintln!(
                "glasshouse: going to `{}` because you named it; the ranking would have chosen \
                 `{automatic}`. `glasshouse route` says why.",
                routed.chosen().id()
            );
        }
        if let glasshouse::routing::session::Continuation::Existing(warm) =
            routed.chosen().continuation()
        {
            eprintln!(
                "glasshouse: continuing session {} ({}, idle {}) rather than starting a new one; \
                 pass --fresh to start one anyway.",
                routed.chosen().id(),
                warm.state,
                crate::commands::shared::format_age(
                    glasshouse::provider::cache::now_unix_seconds() - warm.idle_seconds
                )
            );
            // Map lines 1835 and 1854, on the branch where no session has to
            // be minted for the route to have somewhere to land: the
            // destination *is* the session this work continues, so its id is
            // already the session id. The fresh branch records the same two
            // rows once `store.create` below has produced one.
            let observed_at = glasshouse::evaluation::now_unix();
            glasshouse::evaluation::record_routed_session(
                runtime,
                routed.chosen().id(),
                routed.chosen().id(),
                routed_cost_class(&user, project.as_ref(), routed.chosen()),
                routing_evidence_for(&health, routed.chosen(), observed_at),
                routed_tier(classified.as_ref()),
                observed_at,
            );
            // Map lines 1757 and 1766, on the same instant: the rationale
            // behind the destination this row just attributed.
            glasshouse::evaluation::record_session_route(
                runtime,
                routed.chosen().id(),
                routed.chosen().id(),
                routed.explanation(),
                observed_at,
            );
            // Map line 1855's token half, on the same instant: the launch's
            // own expected output-token size for the class it was classified
            // as, written only when there is a real median to write.
            record_consumption_estimate(
                runtime,
                routed.chosen().id(),
                classified.as_ref(),
                observed_at,
            );
            // Line 1467: the session this work landed on is the sticky one.
            remember_classification(&sticky_cache, classified.as_ref(), routed.chosen().id());
            // Line 1716, on the path that migrates. Taken before
            // `resume_session` so the checkpoint describes the moment the
            // work left, and after the announcement above so the order a
            // person reads matches the order things happened.
            if checkpoint_first {
                crate::commands::resume::checkpoint_before_moving(
                    runtime,
                    Some(routed.chosen().id()),
                )?;
            }
            // Phase 17 line 760, on the branch that continues rather than
            // mints: the session this pane now hosts was recorded somewhere
            // else, so its record is moved here before it is resumed. Opened
            // and dropped before `resume_session` opens its own connection —
            // sequential, never two live handles (practice §65).
            if let Some(pane) = &hosted_pane {
                let sessions = ProjectSessions::open(runtime)?;
                sessions.store().set_presentation(
                    &SessionId::new(routed.chosen().id()),
                    SessionPresentation::External,
                    Some(pane.as_str()),
                )?;
            }
            return crate::commands::resume::resume_session(
                runtime,
                routed.chosen().id(),
                harness_args,
                headless,
                crate::commands::resume::RouteOnResume::AlreadyRouted,
            );
        }
        // A fresh session the *ranking* chose, with sessions it could have
        // continued and did not. Said out loud for the same reason the
        // continuation above is: the person is about to start over, and the
        // moment to learn that this project already had somewhere warm to go
        // is before the new session exists rather than after.
        //
        // Only when the ranking chose it. A `--fresh` the person typed is
        // already reported by the override line above, and repeating it as an
        // automation decision would attribute their own choice to Glasshouse.
        if routed.overrode().is_none() {
            let continuable = routed
                .considered()
                .iter()
                .filter(|(destination, _)| !destination.is_fresh())
                .count();
            if continuable > 0 {
                eprintln!(
                    "glasshouse: starting a new session; the ranking weighed {continuable} \
                     session(s) this project could have continued and preferred a new one. \
                     `glasshouse route` says why, and `--to <id>` overrules it."
                );
            }
            // Map line 372: no profile was pinned, so this fresh destination
            // is the ranking's own pick among every enabled profile rather
            // than the implied fallback — said out loud, reusing the same
            // `render()` `glasshouse route` prints rather than inventing a
            // second explanation for the same decision.
            if profile_selection {
                eprintln!(
                    "glasshouse: launching under profile `{}` — automatic routing's choice \
                     among the enabled profiles.\n{}",
                    routed.chosen().launch_profile(),
                    routed.render()
                );
            }
        }
    }

    // Line 1716 on every path that did **not** migrate into an existing
    // session — the fresh launches, and a project with nothing recorded. The
    // flag is a no-op here and says so rather than passing silently, because
    // a person who asked for a checkpoint and got none needs to know which of
    // the two happened.
    if checkpoint_first {
        crate::commands::resume::checkpoint_before_moving(runtime, None)?;
    }

    // The chosen fresh destination names the profile this launch resolves.
    // `routed` is `None` only when there was nowhere at all for the work to
    // go, which for a fresh launch means no profile resolved for this
    // harness; the implied Native profile always does, so the fallback below
    // is unreachable in practice and is written as the same answer this
    // function gave before the router existed rather than as a panic.
    let requested_profile = routed
        .as_ref()
        .map(|routed| routed.chosen().launch_profile().to_owned())
        // …and, since line 1712, the ordinary answer whenever routing is off:
        // the profile this launch already resolved, which is `--profile`, the
        // profile named inside a `--to fresh:<harness>:<profile>`, or the
        // implied Native one. Reading `fresh_profile` rather than
        // `profile_name` is what makes a `--to` identifier mean the same
        // thing with the ranking off as it does with it on.
        .unwrap_or_else(|| fresh_profile.to_owned());

    // Resolve the launch profile *before* anything is recorded or started.
    // A refusal here must cost nothing: no session record, no process. See
    // `glasshouse::profile::resolve`'s doc for why a refusal never falls back
    // to a different mode.
    //
    // Resolved *before* the response profile below, on purpose: line 353's
    // sixth axis lives on this profile, and the response request has to be
    // able to read it.
    let launch_profile = match effective.launch_profile(&requested_profile, selection.id()) {
        Ok(resolved) => resolved.value,
        Err(err) => {
            eprintln!("glasshouse: {err}");
            return Ok(ExitCode::FAILURE);
        }
    };

    // Phase 56 line 1954, on the path that starts a session: which
    // entitlement it will be charged to, said before anything is recorded or
    // started — and the harness half of that entitlement's rule applied once
    // more here, through the same `EntitlementRules::refusal` the router
    // asked, for the one launch the router never saw: line 1712's routing-off
    // launch, where `routing_destinations` and `choose` do not run at all. A
    // rule about *this harness* needs no classification to apply; the tier
    // half does, and it is the router's (above). A contradiction in the
    // `[entitlements]` tables is refused here for the same reason a bad
    // profile is: it must cost nothing.
    //
    // 56A line 1969, the routing half: when the router chose a candidate
    // that carries an entitlement, THAT entry serves — resolved by name from
    // the same tables, never re-derived through the one-account lookup,
    // which a provider several accounts legitimately back would refuse as
    // ambiguous. The one-account lookup remains the answer for a launch the
    // router never saw (routing off) and for a chosen candidate no entry
    // describes; with routing off, a several-account provider is still
    // refused, because without the broker's ranking there is nothing honest
    // to pick an account by.
    let chosen_entitlement_name = routed
        .as_ref()
        .and_then(|routed| routed.chosen().entitlement())
        .map(|entitlement| entitlement.name().to_owned());
    // `mut`: the `GlasshouseGateway` arm below overwrites this once the
    // gateway has started and its serving provider is known — see the
    // consult after `start_if_required_with_degrade_sink`. For every other
    // backend this is the final value.
    let mut entitlement = match &chosen_entitlement_name {
        Some(name) => match effective.entitlements() {
            Ok(pool) => pool.into_iter().find(|entry| entry.name() == name),
            Err(err) => {
                eprintln!("glasshouse: {err}");
                return Ok(ExitCode::FAILURE);
            }
        },
        None => match effective.entitlement_for(launch_profile.harness, &launch_profile.backend) {
            Ok(entitlement) => entitlement,
            Err(err) => {
                eprintln!("glasshouse: {err}");
                return Ok(ExitCode::FAILURE);
            }
        },
    };
    // Every backend but the gateway asks and announces right here, before
    // anything else is resolved. A `GlasshouseGateway` profile cannot be
    // asked yet — `entitlement_for` returns `None` for it by construction,
    // because no provider is assigned until the gateway starts below — so
    // its consult, refusal and announcement happen once that provider is
    // known (see `start_if_required_with_degrade_sink`, further down).
    let is_gateway_backend = matches!(
        launch_profile.backend,
        glasshouse::profile::BackendResource::GlasshouseGateway
    );
    if !is_gateway_backend {
        if let Some(message) = crate::commands::routing_destinations::entitlement_refusal_message(
            entitlement.as_ref(),
            launch_profile.harness,
            &launch_profile.name,
        ) {
            eprintln!("{message}");
            return Ok(ExitCode::FAILURE);
        }
        crate::commands::routing_destinations::announce_entitlement(
            entitlement.as_ref(),
            &launch_profile,
            None,
        );
    }

    // Phase 9K: the response profile is resolved *here*, on the production
    // launch path, through the same `EffectiveConfig::response_profile`
    // `glasshouse response` prints — so what a user is shown and what a
    // session gets cannot disagree. Line 617 is why it happens at session
    // creation rather than per turn: the instruction becomes part of the
    // session's system prefix, and moving it later would invalidate the
    // prompt cache on every turn.
    //
    // Phase 9A line 353's sixth axis, given a production caller: a launch
    // profile that names a response preset supplies it at the `Session`
    // layer of `EffectiveConfig::response_stack` — the layer that doc already
    // describes as "a preset named for this session", which is exactly what
    // choosing this profile is. An explicit `--response-preset` (or
    // `--response-role`'s own preset) on the command line is a stronger,
    // one-time statement than a profile's standing default, so it is only
    // consulted when the request came with none of its own. This is
    // deliberately *not* a seventh `PrecedenceLayer`: the map's line 596
    // fixes that chain at six named layers and the box for it is already
    // closed, so a profile's answer has to arrive through one of the six
    // rather than beside them.
    let mut response_request = response.clone();
    if response_request.session_preset.is_none()
        && let Some(preset) = &launch_profile.response_preset
    {
        response_request.session_preset = Some(preset.clone());
    }
    let response_profile = effective.response_profile(&response_request);
    for problem in response_profile.problems() {
        // Reported, never guessed at — see `ResponseProfileEntry`.
        eprintln!("glasshouse: {problem}");
    }
    // Line 605: a session's response profile is always explicit. A worker
    // does not inherit a communication style from whatever started it; the
    // role was resolved above and the mechanism is recorded below.
    //
    // `mut`: `GH-LAUNCH-BRIEFING`'s rung one appends a second additive block
    // onto this same `Application`, below, once the session id exists.
    let mut response_application =
        glasshouse::harness::response::apply(selection.adapter(), response_profile.resolved());
    tracing::info!(
        harness = selection.id().slug(),
        profile = %config::response::one_line(&response_profile),
        mechanism = response_application.mechanism().category(),
        applied = %response_application.mechanism().describe(),
        "resolved the session's response profile"
    );

    // Resolved here, beside the profile, and for the same reason: a bad
    // identifier must cost nothing. No session record, no process — see
    // `glasshouse::profile::resolve`'s doc.
    let bootstrap =
        match crate::commands::resume::resolve_bootstrap_prompt(runtime, from_checkpoint) {
            Ok(prompt) => prompt,
            Err(err) => {
                eprintln!("glasshouse: {err:#}");
                return Ok(ExitCode::FAILURE);
            }
        };

    let acknowledged_bypass = effective.bypass_acknowledged(selection.id()).value;
    // A direct-provider profile names a provider; the *lookup* is the
    // caller's job, so `glasshouse::profile` never has to import
    // `glasshouse::config`. An unknown name is reported exactly as an unknown
    // profile name is, one step above: a line on stderr, `ExitCode::FAILURE`,
    // nothing recorded and nothing started.
    let provider = match &launch_profile.backend {
        glasshouse::profile::BackendResource::DirectProvider { provider } => {
            match effective.configured_provider(provider) {
                Ok(resolved) => Some(resolved.value),
                Err(err) => {
                    eprintln!("glasshouse: {err}");
                    return Ok(ExitCode::FAILURE);
                }
            }
        }
        _ => None,
    };
    // Phase 9E: prefer the operating system's own secure store where one is
    // available, and fall back to the environment where it is not — the
    // fallback is *labelled* rather than silent, so `glasshouse doctor` and
    // the settings surface both say which store answered.
    //
    // This is the line that puts the native store on the path that actually
    // starts a session. Without it "prefer the macOS Keychain" would be true
    // of the store, of `doctor` and of settings, but not of `glasshouse run`
    // — and a mechanism with no production caller does not get its box.
    let secrets = glasshouse::secret::native::PreferNativeSecretStore::detect();

    // Phase 9G: whether a local gateway exists at all is decided from the
    // active launch profiles, never from a flag — see
    // `glasshouse::gateway::gateway_is_required`. It now has to be bound
    // *before* the resolution below, because a gateway-backed profile
    // resolves into this gateway's own address and token. Nothing is bound
    // and no credential is resolved for a launch that needs no gateway: the
    // upstream is a closure, called only after the predicate says yes.
    // The guard lives to the end of this function, so the listener goes away
    // with the instance on every path out.
    //
    // Map line 1735: the relay is built here, before the gateway, because the
    // sink has to exist before the thing it writes into does — see
    // `DegradeRelay`. It is installed below, once the session record and the
    // event recorder are both real.
    let degrade_relay = crate::commands::resume::DegradeRelay::new();
    let gateway = match glasshouse::gateway::start_if_required_with_degrade_sink(
        std::slice::from_ref(&launch_profile),
        || crate::commands::resume::gateway_upstream(&user, project.as_ref(), &effective, &secrets),
        Some(glasshouse::provider::telemetry::GatewayQuotaCache::new(
            runtime.paths(),
        )),
        // Phase 33A: the routing evidence ledger, reached from the shipped
        // binary only here — the same shape `GatewayQuotaCache` had for a
        // batch before `QUOTA-LIVE` wired it.
        //
        // **Never `?`.** This argument is evaluated on every launch, gateway
        // or not, and a ledger that cannot be opened must cost an observation
        // rather than the user's session. Telemetry is the one subsystem in
        // this binary whose failure is always survivable, and a `?` here would
        // make a read-only data directory or a locked database into "glasshouse
        // will not start".
        crate::commands::resume::evidence_ledger(runtime, std::slice::from_ref(&launch_profile)),
        // Capability map lines 1311/1321/1322/1324: the durable resource-
        // health cache, the same additive shape as the quota cache above and
        // read back by exactly the same `glasshouse resources` invocation.
        Some(glasshouse::provider::telemetry::GatewayHealthCache::new(
            runtime.paths(),
        )),
        Some(degrade_relay.sink()),
        // Capability map line 1851: what the failure-domain term did to each
        // failover this gateway takes. A sink rather than a ledger handle —
        // `gateway::session::FailoverPreventionSink`'s own doc comment has
        // practice §65's reason — so the evaluation ledger is opened, written
        // and dropped inside the exchange thread that decided the failover,
        // and never held open across the provider hop.
        Some(crate::commands::routing_destinations::failover_prevention_sink(runtime)),
    ) {
        Ok(gateway) => gateway,
        Err(err) => {
            eprintln!("glasshouse: {err}");
            return Ok(ExitCode::FAILURE);
        }
    };

    // Phase 56/1954, the gateway shape: now that the gateway has started,
    // its serving provider is known (`Gateway::serving_provider`), and this
    // asks the same question the direct/native path already asked above —
    // the same `EntitlementRules::refusal` check, the same refusal text
    // (`entitlement_refusal_message`), the same announcement — for the one
    // launch that could not be asked before the gateway existed.
    // `pool_entitlements_for` still returns nothing for `GlasshouseGateway`
    // (map line 1954's cause 3 stays true of the *router*), so
    // `chosen_entitlement_name` above is never `Some` for this backend; this
    // is the whole of the gateway's consult.
    if is_gateway_backend {
        let gateway_provider = gateway.as_ref().map(|gateway| gateway.serving_provider());
        entitlement = match gateway_provider {
            Some(provider) => match effective.entitlement_for_provider(provider) {
                Ok(entitlement) => entitlement,
                Err(err) => {
                    eprintln!("glasshouse: {err}");
                    return Ok(ExitCode::FAILURE);
                }
            },
            None => None,
        };
        if let Some(message) = crate::commands::routing_destinations::entitlement_refusal_message(
            entitlement.as_ref(),
            launch_profile.harness,
            &launch_profile.name,
        ) {
            eprintln!("{message}");
            return Ok(ExitCode::FAILURE);
        }
        crate::commands::routing_destinations::announce_entitlement(
            entitlement.as_ref(),
            &launch_profile,
            gateway_provider,
        );
    }

    // 56A line 1969: the overlay may only resolve the serving account's own
    // credential — see `EntitlementScopedSecrets`. With zero or one
    // configured entitlement the foreign list is empty or names other
    // resources' accounts, and resolution answers exactly as before. The
    // gateway's own upstream resolution above deliberately keeps the
    // unwrapped store: which account serves a gateway-backed session is
    // assigned when the session starts (56A-4), not at this launch.
    let scoped_secrets = EntitlementScopedSecrets {
        inner: &secrets,
        foreign: effective
            .foreign_entitlement_credential_refs(entitlement.as_ref().map(|e| e.name())),
    };
    let resolution = glasshouse::profile::Resolution {
        adapter: selection.adapter(),
        acknowledged_bypass,
        provider: provider.as_ref(),
        secrets: &scoped_secrets,
    };
    // Phase 9J line 576: the user's configured native-pairing preference and
    // corrections, resolved here — the same place `provider` above is — and
    // handed to the gateway path rather than looked up inside `profile/**`,
    // which may not import `crate::config`. See `resolved_gateway_pairing`.
    let pairing = resolved_gateway_pairing(&effective);
    let mut overlay = match glasshouse::profile::resolve_with_gateway(
        &launch_profile,
        &resolution,
        gateway.as_ref(),
        &pairing,
    ) {
        Ok(overlay) => overlay,
        Err(refusal) => {
            eprintln!("glasshouse: {refusal}");
            return Ok(ExitCode::FAILURE);
        }
    };

    // Phase 9F line 468: verify the combination this profile resolved to
    // before the session starts, when a cheap check is available.
    //
    // **After the resolution, never before it.** The backend is chosen from
    // the profile's declaration alone, and running the check on this side of
    // `resolve_with_gateway` is what makes that structurally true on the
    // production path rather than merely asserted in a unit test — see
    // `profile::preflight`'s own doc and
    // `a_capability_probe_cannot_influence_which_backend_resolve_selects`.
    //
    // And before `ProjectSessions::open` below, which is what "before
    // starting" buys the user: whatever this reports, they read it while
    // nothing has been recorded and no process exists.
    //
    // It reports; it decides nothing. A profile with no check available —
    // every `Native` and every gateway-backed one, so every launch that did
    // not name a direct provider — pays no request and gets one line in the
    // log. A check that fails still starts the session, on purpose: see the
    // four reasons on `profile::Preflight`, of which the shortest is that a
    // `GET` to a base URL serving none answers `404` for a healthy provider.
    let preflight = glasshouse::profile::preflight(&launch_profile, &resolution);
    tracing::info!(
        profile = %launch_profile.name,
        backend = %launch_profile.backend.slug(),
        preflight = preflight.summary(),
        "pre-flight capability check"
    );
    if let Some(warning) = preflight.warning() {
        // Not a refusal, and it must not read like one — the next thing this
        // process does is start the session.
        eprintln!("glasshouse: pre-flight check did not confirm {warning}");
        eprintln!("glasshouse: starting the session anyway; this check never refuses a launch.");
    }

    // Record the session before the harness exists, so a session that dies
    // during startup still leaves a trace. Failing to open the project
    // database is fatal here rather than a warning: `bootstrap` already
    // validated it, so a failure now means the project's state directory
    // broke underneath us, and starting a session Glasshouse cannot account
    // for is worse than not starting one.
    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    // Minted before the process exists, for a harness that accepts one, so
    // the session is identifiable even if the harness dies during startup.
    let native = selection
        .assigns_native_session_id()
        .then(|| store.new_native_session_id())
        .transpose()?;
    // The presentation is recorded before the process exists and is the same
    // value `run_headless` starts the session under, so a session's stored
    // presentation and its running one cannot disagree — which is what lets
    // the shell's overview say `headless` about a session it did not start.
    //
    // `External` when a pane hosts this process (Phase 17 line 760): the
    // runtime below still starts the session as embedded or headless —
    // that is what it *is* to the pane's terminal — and only the record
    // says the pane is where a person will find it.
    let presentation = if hosted_pane.is_some() {
        SessionPresentation::External
    } else if headless {
        SessionPresentation::Headless
    } else {
        SessionPresentation::Embedded
    };
    // Phase 10 line 645: the seven facts, recorded as seven facts.
    //
    // `pairing` is asked once and its three answers are read off separately —
    // the model, the class and the wire protocol — because they are three
    // different questions about the same session and a single "agent" string
    // holding all of them is exactly what this phase's second architectural
    // requirement forbids. The response profile beside them is communication
    // policy and nothing else: it cannot say which model ran, and the model
    // cannot say how the answer should read.
    let pairing =
        crate::commands::routing_destinations::session_pairing(&effective, &launch_profile);
    let record = store.create(
        NewSession::embedded(selection.id().slug())
            .with_presentation(presentation)
            .with_presentation_ref(hosted_pane.as_ref().map(|pane| pane.as_str().to_owned()))
            .with_native_session_id(native.clone())
            .with_launch_profile(Some(launch_profile.name.clone()))
            .with_backend_resource(Some(launch_profile.backend.slug()))
            .with_model(Some(pairing.model().clone()))
            .with_pairing_class(Some(session::session_pairing_class(pairing.class())))
            .with_protocol(Some(session::session_protocol(pairing.route().protocol)))
            .with_response_profile(Some(response_profile.resolved().profile()))
            .with_response_mechanism(Some(session::session_response_mechanism(
                response_application.mechanism(),
            )))
            // Phase 40 line 1646: the session this one was bootstrapped from,
            // if this launch is a `--from-checkpoint` handoff. `None` for
            // every other launch — a session not started from a checkpoint
            // must never record an invented source.
            .with_source_session(bootstrap.as_ref().map(|(_, source)| source.clone()))
            // Phase 56A line 1972, the durable half: the account that will be
            // charged for this session, recorded by name so that
            // `glasshouse entitlements` can answer *what it served* later.
            //
            // `entitlement` is the value resolved above and already announced
            // to the user by `announce_entitlement` — deliberately the same
            // binding and not a second lookup, so what a person was told and
            // what the record says cannot disagree. Where the router ran that
            // is `Routed::chosen`'s own account re-resolved by name; where it
            // did not (a routing-off launch), it is the one-account lookup,
            // which refuses a several-account provider rather than guessing.
            // Either way it is the entitlement that serves, which is the only
            // thing this column may hold.
            //
            // `backend_resource` above stays exactly as it was: it records
            // the KIND of resource and this records the INSTANCE, and the two
            // accounts of one vendor that motivate the column both slug to
            // `native` there.
            .with_entitlement(entitlement.as_ref().map(|entry| entry.name().to_owned())),
    )?;
    // Capability map line 2019 and `glasshouse::database` migration 24: tell
    // the gateway which session it is serving, so every routing-observation
    // row it writes from here on can name one.
    //
    // **Here, and not beside the gateway's own start**, for
    // `record_routing_decision`'s reason a few lines down: the id does not
    // exist up there. The gateway is started before the record so that the
    // overlay can name its address, and the record is minted by
    // `store.create` — so this is the first line at which both are real, and
    // it is still before the harness is spawned, which is before any
    // exchange can arrive.
    if let Some(gateway) = gateway.as_ref() {
        gateway.routing().serve_session(record.id.as_str());
        // Map line 1301 (`GH-TASK-CLASS-COST-JOIN`): the same routing
        // decision `record_routing_latency` already read a class off, so
        // every row this gateway's own `record_routing_observation` writes
        // from here on can join to it too. `None` for a routing-off launch
        // or one that classified no task — `classified` is `None` in both,
        // and the gateway stamps `NULL` exactly as it does when
        // `serve_session` above is never called at all.
        gateway.routing().serve_task_class(
            classified
                .as_ref()
                .map(|classified| classified.answer.task_class()),
        );
    }
    // Line 1467, the fresh half: the session just recorded is the one the
    // next low-risk turn will be in.
    remember_classification(&sticky_cache, classified.as_ref(), record.id.as_str());

    // `GH-LAUNCH-BRIEFING`: this project's memory, briefed to this session the
    // same way a door-spawned one already is — map lines 1125-1135, applied
    // to the CLI launch path. After `store.create` (the session id this
    // records against exists) and before `install_session_document` below
    // (rung one still needs to append to `response_application`'s
    // arguments). Rung two (headless, no adapter additive mechanism) cannot
    // be delivered yet — no session runtime exists — so it rides forward as
    // `deferred_briefing` into `run_headless`.
    let launch_briefing = brief_launch_session(
        runtime,
        &record.id,
        selection.adapter(),
        headless,
        no_memory,
        effective.inject_memory_at_launch().value,
        bootstrap.as_ref().map(|(text, _)| text.as_str()),
        &mut response_application,
    );
    let mut deferred_briefing = None;
    match launch_briefing {
        LaunchBriefing::Delivered(line) => eprintln!("glasshouse: {line}"),
        LaunchBriefing::Deferred(briefing) => deferred_briefing = Some(briefing),
        LaunchBriefing::NotBriefed(reason) => eprintln!("glasshouse: not briefed: {reason}"),
        LaunchBriefing::Nothing => {}
    }

    // Phase 21K line 1008: the person's per-task guardrail override,
    // recorded before the harness starts so that no preflight the agent runs
    // in this session answers without it. Best effort, like the hook
    // installation below: a launch is not refused for a bookkeeping row,
    // but the failure is said out loud, because a session gated against the
    // user's stated wish is the one outcome the override exists to prevent.
    if let Some(kind) = guardrail {
        match glasshouse::guardrails::record_override(
            runtime,
            record.id.as_str(),
            kind,
            glasshouse::guardrails::Origin::User,
        ) {
            Ok(row) => tracing::info!(
                session = %record.id,
                guardrail = %kind,
                seq = row.seq,
                "recorded a per-task guardrail override"
            ),
            Err(err) => eprintln!(
                "glasshouse: warning: `--guardrail {kind}` could not be recorded for session \
                 {}: {err:#}",
                record.id
            ),
        }
    }

    // Read before the harness runs, for a harness that keeps its identifiers
    // in one shared index: such an index carries no per-entry timestamp, so
    // "this project's entry changed during the session" is the only thing
    // standing between Glasshouse and adopting a stale entry somebody else's
    // session refreshed. Empty, and free, for every other harness — see
    // `session::native_id::snapshot`.
    let index_before = session::native_id::snapshot(&record.harness, runtime.project().root());

    // Map lines 1835 and 1854: the route this launch chose, attributed to the
    // session it just produced.
    //
    // **Here, and not beside `record_routing_decision` above, because the id
    // does not exist up there.** A fresh launch mints its session id at
    // `store.create`, so a decision recorded before it can carry no session
    // and an outcome learned a turn later would have nothing to attach to.
    // Recording the decision itself later was the alternative and it is
    // rejected: lines 1829 and 1830 count decisions, and a launch refused
    // while resolving its profile made a decision and never reaches this
    // line. So the decision keeps its own moment and this row records what
    // that decision became — two rows, never an `UPDATE` of one.
    if let Some(routed) = &routed {
        let observed_at = glasshouse::evaluation::now_unix();
        glasshouse::evaluation::record_routed_session(
            runtime,
            record.id.as_str(),
            routed.chosen().id(),
            routed_cost_class(&user, project.as_ref(), routed.chosen()),
            routing_evidence_for(&health, routed.chosen(), observed_at),
            routed_tier(classified.as_ref()),
            observed_at,
        );
        // Map lines 1757 and 1766, on the same instant: the rationale
        // behind the destination this row just attributed.
        glasshouse::evaluation::record_session_route(
            runtime,
            record.id.as_str(),
            routed.chosen().id(),
            routed.explanation(),
            observed_at,
        );
        // Map line 1855's token half, on the same instant: the launch's own
        // expected output-token size for the class it was classified as,
        // written only when there is a real median to write.
        record_consumption_estimate(
            runtime,
            record.id.as_str(),
            classified.as_ref(),
            observed_at,
        );
    }

    tracing::info!(
        session = %record.id,
        harness = selection.id().slug(),
        // The resolved path and the layer that chose it are diagnostics a
        // user needs when a session starts the wrong binary. Neither is a
        // secret; harness *arguments* are never logged, because those can
        // carry session tokens.
        executable = %selection.executable().path().display(),
        source = %selection.source(),
        root = %runtime.project().display_root().display(),
        profile = %launch_profile.name,
        backend = %launch_profile.backend.slug(),
        mechanisms = %crate::commands::resume::mechanism_summary(&overlay),
        presentation = %presentation,
        "opening a harness session"
    );

    // Phase 9A line 362. The generated configuration documents this profile
    // needs are written now — the session directory exists only once the
    // record does — into the directory Glasshouse owns for this session, and
    // removed when `_generated` drops at the end of this function, which is
    // after `session::attach` has returned. Fatal rather than best effort: a
    // harness pointed at a configuration document that was not written would
    // start on the user's own account instead of the backend they asked for.
    let session_dir = runtime.session_dir(record.id.as_str());
    let _generated =
        overlay.install(glasshouse::harness::GeneratedConfigSite::new(&session_dir))?;

    // Adapter args (and, for a harness that lets Glasshouse assign one, its
    // session identifier) first — no user arguments yet, so the overlay's
    // arguments land strictly between them and the user's own.
    let mut args = selection.start_args(native.as_deref(), std::iter::empty::<&str>());
    let project_hooks_consent = effective.project_hooks(selection.id()).value;
    args.splice(
        0..0,
        crate::commands::resume::install_session_document(
            runtime,
            &selection,
            &record.id,
            project_hooks_consent,
            &response_application,
        ),
    );
    // Map lines 1991-1996: the context firewall's Claude Code bridge. Never
    // changes `args` itself — it only merges a `PostToolUse` entry into the
    // settings document `install_session_document` just wrote (a second
    // `--settings` flag would silently discard the first, so this can never
    // add one of its own), which keeps `mode = "off"` byte-identical to a
    // session built before this phase existed by construction: the function
    // returns before touching anything in that case.
    //
    // Map lines 2023/2024: the resolved entitlement and this launch's own
    // backend/profile name travel in too, so the reduction policy can be
    // keyed on the entitlement's kind and overridden by the profile or the
    // entitlement — never by the firewall core or the hook subprocess, which
    // stay entitlement-blind (see `install_context_firewall_hook`'s own doc).
    crate::commands::resume::install_context_firewall_hook(
        runtime,
        &selection,
        effective,
        &session_dir,
        entitlement.as_ref(),
        &launch_profile.backend,
        &launch_profile.name,
        &record.id,
    );
    // Map lines 2402-2405: Phase 60's edit-intent coordination hook, merged
    // into the same settings document and after the firewall's own entry —
    // the two touch different event keys, so neither can disturb the other
    // (`claude_code::merge_hook_entry`, pinned by
    // `both_tool_hooks_coexist_in_one_document`). Ordered second so that a
    // failure here is one a session with a working firewall survives; both
    // are best effort and neither touches `args`.
    install_edit_intent_hook(&selection, effective, &session_dir, &record.id);
    let mut launch = HarnessLaunch::new(selection.into_executable(), runtime.project()).args(args);
    // Map line 1973: the child inherits this process's environment, so
    // another entitlement's credential variable would reach a session that
    // account is not serving. Removed before the overlay applies, so the
    // overlay's own `env` entries — the serving credential among them —
    // always win per key.
    for var in effective.foreign_entitlement_credential_vars(entitlement.as_ref().map(|e| e.name()))
    {
        launch = launch.env_remove(var);
    }
    // The overlay is the only thing that may put its own arguments or
    // environment onto the launch — see `LaunchOverlay::apply`'s doc.
    let launch = overlay.apply(launch);
    // A checkpoint's handoff, if one was named, as the harness's opening
    // prompt — exactly where a person typing it after `--` would have put it.
    let launch = match &bootstrap {
        Some((prompt, _)) => launch.args(std::iter::once(prompt.as_str())),
        None => launch,
    };
    // The user's own `--` arguments always come last, so they can win.
    let launch = launch.args(harness_args.iter().map(String::as_str));

    // From here on, a bookkeeping failure must never change what the user
    // sees. The session is real and running; losing a state transition is a
    // diagnostics problem, whereas turning it into an error would make a
    // database hiccup look like a harness failure.
    crate::commands::resume::note_lifecycle(&store, &record.id, SessionLifecycle::Running);

    // Phase 18's "record session creation events", on the path that actually
    // creates one from the command line. The shell's own runtime publishes
    // the same event for a session started there; this is the other entry
    // point, and a log that only knew about one of them would be a log with a
    // hole in it exactly where a user was not using the interactive
    // interface.
    let events = Arc::new(crate::commands::resume::EventRecorder::open(runtime));
    events.record(&record.id, LifecycleEvent::SessionStarted);

    // Map line 1735, the other half of `DegradeRelay`: from here on a failed
    // gateway upstream is recorded against this session, by the gateway's own
    // thread, while the harness below keeps running. The record is the one
    // this process owns and its `backend_resource` was written above, so
    // `degrade_resource` can already tell whether this session was on the
    // resource that failed.
    degrade_relay.install(Arc::clone(&events), vec![record.clone()]);

    let session = if headless {
        crate::commands::resume::run_headless(
            runtime,
            &store,
            &record.id,
            launch,
            deferred_briefing,
        )
    } else {
        session::attach(launch)
    };
    let status = match session {
        Ok(status) => status,
        Err(err) => {
            crate::commands::resume::note_lifecycle(&store, &record.id, SessionLifecycle::Failed);
            return Err(err);
        }
    };

    // The session is over, so this is the tightest the discovery window will
    // ever be — see `session::native_id::capture`'s doc comment.
    session::native_id::capture(&store, &record, runtime.project().root(), &index_before);

    // One definition of "did it crash", and it is `ProcessExit`'s. This used
    // to be an inline `status.success()` split, which is a second place the
    // same classification lived — and two definitions of that eventually
    // disagree about a signal, which is the case that matters least often and
    // costs most when it is wrong.
    let exit = ProcessExit::from_status(&status);
    events.record(
        &record.id,
        LifecycleEvent::ProcessExited { exit: exit.clone() },
    );
    crate::commands::resume::note_lifecycle(&store, &record.id, exit.session_state());

    if !status.success() {
        // The harness failing is not Glasshouse failing, so this is a plain
        // note on stderr rather than an error: the exit code below already
        // carries the outcome to whatever invoked Glasshouse.
        eprintln!("glasshouse: the harness {status}");
    }
    Ok(crate::commands::resume::exit_code_for(&status))
}

/// A briefing selected for a launch but not yet delivered — `GH-LAUNCH-BRIEFING`'s
/// rung two, handed from [`brief_launch_session`] to [`run_headless`] because
/// nothing can deliver it until a session runtime holds the PTY.
#[derive(Debug)]
pub(crate) struct DeferredBriefing {
    pub(crate) injection: glasshouse::memory::inject::Injection,
    binding: usize,
    failed_attempts: usize,
}

impl DeferredBriefing {
    /// The line printed once this briefing is actually delivered — shared
    /// between the rung-one and rung-two paths so the two report identically.
    pub(crate) fn announcement(&self) -> String {
        briefing_announcement(
            self.injection.memories().len(),
            self.binding,
            self.failed_attempts,
        )
    }
}

/// The `briefed with ...` line both delivery rungs print, once, on a
/// successful delivery — never composed twice so the wording cannot drift
/// between rungs.
/// Map lines 2402-2405: register Phase 60's edit-intent `PreToolUse` hook
/// for a Claude Code session, unless a configuration layer turned
/// coordination off.
///
/// **Never a second `--settings` flag**, for the reason
/// [`crate::commands::resume::install_context_firewall_hook`] states at
/// length: Claude Code keeps only the last one, so the only safe way to add
/// a hook is to merge it into the document `install_session_document`
/// already wrote. This reads that file back, adds one `PreToolUse` key, and
/// writes it in place; `args` is never touched.
///
/// **`mode = "off"` installs nothing at all** — line 2405's own words, and
/// the reason this returns before reading the executable path or the session
/// directory. Not installed-and-inert: an inert hook would still spawn a
/// process for every `Edit` the session makes.
///
/// Best effort, matching every other registration on this path: a failure
/// here is a session that starts without coordination rather than one that
/// fails to start, and it is logged rather than propagated. There is no
/// version floor and no probe — unlike the firewall's `updatedToolOutput`,
/// nothing this hook returns needs a Claude Code newer than the one that
/// first accepted a `PreToolUse` entry, and the worst a build that ignores
/// the entry can do is not run it.
fn install_edit_intent_hook(
    selection: &session::HarnessSelection,
    effective: EffectiveConfig<'_>,
    session_dir: &std::path::Path,
    session: &SessionId,
) {
    use glasshouse::config::firewall::EditIntentMode;
    use glasshouse::harness::claude_code;

    if selection.id() != glasshouse::integrations::IntegrationId::ClaudeCode {
        // Map line 2404: where a harness exposes no structured pre-tool
        // hook, the feature is simply absent for that harness and nothing is
        // substituted for it. `glasshouse doctor` says so out loud per
        // adapter (`integrations::write_adapter_report`); this line is the
        // per-launch trace, at `debug` so it is not spam.
        tracing::debug!(
            harness = selection.id().slug(),
            "edit intent: no verified PreToolUse hook for this harness; coordination is              absent for this session"
        );
        return;
    }

    if effective.edit_intent_mode().value == EditIntentMode::Off {
        return;
    }

    let program = match std::env::current_exe() {
        Ok(program) => program,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "edit intent: could not find the Glasshouse executable; not registered"
            );
            return;
        }
    };

    let command_line = claude_code::edit_intent_command_line(&program, session.as_str());
    let hook_entry = claude_code::edit_intent_hook_entry(&command_line);
    let path = session_dir.join(claude_code::SETTINGS_FILE_NAME);
    let existing = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) => {
            tracing::warn!(
                error = %err,
                path = %path.display(),
                "edit intent: could not read the settings document to merge its hook into;                  not registered"
            );
            return;
        }
    };
    match claude_code::merge_edit_intent_hook(&existing, &hook_entry) {
        Ok(merged) => {
            if let Err(err) = std::fs::write(&path, merged) {
                tracing::warn!(
                    error = %err,
                    path = %path.display(),
                    "edit intent: could not write the merged settings document; not registered"
                );
            }
        }
        Err(err) => {
            tracing::warn!(
                error = %err,
                "edit intent: could not merge the PreToolUse hook; not registered"
            );
        }
    }
}

fn briefing_announcement(memories: usize, binding: usize, failed_attempts: usize) -> String {
    format!(
        "briefed with {memories} memories ({binding} binding, {failed_attempts} failed approaches)"
    )
}

/// What `GH-LAUNCH-BRIEFING`'s delivery ladder decided for one launch — map
/// lines 1125-1135's briefing, applied to `glasshouse launch` itself rather
/// than only to a door-spawned session (`docs/product/design-decisions.md`,
/// *Memory is the project's, not the launch path's*).
///
/// Every variant except [`Self::Deferred`] is a launch that already knows its
/// final outcome; [`Self::Deferred`] is the one rung whose delivery depends on
/// a session runtime that does not exist yet.
#[derive(Debug)]
pub(crate) enum LaunchBriefing {
    /// Rung one: delivered by riding the adapter's own additive mechanism,
    /// already appended to the response application's arguments.
    Delivered(String),
    /// Rung two: no additive mechanism, but this launch is headless, so a
    /// session runtime will hold the PTY and can carry the door's own
    /// labelled machine message once it starts — see [`run_headless`].
    Deferred(DeferredBriefing),
    /// Rung three: neither exists for this launch.
    NotBriefed(String),
    /// The opt-out fired, or there was nothing this project's memory had to
    /// say. Not an error and not announced as one — a launch with memory
    /// disabled or empty must read exactly as it did before this feature
    /// existed.
    Nothing,
}

/// `GH-LAUNCH-BRIEFING`: select and, where a rung can deliver it immediately,
/// deliver this project's memory to a session `glasshouse launch` is about to
/// start — the same briefing a door-spawned session already gets (map lines
/// 1125-1135), applied to the CLI launch path the design ruling found never
/// called it at all.
///
/// Called in `launch_session` between `store.create` (`session` exists) and
/// `install_session_document` (`response_application`'s arguments are read),
/// so a rung-one delivery can still ride `response_application`.
///
/// `query` is the checkpoint's bootstrap text when this launch resumes one —
/// [`glasshouse::memory::inject::select_briefing`]'s `Some` case — and `None`
/// otherwise, which selects the standing set instead of running no search at
/// all.
#[allow(clippy::too_many_arguments)]
pub(crate) fn brief_launch_session(
    runtime: &Runtime,
    session: &SessionId,
    adapter: &dyn glasshouse::harness::HarnessAdapter,
    headless: bool,
    no_memory: bool,
    inject_at_launch: bool,
    query: Option<&str>,
    response_application: &mut glasshouse::harness::response::Application,
) -> LaunchBriefing {
    use glasshouse::memory::inject::{self, BriefingOutcome};
    use glasshouse::memory::{MemoryAuthority, MemoryKind, ProjectMemory};

    // Opt-out, not opt-in (the design ruling's own wording): neither the
    // store nor anything else on this path is even touched, so a launch with
    // memory disabled is byte-identical to one built before this feature
    // existed.
    if no_memory || !inject_at_launch {
        return LaunchBriefing::Nothing;
    }

    let project = match ProjectMemory::open(runtime) {
        Ok(project) => project,
        Err(err) => {
            tracing::warn!(
                session = %session,
                error = %format!("{err:#}"),
                "could not open this project's memory to brief a launch"
            );
            return LaunchBriefing::Nothing;
        }
    };
    let rerank_model =
        crate::commands::routing_classification::disposable_rerank_model(runtime, session);
    let diagnostics =
        crate::commands::routing_classification::memory_retrieval_diagnostics_enabled(runtime)
            .then_some(inject::DiagnosticsRequest {
                runtime,
                session: Some(session.as_str()),
            });
    let outcome = match inject::select_briefing_traced(
        &project.store(),
        query,
        &std::collections::HashSet::new(),
        rerank_model.as_deref(),
        diagnostics,
        Some(runtime.project().root()),
    ) {
        Ok((outcome, _trace)) => Some(outcome),
        Err(err) => {
            tracing::warn!(
                session = %session,
                error = %err,
                "could not select project memory to brief a launch"
            );
            None
        }
    };

    let (injection, binding, failed_attempts) = match outcome {
        Some(BriefingOutcome::Injected(injection)) => {
            // Counted while the connection is still open, using the ids the
            // selection just chose — cheap (at most `MAX_INJECTED_MEMORIES`
            // lookups) and avoids a second retrieval implementation ranking
            // candidates a second way.
            let mut binding = 0usize;
            let mut failed_attempts = 0usize;
            for id in injection.memories() {
                if let Ok(Some(record)) = project.store().get(id) {
                    if record.authority.is_some_and(MemoryAuthority::is_binding) {
                        binding += 1;
                    }
                    if record.kind == MemoryKind::FailedAttempt {
                        failed_attempts += 1;
                    }
                }
            }
            (injection, binding, failed_attempts)
        }
        Some(BriefingOutcome::NothingMatched) => {
            // Map line 1865: this launch is a briefing door too, so a search
            // that matched nothing is a retrieval miss exactly as it is for
            // the machine door.
            glasshouse::evaluation::record_memory_retrieval_miss(
                runtime,
                glasshouse::evaluation::RetrievalScope::Injection,
                glasshouse::evaluation::now_unix(),
            );
            drop(project);
            return LaunchBriefing::Nothing;
        }
        Some(BriefingOutcome::NothingNew) | None => {
            drop(project);
            return LaunchBriefing::Nothing;
        }
    };
    // Practice §65: the memory connection is dropped before the evaluation
    // ledger below opens, the same shape `select_memory`'s own caller uses.
    drop(project);

    if response_application.append_additive_text(adapter, injection.text()) {
        glasshouse::evaluation::record_memory_retrieval(
            runtime,
            glasshouse::evaluation::RetrievalScope::Injection,
            injection
                .memories()
                .iter()
                .map(glasshouse::memory::MemoryId::as_str),
            Some(session.as_str()),
            glasshouse::evaluation::now_unix(),
        );
        return LaunchBriefing::Delivered(briefing_announcement(
            injection.memories().len(),
            binding,
            failed_attempts,
        ));
    }

    if headless {
        return LaunchBriefing::Deferred(DeferredBriefing {
            injection,
            binding,
            failed_attempts,
        });
    }

    LaunchBriefing::NotBriefed(format!(
        "{} declares no mechanism for adding an instruction beside its own system prompt, and \
         this launch has no session runtime to deliver a machine message through",
        glasshouse::harness::response::harness_name(adapter.id())
    ))
}

/// Where a launch is presented, beyond this terminal — Phase 17 lines 757
/// and 761, decided from `--presentation` and `--presentation-ref` before
/// anything is resolved.
///
/// The two flags are the two sides of one pane: the outer process asks to
/// *spawn into* a backend, and the process it starts inside the pane is told
/// it is *hosted by* one. `clap` refuses both on one command line.
#[derive(Debug)]
pub(crate) enum ExternalPresentation {
    /// Neither flag: the session is shown where it always was.
    Embedded,
    /// `--presentation <backend>`: open a pane and run this launch again
    /// inside it. `pane_command` is the whole command line the pane runs,
    /// already quoted for the shell.
    SpawnIn { pane_command: String },
    /// `--presentation-ref <ref|caller>`: this process is the one inside the
    /// pane; record where it is and otherwise launch normally.
    HostedBy(cmux::PaneRefRequest),
}

/// Read the two flags into an [`ExternalPresentation`], building the pane's
/// command only when one is actually needed.
///
/// An unknown backend and a malformed reference are both refused here, by
/// name, before a harness is selected or a database opened: a launch that
/// cannot say where it wants to be shown has not asked for anything yet.
pub(crate) fn external_presentation(
    backend: Option<&str>,
    reference: Option<&str>,
    pane_command: impl FnOnce() -> anyhow::Result<String>,
) -> anyhow::Result<ExternalPresentation> {
    match (backend, reference) {
        (Some(word), _) => {
            let cmux::Backend::Cmux = cmux::Backend::parse(word)?;
            Ok(ExternalPresentation::SpawnIn {
                pane_command: pane_command()?,
            })
        }
        (None, Some(reference)) => Ok(ExternalPresentation::HostedBy(cmux::PaneRefRequest::parse(
            reference,
        )?)),
        (None, None) => Ok(ExternalPresentation::Embedded),
    }
}

/// The process-wide flags a pane's Glasshouse needs to be *this* Glasshouse:
/// the same project, the same data and configuration directories — resolved
/// values, not whatever the pane's login shell would derive — and the same
/// logging choices. Nothing else: no credential is a flag, and none becomes
/// one here.
pub(crate) fn pane_global_args(cli: &Cli, runtime: &Runtime) -> Vec<OsString> {
    let paths = runtime.paths();
    let mut args: Vec<OsString> = vec![
        "--scope".into(),
        runtime.project().display_root().as_os_str().to_owned(),
        "--data-dir".into(),
        paths.data_dir().as_os_str().to_owned(),
        "--config-dir".into(),
        paths.config_dir().as_os_str().to_owned(),
    ];
    if cli.allow_unsafe_scope {
        args.push("--allow-unsafe-scope".into());
    }
    if let Some(level) = &cli.log_level {
        args.push("--log-level".into());
        args.push(level.into());
    }
    if let Some(file) = &cli.log_file {
        args.push("--log-file".into());
        args.push(file.into());
    }
    if cli.log_stderr {
        args.push("--log-stderr".into());
    }
    args
}

/// The launch a pane runs: the same launch the person typed, minus
/// `--presentation` and plus `--presentation-ref caller`, so the process
/// inside the pane records where it is and otherwise does exactly what this
/// one would have done. One field per flag `launch` takes, so a flag added
/// to `Command::Launch` and not carried here is a compile error at the call
/// site rather than a pane that silently ignores it.
pub(crate) struct PaneLaunch<'a> {
    pub(crate) harness: Option<&'a str>,
    pub(crate) response_profile: Option<&'a str>,
    pub(crate) response_role: Option<&'a str>,
    pub(crate) profile: Option<&'a str>,
    pub(crate) from_checkpoint: Option<&'a str>,
    pub(crate) to: Option<&'a str>,
    pub(crate) fresh: bool,
    pub(crate) headless: bool,
    pub(crate) harness_args: &'a [String],
}

pub(crate) fn pane_launch_args(launch: PaneLaunch<'_>) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["launch".into()];
    if let Some(harness) = launch.harness {
        args.push(harness.into());
    }
    for (flag, value) in [
        ("--response-profile", launch.response_profile),
        ("--response-role", launch.response_role),
        ("--profile", launch.profile),
        ("--from-checkpoint", launch.from_checkpoint),
        ("--to", launch.to),
    ] {
        if let Some(value) = value {
            args.push(flag.into());
            args.push(value.into());
        }
    }
    if launch.fresh {
        args.push("--fresh".into());
    }
    if launch.headless {
        args.push("--headless".into());
    }
    args.push("--presentation-ref".into());
    args.push("caller".into());
    if !launch.harness_args.is_empty() {
        args.push("--".into());
        args.extend(launch.harness_args.iter().map(OsString::from));
    }
    args
}

/// Open a cmux workspace in the project root running `pane_command`, wait
/// briefly for the session inside it to record itself, and say what
/// happened — Phase 17 lines 757 and 761.
///
/// This process starts nothing else: no harness, no record, no runtime. The
/// pane hosts a normal launch, and that launch is what writes the session
/// down (with `External` and the workspace it asked cmux for). The wait is
/// bounded and its expiry is reported, not treated as failure — the pane is
/// real either way, and `glasshouse sessions` lists the session once it has
/// recorded itself.
fn open_cmux_pane(
    runtime: &Runtime,
    control: &impl cmux::CmuxControl,
    harness: &str,
    pane_command: &str,
) -> anyhow::Result<ExitCode> {
    let sessions = ProjectSessions::open(runtime)?;
    let store = sessions.store();
    let before = cmux::recorded_panes(&store)?;
    let workspace = cmux::NewWorkspace {
        name: format!("glasshouse {harness}"),
        cwd: runtime.project().display_root().to_path_buf(),
        command: pane_command.to_owned(),
        // A person asked to see it.
        focus: true,
    };
    let pane = control
        .create_workspace(&workspace)
        .map_err(|err| anyhow::anyhow!("cmux could not open a workspace for the session: {err}"))?;
    match cmux::await_session_at(&store, &pane, &before, cmux::RECORD_WAIT)? {
        Some(id) => println!("glasshouse: session {id} is running in cmux {pane}"),
        None => println!(
            "glasshouse: opened cmux {pane}; the session has not recorded itself yet — \
             `glasshouse sessions` lists it once it has"
        ),
    }
    Ok(ExitCode::SUCCESS)
}
