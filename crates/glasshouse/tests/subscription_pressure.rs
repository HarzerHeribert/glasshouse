//! Phase 35D lines 1570–1577 (and 1606, 1610, 1612 as Phase 38 restates
//! them) — routing under subscription pressure, entered the way production
//! enters it.
//!
//! Two halves, for the reason `tests/route_command.rs` gives for its own
//! existence. The first goes through [`SessionRouter::choose`] with
//! hand-built destinations and, for every term, holds two destinations that
//! differ **in that axis alone** and shows them resolving differently
//! (`docs/product/evidence/phase-9j.md`'s constant-signal rule; practice
//! §35). Each pair is built so that `known quota pressure` — the term that
//! already reads the same quota figure — is *equal* across it: same
//! percentage, different band or reset. A pair that also differed in the
//! percentage would pass on a build where the new terms were dead.
//!
//! The second half runs the shipped binary. Nothing in the first half can
//! fail on a build where `main.rs::destination_capacity` stops attaching the
//! band and reset, or where `destination_backend` goes back to calling every
//! destination metered — and those two calls are the whole of what this
//! package wires. Practice §35: *a caller you can delete without a test
//! noticing is, to the test suite, not a caller.*

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Instant;

use glasshouse::config::pairing::{WarmSession, WarmSessionState};
use glasshouse::harness::Declared;
use glasshouse::harness::pairing::PairingOverrides;
use glasshouse::integrations::IntegrationId;
use glasshouse::provider::quota::{
    Capacity, CapacityBand, CapacityState, NativeAmount, Pool, Reading, ReadingSource,
    RemainingCapacityScore,
};
use glasshouse::routing::capability::ResourceFacts;
use glasshouse::routing::classify::{HardCapability, WorkloadTier};
use glasshouse::routing::free::FreePool;
use glasshouse::routing::pressure::{
    self, Alternatives, CapacityFacts, LOW_TIER_SPEND_PENALTY, PressureInputs,
    RESERVE_DENIED_PENALTY, ReservePolicies, ReservePolicy, ReserveScope, TIGHT_BAND_PENALTY,
};
use glasshouse::routing::session::{
    CheckpointQuality, Destination, Routed, RouterInputs, RoutingMoment, SessionRouter,
    TaskRequirements,
};
use glasshouse::routing::{AssignedModel, Backend, Cost, CredentialId, ToolSemantics};
use glasshouse::secret::SecretRef;

// ===========================================================================
// Half one — through `SessionRouter::choose`.
// ===========================================================================

const PROTOCOL: &str = "anthropic-messages";
const HARNESS: IntegrationId = IntegrationId::ClaudeCode;

fn backend(provider: &str, cost: Cost) -> Backend {
    Backend::new(
        provider,
        PROTOCOL,
        AssignedModel::named("the-same-model"),
        CredentialId::new(
            provider,
            SecretRef::Environment {
                var: format!("{}_KEY", provider.to_uppercase().replace('-', "_")),
            },
        ),
        cost,
        ToolSemantics::Verified,
    )
}

/// A real, fully-measured remaining-capacity score at `percent` — the same
/// construction `tests/session_router.rs` uses, so `known quota pressure`
/// reads an exact figure and is equal across any pair built at one percent.
fn capacity(percent: i64) -> RemainingCapacityScore {
    const OBSERVED: i64 = 1_800_000_000;
    let measured = |value: i64| {
        Capacity::Measured(Reading::new(
            NativeAmount::whole(value, "tokens"),
            OBSERVED,
            ReadingSource::ResponseHeader("x-ratelimit".to_owned()),
        ))
    };
    CapacityState::metered_balance()
        .with_credits(
            Pool::inapplicable()
                .with_remaining(measured(percent))
                .with_limit(measured(100)),
        )
        .remaining_capacity_score()
        .expect("both halves of the credits pool are measured")
}

fn live() -> WarmSession {
    WarmSession {
        state: WarmSessionState::Live,
        idle_seconds: 0,
    }
}

/// A fresh, metered destination at `percent`, with the band and reset the
/// caller states — the band is attached rather than derived so a pair can
/// hold the percentage equal and vary the band alone.
fn fresh(
    id: &str,
    percent: i64,
    band: Option<CapacityBand>,
    reset: Option<i64>,
    checkpoint: Option<CheckpointQuality>,
) -> Destination {
    Destination::fresh(
        id,
        HARNESS,
        "profile",
        backend(&format!("{id}-provider"), Cost::Metered),
        checkpoint,
    )
    .with_capacity(Some(capacity(percent)))
    .with_capacity_facts(CapacityFacts::new(band, reset))
}

/// A live, zero-idle existing session — the warmest destination this router
/// can be handed.
fn warm(id: &str, percent: i64, band: Option<CapacityBand>, reset: Option<i64>) -> Destination {
    Destination::existing(
        id,
        HARNESS,
        "profile",
        backend(&format!("{id}-provider"), Cost::Metered),
        live(),
    )
    .with_capacity(Some(capacity(percent)))
    .with_capacity_facts(CapacityFacts::new(band, reset))
}

struct Fixture {
    overrides: PairingOverrides,
    health: FreePool,
    now: Instant,
}

impl Fixture {
    fn new() -> Self {
        Self {
            overrides: PairingOverrides::from_parts(
                "no configuration",
                BTreeMap::new(),
                BTreeMap::new(),
            ),
            health: FreePool::new(),
            now: Instant::now(),
        }
    }

    fn inputs(&self, tier: Option<WorkloadTier>) -> RouterInputs<'_> {
        RouterInputs {
            overrides: &self.overrides,
            health: &self.health,
            now: self.now,
            requirements: TaskRequirements {
                minimum_tier: tier,
                ..TaskRequirements::default()
            },
        }
    }

    fn choose(
        &self,
        router: &SessionRouter,
        destinations: &[Destination],
        tier: Option<WorkloadTier>,
    ) -> Routed {
        router
            .choose(
                RoutingMoment::SessionStart,
                None,
                destinations,
                &self.inputs(tier),
            )
            .expect("a non-empty candidate set is always routed")
    }

    fn choose_with(
        &self,
        router: &SessionRouter,
        destinations: &[Destination],
        inputs: &RouterInputs<'_>,
    ) -> Routed {
        router
            .choose(RoutingMoment::SessionStart, None, destinations, inputs)
            .expect("a non-empty candidate set is always routed")
    }
}

/// The evidence string a destination's named term carried, from the
/// ranking's full record rather than only the winner's.
fn evidence(routed: &Routed, destination: &str, term: &str) -> String {
    let (_, explanation) = routed
        .considered()
        .iter()
        .find(|(d, _)| d.id() == destination)
        .unwrap_or_else(|| {
            panic!(
                "`{destination}` was not ranked:\n{}",
                routed.render_overview()
            )
        });
    explanation
        .contributions()
        .iter()
        .find(|c| c.name() == term)
        .unwrap_or_else(|| {
            panic!(
                "`{destination}` carried no `{term}` term:\n{}",
                explanation.render()
            )
        })
        .evidence()
        .to_owned()
}

