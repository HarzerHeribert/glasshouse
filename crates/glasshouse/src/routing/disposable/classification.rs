use super::candidates::{DisposableCandidate, MeteredUse};
use crate::provider::pricing::ModelPrice;
use crate::provider::registry::Locality;
use crate::routing::classify::CLASSIFICATION_PROMPT_CONTRACT;
use crate::routing::evidence::MIN_SAMPLE_FOR_SUMMARY;
use crate::routing::request::TASK_TEXT_CEILING_BYTES;
use crate::routing::{Contribution, Cost};

/// Capability map lines 1422 and 1432: the share of classification calls
/// that must have come back in the schema for a candidate to stay eligible,
/// once it has enough history to be judged at all.
///
/// Four in five. Every reply that fails to parse costs a full model call
/// *and* falls back to the heuristic answer anyway, so a classifier below
/// this line is paying for a call on more than one request in five to
/// produce exactly what the heuristic would have produced for nothing.
pub const CLASSIFICATION_RELIABILITY_FLOOR: f64 = 0.8;

/// How many outcome-carrying classification calls a candidate needs before
/// [`CLASSIFICATION_RELIABILITY_FLOOR`] applies to it — the evidence
/// ledger's own [`MIN_SAMPLE_FOR_SUMMARY`], so "enough to judge" means one
/// number across this crate. Below it a candidate is *unproven*, which is a
/// different fact from *unreliable* and is never grounds for exclusion.
pub const CLASSIFICATION_RELIABILITY_MIN_OBSERVATIONS: usize = MIN_SAMPLE_FOR_SUMMARY;

/// The dimension name `crate::provider::quota::CapacityState::remaining_capacity_score`
/// stamps on a score bound by the per-minute request ceiling. Matched by
/// string because that function names its dimension as prose for a
/// diagnostic, and `tests/routing_economics.rs` pins the spelling by
/// building an RPM-bound score and asserting the preference fires.
pub(super) const REQUESTS_PER_MINUTE_DIMENSION: &str = "requests per minute";

/// How much each classification preference can add to a candidate's score
/// at its best — capability map lines 1420, 1421, 1422 and 1438. The same
/// for all four, so no measured preference outranks another by
/// construction: a candidate that wins on one and loses on another is
/// decided by the size of the margins, which is the only thing this module
/// can say about them honestly. The weight never competes with the cost
/// term — free and metered candidates are ranked in separate loops — nor
/// with the user's own order, which `FreePreferences::arrange` applies
/// after these have been summed.
pub(super) const CLASSIFICATION_PREFERENCE_WEIGHT: f64 = 0.25;

/// Capability map line 1539's own weight, for the expected-latency term
/// [`DisposableRouting::score`] adds beside classification latency's.
/// Deliberately equal to [`CLASSIFICATION_PREFERENCE_WEIGHT`] rather than a
/// second number: both terms are the same kind of fact — a measured median
/// duration, preferred lower — so giving support-work latency a different
/// weight would claim, with no evidence behind it, that one matters more
/// than the other.
pub(super) const LATENCY_PREFERENCE_WEIGHT: f64 = CLASSIFICATION_PREFERENCE_WEIGHT;

/// The classification-side requirements a candidate must meet before it may
/// be asked to classify — capability map lines 1427 (local only) and 1435
/// (latency ceiling) — carried on the policy, like [`super::ReserveOverride`],
/// because they are this instance's standing rules rather than one call's
/// argument.
///
/// Both default to *not applied*: no ceiling, and no confinement. `main.rs`
/// fills them from the layered `[routing]` configuration for the automatic
/// classification path and nowhere else, so memory extraction and every
/// other [`super::candidates::JobKind`] keep exactly the behaviour they had.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClassificationPolicy {
    max_latency_ms: Option<u32>,
    local_only: bool,
    max_marginal_cost_micro_usd: Option<u32>,
    /// Capability map line 1419: the per-token price of the destination a
    /// task lands on when classification does nothing — the launch
    /// profile's own backend, priced through `pricing.toml`, and `None`
    /// when that backend is a harness's own sign-in or otherwise unpriced.
    /// `main.rs::classify_for_routing` is the only production caller that
    /// ever has one to give: every diagnostic path (`glasshouse resources`,
    /// `glasshouse classify`, `glasshouse route`) has not chosen a launch
    /// profile and passes `None`, which leaves the comparison inert rather
    /// than guessed from a plan name.
    protected_capacity_price: Option<ModelPrice>,
}

