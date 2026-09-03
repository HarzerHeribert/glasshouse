use crate::provider::pricing::ModelPrice;
use crate::provider::quota::{CapacityBand, RemainingCapacityScore};
use crate::provider::registry::Locality;
use crate::routing::evidence::{ClassificationRecord, LatencyRecord};
use crate::routing::free::{FreeResource, FreeResourceKey};
use crate::routing::{Cost, CredentialId};

/// The kind of bounded internal work a choice is being made for.
///
/// Carried so that a chosen resource can be recorded against the job that
/// used it — Phase 39's "record which resource performed important memory
/// extraction or classification for debugging" needs the pair, and a job
/// kind is a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Classification,
    MemoryExtraction,
    Reranking,
    /// Glasshouse's own automated evaluation or test run.
    Evaluation,
    /// The context firewall's semantic reducer (Phase 57B, map line 1997) —
    /// a disposable job that selects which of the deterministic ladder's
    /// retained candidates a coding session actually needs. Never a
    /// reranking job: it never reorders the candidates it is given, only
    /// keeps or drops them by id. Adding this variant fires
    /// `disposable_interface.rs`'s roster tripwire by design — see that
    /// test's own doc comment and Phase 39's 1625 refusal, which this
    /// variant does not by itself make reachable (1625 is about
    /// *reranking*, and this job never reorders).
    ContextReduction,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Classification => "classification",
            Self::MemoryExtraction => "memory extraction",
            Self::Reranking => "reranking",
            Self::Evaluation => "evaluation",
            Self::ContextReduction => "context-reduction",
        }
    }
}

impl std::fmt::Display for JobKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// Whether this policy may spend metered capacity at all.
///
/// Three states, because two would not distinguish the two ways a policy can
/// be allowed to spend: ordinary support work may fall back to a metered
/// resource when no free one can serve, whereas Glasshouse's own runs may do
/// so only after somebody said so by name. Collapsing them would make
/// line 539's "explicit opt-in" indistinguishable from a default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeteredUse {
    /// Ordinary support work: a metered resource is a legitimate last resort.
    Permitted,
    /// Withheld. Nothing metered will be chosen, and a job with no free
    /// resource available fails instead.
    Withheld,
    /// Withheld by default, and then given. `by` names what gave it, so a
    /// later reader can find the switch that was thrown.
    OptedIn { by: &'static str },
}

impl MeteredUse {
    /// The environment variable an automated Glasshouse run opts in through.
    ///
    /// One name, spelled once. A second spelling is how "never without an
    /// explicit opt-in" becomes "unless you set the other one".
    pub const OPT_IN_VAR: &'static str = "GLASSHOUSE_ALLOW_METERED_MODELS";

    /// Read the opt-in for an automated run, defaulting to
    /// [`MeteredUse::Withheld`].
    ///
    /// `read` is injected rather than calling [`std::env::var`] here: this
    /// module is pure by rule (see [`mod@super`]), and a test that had to set
    /// a process-wide environment variable to check the default would be a
    /// test that raced every other test in the binary.
    ///
    /// Anything other than exactly `1` leaves it withheld. Not
    /// case-insensitive `true`, not "any non-empty value": the fail-closed
    /// direction, where a stray value spends nothing.
    pub fn for_automated_run(read: impl Fn(&str) -> Option<String>) -> Self {
        match read(Self::OPT_IN_VAR).as_deref() {
            Some("1") => Self::OptedIn {
                by: "GLASSHOUSE_ALLOW_METERED_MODELS=1",
            },
            _ => Self::Withheld,
        }
    }

    pub fn permits_metered(&self) -> bool {
        matches!(self, Self::Permitted | Self::OptedIn { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Permitted => "metered resources may be used".to_owned(),
            Self::Withheld => "metered resources are withheld".to_owned(),
            Self::OptedIn { by } => format!("metered resources were opted in through {by}"),
        }
    }
}

/// What a caller may know about one candidate's live capacity, beyond the
/// static configuration [`DisposableCandidate`] itself carries.
///
/// Every field is `None` (or `Plenty`, capacity's most permissive band) until
/// a caller supplies a real reading — [`mod@super`]'s "every function is a
/// pure function of values the caller supplies" applies here too: this
/// module reads no telemetry itself, and none of it opens a connection to
/// get one (`tests::no_routing_policy_can_make_a_request` in `mod.rs` would
/// catch it if it tried). `main.rs` is the caller that has a real
/// [`crate::provider::telemetry::GatewayQuotaCache`] to read from, the same
/// one `glasshouse resources` already reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CandidateCapacity {
    /// Map line 1536: this candidate's own normalized remaining-capacity
    /// score, when real telemetry has been cached for its provider.
    pub(super) remaining_capacity: Option<RemainingCapacityScore>,
    /// Map line 1549: seconds until this candidate's provider quota resets,
    /// when a real reading has stated one.
    pub(super) seconds_until_reset: Option<i64>,
    /// This candidate's capacity band against the user's own thresholds —
    /// feeds Phase 32F's protected-reserve policy on the metered-fallback
    /// path (map line 1550). `None` (treated as [`CapacityBand::Plenty`],
    /// the least protective band) for a resource nothing has been read
    /// about: an unread resource is not withheld from support work by a
    /// band it has never been observed to cross.
    pub(super) band: Option<CapacityBand>,
}