fn magnitude(routed: &Routed, destination: &str, term: &str) -> f64 {
    let (_, explanation) = routed
        .considered()
        .iter()
        .find(|(d, _)| d.id() == destination)
        .unwrap_or_else(|| {
            panic!(
                "`{destination}` was not ranked:\n{}",
                routed.render_overview()
            )
        });
    explanation
        .contributions()
        .iter()
        .find(|c| c.name() == term)
        .unwrap_or_else(|| {
            panic!(
                "`{destination}` carried no `{term}` term:\n{}",
                explanation.render()
            )
        })
        .magnitude()
}

// --- line 1570 ---------------------------------------------------------------

/// **Line 1570, as the discriminating pair.** Same percentage, so `known
/// quota pressure` is equal; the only difference is the band the caller read
/// — which is what a per-provider reserve percentage or a user's thresholds
/// produce for one figure. The tight one loses, whichever is listed first.
///
/// And the line's own adjective: an **inadequate** alternative — one
/// established to lack a capability the task needs — does not win on the
/// strength of the other's tightness. `TIGHT_BAND_PENALTY` is placed below
/// the capability-absent cost on purpose, and this is the assertion that
/// pins it there.
#[test]
fn a_tight_premium_destination_loses_to_a_healthy_adequate_alternative() {
    let fixture = Fixture::new();
    let router = SessionRouter::new();
    let tight = fresh("tight", 30, Some(CapacityBand::Tight), None, None);
    let healthy = fresh("healthy", 30, Some(CapacityBand::Healthy), None, None);

    for order in [
        vec![tight.clone(), healthy.clone()],
        vec![healthy.clone(), tight.clone()],
    ] {
        let routed = fixture.choose(&router, &order, Some(WorkloadTier::Standard));
        assert_eq!(
            routed.chosen().id(),
            "healthy",
            "the healthy band must win over the tight one at the same percentage, in either \
             order:\n{}",
            routed.render_overview()
        );
        assert_eq!(
            magnitude(&routed, "tight", "capacity band"),
            TIGHT_BAND_PENALTY
        );
        assert!(
            evidence(&routed, "tight", "capacity band").contains("tight band"),
            "{}",
            routed.render_overview()
        );
        assert_eq!(magnitude(&routed, "healthy", "capacity band"), 0.0);
    }

    // The inadequate alternative: same pair, but the task needs browser
    // interaction and `healthy` is established not to have it.
    let inadequate = healthy.with_resource_facts(ResourceFacts {
        browser_use: Declared::Verified {
            value: false,
            evidence: "declared absent by this test",
        },
        ..ResourceFacts::UNVERIFIED
    });
    let inputs = RouterInputs {
        overrides: &fixture.overrides,
        health: &fixture.health,
        now: fixture.now,
        requirements: TaskRequirements {
            hard_capabilities: vec![HardCapability::BrowserInteraction],
            minimum_tier: Some(WorkloadTier::Standard),
            ..TaskRequirements::default()
        },
    };
    let routed = fixture.choose_with(&router, &[inadequate, tight], &inputs);
    assert_eq!(
        routed.chosen().id(),
        "tight",
        "an alternative established to lack a required capability is not adequate, and \
         tightness alone must not send the work there:\n{}",
        routed.render_overview()
    );
}

// --- lines 1571 and 1606 -----------------------------------------------------

/// **Lines 1571 and 1606, as the discriminating pair.** The candidate set is
/// fixed — a warm session on a subscription in its reserve band, and a fresh
/// destination in a healthy band booting from a good checkpoint — and the
/// only thing that changes is the task's tier. Below the heavy tier the
/// reserve is protected and the work goes to the alternative; at or above it
/// the reserve is admitted and the warm session keeps the work. An
/// unestablished tier is treated like the lowest one (line 1459) and the
/// explanation says so rather than calling the task light.
#[test]
fn the_reserve_band_is_kept_for_top_tier_work() {
    let fixture = Fixture::new();
    let router = SessionRouter::new();
    let reserve = warm("reserve", 10, Some(CapacityBand::Reserve), None);
    let healthy = fresh(
        "healthy",
        10,
        Some(CapacityBand::Healthy),
        None,
        Some(CheckpointQuality::new(true, true)),
    );
    let set = [reserve, healthy];

    for tier in [
        WorkloadTier::Deterministic,
        WorkloadTier::Leaf,
        WorkloadTier::Standard,
    ] {
        let routed = fixture.choose(&router, &set, Some(tier));
        assert_eq!(
            routed.chosen().id(),
            "healthy",
            "a {tier}-tier task must not spend the reserve while an adequate alternative \
             exists:\n{}",
            routed.render_overview()
        );
        assert_eq!(
            magnitude(&routed, "reserve", "capacity band"),
            RESERVE_DENIED_PENALTY
        );
        let why = evidence(&routed, "reserve", "capacity band");
        assert!(why.contains("denies the spend"), "{why}");
        assert!(
            why.contains("`healthy` is the cheaper adequate alternative"),
            "{why}"
        );
        assert!(
            why.contains("interactive reserve policy is `protect`"),
            "{why}"
        );
    }

    for tier in [WorkloadTier::Heavy, WorkloadTier::Frontier] {
        let routed = fixture.choose(&router, &set, Some(tier));
        assert_eq!(
            routed.chosen().id(),
            "reserve",
            "a {tier}-tier task is what the reserve is kept for:\n{}",
            routed.render_overview()
        );
        let why = evidence(&routed, "reserve", "capacity band");
        assert!(why.contains("the spend is admitted"), "{why}");
        assert!(why.contains("line 1289"), "{why}");
    }

    let routed = fixture.choose(&router, &set, None);
    assert_eq!(
        routed.chosen().id(),
        "healthy",
        "an unestablished tier is not known to justify the reserve:\n{}",
        routed.render_overview()
    );
    let why = evidence(&routed, "reserve", "capacity band");
    assert!(why.contains("not established"), "{why}");
    assert!(!why.contains("does not require"), "{why}");
}

// --- line 1572 ---------------------------------------------------------------

/// **Line 1572.** A live warm session on a subscription that has entered the
/// tight band, against a cold fresh alternative in a healthy band. Tightness
/// alone does not move the work — and the explanation says that is why.
///
/// The discriminating pair is this test against
/// [`a_low_tier_task_does_not_spend_a_subscription_when_a_free_adequate_resource_is_healthy`]:
/// the same warm-versus-fresh shape, and only the tier and the alternative's
/// cost differ.
#[test]
fn a_warm_high_value_session_is_not_abandoned_over_tightness_alone() {
    let fixture = Fixture::new();
    let router = SessionRouter::new();
    let warm_tight = warm("warm-tight", 30, Some(CapacityBand::Tight), None);
    let cold_healthy = fresh("cold-healthy", 30, Some(CapacityBand::Healthy), None, None);

    let routed = fixture.choose(
        &router,
        &[cold_healthy, warm_tight],
        Some(WorkloadTier::Standard),
    );
    assert_eq!(
        routed.chosen().id(),
        "warm-tight",
        "a warm session is worth more than tightness costs:\n{}",
        routed.render_overview()
    );
    let why = evidence(&routed, "warm-tight", "capacity band");
    assert!(why.contains("tight band"), "{why}");
    assert!(why.contains("line 1572"), "{why}");
    assert!(
        TIGHT_BAND_PENALTY.abs() < 1.5,
        "the tight penalty must stay below a live session's warmth, or line 1572 is a lie"
    );
}