impl ClassificationPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Capability map line 1435: exclude a candidate whose *measured*
    /// median classification latency exceeds this many milliseconds. A
    /// candidate with no median is not measured and is never excluded by
    /// it.
    #[must_use]
    pub fn with_max_latency_ms(mut self, max_latency_ms: Option<u32>) -> Self {
        self.max_latency_ms = max_latency_ms;
        self
    }

    /// Capability map line 1427: admit only candidates known to run
    /// locally.
    #[must_use]
    pub fn with_local_only(mut self, local_only: bool) -> Self {
        self.local_only = local_only;
        self
    }

    /// Capability map line 1436: exclude a metered candidate whose
    /// [`estimated_classification_cost_micro_usd`] exceeds this many
    /// millionths of a US dollar. `None` applies no ceiling — the default,
    /// so every test double that predates this line keeps its exact
    /// existing behaviour. A free candidate is never affected, and a
    /// metered candidate with no price is *unpriced*, never excluded by it.
    #[must_use]
    pub fn with_max_marginal_cost_micro_usd(
        mut self,
        max_marginal_cost_micro_usd: Option<u32>,
    ) -> Self {
        self.max_marginal_cost_micro_usd = max_marginal_cost_micro_usd;
        self
    }

    pub fn max_latency_ms(&self) -> Option<u32> {
        self.max_latency_ms
    }

    pub fn local_only(&self) -> bool {
        self.local_only
    }

    pub fn max_marginal_cost_micro_usd(&self) -> Option<u32> {
        self.max_marginal_cost_micro_usd
    }

    /// Capability map line 1419: the premium capacity this classification
    /// protects, priced. `None` leaves the *protected capacity* term inert
    /// — see `classification_verdict`.
    #[must_use]
    pub fn with_protected_capacity_price(
        mut self,
        protected_capacity_price: Option<ModelPrice>,
    ) -> Self {
        self.protected_capacity_price = protected_capacity_price;
        self
    }

    pub fn protected_capacity_price(&self) -> Option<ModelPrice> {
        self.protected_capacity_price
    }
}

/// Map line 1436: bytes of English prose approximated as one token — the
/// conventional ratio, conservative for JSON or code (a token there is
/// usually shorter, so this over-counts, never under-counts, the true
/// input). The only place in this crate that turns a byte count into a
/// token estimate.
const BYTES_PER_TOKEN_ESTIMATE: usize = 4;

/// Map line 1436: a stated upper bound on the classification reply's length
/// in tokens — the schema is ten flat keys, mostly booleans and short enum
/// tags (see [`crate::routing::classify::CLASSIFICATION_RESPONSE_SCHEMA`]),
/// which is a handful of short fields, not a measurement of any real reply.
const CLASSIFICATION_REPLY_TOKENS: u64 = 64;

/// Map line 1436's estimate: the most one classification call to a model
/// priced `price` could cost, in millionths of a US dollar.
///
/// # This is a ceiling estimate, not a prediction
///
/// The input side assumes the whole task-text budget
/// ([`TASK_TEXT_CEILING_BYTES`]) is spent on top of the fixed prompt
/// contract ([`CLASSIFICATION_PROMPT_CONTRACT`]), and the output side
/// assumes the reply uses every one of `CLASSIFICATION_REPLY_TOKENS`. A
/// candidate is excluded by `classification_verdict` only when even this
/// largest permitted call would be over the ceiling — a real call, almost
/// always shorter, could still come in under it.
///
/// Bytes become tokens at `BYTES_PER_TOKEN_ESTIMATE` bytes per token.
/// [`ModelPrice`]'s fields are dollars per **million** tokens, so a token
/// count times a per-million price is already millionths of a dollar per
/// token — micro-USD — with no further scaling.
pub fn estimated_classification_cost_micro_usd(price: ModelPrice) -> u64 {
    let input_tokens = (CLASSIFICATION_PROMPT_CONTRACT.len() + TASK_TEXT_CEILING_BYTES)
        .div_ceil(BYTES_PER_TOKEN_ESTIMATE) as u64;
    let input_micro_usd = input_tokens as f64 * price.input_per_million_usd;
    let output_micro_usd = CLASSIFICATION_REPLY_TOKENS as f64 * price.output_per_million_usd;
    (input_micro_usd + output_micro_usd).round() as u64
}

