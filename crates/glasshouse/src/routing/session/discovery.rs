//! Candidate discovery: eligibility, hard constraints, destination classification
//! and the session-affinity catalogue lookups behind [`super::session_affinity`].

use super::*;

/// One named term of the affinity score — Phase 36's unit of inspection.
///
/// `known` separates *"this facet read its signal and it is worth this"*
/// from *"this facet's signal did not arrive"*: both can be `0.0`, and a
/// reader of line 1588's explanation is owed the difference, because an
/// unread signal is a producer to go and look for and a read zero is not.
#[derive(Debug, Clone, PartialEq)]
pub struct AffinityFacet {
    name: &'static str,
    line: u16,
    magnitude: f64,
    known: bool,
    evidence: String,
}

impl AffinityFacet {
    fn known(name: &'static str, line: u16, magnitude: f64, evidence: String) -> Self {
        Self {
            name,
            line,
            magnitude,
            known: true,
            evidence,
        }
    }

    fn unknown(name: &'static str, line: u16, evidence: String) -> Self {
        Self {
            name,
            line,
            magnitude: 0.0,
            known: false,
            evidence,
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The capability-map line this facet answers.
    pub fn line(&self) -> u16 {
        self.line
    }

    pub fn magnitude(&self) -> f64 {
        self.magnitude
    }

    /// `false` when the signal this facet reads did not arrive, in which case
    /// `magnitude` is `0.0` and `evidence` says what is missing.
    pub fn is_known(&self) -> bool {
        self.known
    }

    pub fn evidence(&self) -> &str {
        &self.evidence
    }
}

/// Line 1581: the session-affinity score of one existing session, as its
/// facets — **the struct is the score** ([`Self::total`]) **and its
/// `Display` is the explanation** (line 1588).
///
/// Seven named terms, one per map line, each with its own evidence
/// sentence, summed into the one `session affinity` contribution
/// [`session_affinity`] has always pushed. Nothing here is a filter: every
/// facet is additive, an unknown facet is `0.0` and says so, and the
/// bounded magnitudes above keep warmth — the only measured signal — the
/// largest single term.
///
/// A fresh destination has no breakdown: it has no context to be affine to,
/// and [`session_affinity`] prices it at `FRESH_SESSION_AFFINITY` with the
/// sentence it always used.
#[derive(Debug, Clone, PartialEq)]
pub struct AffinityBreakdown {
    /// Lines 569 and 1596, the term as it was: live or resumable, and how
    /// long idle, through `crate::config::pairing`'s one definition.
    pub warmth: AffinityFacet,
    /// Line 1582.
    pub same_task: AffinityFacet,
    /// Line 1583.
    pub touched_files: AffinityFacet,
    /// Line 1584.
    pub native_context: AffinityFacet,
    /// Line 1585.
    pub prompt_cache: AffinityFacet,
    /// Line 1586.
    pub noise: AffinityFacet,
    /// Line 1587.
    pub quota_pressure: AffinityFacet,
}

impl AffinityBreakdown {
    /// Every facet, in the order a reader compares them.
    pub fn facets(&self) -> [&AffinityFacet; 7] {
        [
            &self.warmth,
            &self.same_task,
            &self.touched_files,
            &self.native_context,
            &self.prompt_cache,
            &self.noise,
            &self.quota_pressure,
        ]
    }

    /// The facet answering `line`, if any.
    pub fn for_line(&self, line: u16) -> Option<&AffinityFacet> {
        self.facets().into_iter().find(|facet| facet.line() == line)
    }

    /// The score — the sum of the facets, and the magnitude of the
    /// `session affinity` contribution.
    pub fn total(&self) -> f64 {
        self.facets().iter().map(|facet| facet.magnitude()).sum()
    }

    /// How many facets read no signal.
    pub fn unknown_count(&self) -> usize {
        self.facets()
            .iter()
            .filter(|facet| !facet.is_known())
            .count()
    }
}

impl std::fmt::Display for AffinityBreakdown {
    /// Line 1588: one summary line, then one line per facet — signed
    /// magnitude, name, and its evidence — so the explanation a person
    /// reads carries every term the score was built from.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let unknown = self.unknown_count();
        write!(
            f,
            "the sum of {} facets, {} of which read no signal and weigh nothing:",
            self.facets().len(),
            unknown
        )?;
        for facet in self.facets() {
            write!(
                f,
                "\n    {:+.3}  {} (line {}{}) — {}",
                facet.magnitude(),
                facet.name(),
                facet.line(),
                if facet.is_known() { "" } else { ", unknown" },
                facet.evidence()
            )?;
        }
        Ok(())
    }
}

/// Lines 1581–1588: what an existing session's affinity contributes, and
/// every facet behind it.
///
/// One contribution, as before, so that the ranking, the overview and every
/// existing assertion on the `session affinity` term keep reading one
/// number; the number is now [`AffinityBreakdown::total`] and the evidence
/// is its `Display`. [`affinity_breakdown`] is the same computation with the
/// facets kept apart, for a caller or a test that wants one of them.
///
/// `current` is where the work is now — `None` at a session start — and is
/// read by exactly one facet, line 1585's, for the cache locality of the
/// move. `requirements` carries the classification of the work in hand,
/// read by lines 1582 and 1586; a launch that stated no task leaves both
/// facets unknown rather than inventing a task to compare against.
pub fn session_affinity(
    destination: &Destination,
    current: Option<&Destination>,
    requirements: &TaskRequirements,
) -> Contribution {
    match affinity_breakdown(destination, current, requirements) {
        Some(breakdown) => {
            Contribution::new("session affinity", breakdown.total(), breakdown.to_string())
        }
        None => Contribution::new(
            "session affinity",
            FRESH_SESSION_AFFINITY,
            "a fresh session has no accumulated context to be affine to — not a penalty, only \
             the absence of the term (the bootstrap cost is where starting from nothing is \
             priced)",
        ),
    }
}

/// [`session_affinity`] with the facets kept apart. `None` for a fresh
/// destination, which has no context and therefore no breakdown.
pub fn affinity_breakdown(
    destination: &Destination,
    current: Option<&Destination>,
    requirements: &TaskRequirements,
) -> Option<AffinityBreakdown> {
    let Continuation::Existing(warm) = destination.continuation() else {
        return None;
    };
    let id = destination.id();
    let facts = destination.session_context();
    let current_task = requirements
        .classification
        .as_ref()
        .map(RouterAnswer::classification);

    // Lines 569 and 1596 — warmth, exactly as the term has always computed
    // it: the decay window and the live/resumable ratio have one definition
    // and it is not here.
    let reused = crate::config::pairing::session_continuity_contribution(
        &evidence_key_for(destination),
        &OneWarmSession(warm),
    );
    let warmth = AffinityFacet::known(
        "warmth",
        1596,
        reused.magnitude(),
        format!(
            "`{id}` is a {} session, idle {}s — {}",
            warm.state,
            warm.idle_seconds.max(0),
            reused.evidence()
        ),
    );
    let stale = warmth.magnitude() <= 0.0;

    // Line 1582 — the same task, as far as the sticky classification cache
    // can say it.
    let same_task_verdict = match (facts.last_task(), current_task) {
        (Some(previous), Some(now)) => Some(same_work(previous, now)),
        _ => None,
    };
    let same_task = match (facts.last_task(), current_task) {
        (Some(previous), Some(now)) if same_work(previous, now) => AffinityFacet::known(
            "same task",
            1582,
            SAME_TASK_AFFINITY,
            format!(
                "the last task classified onto `{id}` was classed the way this one is — tier \
                 `{}`, {} — which is the nearest thing to task identity this build records; \
                 the sticky classification cache keeps a classification, never the task text",
                now.workload_tier(),
                describe_capabilities(now),
            ),
        ),
        (Some(previous), Some(now)) => AffinityFacet::known(
            "same task",
            1582,
            0.0,
            format!(
                "the last task classified onto `{id}` was classed differently from this one \
                 (tier `{}` then, `{}` now) — the noise facet prices that",
                previous.workload_tier(),
                now.workload_tier(),
            ),
        ),
        (Some(_), None) => AffinityFacet::unknown(
            "same task",
            1582,
            format!(
                "a last classified task is recorded against `{id}` and this launch stated no \
                 task — nothing to compare it with"
            ),
        ),
        (None, Some(_)) => AffinityFacet::unknown(
            "same task",
            1582,
            format!(
                "no classified task is recorded against `{id}` — the sticky classification \
                 cache names another session, or was never written"
            ),
        ),
        (None, None) => AffinityFacet::unknown(
            "same task",
            1582,
            format!("no task was stated and none is recorded against `{id}`"),
        ),
    };

    // Line 1583 — the files this session touched, against the paths the
    // task names.
    let hits: Option<Vec<&str>> = match (facts.task_named_paths(), facts.touched_files()) {
        (Some(named), Some(touched)) if !named.is_empty() && !touched.is_empty() => Some(
            named
                .iter()
                .filter(|name| touched.iter().any(|path| path_names(path, name)))
                .map(String::as_str)
                .collect(),
        ),
        _ => None,
    };
    let touched_files = match (facts.task_named_paths(), facts.touched_files(), &hits) {
        (Some(named), Some(_), Some(hits)) if hits.is_empty() => AffinityFacet::known(
            "touched files",
            1583,
            0.0,
            format!(
                "the task names {} path{} and `{id}`'s latest checkpoint lists none of them — \
                 the noise facet prices that",
                named.len(),
                if named.len() == 1 { "" } else { "s" },
            ),
        ),
        (Some(named), Some(_), Some(hits)) => AffinityFacet::known(
            "touched files",
            1583,
            TOUCHED_FILES_AFFINITY * hits.len() as f64 / named.len() as f64,
            format!(
                "`{id}`'s latest checkpoint lists {} of the {} path{} the task names ({})",
                hits.len(),
                named.len(),
                if named.len() == 1 { "" } else { "s" },
                hits.join(", "),
            ),
        ),
        (None, _, _) => AffinityFacet::unknown(
            "touched files",
            1583,
            "no task was stated, so there is nothing to intersect the session's files with"
                .to_owned(),
        ),
        (Some([]), _, _) => AffinityFacet::unknown(
            "touched files",
            1583,
            "the task text names no path, so there is nothing to intersect the session's \
             files with"
                .to_owned(),
        ),
        (Some(_), None, _) => AffinityFacet::unknown(
            "touched files",
            1583,
            format!("no checkpoint records which files `{id}` touched"),
        ),
        (Some(_), Some(_), None) => AffinityFacet::unknown(
            "touched files",
            1583,
            format!("`{id}`'s latest checkpoint lists no files"),
        ),
    };

    // Line 1584 — the native context, as compactions and staleness say it.
    let native_context = match facts.observed_compactions() {
        None => AffinityFacet::unknown(
            "native context",
            1584,
            format!(
                "nobody counted `{id}`'s compactions — a row from before the count existed — \
                 and an uncounted history is not a clean one"
            ),
        ),
        Some(_) if stale => AffinityFacet::known(
            "native context",
            1584,
            0.0,
            format!(
                "`{id}` is past the window a warm session stays relevant for, so whatever its \
                 context holds is not credited as still useful"
            ),
        ),
        Some(0) => AffinityFacet::known(
            "native context",
            1584,
            NATIVE_CONTEXT_INTACT,
            format!(
                "no compaction has been observed on `{id}` and it is inside the relevance \
                 window — its native context holds exactly what was said to it"
            ),
        ),
        Some(count) if count < NOISY_COMPACTION_COUNT => AffinityFacet::known(
            "native context",
            1584,
            NATIVE_CONTEXT_INTACT / 2.0,
            format!(
                "`{id}` has been compacted {count} time{} — a summary stands in for part of \
                 its context, so it is credited at half",
                if count == 1 { "" } else { "s" },
            ),
        ),
        Some(count) => AffinityFacet::known(
            "native context",
            1584,
            0.0,
            format!(
                "`{id}` has been compacted {count} times — what survives is mostly summaries \
                 of summaries, credited as neither intact nor useful (the noise facet prices \
                 the count)"
            ),
        ),
    };

    // Line 1585 — is the provider-side prefix likely still there.
    let locality =
        current.map(|current| CacheLocality::between(current.backend(), destination.backend()));
    let prompt_cache = match locality {
        Some(locality @ CacheLocality::Lost(_)) => AffinityFacet::known(
            "prompt cache",
            1585,
            0.0,
            format!("the work is moving off the backend that built `{id}`'s prefix: {locality}"),
        ),
        Some(locality @ CacheLocality::LikelyLost(_)) => AffinityFacet::known(
            "prompt cache",
            1585,
            0.0,
            format!("moving to `{id}` changes the credential: {locality}"),
        ),
        _ if warm.idle_seconds < 0 => AffinityFacet::unknown(
            "prompt cache",
            1585,
            format!(
                "`{id}`'s last activity is in the future — a clock moved backwards — and a \
                 cache lifetime cannot be measured against that"
            ),
        ),
        _ if warm.idle_seconds <= PROMPT_CACHE_TTL_SECONDS => AffinityFacet::known(
            "prompt cache",
            1585,
            PROMPT_CACHE_HOT,
            format!(
                "`{id}` was active {}s ago, inside the {PROMPT_CACHE_TTL_SECONDS}s a \
                 provider-side cached prefix is published to survive by default — likely \
                 hot, not observed: no provider reports a hit",
                warm.idle_seconds
            ),
        ),
        _ => AffinityFacet::known(
            "prompt cache",
            1585,
            0.0,
            format!(
                "`{id}` was active {}s ago, past the {PROMPT_CACHE_TTL_SECONDS}s default \
                 lifetime of a provider-side cached prefix — likely expired",
                warm.idle_seconds
            ),
        ),
    };

    // Line 1586 — the same signals, read for noise and unrelatedness.
    let mut noise_magnitude = 0.0;
    let mut noise_notes: Vec<String> = Vec::new();
    let mut noise_readable = false;
    if let Some(count) = facts.observed_compactions() {
        noise_readable = true;
        if count >= NOISY_COMPACTION_COUNT {
            noise_magnitude +=
                (count as f64 * COMPACTION_NOISE_PENALTY).max(COMPACTION_NOISE_FLOOR);
            noise_notes.push(format!(
                "compacted {count} times, and each compaction replaces context with a summary \
                 of it"
            ));
        }
    }
    if let Some(verdict) = same_task_verdict {
        noise_readable = true;
        if !verdict {
            noise_magnitude += UNRELATED_TASK_PENALTY;
            noise_notes.push(
                "the last task classified onto it was classed differently from this one".to_owned(),
            );
        }
    }
    if let (Some(named), Some(hits)) = (facts.task_named_paths(), &hits) {
        noise_readable = true;
        // A bare `foo.rs` in prose is a weaker claim than `src/foo.rs`; the
        // penalty needs the stronger spelling so a word that merely looks
        // like a file name cannot cost every session a third of a point.
        let names_a_directory_path = named.iter().any(|name| name.contains('/'));
        if hits.is_empty() && names_a_directory_path {
            noise_magnitude += UNRELATED_FILES_PENALTY;
            noise_notes.push(
                "the task names paths and its latest checkpoint lists none of them".to_owned(),
            );
        }
    }
    let noise = if !noise_readable {
        AffinityFacet::unknown(
            "noise",
            1586,
            format!(
                "no compaction count, no classified task to compare and no checkpoint file \
                 list — nothing to read `{id}`'s noise from"
            ),
        )
    } else if noise_notes.is_empty() {
        AffinityFacet::known(
            "noise",
            1586,
            0.0,
            format!(
                "nothing read says `{id}`'s context is noisy or unrelated — the absence of a \
                 signal, not a clean bill"
            ),
        )
    } else {
        AffinityFacet::known(
            "noise",
            1586,
            noise_magnitude,
            format!("`{id}`: {}", noise_notes.join("; ")),
        )
    };

    // Line 1587 — significant pressure on the resource this session spends,
    // from the band the caller derived from the same reading `quota_pressure`
    // prices.
    let credential = destination.backend().credential().label();
    let quota_pressure = match destination.capacity_facts().band() {
        Some(band) if band <= CapacityBand::Reserve => AffinityFacet::known(
            "quota pressure",
            1587,
            QUOTA_PRESSURE_AFFINITY_PENALTY,
            format!(
                "`{credential}` is in the `{}` band — significant pressure on the \
                 resource this session spends; the reading itself is priced once, by the \
                 `known quota pressure` term, and this is the map's own decrease in affinity",
                band.as_str()
            ),
        ),
        Some(band) => AffinityFacet::known(
            "quota pressure",
            1587,
            0.0,
            format!(
                "`{credential}` is in the `{}` band — not significant pressure",
                band.as_str()
            ),
        ),
        None => AffinityFacet::unknown(
            "quota pressure",
            1587,
            format!(
                "nothing has been read about `{credential}`'s remaining quota — an unread \
                 resource is neither preferred nor withheld"
            ),
        ),
    };

    Some(AffinityBreakdown {
        warmth,
        same_task,
        touched_files,
        native_context,
        prompt_cache,
        noise,
        quota_pressure,
    })
}

/// Line 1582's "same task or feature", as far as a stored classification can
/// say it: the same hard capabilities, the same workload tier, and the same
/// answer to whether the work touches the repository and modifies code.
/// Confidence and source are deliberately not compared — the same task
/// classed by heuristics one launch and by a model the next is one task.
fn same_work(previous: &TaskClassification, current: &TaskClassification) -> bool {
    previous.hard_capabilities() == current.hard_capabilities()
        && previous.workload_tier() == current.workload_tier()
        && previous.needs_repo_context() == current.needs_repo_context()
        && previous.needs_code_modification() == current.needs_code_modification()
}

fn describe_capabilities(classification: &TaskClassification) -> String {
    let capabilities = classification.hard_capabilities();
    if capabilities.is_empty() {
        "no hard capability".to_owned()
    } else {
        capabilities
            .iter()
            .map(|capability| capability.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Whether a checkpoint's `path` is the file the task's `name` names: the
/// same repo-relative path, or a bare file name the path ends in. A name is
/// never matched as a substring — `foo.rs` is not `barfoo.rs`.
fn path_names(path: &str, name: &str) -> bool {
    let name = name.trim_start_matches("./");
    path == name || path.ends_with(&format!("/{name}"))
}

/// Line 1583's "relevant": the path-shaped tokens in a task's text.
///
/// A spelling test and not a vocabulary — a token names a path when it
/// contains a `/` (and is not a URL), or ends in a dotted extension of one to
/// five lowercase ASCII alphanumerics with at least one letter, after a stem
/// of at least two characters. The stem rule is what keeps `e.g.` and `i.e.`
/// out; the lowercase rule keeps `Ph.D.` out; the letter rule keeps `v1.2`
/// out. `Node.js` gets in, which is the price of a spelling test, and the
/// reason [`affinity_breakdown`]'s unrelated-files penalty needs a `/`.
///
/// Surrounding punctuation and backticks are stripped, so `` `src/foo.rs`, ``
/// names `src/foo.rs`. Order is first mention, without repeats.
pub fn paths_named_in(task_text: &str) -> Vec<String> {
    let mut named: Vec<String> = Vec::new();
    for raw in task_text.split_whitespace() {
        let token = raw
            .trim_matches(|c: char| !(c.is_alphanumeric() || matches!(c, '/' | '.' | '_' | '-')));
        if token.is_empty() || token.contains("://") {
            continue;
        }
        let has_separator = token.contains('/') && token.trim_matches('/').len() > 1;
        if (has_separator || has_file_extension(token)) && !named.iter().any(|n| n == token) {
            named.push(token.to_owned());
        }
    }
    named
}

fn has_file_extension(token: &str) -> bool {
    let Some((stem, extension)) = token.rsplit_once('.') else {
        return false;
    };
    stem.chars().count() >= 2
        && !stem.ends_with('/')
        && (1..=5).contains(&extension.len())
        && extension
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && extension.chars().any(|c| c.is_ascii_lowercase())
}

/// What the other eligible candidates offer `candidates[index]` as an
/// alternative — the two set-level facts `super::pressure` reads, computed
/// here because only the router holds the set.
///
/// "Adequate" is [`is_adequate`]: no required hard capability established
/// absent, the same fact [`capability_fit`] prices. "Available" is the
/// provider's observed health, the same fact [`provider_health`] prices.
/// Neither is re-decided here; both are read off the destination the way the
/// pricing terms read them, so the alternative an explanation names is one
/// those terms would also have scored well.
pub(super) fn alternatives_for(
    index: usize,
    candidates: &[Destination],
    inputs: &RouterInputs<'_>,
) -> Alternatives {
    let mut alternatives = Alternatives::none();
    for (other_index, other) in candidates.iter().enumerate() {
        // A candidate that cannot serve right now — refused by its provider,
        // or cooling down — is not an alternative anything can be routed to
        // instead, whatever its band. Without this, a reserve-band
        // destination would be denied in favour of a provider that
        // `provider_health` is about to score as unavailable, and the work
        // would go to the one place it cannot run.
        if other_index == index
            || is_adequate(other, &inputs.requirements).is_some()
            || !provider_available(other, inputs.health, inputs.now)
        {
            continue;
        }
        let free = other.backend().cost().is_free();
        let band = other.capacity_facts().band();
        if alternatives.healthy_free_adequate().is_none()
            && free
            && band.is_none_or(|band| band >= CapacityBand::Healthy)
        {
            alternatives = alternatives.with_healthy_free_adequate(other.id());
        }
        if alternatives.cheaper_adequate().is_none()
            && (free || band.is_some_and(|band| band > CapacityBand::Reserve))
        {
            alternatives = alternatives.with_cheaper_adequate(other.id());
        }
    }
    alternatives
}

/// Whether `destination` is established to lack none of the task's required
/// hard capabilities — the negative half of [`capability_fit`]'s reading,
/// as a fact rather than a price. Unverified is not a `no`, here as there.
///
/// `None` means adequate. `Some((axis, evidence))` names the first
/// requirement whose axis is established absent — the axis
/// [`HardConstraint::Capability`] carries, not merely the first requirement
/// in `requirements.hard_capabilities` (a requirement that is `Unverified`
/// or present is skipped rather than reported).
pub(super) fn is_adequate(
    destination: &Destination,
    requirements: &TaskRequirements,
) -> Option<(capability::CapabilityAxis, &'static str)> {
    if requirements.hard_capabilities.is_empty() {
        return None;
    }
    let harness_caps = crate::harness::adapter_for(destination.harness())
        .map(|adapter| adapter.describe().capabilities)
        .unwrap_or(HarnessCapabilities::UNVERIFIED);
    let resource =
        capability::ResourceCapabilities::describe(&harness_caps, destination.resource_facts());
    requirements
        .hard_capabilities
        .iter()
        .find_map(|requirement| {
            let axis = capability::axis_for(*requirement);
            match resource.axis(axis) {
                Declared::Verified {
                    value: false,
                    evidence,
                } => Some((axis, evidence)),
                _ => None,
            }
        })
}

/// Whether the provider behind `destination` is currently usable by its
/// observed health — not refused, not cooling down. The same two facts
/// [`provider_health`] prices at [`HEALTH_UNAVAILABLE_PENALTY`].
pub(super) fn provider_available(destination: &Destination, pool: &FreePool, now: Instant) -> bool {
    let health = pool.health(&FreeResource::new(
        destination.backend().credential().clone(),
        destination.backend().model().label(),
    ));
    !health.credential_was_rejected() && health.is_available(now)
}

/// Line 1518's exclusion, read directly from `pool.health` rather than
/// through [`provider_available`]. `provider_available` folds credential
/// rejection and *any* cooldown — declared or invented — into one boolean
/// for its two existing callers ([`decide_tier_movement`],
/// [`alternatives_for`]), and both must keep pricing an invented cooldown
/// as a soft penalty rather than excluding on it (line 534). Extending it
/// with the cause would hand that distinction to callers that must not act
/// on it; a second, narrower read is smaller than teaching the existing one
/// a case its callers need to ignore.
///
/// `None` when nothing here excludes: no rejection, no cooldown, or a
/// cooldown whose cause is `Invented` or was never established (adopted
/// health — see `FreePool::adopt_observed`) — [`super::free::ResourceHealth::declared_wait_remaining`]
/// already answers exactly that question.
fn provider_unavailable_cause(
    destination: &Destination,
    pool: &FreePool,
    now: Instant,
) -> Option<ProviderUnavailableCause> {
    let health = pool.health(&FreeResource::new(
        destination.backend().credential().clone(),
        destination.backend().model().label(),
    ));
    if health.credential_was_rejected() {
        return Some(ProviderUnavailableCause::CredentialRejected);
    }
    if health.declared_wait_remaining(now).is_some() {
        return Some(ProviderUnavailableCause::DeclaredCooldown);
    }
    None
}

/// The gate step 2 runs. Five constraints and no others, each a fact about
/// whether the destination *can* serve, not a preference about whether it
/// *should*.
///
/// Two — map lines 1517 and 1518 — are asked on both passes: whether a
/// destination lacks a required hard capability, or its provider has
/// refused the credential or declared a still-active cooldown, does not
/// depend on which tier the movement settled. Both follow the same
/// "established, not merely unread" rule: an unverified capability axis and
/// an *invented* cooldown are not "cannot," so neither excludes.
///
/// The fifth — map line 1516 — fires only on an **established** ceiling
/// strictly below the required tier; a destination with no ceiling stated
/// passes, since "nobody has said" is not "cannot."
///
/// `minimum_tier` is [`TierMovement::gate_tier`] once the movement is
/// decided, `None` for the pass that decides it — an argument rather than
/// `inputs.requirements.minimum_tier` so a downgrade (line 1562) can admit
/// a resource the classified tier would have refused, in exactly one place.
// History: design-decisions.md, "Trims: routing module docs", routing/session/discovery.rs `fn hard_constraint`.
pub(super) fn hard_constraint(
    destination: &Destination,
    inputs: &RouterInputs<'_>,
    minimum_tier: Option<WorkloadTier>,
    entitlement_axis: bool,
) -> Result<(), HardConstraint> {
    // Phase 56 line 1954, asked first: the user's own rule about what a
    // entitlement may be charged for is the strongest statement in this
    // gate, and when a destination fails it *and* a capability fact, the
    // constraint a person reads should be the one they wrote. The harness
    // half is asked on both passes; the tier half reads `minimum_tier`, so it
    // — like line 1516's ceiling gate below — fires only on the pass that
    // knows the tier the movement settled, and never against an unknown one
    // (`super::EntitlementRules::refusal`).
    if let Some(entitlement) = destination.entitlement() {
        entitlement.constraint(destination.harness(), minimum_tier)?;
        // Map line 1971's fourth axis, asked beside the other three and on
        // both passes: a spend ceiling is a rule the **user wrote**, not a
        // reading this build took, so — unlike the model half below — it is
        // not gated on the pool axis. A person who set a ceiling on their
        // one account meant it, and an account over its ceiling is over it
        // whether or not a second one exists. It refuses only when the
        // ceiling and the spend are BOTH established; see
        // `super::Entitlement::spend_constraint`.
        entitlement.spend_constraint()?;
        // Map line 1519, asked beside the ceiling above and for the same
        // reason: the **provider's** own money budget, not this
        // entitlement's token ceiling, has been counted as exhausted. Gated
        // on cost here rather than inside the constraint itself, because a
        // free-tier destination spends nothing against a money budget no
        // matter which provider serves it — `Entitlement::budget_constraint`
        // does not know the destination's cost, only `hard_constraint` does.
        if !destination.backend().cost().is_free() {
            entitlement.budget_constraint()?;
        }
        // Line 1953's model half, asked on both passes like the harness
        // half (the destination's model is known independently of any
        // tier), and only when the offered set carries the entitlement axis
        // at all — see `gate` for why a pool of one is exempt. A declared
        // catalogue that does not name the model refuses the candidate by
        // name; harness-decided and unknown facets constrain nothing.
        if entitlement_axis {
            entitlement.model_constraint(destination.backend().model())?;
        }
    }
    if inputs.requirements.needs_tool_calls
        && destination.backend().tools() == ToolSemantics::KnownAbsent
    {
        // The `Declared` evidence behind `KnownAbsent` now arrives here:
        // `harness::pairing::classify` keeps it beside the bare verdict
        // (`Pairing::tool_evidence`), `main.rs::destination_backend` carries
        // it onto the `Backend` it builds (`Backend::with_tools_evidence`),
        // and `Backend::tools_evidence()` reads it back — `Some` exactly
        // when `tools()` is `KnownAbsent`, by construction on the producer
        // side.
        return Err(HardConstraint::ToolSemantics {
            evidence: destination.backend().tools_evidence(),
        });
    }
    if classify_destination(destination, inputs.overrides).protocol_fit()
        == ProtocolFit::Incompatible
    {
        return Err(HardConstraint::Protocol);
    }
    // Line 1517, asked on both passes like the two facts above: whether the
    // destination *can* serve is independent of which tier movement decided
    // to admit, so this does not wait for `minimum_tier` to resolve.
    // `is_adequate` refuses only an axis established absent
    // (`Declared::Verified { value: false }`); an unverified axis is "nobody
    // has said," not "cannot," and keeps passing to be priced by
    // `capability_fit` exactly as before this gate existed.
    if let Some((axis, evidence)) = is_adequate(destination, &inputs.requirements) {
        return Err(HardConstraint::Capability { axis, evidence });
    }
    // Line 1518, same reasoning: a provider that has refused the credential
    // or declared a cooldown still in force cannot serve either pass asks
    // about, so it is excluded rather than merely priced worse by
    // `provider_health`. An *invented* cooldown is Glasshouse's own guess
    // (line 534) and stays a soft penalty — see `provider_unavailable_cause`.
    if let Some(cause) = provider_unavailable_cause(destination, inputs.health, inputs.now) {
        return Err(HardConstraint::ProviderUnavailable {
            credential: destination.backend().credential().label(),
            cause,
        });
    }
    if let (Some(required), Some(offered)) = (minimum_tier, destination.tier_ceiling())
        && offered < required
    {
        return Err(HardConstraint::WorkloadTier { required, offered });
    }
    Ok(())
}

/// One place the pairing query is built, so every consumer asks the same
/// question — the reason `interactive`'s `evidence_key_for` is one function
/// too.
pub(super) fn classify_destination(
    destination: &Destination,
    overrides: &pairing::PairingOverrides,
) -> pairing::Pairing {
    let query = pairing::PairingQuery {
        harness: destination.harness(),
        model: destination.backend().model().clone(),
        route: serving_route(destination.backend()),
        // `crate::harness::Declared` carries a `&'static str` evidence
        // string that `crate::routing::Backend` deliberately does not keep
        // (see `Backend::tools`' own doc comment), so there is nothing
        // honest to reconstruct one from. `classify` reads this field only
        // for `Pairing::tool_semantics`, which neither
        // `harness_capability_fit` nor `hard_constraint` looks at — the
        // hard constraint reads `Backend::tools()` directly, which is the
        // fact rather than a round trip through a type that would have to
        // invent its provenance. Same degradation, same reason, as
        // `crate::routing::interactive`'s own `score_candidate`.
        tool_calls: crate::harness::Declared::Unverified,
        provider_protocols: destination.provider_protocols().to_vec(),
    };
    pairing::classify(&query, overrides)
}

fn serving_route(backend: &Backend) -> pairing::ServingRoute {
    pairing::ServingRoute {
        provider: Some(backend.provider().to_owned()),
        gateway: None,
        protocol: pairing::wire_protocol_from_slug(backend.protocol()),
    }
}

fn evidence_key_for(destination: &Destination) -> pairing::EvidenceKey {
    pairing::EvidenceKey::new(
        destination.harness(),
        destination.launch_profile(),
        destination.backend().model().clone(),
        serving_route(destination.backend()),
    )
}

/// A [`ContinuitySource`] answering with the one warm session the caller
/// already attached to this destination.
///
/// The adapter exists so the decay window and the live/resumable ratio have
/// exactly one definition — `crate::config::pairing`'s — rather than a second
/// copy here that could drift from it. It answers the same for every key
/// because it is only ever asked about the destination it was built from.
struct OneWarmSession(WarmSession);

impl ContinuitySource for OneWarmSession {
    fn warm_session(&self, _key: &pairing::EvidenceKey) -> Option<WarmSession> {
        Some(self.0)
    }
}