// --- lines 1573 and 1574 -----------------------------------------------------

/// **Lines 1573 and 1574, as the discriminating pair.** Two fresh
/// destinations, both tight at the same percentage; the only difference is
/// when each resets. The one resetting within the relief horizon is not
/// penalised at all and says its capacity would otherwise expire unused; the
/// one resetting in hours pays the full penalty; one between the two pays a
/// reduced penalty and says by how much.
#[test]
fn an_imminent_reset_relaxes_conservation_and_the_explanation_says_so() {
    let fixture = Fixture::new();
    let router = SessionRouter::new();
    let soon = fresh("soon", 30, Some(CapacityBand::Tight), Some(60), None);
    let later = fresh("later", 30, Some(CapacityBand::Tight), Some(7_200), None);
    let between = fresh("between", 30, Some(CapacityBand::Tight), Some(1_950), None);

    let routed = fixture.choose(
        &router,
        &[later.clone(), between.clone(), soon.clone()],
        Some(WorkloadTier::Standard),
    );
    assert_eq!(
        routed.chosen().id(),
        "soon",
        "the destination whose reset is imminent must win the tie on tightness:\n{}",
        routed.render_overview()
    );

    assert_eq!(magnitude(&routed, "soon", "capacity band"), 0.0);
    let why = evidence(&routed, "soon", "capacity band");
    assert!(why.contains("resetting in 60s"), "{why}");
    assert!(why.contains("waived"), "{why}");
    assert!(why.contains("expire unused"), "{why}");
    assert!(why.contains("lines 1573, 1574"), "{why}");

    assert_eq!(
        magnitude(&routed, "later", "capacity band"),
        TIGHT_BAND_PENALTY
    );
    let why = evidence(&routed, "later", "capacity band");
    assert!(why.contains("resetting in 7200s"), "{why}");
    assert!(why.contains("full conservation penalty"), "{why}");

    let reduced = magnitude(&routed, "between", "capacity band");
    assert!(
        reduced > TIGHT_BAND_PENALTY && reduced < 0.0,
        "a reset between the horizon and the fade pays a reduced penalty, got {reduced}"
    );
    let why = evidence(&routed, "between", "capacity band");
    assert!(why.contains("reduced by 50%"), "{why}");

    // The mirrored order, so a fixed ordering cannot also satisfy the claim.
    let routed = fixture.choose(
        &router,
        &[soon, between, later],
        Some(WorkloadTier::Standard),
    );
    assert_eq!(routed.chosen().id(), "soon", "{}", routed.render_overview());
}

// --- line 1575 ---------------------------------------------------------------

/// **Line 1575, as the discriminating pair.** The same warm-tight session as
/// line 1572's test, against a fresh **zero-cost** alternative with nothing
/// read about it. For a leaf task the free alternative takes the work even
/// from a warm session; for a standard task the warm session keeps it. The
/// tier is the only thing that differs between the two halves.
#[test]
fn a_low_tier_task_does_not_spend_a_subscription_when_a_free_adequate_resource_is_healthy() {
    let fixture = Fixture::new();
    let router = SessionRouter::new();
    let warm_premium = warm("warm-premium", 30, Some(CapacityBand::Tight), None);
    let free = Destination::fresh(
        "free",
        HARNESS,
        "profile",
        backend("free-provider", Cost::Free),
        None,
    );
    let set = [warm_premium, free];

    let routed = fixture.choose(&router, &set, Some(WorkloadTier::Leaf));
    assert_eq!(
        routed.chosen().id(),
        "free",
        "leaf work leaves a tight subscription for a healthy free resource:\n{}",
        routed.render_overview()
    );
    assert_eq!(
        magnitude(&routed, "warm-premium", "low-tier spend"),
        LOW_TIER_SPEND_PENALTY
    );
    let why = evidence(&routed, "warm-premium", "low-tier spend");
    assert!(why.contains("leaf-tier task"), "{why}");
    assert!(
        why.contains("`free` is a healthy zero-cost resource"),
        "{why}"
    );
    assert!(why.contains("line 1575"), "{why}");
    // The free destination's own terms are inert and say why.
    let why = evidence(&routed, "free", "capacity band");
    assert!(why.starts_with("inert: a zero-cost resource"), "{why}");

    let routed = fixture.choose(&router, &set, Some(WorkloadTier::Standard));
    assert_eq!(
        routed.chosen().id(),
        "warm-premium",
        "standard work stays on the warm session; only low-tier work is sent away:\n{}",
        routed.render_overview()
    );
    assert_eq!(magnitude(&routed, "warm-premium", "low-tier spend"), 0.0);
    let why = evidence(&routed, "warm-premium", "low-tier spend");
    assert!(why.contains("above the leaf ceiling"), "{why}");

    // And an unestablished tier makes no low-tier claim.
    let routed = fixture.choose(&router, &set, None);
    assert_eq!(
        routed.chosen().id(),
        "warm-premium",
        "{}",
        routed.render_overview()
    );
    let why = evidence(&routed, "warm-premium", "low-tier spend");
    assert!(why.starts_with("inert"), "{why}");
    assert!(why.contains("not established"), "{why}");
}

// --- line 1577 ---------------------------------------------------------------