/// Render exact micro-USD as a compact decimal dollar amount — the same
/// shape `crate::shell::state::format_usd` renders for a
/// [`crate::config::RouterCostMicroUsd`], reproduced here on a bare `u64`
/// because an estimate is not bounded by that type's range and this module
/// carries no dependency on `crate::config` or `crate::shell`.
fn format_micro_usd(value: u64) -> String {
    let dollars = value / 1_000_000;
    let fraction = value % 1_000_000;
    format!("${dollars}.{fraction:06}")
}

/// One candidate's standing against [`ClassificationPolicy`] and the
/// reliability floor, decided by [`classification_verdict`].
pub(super) enum ClassificationVerdict {
    /// Admitted, with one [`Contribution`] per requirement saying whether
    /// it applied or was inert — rendered on the winner's explanation so a
    /// reader can see which requirements were actually *measured*.
    Admitted {
        notes: Vec<Contribution>,
    },
    Excluded {
        reason: String,
    },
}

/// Capability map line 1419: whether `candidate`'s own estimated
/// classification cost is materially lower than the premium capacity
/// `policy` protects — the destination a task lands on when classification
/// does nothing (`design-decisions.md`, *"The premium capacity a classifier
/// protects"*). `+1.0` at or under one tenth of that cost, a bounded
/// negative magnitude above it, `0.0` with a reason when either side cannot
/// be compared. **Never excludes** — that is the 1436 ceiling's job, right
/// above this term's call site in [`classification_verdict`]; this term
/// only orders.
fn protected_capacity_note(
    policy: &ClassificationPolicy,
    candidate: &DisposableCandidate,
) -> Contribution {
    const NAME: &str = "protected capacity";
    if candidate.cost == Cost::Free {
        return Contribution::new(
            NAME,
            0.0,
            "free — protects everything it is asked to (map line 1419)".to_owned(),
        );
    }
    let Some(protected_price) = policy.protected_capacity_price else {
        return Contribution::new(
            NAME,
            0.0,
            "the launch's destinations are unpriced — nothing to compare against (map line 1419)"
                .to_owned(),
        );
    };
    let Some(candidate_price) = candidate.price else {
        return Contribution::new(NAME, 0.0, "unpriced candidate (map line 1419)".to_owned());
    };
    let candidate_cost = estimated_classification_cost_micro_usd(candidate_price);
    let protected_cost = estimated_classification_cost_micro_usd(protected_price);
    let ratio = candidate_cost as f64 / protected_cost as f64;
    if ratio <= 0.1 {
        Contribution::new(
            NAME,
            1.0,
            format!(
                "estimated classification cost {} is {:.1}% of the protected destination's {} \
                 — materially lower (map line 1419)",
                format_micro_usd(candidate_cost),
                ratio * 100.0,
                format_micro_usd(protected_cost)
            ),
        )
    } else {
        Contribution::new(
            NAME,
            -(ratio - 0.1).min(1.0),
            format!(
                "estimated classification cost {} is {:.1}% of the protected destination's {} \
                 — not materially lower (map line 1419)",
                format_micro_usd(candidate_cost),
                ratio * 100.0,
                format_micro_usd(protected_cost)
            ),
        )
    }
}