impl CandidateCapacity {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_remaining_capacity(mut self, score: Option<RemainingCapacityScore>) -> Self {
        self.remaining_capacity = score;
        self
    }

    pub fn with_seconds_until_reset(mut self, seconds: Option<i64>) -> Self {
        self.seconds_until_reset = seconds;
        self
    }

    pub fn with_band(mut self, band: Option<CapacityBand>) -> Self {
        self.band = band;
        self
    }
}

/// One resource a disposable job could be sent to.
///
/// Deliberately not a `super::Backend`: a backend carries a wire protocol and
/// tool semantics because an interactive session's harness depends on both,
/// and a disposable job has neither a harness nor tools. Sharing the type
/// would invite sharing the policy.
#[derive(Debug, Clone, PartialEq)]
pub struct DisposableCandidate {
    provider: String,
    model: String,
    credential: CredentialId,
    pub(super) cost: Cost,
    /// This candidate's real per-token price, when the user's own
    /// `pricing.toml` names it — capability map line 1436's producer,
    /// `crate::provider::pricing::PriceTable::price_for`. `None` is
    /// *unpriced*: a metered candidate nobody has priced, which
    /// [`classification_verdict`]'s price-ceiling gate treats as inert
    /// rather than expensive, exactly as an unmeasured latency is. Always
    /// irrelevant for a [`Cost::Free`] candidate — [`Self::cost`] stays the
    /// category, this is the number.
    pub(super) price: Option<ModelPrice>,
    /// Real capacity data the caller supplied for this candidate — see
    /// [`CandidateCapacity`]. Defaults to nothing known, which
    /// [`DisposableRouting::score`] renders as an honest `0.0` contribution
    /// naming the missing source, per this packet's design decision 3.
    pub(super) capacity: CandidateCapacity,
    /// Where this candidate's compute runs, when the caller said —
    /// capability map lines 1427 and 1438. `None` is a caller that did not
    /// say, which [`ClassificationPolicy::local_only`] treats as *not known
    /// to be local* (see [`classification_verdict`]) and the locality
    /// preference treats as inert.
    pub(super) locality: Option<Locality>,
    /// What the evidence ledger recorded about this candidate as a
    /// classifier, when the caller read it — capability map lines
    /// 1422/1432 and 1421/1435. `None` is "nothing was read", and every
    /// filter and preference built on it is inert for that candidate.
    pub(super) classification: Option<ClassificationRecord>,
    /// What the evidence ledger recorded about this candidate as a
    /// **support-work** resource — capability map line 1539,
    /// `crate::routing::evidence::EvidenceLedger::support_work_latency`'s
    /// answer. `None` is "nothing was read", and the expected-latency term
    /// is inert for that candidate.
    pub(super) latency: Option<LatencyRecord>,
    /// The entitlement charged for work sent to this candidate's provider,
    /// when a `[entitlements.<name>]` entry names it — map line 1947's
    /// job-kind clause. `main.rs::disposable_candidates` attaches it from
    /// `EffectiveConfig::entitlement_for_provider`; this module reads no
    /// configuration of its own. `None` is "no entry describes this
    /// resource", and no rule can refuse what no rule describes.
    entitlement: Option<crate::routing::Entitlement>,
}