/// **Line 1577.** The same reserve-band pair as line 1571's test, with a
/// standard-tier task, routed under three configurations. Changing the
/// *background* policy moves nothing in an interactive ranking; changing the
/// *interactive* one does — and at the pure level, the background scope reads
/// the background field.
#[test]
fn interactive_and_background_reserve_policies_are_independent() {
    let fixture = Fixture::new();
    let reserve = warm("reserve", 10, Some(CapacityBand::Reserve), None);
    let healthy = fresh(
        "healthy",
        10,
        Some(CapacityBand::Healthy),
        None,
        Some(CheckpointQuality::new(true, true)),
    );
    let set = [reserve, healthy];
    let tier = Some(WorkloadTier::Standard);

    let both_protect = fixture.choose(&SessionRouter::new(), &set, tier);
    assert_eq!(
        both_protect.chosen().id(),
        "healthy",
        "{}",
        both_protect.render_overview()
    );

    let background_spend = fixture.choose(
        &SessionRouter::new().with_reserve_policies(ReservePolicies {
            interactive: ReservePolicy::Protect,
            background: ReservePolicy::Spend,
        }),
        &set,
        tier,
    );
    assert_eq!(
        background_spend.chosen().id(),
        "healthy",
        "the background policy must not move an interactive ranking:\n{}",
        background_spend.render_overview()
    );
    assert_eq!(
        evidence(&background_spend, "reserve", "capacity band"),
        evidence(&both_protect, "reserve", "capacity band"),
        "the interactive explanation must not change with the background policy"
    );

    let interactive_spend = fixture.choose(
        &SessionRouter::new().with_reserve_policies(ReservePolicies {
            interactive: ReservePolicy::Spend,
            background: ReservePolicy::Protect,
        }),
        &set,
        tier,
    );
    assert_eq!(
        interactive_spend.chosen().id(),
        "reserve",
        "an interactive `spend` policy lets the warm session keep the work:\n{}",
        interactive_spend.render_overview()
    );
    let why = evidence(&interactive_spend, "reserve", "capacity band");
    assert!(
        why.contains("interactive reserve policy is `spend`"),
        "{why}"
    );
    assert_eq!(
        magnitude(&interactive_spend, "reserve", "capacity band"),
        TIGHT_BAND_PENALTY
    );

    // The pure function, asked for the background scope, reads the
    // background field — the half the session router never selects.
    let alternatives = Alternatives::none().with_cheaper_adequate("healthy");
    let mut inputs = PressureInputs {
        premium: true,
        facts: CapacityFacts::new(Some(CapacityBand::Reserve), None),
        tier,
        existing: true,
        alternatives: &alternatives,
        policies: ReservePolicies {
            interactive: ReservePolicy::Spend,
            background: ReservePolicy::Protect,
        },
        scope: ReserveScope::Background,
        user_override: false,
        task_nearly_complete: false,
        forecast: None,
    };
    let background = pressure::capacity_band_pressure(&inputs);
    assert_eq!(background.magnitude(), RESERVE_DENIED_PENALTY);
    assert!(
        background
            .evidence()
            .contains("background reserve policy is `protect`"),
        "{}",
        background.evidence()
    );
    inputs.scope = ReserveScope::Interactive;
    assert_eq!(
        pressure::capacity_band_pressure(&inputs).magnitude(),
        TIGHT_BAND_PENALTY
    );
}

/// **Line 1290, at this router.** The user naming an existing session as
/// allowed to spend the reserve admits it — and only that session.
#[test]
fn a_session_the_user_named_may_spend_the_reserve_and_an_unnamed_one_may_not() {
    let fixture = Fixture::new();
    let reserve = warm("reserve", 10, Some(CapacityBand::Reserve), None);
    let healthy = fresh(
        "healthy",
        10,
        Some(CapacityBand::Healthy),
        None,
        Some(CheckpointQuality::new(true, true)),
    );
    let set = [reserve, healthy];
    let tier = Some(WorkloadTier::Standard);

    let named = fixture.choose(
        &SessionRouter::new().with_reserve_override_sessions(["reserve"]),
        &set,
        tier,
    );
    assert_eq!(
        named.chosen().id(),
        "reserve",
        "{}",
        named.render_overview()
    );
    assert!(
        evidence(&named, "reserve", "capacity band").contains("line 1290"),
        "{}",
        named.render_overview()
    );

    let other = fixture.choose(
        &SessionRouter::new().with_reserve_override_sessions(["some-other-session"]),
        &set,
        tier,
    );
    assert_eq!(
        other.chosen().id(),
        "healthy",
        "{}",
        other.render_overview()
    );
}

/// **A resource that cannot serve is not an alternative.** The same
/// reserve-band pair, but the healthy candidate's credential was refused by
/// its provider. It is no longer a *cheaper adequate resource* — nothing can
/// be routed to it instead — so the reserve is admitted as the least-bad
/// option and the warm session keeps the work, rather than being denied in
/// favour of a destination `provider health` is about to score as
/// unavailable. Found by `tests/route_command.rs`'s line 1599 tests on the
/// first run, and pinned here so it is not re-found.
#[test]
fn a_reserve_band_destination_is_not_denied_in_favour_of_an_unavailable_alternative() {
    use glasshouse::routing::free::{FreeResource, WorkloadOutcome};

    let mut fixture = Fixture::new();
    let reserve = warm("reserve", 10, Some(CapacityBand::Reserve), None);
    let healthy = fresh(
        "healthy",
        10,
        Some(CapacityBand::Healthy),
        None,
        Some(CheckpointQuality::new(true, true)),
    );
    fixture.health.observe(
        &FreeResource::new(
            healthy.backend().credential().clone(),
            healthy.backend().model().label(),
        ),
        WorkloadOutcome::CredentialRejected,
        fixture.now,
    );
    let set = [reserve, healthy];

    let routed = fixture.choose(&SessionRouter::new(), &set, Some(WorkloadTier::Standard));
    assert_eq!(
        routed.chosen().id(),
        "reserve",
        "a refused provider is not somewhere the work can go instead:\n{}",
        routed.render_overview()
    );
    let why = evidence(&routed, "reserve", "capacity band");
    assert!(why.contains("least-bad"), "{why}");
    assert!(!why.contains("denies the spend"), "{why}");
}

// --- line 1576, the negative control ------------------------------------------

/// **Line 1576's other half.** With nothing read and no tier, both terms are
/// present on every candidate, weigh exactly nothing, and say they are inert
/// — so a reader can tell "no pressure" from "nothing was read".
#[test]
fn pressure_terms_with_no_reading_are_inert_and_named_as_such() {
    let fixture = Fixture::new();
    let router = SessionRouter::new();
    let unread = |id: &str| {
        Destination::fresh(
            id,
            HARNESS,
            "profile",
            backend(&format!("{id}-provider"), Cost::Metered),
            None,
        )
    };
    let routed = fixture.choose(&router, &[unread("one"), unread("two")], None);

    for id in ["one", "two"] {
        for term in ["capacity band", "low-tier spend"] {
            assert_eq!(
                magnitude(&routed, id, term),
                0.0,
                "{}",
                routed.render_overview()
            );
            let why = evidence(&routed, id, term);
            assert!(why.starts_with("inert:"), "{term} on {id}: {why}");
        }
        assert!(
            evidence(&routed, id, "capacity band").contains("no capacity reading"),
            "{}",
            routed.render_overview()
        );
    }

    // A band read but no tier: the band term acts, the low-tier term is
    // inert on the tier and names that as the reason.
    let tight = fresh("tight", 30, Some(CapacityBand::Tight), None, None);
    let routed = fixture.choose(&router, &[tight, unread("unread")], None);
    assert_eq!(
        magnitude(&routed, "tight", "capacity band"),
        TIGHT_BAND_PENALTY
    );
    let why = evidence(&routed, "tight", "low-tier spend");
    assert!(
        why.starts_with("inert:") && why.contains("not established"),
        "{why}"
    );
}

// --- line 1612 and line 1610 ---------------------------------------------------

const PRESSURE_SOURCE: &str = include_str!("../src/routing/pressure.rs");

/// The **other** production construction site of `ReserveDecisionInputs`.
///
/// `routing/disposable/mod.rs`'s per-candidate loop builds the same struct
/// and decides per candidate, and until 2026-09-03 nothing scanned it — so
/// it could have gained a `task_nearly_complete: true` literal without this
/// file noticing. Both sites are scanned now; that is the reason the pin
/// exists rather than optional tidying.
const DISPOSABLE_SOURCE: &str = include_str!("../src/routing/disposable/mod.rs");