/// Decide whether `candidate` may be asked to classify — the four filters
/// capability map lines 1427, 1436, 1432 and 1435 name, in that order.
///
/// Reliability, latency and price are **measurements**: one not yet taken
/// never excludes — a candidate with no record, fewer than
/// [`CLASSIFICATION_RELIABILITY_MIN_OBSERVATIONS`] outcomes, no median, or no
/// `pricing.toml` entry is admitted with a note that the requirement was
/// inert, the same rule [`has_no_known_headroom`] applies to capacity,
/// because turning "nothing measured" into "fails the bar" is a fabrication.
///
/// Locality is **not a measurement**: it is a fact the provider registry
/// states, and a caller that attaches none has declined to say rather than
/// failed to measure. Under [`ClassificationPolicy::local_only`] that fails
/// **closed** — a candidate nobody could say is local is not sent anything —
/// because a privacy constraint that admits on silence would send a request
/// off the machine on the strength of nobody having said where it runs.
// History: design-decisions.md, "Trims: routing module docs", routing/disposable/classification.rs `fn classification_verdict`.
pub(super) fn classification_verdict(
    policy: &ClassificationPolicy,
    candidate: &DisposableCandidate,
) -> ClassificationVerdict {
    let mut notes = Vec::with_capacity(4);

    // Map line 1427, first: a candidate this policy may never send to is
    // excluded before anything about its quality is even considered.
    if policy.local_only {
        match candidate.locality {
            Some(Locality::Local) => notes.push(Contribution::new(
                "local only",
                0.0,
                "local inference — admitted under classification_local_only (map line 1427)"
                    .to_owned(),
            )),
            Some(Locality::Remote) => {
                return ClassificationVerdict::Excluded {
                    reason: "remote, and classification is confined to local models — nothing \
                             is sent to it (map line 1427)"
                        .to_owned(),
                };
            }
            None => {
                return ClassificationVerdict::Excluded {
                    reason: "its locality was not stated, and classification is confined to \
                             local models — a candidate not known to be local is not sent \
                             anything (map line 1427)"
                        .to_owned(),
                };
            }
        }
    }

    // Map line 1436, second: a candidate the user's own price policy
    // forbids is excluded before its quality is weighed, the same
    // reasoning 1427's locality gate states above.
    match candidate.cost {
        Cost::Free => notes.push(Contribution::new(
            "price ceiling",
            0.0,
            "free — the price ceiling does not apply (map line 1436)".to_owned(),
        )),
        Cost::Metered => match policy.max_marginal_cost_micro_usd {
            None => notes.push(Contribution::new(
                "price ceiling",
                0.0,
                "no maximum marginal cost is configured for this decision (map line 1436)"
                    .to_owned(),
            )),
            Some(ceiling) => match candidate.price {
                None => notes.push(Contribution::new(
                    "price ceiling",
                    0.0,
                    "unpriced: no entry in pricing.toml — the ceiling is inert; unpriced, not \
                     expensive (map line 1436)"
                        .to_owned(),
                )),
                Some(price) => {
                    let estimate = estimated_classification_cost_micro_usd(price);
                    if estimate > u64::from(ceiling) {
                        return ClassificationVerdict::Excluded {
                            reason: format!(
                                "estimated classification cost {} exceeds the {} price ceiling \
                                 (map line 1436)",
                                format_micro_usd(estimate),
                                format_micro_usd(u64::from(ceiling))
                            ),
                        };
                    }
                    notes.push(Contribution::new(
                        "price ceiling",
                        0.0,
                        format!(
                            "estimated classification cost {} is within the {} price ceiling \
                             (map line 1436)",
                            format_micro_usd(estimate),
                            format_micro_usd(u64::from(ceiling))
                        ),
                    ));
                }
            },
        },
    }

    // Map line 1419, right after the 1436 ceiling: whether this candidate's
    // own estimated call is materially cheaper than the premium capacity it
    // protects — an *ordering* note, never an exclusion; only the 1436
    // ceiling above excludes on price.
    notes.push(protected_capacity_note(policy, candidate));

    // Map line 1432.
    match candidate.classification.as_ref() {
        None => notes.push(Contribution::new(
            "reliability floor",
            0.0,
            format!(
                "no classification history was read for this candidate — the {:.0}% floor is \
                 inert; unproven, not unreliable (map line 1432)",
                CLASSIFICATION_RELIABILITY_FLOOR * 100.0
            ),
        )),
        Some(record) if record.outcomes_recorded < CLASSIFICATION_RELIABILITY_MIN_OBSERVATIONS => {
            notes.push(Contribution::new(
                "reliability floor",
                0.0,
                format!(
                    "unproven, not unreliable: {} of {} classification calls parsed, fewer than \
                     the {} needed before the {:.0}% floor applies (map line 1432)",
                    record.parsed,
                    record.outcomes_recorded,
                    CLASSIFICATION_RELIABILITY_MIN_OBSERVATIONS,
                    CLASSIFICATION_RELIABILITY_FLOOR * 100.0
                ),
            ));
        }
        Some(record) => {
            let fraction = record
                .parsed_fraction()
                .expect("outcomes_recorded is at least the minimum, so above zero");
            if fraction < CLASSIFICATION_RELIABILITY_FLOOR {
                return ClassificationVerdict::Excluded {
                    reason: format!(
                        "only {} of {} classification calls came back in the schema ({:.0}%), \
                         below the {:.0}% reliability floor (map line 1432)",
                        record.parsed,
                        record.outcomes_recorded,
                        fraction * 100.0,
                        CLASSIFICATION_RELIABILITY_FLOOR * 100.0
                    ),
                };
            }
            notes.push(Contribution::new(
                "reliability floor",
                0.0,
                format!(
                    "{} of {} classification calls came back in the schema ({:.0}%), at or above \
                     the {:.0}% floor (map line 1432)",
                    record.parsed,
                    record.outcomes_recorded,
                    fraction * 100.0,
                    CLASSIFICATION_RELIABILITY_FLOOR * 100.0
                ),
            ));
        }
    }

    // Map line 1435.
    let median = candidate
        .classification
        .as_ref()
        .and_then(|record| record.median_duration_ms);
    let timed = candidate
        .classification
        .as_ref()
        .map_or(0, |record| record.timed);
    match (policy.max_latency_ms, median) {
        (None, _) => notes.push(Contribution::new(
            "latency ceiling",
            0.0,
            "no maximum routing latency is configured for this decision (map line 1435)".to_owned(),
        )),
        (Some(ceiling), None) => notes.push(Contribution::new(
            "latency ceiling",
            0.0,
            format!(
                "no latency figure yet ({timed} of {MIN_SAMPLE_FOR_SUMMARY} timed classification \
                 calls) — the {ceiling}ms ceiling is inert (map line 1435)"
            ),
        )),
        (Some(ceiling), Some(median)) if median > i64::from(ceiling) => {
            return ClassificationVerdict::Excluded {
                reason: format!(
                    "median classification latency {median}ms over {timed} timed calls exceeds \
                     the {ceiling}ms ceiling (map line 1435)"
                ),
            };
        }
        (Some(ceiling), Some(median)) => notes.push(Contribution::new(
            "latency ceiling",
            0.0,
            format!(
                "median classification latency {median}ms over {timed} timed calls is within the \
                 {ceiling}ms ceiling (map line 1435)"
            ),
        )),
    }

    ClassificationVerdict::Admitted { notes }
}