impl DisposableCandidate {
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        credential: CredentialId,
        cost: Cost,
    ) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            credential,
            cost,
            price: None,
            capacity: CandidateCapacity::default(),
            locality: None,
            classification: None,
            latency: None,
            entitlement: None,
        }
    }

    /// Attach the entitlement whose rules govern this candidate — map line
    /// 1947's job-kind clause. [`super::DisposableRouting::choose`] refuses the
    /// candidate, by the entitlement's name, for any [`JobKind`] the rules
    /// do not serve.
    #[must_use]
    pub fn with_entitlement(mut self, entitlement: Option<crate::routing::Entitlement>) -> Self {
        self.entitlement = entitlement;
        self
    }

    pub fn entitlement(&self) -> Option<&crate::routing::Entitlement> {
        self.entitlement.as_ref()
    }

    /// Attach real capacity data a caller has read for this candidate — map
    /// lines 1536, 1549 and 1550.
    pub fn with_capacity(mut self, capacity: CandidateCapacity) -> Self {
        self.capacity = capacity;
        self
    }

    /// Attach this candidate's real per-token price, when the caller read one
    /// from `pricing.toml` — capability map line 1436's producer. `None`
    /// leaves the price-ceiling gate inert for it: unpriced, not expensive.
    #[must_use]
    pub fn with_price(mut self, price: Option<ModelPrice>) -> Self {
        self.price = price;
        self
    }

    pub fn price(&self) -> Option<ModelPrice> {
        self.price
    }

    /// State where this candidate's compute runs — capability map lines
    /// 1427 and 1438. `main.rs` reads it from
    /// [`crate::provider::registry::ResourceKind::from_direct_provider`],
    /// the one place this build already says which provider names are
    /// local-inference servers.
    #[must_use]
    pub fn with_locality(mut self, locality: Locality) -> Self {
        self.locality = Some(locality);
        self
    }

    /// Attach what the evidence ledger recorded about this candidate as a
    /// classifier — `crate::routing::evidence::EvidenceLedger::classification_record`'s
    /// answer. `None` leaves every classification filter and preference
    /// inert for it, explained as unmeasured.
    #[must_use]
    pub fn with_classification_record(mut self, record: Option<ClassificationRecord>) -> Self {
        self.classification = record;
        self
    }

    /// Attach what the evidence ledger recorded about this candidate as a
    /// support-work resource — capability map line 1539,
    /// `crate::routing::evidence::EvidenceLedger::support_work_latency`'s
    /// answer. `None` leaves the expected-latency term inert for it,
    /// explained as unmeasured.
    #[must_use]
    pub fn with_latency(mut self, latency: Option<LatencyRecord>) -> Self {
        self.latency = latency;
        self
    }

    pub fn locality(&self) -> Option<Locality> {
        self.locality
    }

    pub fn classification_record(&self) -> Option<&ClassificationRecord> {
        self.classification.as_ref()
    }

    pub fn latency_record(&self) -> Option<&LatencyRecord> {
        self.latency.as_ref()
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn credential(&self) -> &CredentialId {
        &self.credential
    }

    pub fn cost(&self) -> Cost {
        self.cost
    }

    /// Whether a *read* band puts this resource outside its protected
    /// reserve, so that spending it costs nobody's reserve — the predicate
    /// [`cheaper_adequate_resource_exists`] is built from.
    ///
    /// `None` is **not** outside the reserve here. That is deliberately the
    /// opposite of [`super::DisposableRouting::choose`]'s own
    /// `unwrap_or(CapacityBand::Plenty)` one field away, and both are the same
    /// rule applied to the two different questions being asked: an unread
    /// resource is never *withheld* by a band nobody observed, and it is never
    /// *offered* as the reason to withhold another one either.
    pub(super) fn is_outside_reserve(&self) -> bool {
        self.capacity
            .band
            .is_some_and(|band| band > CapacityBand::Reserve)
    }

    pub(super) fn as_free_resource(&self) -> FreeResource {
        FreeResource::new(self.credential.clone(), self.model.clone())
    }

    pub(super) fn key(&self) -> FreeResourceKey {
        FreeResourceKey::new(self.provider.clone(), self.model.clone())
    }
}

/// Map line 1434: whether `candidate` is known to have no headroom left on
/// whichever dimension `crate::provider::quota::CapacityState::remaining_capacity_score`
/// found tightest — the reading `candidate.capacity.remaining_capacity`
/// carries, which may be bound by requests-per-minute or another dimension
/// depending on what that call found.
///
/// `false` whenever nothing is known (`None`): an unread candidate is not a
/// candidate known to be exhausted, and eliminating on absence would turn "we
/// have no telemetry" into "this provider is full" — precisely what
/// [`CandidateCapacity`]'s own doc comment already refuses for the *scoring*
/// path, and this is the same rule applied to elimination.
pub(super) fn has_no_known_headroom(candidate: &DisposableCandidate) -> bool {
    candidate
        .capacity
        .remaining_capacity
        .as_ref()
        .is_some_and(|score| score.fraction() <= 0.0)
}