/// The production half of `routing/pressure.rs` — everything above its
/// first `#[cfg(test)]`, which is practice §81's own boundary. The module's
/// unit tests legitimately name inputs the production code must not, and
/// scanning them would be scanning the wrong thing.
fn production_source() -> &'static str {
    let boundary = PRESSURE_SOURCE
        .find("#[cfg(test)]")
        .expect("routing/pressure.rs carries its own unit tests");
    &PRESSURE_SOURCE[..boundary]
}

/// The production half of `routing/disposable/mod.rs`, which is **all of
/// it** — and the slice practice §81 describes would be wrong here.
///
/// That file's only `#[cfg(test)]` is the `mod tests;` declaration near the
/// top, because its unit tests live in a sibling file rather than inline.
/// Slicing at the first occurrence, as `production_source` correctly does
/// for a file with inline tests, would discard roughly 1,470 lines of
/// production code including the construction site this scan exists to
/// watch — a scan that reads as passing while covering almost nothing, which
/// is §68's shape. The boundary rule is *"a call site is production if it is
/// below the file's first `#[cfg(test)]`"*; where the marker is a module
/// declaration rather than a module body, there is no test code in the file
/// to exclude.
fn disposable_production_source() -> &'static str {
    let boundary = DISPOSABLE_SOURCE
        .find("#[cfg(test)]")
        .expect("routing/disposable/mod.rs declares its test module");
    assert!(
        DISPOSABLE_SOURCE[boundary..].starts_with("#[cfg(test)]\nmod tests;"),
        "routing/disposable/mod.rs has grown an inline `#[cfg(test)]` block; this scan assumes \
         the whole file is production because its tests live in a sibling file, and that \
         assumption has to be re-decided rather than silently kept"
    );
    DISPOSABLE_SOURCE
}