/// One evaluation of map line 1439's preference — [`time_price_preference`]'s
/// own return value, so a caller can tell "the candidate is switching" from
/// "nothing changed, and here is why" without parsing [`Contribution::evidence`]'s
/// prose.
pub(super) enum TimePricePreference {
    /// Both conditions held: the free candidate is unreliable enough and the
    /// metered candidate is cheap enough.
    Fires(Contribution),
    /// At least one condition failed, or a measurement or a price is
    /// missing; the contribution's own text says which.
    Inert(Contribution),
}

impl TimePricePreference {
    fn inert(evidence: String) -> Self {
        Self::Inert(Contribution::new("time versus price", 0.0, evidence))
    }
}

/// Map line 1439 — `design-decisions.md`'s *"Preferring a cheap metered
/// classifier over an unreliable free one"*, amended 2026-09-02: prefer
/// `metered` over `free` when `free`'s expected wasted retry time — `(1 -
/// parsed_fraction) * median_ms` over its own classification record, above
/// the reliability sample floor — **exceeds `metered`'s own median
/// classification latency**, also above the floor, and `metered`'s
/// estimated call cost is at or below `policy`'s marginal-cost ceiling.
/// `[routing] max_router_latency` plays no part here — that knob stays
/// 1435's alone. No exchange rate between milliseconds and micro-dollars
/// exists anywhere in this build: the comparison is between two *times*,
/// and the cost half is checked only against the user's own ceiling.
///
/// This compares `free`'s wasted time against `metered`'s **own** measured
/// latency rather than against `max_router_latency` (an earlier version of
/// this rule, withdrawn): both times come from candidates that already pass
/// 1432/1435/1436 on their own terms, so this rule can fire on a candidate
/// the router was genuinely about to choose — comparing against
/// `max_router_latency` instead could only ever fire on a candidate 1435
/// had already excluded, an account of an exclusion rather than a
/// preference that could change a choice.
// History: design-decisions.md, "Trims: routing module docs", routing/disposable/classification.rs `fn time_price_preference`.
pub(super) fn time_price_preference(
    policy: &ClassificationPolicy,
    free: &DisposableCandidate,
    metered: &DisposableCandidate,
) -> TimePricePreference {
    let Some(free_record) = free.classification.as_ref() else {
        return TimePricePreference::inert(format!(
            "no classification history was read for free {} — the time-versus-price preference \
             is inert; unmeasured, not unreliable (map line 1439)",
            free.model()
        ));
    };
    if free_record.outcomes_recorded < CLASSIFICATION_RELIABILITY_MIN_OBSERVATIONS {
        return TimePricePreference::inert(format!(
            "unmeasured: {} of {} classification calls parsed for free {}, fewer than the {} \
             needed before the time-versus-price preference applies (map line 1439)",
            free_record.parsed,
            free_record.outcomes_recorded,
            free.model(),
            CLASSIFICATION_RELIABILITY_MIN_OBSERVATIONS
        ));
    }
    let Some(free_median_ms) = free_record.median_duration_ms else {
        return TimePricePreference::inert(format!(
            "no median classification latency yet for free {} — the time-versus-price \
             preference is inert; unmeasured, not unreliable (map line 1439)",
            free.model()
        ));
    };
    let Some(metered_median_ms) = metered
        .classification
        .as_ref()
        .and_then(|record| record.median_duration_ms)
    else {
        return TimePricePreference::inert(format!(
            "no median classification latency yet for metered {} — the time-versus-price \
             preference is inert; unmeasured, not unreliable (map line 1439)",
            metered.model()
        ));
    };
    let fraction = free_record
        .parsed_fraction()
        .expect("outcomes_recorded is at least the minimum, so above zero");
    let wasted_ms = ((1.0 - fraction) * free_median_ms as f64).round() as i64;

    if wasted_ms <= metered_median_ms {
        return TimePricePreference::inert(format!(
            "free {} expects {wasted_ms}ms of wasted retries per call, within metered {}'s own \
             {metered_median_ms}ms median classification latency (map line 1439)",
            free.model(),
            metered.model()
        ));
    }

    let Some(max_cost) = policy.max_marginal_cost_micro_usd else {
        return TimePricePreference::inert(format!(
            "free {} expects {wasted_ms}ms of wasted retries per call, over metered {}'s own \
             {metered_median_ms}ms median classification latency, but no maximum marginal cost \
             is configured (map line 1439)",
            free.model(),
            metered.model()
        ));
    };
    let Some(price) = metered.price() else {
        return TimePricePreference::inert(format!(
            "free {} expects {wasted_ms}ms of wasted retries per call, over metered {}'s own \
             {metered_median_ms}ms median classification latency, but metered {} is unpriced — \
             unpriced is never cheap enough (map line 1439)",
            free.model(),
            metered.model(),
            metered.model()
        ));
    };
    let estimate = estimated_classification_cost_micro_usd(price);
    if estimate > u64::from(max_cost) {
        return TimePricePreference::inert(format!(
            "free {} expects {wasted_ms}ms of wasted retries per call, over metered {}'s own \
             {metered_median_ms}ms median classification latency, but metered {} at {} is over \
             the {} cost ceiling (map line 1439)",
            free.model(),
            metered.model(),
            metered.model(),
            format_micro_usd(estimate),
            format_micro_usd(u64::from(max_cost))
        ));
    }

    TimePricePreference::Fires(Contribution::new(
        "time versus price",
        0.0,
        format!(
            "free {} expects {wasted_ms}ms of wasted retries per call, over metered {}'s own \
             {metered_median_ms}ms median classification latency; metered {} at ~{} per call is \
             under the {} ceiling (map line 1439)",
            free.model(),
            metered.model(),
            metered.model(),
            format_micro_usd(estimate),
            format_micro_usd(u64::from(max_cost))
        ),
    ))
}

/// Map line 1439: the cheapest priced metered candidate among the ones
/// [`classification_verdict`] has already admitted — so whatever this
/// preference prefers has passed every other gate — ties broken by the
/// admitted list's own order (`Iterator::min_by_key` keeps the first of
/// equal elements, matching every other free-vs-metered tie-break in this
/// module).
///
/// `None` when [`MeteredUse`] withholds metered spending entirely — a
/// withheld policy has no metered candidate this preference may ever choose,
/// so there is nothing to compare the free candidate's reliability against —
/// or when nothing admitted and metered is priced.
pub(super) fn cheapest_priced_metered<'a>(
    admitted_candidates: &'a [DisposableCandidate],
    metered_use: &MeteredUse,
) -> Option<&'a DisposableCandidate> {
    if !metered_use.permits_metered() {
        return None;
    }
    admitted_candidates
        .iter()
        .filter(|candidate| !candidate.cost().is_free() && candidate.price().is_some())
        .min_by_key(|candidate| {
            estimated_classification_cost_micro_usd(
                candidate.price().expect("filtered to priced candidates"),
            )
        })
}