/// Whether `name` occurs in `source` as a whole word — bounded on both sides
/// by something that is not a letter, digit or underscore — so a two-letter
/// template name cannot match inside an ordinary English word.
fn names_word(source: &str, name: &str) -> bool {
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    source.match_indices(name).any(|(start, _)| {
        let before = source[..start].chars().next_back();
        let after = source[start + name.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

/// **Line 1612.** The policy is tunable through configuration and never
/// through a hierarchy written into the source: no harness, provider
/// template or model family is named anywhere in `routing/pressure.rs`.
#[test]
fn the_policy_names_no_provider_or_model() {
    let source = production_source().to_lowercase();
    let mut forbidden: Vec<String> = glasshouse::provider::templates()
        .into_iter()
        .map(|template| template.name.to_lowercase())
        .collect();
    forbidden.extend(
        IntegrationId::ALL
            .iter()
            .flat_map(|id| [id.slug().to_lowercase(), id.display_name().to_lowercase()]),
    );
    forbidden.extend(
        [
            "claude",
            "gpt",
            "gemini",
            "llama",
            "sonnet",
            "opus",
            "haiku",
            "deepseek",
            "mistral",
            "qwen",
            "codex",
            "anthropic",
            "openai",
            "google",
            "ollama",
            "groq",
            "openrouter",
        ]
        .map(str::to_owned),
    );
    forbidden.sort();
    forbidden.dedup();
    assert!(!forbidden.is_empty());
    for name in &forbidden {
        assert!(
            !names_word(&source, name),
            "`routing/pressure.rs` names `{name}` — the policy must stay tunable rather than \
             hard-code a hierarchy (line 1612); scanned for: {forbidden:?}"
        );
    }
    // And what *is* tunable is named, so a reader finds the knobs.
    for knob in [
        "routing.reserve",
        "capacity_band_thresholds",
        "reserve_percent",
    ] {
        assert!(
            source.contains(knob),
            "the module must name its knob `{knob}`"
        );
    }
}

/// **Lines 1294 and 1610: the policy still does not invent task completion,
/// and now it does not have to.** This test's predecessor asserted that
/// `task_nearly_complete: false` appeared exactly once and `true` nowhere,
/// and said in its own words: *"If this ever fails because a producer of
/// task progress was found, that decision and line 1610 re-open together; do
/// not relax it."* On 2026-09-03 that is what happened. The producer is a
/// **declaration** — `glasshouse task-progress` writing an expiring,
/// session-scoped row that
/// `crate::session::SessionStore::active_task_progress` reads back — so the
/// two lines closed together and this pin is **re-stated, not relaxed**.
///
/// What it asserts now is the part that was always load-bearing: **no
/// production construction site writes a literal `true`, and neither writes
/// a literal `false` any more either** — each reads the declaration. The old
/// count-of-`false` was a proxy for "the field is not being fabricated"; a
/// literal of either polarity is now the thing to catch, because a hard
/// `true` fabricates the protection and a hard `false` silently withdraws it
/// from the site that still compiles perfectly without it. `at least`
/// appears nowhere below on purpose: an inequality is what relaxing this
/// would look like.
///
/// **And it scans both sites.** `routing/pressure.rs::reserve_verdict` and
/// `routing/disposable/mod.rs`'s per-candidate loop both build
/// `ReserveDecisionInputs`. Only the first was ever scanned, so the second —
/// the one that decides per candidate — could have gained a `true` without
/// this test noticing. See `disposable_production_source` for why that
/// file's whole body is the production half.
///
/// **The citation this pins MOVED on 2026-09-01, and the substance did not.**
/// This test used to require the production source to name the process
/// document that recorded the refusal. `scripts/check-doc-boundary.sh`
/// forbids a product source file from citing a process document at all —
/// shipped code cannot act on how the project is run — so the two rules were
/// in direct contradiction, and this file was one of three that had been
/// shipping past the boundary gate. (This comment names no such path for the
/// same reason: the gate matches the path literal, wherever it appears.)
#[test]
fn the_policy_does_not_invent_task_completion() {
    for (file, source) in [
        ("routing/pressure.rs", production_source()),
        ("routing/disposable/mod.rs", disposable_production_source()),
    ] {
        assert!(
            !source.contains("task_nearly_complete: true"),
            "`{file}` sets task progress from a literal `true`: the reserve policy's first \
             branch would fire for work nobody declared, which inverts the protection instead \
             of applying it (lines 1294, 1610)"
        );
        assert!(
            !source.contains("task_nearly_complete: false"),
            "`{file}` sets task progress from a literal `false`: the declaration has a \
             producer now, and a site pinned to a literal is one that silently stopped \
             reading it (lines 1294, 1610)"
        );
        assert!(
            source.contains("task_nearly_complete"),
            "`{file}` no longer names the input at all"
        );
    }

    // Each site reads the value somebody declared, through the scoped type
    // that owns "for which session" — the `bool`-here/scope-at-the-producer
    // arrangement `user_override` already uses.
    assert!(
        production_source().contains("task_nearly_complete,"),
        "`routing/pressure.rs` must forward the declaration its caller passed in"
    );
    assert!(
        disposable_production_source()
            .contains("task_nearly_complete: self.task_progress.applies()"),
        "`routing/disposable/mod.rs` must read the scoped declaration rather than a literal"
    );

    // The ban on inferring it is what did not change, and it is recorded
    // where behaviour decisions are recorded.
    for cited in ["1610", "design-decisions", "declaration"] {
        assert!(
            production_source().contains(cited),
            "the decision must still cite `{cited}`"
        );
    }
}

// A scan for forbidden *words* — `turn_count`, `elapsed` — was written here
// and removed: it fails on the module's own doc comments, which say in as
// many words that a proxy from turn counts or elapsed time would invert the
// policy. A pin that punishes stating the invariant is worse than no pin,
// and this module's habit is that every branch explains itself. What
// actually holds the line is above and is structural rather than lexical:
// neither site writes a literal of either polarity, `routing/pressure.rs`
// forwards the value its caller passed in, and
// `routing/disposable/mod.rs` reads the scoped declaration. A derivation
// would have to replace one of those three, and each is asserted by name.

// ===========================================================================
// Half two — the shipped binary.
//
// The fixture is `tests/route_command.rs`'s, reproduced rather than shared
// because integration tests are separate crates; the fake harness, the argv
// log and the planted quota cache are the same mechanism for the same
// reasons that file gives.
// ===========================================================================

const CREDENTIAL_VAR: &str = "GLASSHOUSE_PRESSURE_TEST_KEY";

/// Two launch profiles that differ in nothing the router scores except the
/// provider whose quota cache they read.
const QUOTA_PROFILES: &str = "\n\
     [providers.alpha-probe]\ntemplate = \"openrouter\"\n\
     credential_env = [\"GLASSHOUSE_PRESSURE_TEST_KEY\"]\n\n\
     [providers.beta-probe]\ntemplate = \"openrouter\"\n\
     credential_env = [\"GLASSHOUSE_PRESSURE_TEST_KEY\"]\n\n\
     [profiles.alpha]\nharness = \"claude-code\"\n\
     expected_protocol = \"anthropic-messages\"\n\n\
     [profiles.alpha.backend]\nkind = \"direct-provider\"\n\
     provider = \"alpha-probe\"\n\n\
     [profiles.beta]\nharness = \"claude-code\"\n\
     expected_protocol = \"anthropic-messages\"\n\n\
     [profiles.beta.backend]\nkind = \"direct-provider\"\n\
     provider = \"beta-probe\"\n";

struct Binary {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    argv_log: PathBuf,
}

impl Binary {
    fn with_config(extra: &str) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().to_path_buf();
        let root = base.join("workspace");
        std::fs::create_dir_all(root.join(".git")).expect("create project root");
        let root = std::fs::canonicalize(&root).expect("canonicalize project root");

        let bin_dir = base.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let argv_log = base.join("argv.log");
        let harness = install_fake_harness(&bin_dir);
        let escaped = harness.display().to_string().replace('\\', "\\\\");

        let config_dir = base.join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "version = 1\n\n\
                 [integrations.claude-code]\nenabled = true\nexecutable = \"{escaped}\"\n\
                 {extra}"
            ),
        )
        .expect("write user config");

        Self {
            _tmp: tmp,
            base,
            root,
            argv_log,
        }
    }

    fn glasshouse(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glasshouse"))
            .arg("--scope")
            .arg(&self.root)
            .arg("--data-dir")
            .arg(self.base.join("data"))
            .arg("--config-dir")
            .arg(self.base.join("config"))
            .args(args)
            .env(CREDENTIAL_VAR, "planted-opaque-pressure-value-35d")
            .env(ARGV_LOG_VAR, &self.argv_log)
            .env("PATH", self.base.join("empty-path"))
            .output()
            .expect("the glasshouse binary must be runnable")
    }

    fn stdout(&self, args: &[&str]) -> String {
        String::from_utf8_lossy(&self.glasshouse(args).stdout).into_owned()
    }

    fn data_dir(&self) -> PathBuf {
        self.base.join("data")
    }

    fn both_streams(output: &Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    fn harness_invocations(&self) -> Vec<String> {
        match std::fs::read_to_string(&self.argv_log) {
            Ok(log) => log.lines().map(str::to_owned).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Plant a gateway quota reading exactly where `GatewayQuotaCache::new`
    /// resolves one from this run's `--data-dir`, and prove it landed. The
    /// reset, when given, is the IETF delta the binary's own header reader
    /// turns into an absolute reset time.
    fn plant_quota(&self, provider: &str, remaining: i64, limit: i64, reset_seconds: Option<i64>) {
        let cache = glasshouse::provider::telemetry::GatewayQuotaCache::at(
            self.data_dir().join("gateway-quota"),
        );
        let limit = limit.to_string();
        let remaining = remaining.to_string();
        let reset = reset_seconds.map(|s| s.to_string());
        let mut headers = vec![
            ("ratelimit-limit", limit.as_str()),
            ("ratelimit-remaining", remaining.as_str()),
        ];
        if let Some(reset) = reset.as_deref() {
            headers.push(("ratelimit-reset", reset));
        }
        cache.store(
            provider,
            &glasshouse::provider::telemetry::RateLimitHeaders::read(headers),
            now_unix(),
        );
        assert!(
            cache.load(provider).is_some(),
            "the planted reading for `{provider}` must be on disk and readable"
        );
    }

    /// Start one session under each of the two profiles and return
    /// `(alpha_session, beta_session)`, read off the harness's own argv log.
    fn two_sessions(&self) -> (String, String) {
        for profile in ["alpha", "beta"] {
            let out =
                self.glasshouse(&["launch", "claude-code", "--headless", "--profile", profile]);
            assert!(
                out.status.success(),
                "launching under `{profile}` must succeed:\n{}",
                Self::both_streams(&out)
            );
        }
        let invocations = self.harness_invocations();
        assert_eq!(invocations.len(), 2, "{invocations:?}");
        let alpha = session_arg(&invocations[0], "--session-id");
        let beta = session_arg(&invocations[1], "--session-id");
        assert_ne!(alpha, beta);
        (alpha, beta)
    }

    /// Launch with no destination flags and return the session that was
    /// resumed.
    fn launch_and_read_resumed(&self) -> String {
        let out = self.glasshouse(&["launch", "claude-code", "--headless"]);
        let said = Self::both_streams(&out);
        assert!(
            out.status.success(),
            "the deciding launch must succeed:\n{said}"
        );
        let invocations = self.harness_invocations();
        assert_eq!(
            invocations.len(),
            3,
            "the deciding launch must have continued one of the two existing sessions:\n\
             {invocations:?}\n{said}"
        );
        session_arg(&invocations[2], "--resume")
    }
}

/// The env var each spawned harness reads its argv-log destination from,
/// set per spawn by [`Binary::glasshouse`] rather than baked into the
/// script bytes — see [`shared_fixture`]'s doc for why.
const ARGV_LOG_VAR: &str = "GLASSHOUSE_TEST_ARGV_LOG";

/// Write each distinct fixture executable once per test binary instead of
/// once per test, so macOS Gatekeeper (`syspolicyd`/XProtect) validates it
/// once per run instead of once per test — see the project memory
/// `gatekeeper-scans-make-pty-fixtures-flaky` and GH-FIXTURE-REUSE /
/// GH-ARGV-LOG-HOIST. The argv-log destination used to be interpolated into
/// the script bytes, which made every call's content distinct; it is now
/// read from `ARGV_LOG_VAR` at spawn time (set by the caller's `Command`),
/// so the script bytes are constant and every call below collapses onto the
/// one file the first caller writes.
///
/// Sharing is keyed by content, never by the caller's requested name, so a
/// name never causes two distinct fixtures to collide, and a repeated name
/// with the same bytes never causes a second write. Race-free the way
/// `provider/cache.rs::write_json_atomically` is: one process-wide mutex
/// serialises the check-and-write, and the write itself lands in a
/// same-directory temporary name before an atomic rename.
fn shared_fixture(unique_name: &str, contents: &str) -> PathBuf {
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};
    use std::sync::{Mutex, OnceLock};

    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    static CACHE: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();

    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().expect("shared fixture cache poisoned");
    if let Some(path) = guard.get(contents) {
        return path.clone();
    }

    let dir = DIR.get_or_init(|| tempfile::tempdir().expect("shared fixture dir"));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    contents.hash(&mut hasher);
    let digest = format!("{:016x}", hasher.finish());
    let named = Path::new(unique_name);
    let stem = named
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(unique_name);
    let filename = match named.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem}-{digest}.{ext}"),
        None => format!("{stem}-{digest}"),
    };
    let path = dir.path().join(&filename);
    let temporary = dir.path().join(format!("{filename}.writing"));
    std::fs::write(&temporary, contents).expect("write shared fixture");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&temporary).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&temporary, perms).unwrap();
    }
    std::fs::rename(&temporary, &path).expect("rename shared fixture into place");
    guard.insert(contents.to_string(), path.clone());
    path
}

#[cfg(unix)]
fn install_fake_harness(_bin_dir: &Path) -> PathBuf {
    shared_fixture(
        "fake-claude-code",
        &format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"${ARGV_LOG_VAR}\"\nexit 0\n"),
    )
}

#[cfg(windows)]
fn install_fake_harness(_bin_dir: &Path) -> PathBuf {
    shared_fixture(
        "fake-claude-code.cmd",
        &format!("@echo off\r\necho %*>>\"%{ARGV_LOG_VAR}%\"\r\nexit /b 0\r\n"),
    )
}

#[cfg(test)]
mod shared_fixture_proof {
    use super::{Binary, install_fake_harness};

    /// `ARGV_LOG_VAR` is read only by the byte-for-byte fixture check in this
    /// module, which is `#[cfg(unix)]` because the shared fixture is a
    /// `#!/bin/sh` script there and a `.cmd` file on Windows. Gated to the
    /// same cfg as its only user rather than silenced with an `allow`.
    #[cfg(unix)]
    use super::ARGV_LOG_VAR;

    /// **The once-per-binary proof, through the real caller.** Every test in
    /// this file that spawns the harness goes through `Binary::with_config`,
    /// which unconditionally calls `install_fake_harness` — so two
    /// independent per-test tempdirs asking for it, the ordinary shape this
    /// binary runs under, must collapse to one file rather than each
    /// writing its own.
    #[test]
    fn two_tempdirs_installing_the_fake_harness_get_one_shared_file() {
        let tmp_a = tempfile::tempdir().expect("tempdir a");
        let tmp_b = tempfile::tempdir().expect("tempdir b");
        let a = install_fake_harness(tmp_a.path());
        let meta_before = std::fs::metadata(&a).expect("fixture exists after first install");

        let b = install_fake_harness(tmp_b.path());
        assert_eq!(
            a, b,
            "two different tempdirs installing the fixture must share one file"
        );
        assert!(
            !a.starts_with(tmp_a.path()) && !a.starts_with(tmp_b.path()),
            "the shared file must live in the per-binary fixture dir, not either \
             test's own tempdir: {a:?}"
        );

        let meta_after = std::fs::metadata(&b).expect("fixture exists after second install");
        assert_eq!(
            meta_before.modified().unwrap(),
            meta_after.modified().unwrap(),
            "a second install of the same fixture must not rewrite the file"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                meta_before.ino(),
                meta_after.ino(),
                "a second install of the same fixture must return the same inode, \
                 not a second copy"
            );
        }
    }

    /// **Bytes constant.** The shared fixture's bytes read the argv-log
    /// destination from `ARGV_LOG_VAR` rather than embedding a per-test
    /// path, so the script text is the same regardless of which tempdir
    /// asked for it.
    #[cfg(unix)]
    #[test]
    fn the_shared_fixture_reads_its_log_path_from_the_env_var_not_the_script() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = install_fake_harness(tmp.path());
        let content = std::fs::read_to_string(&path).expect("read shared fixture");
        assert_eq!(
            content,
            format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"${ARGV_LOG_VAR}\"\nexit 0\n"),
            "the shared fixture's bytes must read the log destination from the env var, \
             not have a path baked in"
        );
    }

    /// **End-to-end, through the real caller.** The env var the fixture
    /// reads is exactly the one `Binary::glasshouse` sets per spawn —
    /// proven by actually launching and reading the argv log back, not by
    /// inspecting the script text alone.
    #[test]
    fn a_real_launch_through_the_shared_fixture_writes_its_argv_to_the_requested_log() {
        let binary = Binary::with_config("");
        let out = binary.glasshouse(&["launch", "claude-code", "--headless"]);
        assert!(
            out.status.success(),
            "launch must succeed:\n{}",
            Binary::both_streams(&out)
        );
        let invocations = binary.harness_invocations();
        assert_eq!(
            invocations.len(),
            1,
            "the shared, env-driven fixture must still log exactly one invocation \
             into this binary's own argv log:\n{invocations:?}"
        );
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the system clock is after 1970")
        .as_secs() as i64
}

fn session_arg(argv: &str, flag: &str) -> String {
    let mut tokens = argv.split_whitespace();
    while let Some(token) = tokens.next() {
        if token == flag {
            return tokens
                .next()
                .unwrap_or_else(|| panic!("`{flag}` carried no identifier in `{argv}`"))
                .to_owned();
        }
    }
    panic!("no `{flag}` in `{argv}`")
}

/// **Lines 1573, 1574 and 1576 through the acting path, as a mirrored pair.**
///
/// Two existing sessions, both at 30% — the tight band under the default
/// thresholds — so `known quota pressure` is equal and only the reset the
/// binary reads off the planted header can separate them. The one whose
/// window resets in a minute keeps the work; swap the readings and the other
/// does. A build where `destination_capacity` stops attaching
/// `CapacityFacts` fails here, and nothing in half one can keep it passing.
#[test]
fn the_launch_path_reads_the_band_and_the_reset_and_the_explanation_names_them() {
    let alpha_soon = Binary::with_config(QUOTA_PROFILES);
    let (alpha, beta) = alpha_soon.two_sessions();
    alpha_soon.plant_quota("alpha-probe", 30, 100, Some(60));
    alpha_soon.plant_quota("beta-probe", 30, 100, None);
    assert_eq!(
        alpha_soon.launch_and_read_resumed(),
        alpha,
        "the session whose subscription resets in a minute must keep the work. \
         alpha={alpha} beta={beta}"
    );

    let beta_soon = Binary::with_config(QUOTA_PROFILES);
    let (alpha, beta) = beta_soon.two_sessions();
    beta_soon.plant_quota("alpha-probe", 30, 100, None);
    beta_soon.plant_quota("beta-probe", 30, 100, Some(60));
    assert_eq!(
        beta_soon.launch_and_read_resumed(),
        beta,
        "swapping the resets must swap the destination. alpha={alpha} beta={beta}"
    );

    // Line 1576: the explanation a person reads carries the band and the
    // reset, in words, from the binary's own reading.
    let explained = beta_soon.stdout(&["route"]);
    assert!(explained.contains("capacity band"), "{explained}");
    assert!(explained.contains("in the tight band"), "{explained}");
    assert!(explained.contains("penalty is waived"), "{explained}");
    assert!(
        explained.contains("full conservation penalty"),
        "{explained}"
    );
}

/// **Lines 1571 and 1612 through the acting path, as a mirrored pair.**
///
/// Both sessions at 10%. A per-provider `reserve_percent = 5` puts one in
/// the tight band and leaves the other, under the default reserve of 20%, in
/// its reserve band — the same figure, two bands, decided by configuration
/// alone. No tier is established on the launch path, so the reserve is
/// protected conservatively and the tight one keeps the work; move the
/// override to the other provider and the answer follows it.
#[test]
fn the_reserve_band_and_its_per_provider_threshold_reach_the_launch_path() {
    const ALPHA_LOW_RESERVE: &str = "\n[providers.alpha-probe.quota]\nreserve_percent = 5\n";
    const BETA_LOW_RESERVE: &str = "\n[providers.beta-probe.quota]\nreserve_percent = 5\n";

    let alpha_tight = Binary::with_config(&format!("{QUOTA_PROFILES}{ALPHA_LOW_RESERVE}"));
    let (alpha, beta) = alpha_tight.two_sessions();
    alpha_tight.plant_quota("alpha-probe", 10, 100, None);
    alpha_tight.plant_quota("beta-probe", 10, 100, None);
    assert_eq!(
        alpha_tight.launch_and_read_resumed(),
        alpha,
        "the session outside its reserve band must keep the work. alpha={alpha} beta={beta}"
    );

    let beta_tight = Binary::with_config(&format!("{QUOTA_PROFILES}{BETA_LOW_RESERVE}"));
    let (alpha, beta) = beta_tight.two_sessions();
    beta_tight.plant_quota("alpha-probe", 10, 100, None);
    beta_tight.plant_quota("beta-probe", 10, 100, None);
    assert_eq!(
        beta_tight.launch_and_read_resumed(),
        beta,
        "moving the reserve override must move the destination. alpha={alpha} beta={beta}"
    );

    let explained = beta_tight.stdout(&["route"]);
    assert!(explained.contains("in the reserve band"), "{explained}");
    assert!(explained.contains("denies the spend"), "{explained}");
    assert!(
        explained.contains("interactive reserve policy is `protect`"),
        "{explained}"
    );
    assert!(explained.contains("not established"), "{explained}");
}

/// **Line 1577 through the reporting path.** `routing.reserve.interactive =
/// "spend"` is read by the launch and route paths; `routing.reserve.background`
/// alone is not, and the explanation says which policy applied.
#[test]
fn the_interactive_reserve_policy_is_read_from_configuration() {
    const RESERVE_AT_10: &str = "\n[providers.alpha-probe.quota]\nreserve_percent = 5\n";

    let interactive_spend = Binary::with_config(&format!(
        "{QUOTA_PROFILES}{RESERVE_AT_10}\n[routing.reserve]\ninteractive = \"spend\"\n"
    ));
    interactive_spend.two_sessions();
    interactive_spend.plant_quota("alpha-probe", 10, 100, None);
    interactive_spend.plant_quota("beta-probe", 10, 100, None);
    let explained = interactive_spend.stdout(&["route"]);
    assert!(explained.contains("in the reserve band"), "{explained}");
    assert!(
        explained.contains("interactive reserve policy is `spend`"),
        "{explained}"
    );
    assert!(!explained.contains("denies the spend"), "{explained}");

    let background_spend = Binary::with_config(&format!(
        "{QUOTA_PROFILES}{RESERVE_AT_10}\n[routing.reserve]\nbackground = \"spend\"\n"
    ));
    background_spend.two_sessions();
    background_spend.plant_quota("alpha-probe", 10, 100, None);
    background_spend.plant_quota("beta-probe", 10, 100, None);
    let explained = background_spend.stdout(&["route"]);
    assert!(explained.contains("denies the spend"), "{explained}");
    assert!(
        explained.contains("interactive reserve policy is `protect`"),
        "{explained}"
    );
}

/// **Line 1575's producer, on the reporting path.** A profile whose named
/// model the user marked in its provider's `free_models` is a zero-cost
/// destination to the router, and the pressure terms say so; a metered
/// profile is not. A build where `destination_backend` goes back to calling
/// everything metered fails here.
#[test]
fn a_free_model_profile_is_a_zero_cost_destination_on_the_routing_path() {
    const FREE_AND_METERED: &str = "\n\
         [providers.free-probe]\ntemplate = \"openrouter\"\n\
         credential_env = [\"GLASSHOUSE_PRESSURE_TEST_KEY\"]\n\
         free_models = [\"the-free-model\"]\n\n\
         [providers.paid-probe]\ntemplate = \"openrouter\"\n\
         credential_env = [\"GLASSHOUSE_PRESSURE_TEST_KEY\"]\n\n\
         [profiles.free]\nharness = \"claude-code\"\n\
         expected_protocol = \"anthropic-messages\"\nmodel = \"the-free-model\"\n\n\
         [profiles.free.backend]\nkind = \"direct-provider\"\n\
         provider = \"free-probe\"\n\n\
         [profiles.paid]\nharness = \"claude-code\"\n\
         expected_protocol = \"anthropic-messages\"\nmodel = \"the-free-model\"\n\n\
         [profiles.paid.backend]\nkind = \"direct-provider\"\n\
         provider = \"paid-probe\"\n";

    let binary = Binary::with_config(FREE_AND_METERED);
    binary.plant_quota("paid-probe", 30, 100, None);
    let explained = binary.stdout(&["route"]);

    let block = |profile: &str| -> String {
        let marker = format!("fresh:claude-code:{profile}");
        let start = explained
            .find(&marker)
            .unwrap_or_else(|| panic!("`{marker}` is not in the report:\n{explained}"));
        explained[start..]
            .lines()
            .take_while(|line| !line.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let free = block("free");
    assert!(
        free.contains("inert: a zero-cost resource"),
        "the free profile must be a zero-cost destination:\n{free}\n---\n{explained}"
    );
    let paid = block("paid");
    assert!(
        paid.contains("in the tight band"),
        "the same model on a provider that did not mark it free is metered and under \
         pressure:\n{paid}\n---\n{explained}"
    );
}
