//! Reranking the top lexical memory candidates by a cheap language model —
//! capability map lines 1089-1092 and 1094.
//!
//! # What this module refuses to be
//!
//! [`super::inject`]'s ladder — invariants and constraints first, then
//! failed attempts, then everything else — is settled and stays settled.
//! [`rerank`] never sees an invariant or a constraint: it is handed only
//! [`super::search::RetrievalResult::other`], the bucket [`super::inject`]
//! already treats as ordinary matches, and it reorders *within* that bucket
//! and nothing above it. Nothing here can promote a candidate past a rung
//! its own authority did not earn, for the same reason [`super::inject`]'s
//! own module doc gives: the ladder is a stable partition, and this is one
//! more pass over one partition of it.
//!
//! # Every failure is a bypass, never an error the session sees
//!
//! No `[memory] rerank_model` configured, fewer than two candidates, a model
//! that times out, refuses, or answers something this module cannot trust —
//! every one of these is [`RerankOutcome`] carrying a reason, and every one
//! of them leaves `candidates` in the lexical order they arrived in. A
//! reranker that could turn "the model was unreachable" into "no memory is
//! injected" would make Glasshouse's own memory less available exactly when
//! the network is least available — the shape `GH-LOCAL-REDUCER`'s own
//! posture (`docs/product/evidence/phase-58.md`) already refuses.
//!
//! # The reply is a JSON array of ids, and it is strictly parsed
//!
//! The model is not asked to write a memory —
//! [`super::extract::schema::PROMPT_CONTRACT`] is a different contract for a
//! different job — it is asked to return the ids it was given, reordered. So
//! the whole reply is one array of strings, and this module's own reply
//! parsing is stricter than [`super::extract::schema::parse`] in the one way
//! that matters here: an id the reply names that was never sent is not a memory
//! this module can classify as low-confidence and keep half of, the way a
//! malformed extraction field can — it is evidence the reply is not
//! answering about the candidates it was actually given, and the whole
//! reply is a bypass.

use std::collections::HashSet;
use std::io::Write as _;

use super::extract::{ExtractionModel, ModelError, Prompt};
use super::store::MemoryRecord;

/// The most candidates one rerank call is ever sent — line 1091's *"limit
/// reranking to a small candidate set to keep latency and token use low."*
///
/// The rest of the group beyond this window travels unchanged, behind
/// whatever the model reordered — see [`rerank`].
pub const RERANK_CANDIDATES: usize = 8;

/// The most bytes of a candidate's subject and body, combined, that reach
/// the model or a diagnostics record — never the whole body, which line 1091
/// bounds token use against and which [`super::inject`]'s own
/// [`super::inject::MAX_INJECTED_BODY_CHARS`] shows is already more than a
/// selection decision needs.
pub const EXCERPT_BYTES: usize = 200;

/// The rerank call's own timeout bound — shorter than
/// [`super::extract::model::CallTimeouts::default`]'s, which extraction
/// keeps and this seat deliberately does not.
///
/// Line 1091 asks reranking to "keep latency … low," and unlike memory
/// extraction (a background hook), a rerank call sits in the critical path
/// of every routed task: `select_memory` and `brief_launch_session` both
/// wait on it before a session sees anything. The extraction seat's default
/// 30-second ceiling is the right bound for a job nobody is waiting on and
/// the wrong one for a job everybody is.
fn rerank_timeouts() -> super::extract::model::CallTimeouts {
    super::extract::model::CallTimeouts {
        connect: std::time::Duration::from_secs(2),
        response: std::time::Duration::from_secs(5),
        total: std::time::Duration::from_secs(6),
    }
}

/// The instruction half of the prompt [`rerank`] sends, in
/// [`super::extract::schema::PROMPT_CONTRACT`]'s own style: numbered rules,
/// ending with the exact shape the reply must take.
///
/// Line 1092's four optimization targets are rules 2 through 5; rule 1 is
/// the containment property every prompt this crate builds carries, and
/// rule 6 is what makes an untrustworthy reply a bypass rather than a
/// best-effort guess.
pub const RERANK_PROMPT_CONTRACT: &str = "\
You are ranking a small set of already-matched project memories by how useful \
each would be to an agent about to start one task. You are not writing or \
editing any memory, and you must not invent one.

Each candidate below is given as `id: <id> subject: <text>`, in the order a \
plain text search already ranked it.

Rank the candidates by:

 1. Relevance to the task actually stated below — not to a subsystem it \
    merely mentions in passing.
 2. Recency: a more recently updated memory over an older one that is \
    otherwise equally relevant.
 3. Active status: a memory this project still treats as current over one \
    that is only marginally so.
 4. Non-duplication: when two candidates say nearly the same thing, prefer \
    the one that says it more completely and rank the near-duplicate lower \
    rather than dropping it — dropping is not yours to decide.

Reply with a JSON array of the candidate ids you were given, most useful \
first, and nothing else — no prose, no markdown fence, no id you were not \
given. Omitting an id is allowed and means you have no opinion on it; \
inventing one is not.
";

/// Why [`rerank`] left `candidates` exactly as it found them, or how it
/// changed them.
///
/// # Every variant but [`Self::Reordered`] is a bypass in fact if not in name
///
/// [`Self::NotConfigured`] and [`Self::TooFew`] are not failures — no model
/// was owed a call — but they answer the same question a bypass answers
/// ("why is this in lexical order?") and a diagnostics reader should not
/// have to learn two vocabularies for one fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RerankOutcome {
    /// No `[memory] rerank_model` is configured — line 1090's *"reranking
    /// optional so memory search still works offline"*, made concrete: no
    /// call was made because none was consented to.
    NotConfigured,
    /// Fewer than two candidates were offered. Ranking one thing, or
    /// nothing, is not a decision a model call is owed for.
    TooFew,
    /// The model answered inside the window it was sent, and every id it
    /// returned was one it was actually given.
    Reordered {
        /// [`ExtractionModel::describe`]'s own words for the resource that
        /// answered.
        resource: String,
        /// How many ids the reply actually named — at most
        /// [`RERANK_CANDIDATES`], and the rest of the window follows in
        /// their original order.
        returned: usize,
    },
    /// The call could not be made, timed out, was refused, or answered
    /// something this module's reply parsing could not trust — every one of
    /// these leaves the candidates in lexical order. `resource` is `None` only
    /// when nothing could even describe itself, which no configured
    /// [`ExtractionModel`] this build produces ever does; kept as an
    /// `Option` so a future implementation that fails before it can name
    /// itself is still representable.
    Bypassed {
        resource: Option<String>,
        reason: String,
    },
}

/// Reorder `candidates` by relevance to `task`, calling `model` at most
/// once, or say why nothing was reordered.
///
/// `candidates` is [`super::search::RetrievalResult::other`] — see the
/// module documentation for why an invariant or a constraint never reaches
/// this function. At most [`RERANK_CANDIDATES`] of `candidates` are sent;
/// the reply leads in the order it named, ids it omitted follow in their
/// original order, and anything beyond the sent window is untouched and
/// stays behind both.
pub fn rerank(
    candidates: Vec<MemoryRecord>,
    model: Option<&dyn ExtractionModel>,
    task: &str,
) -> (Vec<MemoryRecord>, RerankOutcome) {
    let Some(model) = model else {
        return (candidates, RerankOutcome::NotConfigured);
    };
    if candidates.len() < 2 {
        return (candidates, RerankOutcome::TooFew);
    }

    let resource = model.describe();
    let window_len = candidates.len().min(RERANK_CANDIDATES);
    let prompt = Prompt::from_text(build_prompt(&candidates[..window_len], task));

    let reply = match model.complete_observed(&prompt) {
        Ok(reply) => reply,
        Err(err) => {
            return (
                candidates,
                RerankOutcome::Bypassed {
                    resource: Some(resource),
                    reason: describe_error(&err),
                },
            );
        }
    };

    let sent_ids: HashSet<&str> = candidates[..window_len]
        .iter()
        .map(|record| record.id.as_str())
        .collect();
    let order = match parse_reply(&reply.reply, &sent_ids) {
        Ok(order) => order,
        Err(reason) => {
            return (
                candidates,
                RerankOutcome::Bypassed {
                    resource: Some(resource),
                    reason,
                },
            );
        }
    };

    let returned = order.len();
    (
        apply_order(candidates, window_len, &order),
        RerankOutcome::Reordered { resource, returned },
    )
}

/// The prompt's second half: [`RERANK_PROMPT_CONTRACT`], the candidates
/// being ranked, and the task — assembled here so [`Prompt::from_text`]'s
/// single scrub covers all three, including the task text, which is caller
/// input exactly as [`Prompt::for_request`]'s `request_text` is.
fn build_prompt(window: &[MemoryRecord], task: &str) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(RERANK_PROMPT_CONTRACT.len() + task.len() + 512);
    out.push_str(RERANK_PROMPT_CONTRACT);
    out.push_str("\n## Candidates\n\n");
    for record in window {
        let _ = writeln!(
            out,
            "id: {} subject: {}",
            record.id.as_str(),
            excerpt(record)
        );
    }
    out.push_str("\n## Task\n\n");
    out.push_str(task);
    out.push('\n');
    out
}

/// A candidate's subject when it has one, otherwise its body — both capped
/// at [`EXCERPT_BYTES`], on a `char` boundary so a multi-byte alphabet is
/// never split.
fn excerpt(record: &MemoryRecord) -> String {
    let text = record
        .subject
        .as_deref()
        .filter(|subject| !subject.is_empty())
        .unwrap_or(&record.body);
    truncate_bytes(text, EXCERPT_BYTES)
}

fn truncate_bytes(text: &str, budget: usize) -> String {
    if text.len() <= budget {
        return text.to_owned();
    }
    let mut end = budget;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Read a model's whole reply as a JSON array of candidate ids, tolerating
/// surrounding prose and a ```` ```json ```` fence exactly as
/// [`super::extract::schema::parse`] does for the extraction contract's
/// reply — the same two things every model does to JSON.
///
/// An id the reply names that is not in `sent` is refused, and refuses the
/// **whole** reply rather than dropping just that id: a reply naming an
/// unknown id is not answering about the candidates it was actually given,
/// so nothing in it can be trusted to be about them either. See the module
/// documentation.
fn parse_reply(reply: &str, sent: &HashSet<&str>) -> Result<Vec<String>, String> {
    let body = extract_json_array(reply).ok_or_else(|| "no JSON array in the reply".to_owned())?;
    let elements: Vec<serde_json::Value> =
        serde_json::from_str(body).map_err(|err| err.to_string())?;

    let mut ids = Vec::with_capacity(elements.len());
    for element in elements {
        let Some(id) = element.as_str() else {
            return Err("the reply's array held something other than a string id".to_owned());
        };
        if !sent.contains(id) {
            return Err(format!(
                "the reply named an id outside the candidates it was sent: {id}"
            ));
        }
        ids.push(id.to_owned());
    }
    Ok(ids)
}

/// The outermost `[…]` in `reply`, by bracket balance — [`extract_json_array`]'s
/// own reasoning: `find`/`rfind` would capture a `]` inside a memory's own
/// subject if the model happened to quote one.
fn extract_json_array(reply: &str) -> Option<&str> {
    let start = reply.find('[')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, c) in reply[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&reply[start..start + offset + c.len_utf8()]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Rebuild `candidates` in the order the reply named, then the window's
/// omitted ids in their original order, then whatever was beyond the window
/// untouched — see [`rerank`]'s own documentation for the shape.
fn apply_order(
    mut candidates: Vec<MemoryRecord>,
    window_len: usize,
    order: &[String],
) -> Vec<MemoryRecord> {
    // Everything beyond the window, split off first and untouched from here
    // on. `Vec::remove` below preserves the relative order of what is left
    // in `window`, which is what makes the omitted-ids case correct without
    // a second sort.
    let rest = candidates.split_off(window_len);
    let mut window = candidates;

    let mut reordered = Vec::with_capacity(window.len() + rest.len());
    for id in order {
        if let Some(position) = window.iter().position(|record| record.id.as_str() == id) {
            reordered.push(window.remove(position));
        }
    }
    // Omitted ids, in the order the lexical search already gave them.
    reordered.extend(window);
    reordered.extend(rest);
    reordered
}

fn describe_error(err: &ModelError) -> String {
    err.to_string()
}

// ---------------------------------------------------------------------------
// The reranking seat — the extraction seat's four steps, for `[memory]
// rerank_model`.
// ---------------------------------------------------------------------------

/// Resolve `[memory] rerank_model` into a callable model, or say `None`.
///
/// The extraction seat's four steps (`docs/product/evidence/phase-9i.md`'s
/// `GH-ROUTED-EXTRACTION-CLIENT`) — consent, the local bypass, the choice,
/// the client — for `JobKind::Reranking`. Lives here, in the library, rather
/// than beside `main.rs::disposable_extraction_model`: [`super::inject::briefing`]
/// is reached from **two** doors, `main.rs::brief_launch_session` and
/// `crate::api::unix::select_memory`, and only the library crate is common
/// to both — `main.rs` is a separate binary target that cannot be called
/// from `crate::api`. `main.rs::disposable_rerank_model` is this function by
/// another name, kept there only as the thin call `brief_launch_session`
/// makes.
///
/// # `None`, never a model whose calls would all fail
///
/// Unlike [`super::extract::disposable::RoutedModel`]'s own "route, explain,
/// record, call nothing" posture for memory extraction's unconsented case
/// (Phase 9I line 534: durable evidence of what *would* have been chosen),
/// reranking has no such evidence requirement for a knob nobody set — see
/// map line 1090. So this returns `None` immediately when unconsented,
/// before any candidate, health read, or routing decision is built, rather
/// than returning a model that would only fail when called. `None` is also
/// the answer when a candidate could not be built, the router found none,
/// or the resolved client could not be built — every one of these is
/// `RerankOutcome::Bypassed` at [`rerank`]'s own call site, with the reason
/// [`super::extract::ModelError::Unavailable`] gives.
///
/// # What this deliberately does not carry, unlike extraction's own seat
///
/// No persisted cross-process health ([`crate::routing::free::FreePool::new`]
/// is the honest argument for a caller with no history — that type's own
/// doc comment), and no paced-request reservation claim. At most one
/// rerank call happens per briefing, so the pacing that protects a shared
/// free allowance under memory extraction's own dispatch volume is not
/// reused here; a Green follow-up if reranking's own volume ever earns it.
///
/// No `session` parameter, unlike `main.rs::disposable_extraction_model`:
/// this function records nothing (see above), so it has nothing to key a
/// record by. `main.rs::disposable_rerank_model` keeps `session` in its own
/// signature, matching its sibling's shape, and does not pass it here.
pub fn resolve_rerank_model(runtime: &crate::Runtime) -> Option<Box<dyn ExtractionModel>> {
    use super::ConfiguredModel;
    use crate::config::{EffectiveConfig, UserConfig, load_project_config};
    use crate::routing::CredentialId;
    use crate::routing::disposable::{DisposableCandidate, DisposableRouting, JobKind};
    use crate::routing::free::{FreePool, FreePreferences};
    use crate::routing::pressure::ReserveScope;
    use crate::secret::native::PreferNativeSecretStore;
    use crate::secret::{SecretRef, SecretStore as _};

    let user = UserConfig::load(runtime.paths()).ok()?;
    let project = load_project_config(runtime.project()).ok()?;
    let effective = EffectiveConfig::new(&user, project.as_ref());

    // Step 1: consent. No knob, no candidate, no routing decision, no call —
    // see this function's own documentation for why this returns rather
    // than routing "for the record" the way extraction's seat does.
    let chosen = effective.memory_rerank_model().value?;

    let provider_config = project
        .as_ref()
        .and_then(|p| p.providers().get(chosen.provider()))
        .or_else(|| user.providers().get(chosen.provider()))?;
    if !provider_config.enabled() {
        return None;
    }
    let provider = provider_config.to_provider(chosen.provider()).ok()?;
    let secrets = PreferNativeSecretStore::detect();

    // Step 2: the local bypass. A provider naming no credential variable is
    // not expressible as a `DisposableCandidate` — see
    // `main.rs::configured_extraction_candidate`'s own reasoning, which this
    // mirrors — so it is built and used directly.
    if provider_config.credential_env().is_empty() {
        let client = ConfiguredModel::new(&provider, chosen.model(), None)
            .ok()?
            .with_timeouts(rerank_timeouts());
        return Some(Box::new(client));
    }

    let reference = provider_config
        .credential_env()
        .iter()
        .map(|var| SecretRef::Environment { var: var.clone() })
        .find(|reference| secrets.resolve(reference).is_some())?;
    let credential_value = secrets.resolve(&reference)?;

    // Step 3: the choice. One candidate — the user's own named model — ranked
    // against `[memory] rerank_model` alone: there is nothing else configured
    // for this job kind to choose *among*, but `DisposableRouting::choose`
    // still applies every hard constraint (entitlement, the metered gate,
    // the protected reserve) a second, unconfigured candidate would also
    // have to clear.
    let candidate = DisposableCandidate::new(
        chosen.provider().to_owned(),
        chosen.model().to_owned(),
        CredentialId::new(chosen.provider().to_owned(), reference),
        provider_config.cost_of(chosen.model()),
    );
    let routing = DisposableRouting::for_support_work(
        effective.prefer_free_routing().value,
        FreePreferences::new(),
    )
    .with_reserve_policy(
        effective
            .reserve_policies()
            .for_scope(ReserveScope::Background),
    );
    let pool = FreePool::new();
    let routed = super::RoutedModel::new(
        JobKind::Reranking,
        std::slice::from_ref(&candidate),
        &routing,
        &pool,
    );
    let Ok(choice) = routed.choice() else {
        return None;
    };

    // Step 4: the client, for exactly the resource the policy chose.
    let client = ConfiguredModel::new(&provider, choice.model(), Some(credential_value))
        .map(|client| client.with_timeouts(rerank_timeouts()));
    let credential_label = choice.credential().label();
    let routed = routed.with_client(
        client.map_err(|err| format!("the rerank model cannot be used: {err}")),
        credential_label,
    );

    if routed.can_call() {
        Some(Box::new(routed))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Retrieval diagnostics — map line 1094.
// ---------------------------------------------------------------------------

/// One lexically-ranked candidate, as a diagnostics reader sees it — never
/// the full body, and never anything beyond what `super::inject`'s own
/// entry rendering already treats as safe to show: a subject, capped.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DiagnosticsCandidate {
    pub id: String,
    /// `"invariants_and_constraints"` or `"other"` —
    /// [`super::search::RetrievalResult`]'s own two group names.
    pub group: &'static str,
    /// Position in the lexical search's own order, within its group.
    pub rank: usize,
    /// Capped at [`EXCERPT_BYTES`], `None` when the record had none.
    pub subject: Option<String>,
}

/// [`RerankOutcome`] as one JSON object, for one line of the diagnostics
/// file or one `--explain` answer — never the two different shapes a
/// hand-rolled `match` at each call site would drift into.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum DiagnosticsRerank {
    NotConfigured,
    TooFew,
    Reordered {
        resource: String,
        order: Vec<String>,
    },
    Bypassed {
        resource: Option<String>,
        reason: String,
    },
}

impl DiagnosticsRerank {
    /// `outcome` plus the ids [`rerank`] actually returned, when it
    /// reordered anything — the pair a diagnostics reader needs together,
    /// since [`RerankOutcome::Reordered`] itself only carries a count.
    fn from_outcome(outcome: &RerankOutcome, returned_order: &[String]) -> Self {
        match outcome {
            RerankOutcome::NotConfigured => Self::NotConfigured,
            RerankOutcome::TooFew => Self::TooFew,
            RerankOutcome::Reordered { resource, .. } => Self::Reordered {
                resource: resource.clone(),
                order: returned_order.to_vec(),
            },
            RerankOutcome::Bypassed { resource, reason } => Self::Bypassed {
                resource: resource.clone(),
                reason: reason.clone(),
            },
        }
    }
}

/// One briefing's whole retrieval-and-rerank decision, in the shape both the
/// diagnostics file and `memory search --explain` render — see the module
/// documentation and this crate's `memory/inject.rs`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RetrievalTrace {
    pub ts: i64,
    pub session: Option<String>,
    pub query: String,
    pub candidates: Vec<DiagnosticsCandidate>,
    pub rerank: DiagnosticsRerank,
    pub selected: Vec<String>,
}

impl RetrievalTrace {
    /// Assemble a trace from what a briefing already computed — no query of
    /// its own, so the record can never disagree with what was actually
    /// injected.
    ///
    /// `candidates` is built by [`diagnostics_candidates`] **before**
    /// `grouped.other` is moved into [`rerank`], so the caller in
    /// `memory/inject.rs` calls that first and holds the result across the
    /// rerank call rather than re-deriving it from records `rerank` has
    /// already consumed. `reordered_other_ids` is the `other` bucket's ids
    /// in their final, post-rerank order — meaningful only alongside
    /// [`RerankOutcome::Reordered`], and ignored otherwise.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        now_unix: i64,
        session: Option<&str>,
        query: &str,
        candidates: Vec<DiagnosticsCandidate>,
        outcome: &RerankOutcome,
        reordered_other_ids: &[String],
        selected: &[MemoryRecord],
    ) -> Self {
        Self {
            ts: now_unix,
            session: session.map(str::to_owned),
            query: query.to_owned(),
            candidates,
            rerank: DiagnosticsRerank::from_outcome(outcome, reordered_other_ids),
            selected: selected
                .iter()
                .map(|record| record.id.as_str().to_owned())
                .collect(),
        }
    }
}

/// [`DiagnosticsCandidate`] rows for a whole grouped search result, in the
/// lexical order [`super::store::MemoryStore::search_grouped_for_injection`]
/// produced — built before `other` is handed to [`rerank`], which consumes
/// it.
pub fn diagnostics_candidates(
    invariants_and_constraints: &[MemoryRecord],
    other: &[MemoryRecord],
) -> Vec<DiagnosticsCandidate> {
    let mut candidates = Vec::with_capacity(invariants_and_constraints.len() + other.len());
    for (rank, record) in invariants_and_constraints.iter().enumerate() {
        candidates.push(diagnostics_candidate(
            record,
            "invariants_and_constraints",
            rank,
        ));
    }
    for (rank, record) in other.iter().enumerate() {
        candidates.push(diagnostics_candidate(record, "other", rank));
    }
    candidates
}

fn diagnostics_candidate(
    record: &MemoryRecord,
    group: &'static str,
    rank: usize,
) -> DiagnosticsCandidate {
    DiagnosticsCandidate {
        id: record.id.as_str().to_owned(),
        group,
        rank,
        subject: record
            .subject
            .as_deref()
            .filter(|subject| !subject.is_empty())
            .map(|subject| truncate_bytes(subject, EXCERPT_BYTES)),
    }
}

/// Append one [`RetrievalTrace`] as one JSON line to
/// `<state_dir>/memory-retrieval.jsonl` — project-scoped, since `runtime`
/// resolves exactly one project's state directory.
///
/// A single `create(true).append(true)` open and one `write_all` of the
/// whole line, the same shape every short-lived Glasshouse process appends
/// a durable record with: one syscall per line keeps two processes' writes
/// from interleaving mid-line, which a multi-write sequence could not
/// promise. A failure here is one debug line — this runs inside a hook or
/// launch process, and Glasshouse's own bookkeeping is never more important
/// than the session it keeps books about, matching
/// `main.rs::persist_support_work_health`'s own posture.
pub fn append_diagnostics(runtime: &crate::Runtime, trace: &RetrievalTrace) {
    let path = runtime.state_dir().join("memory-retrieval.jsonl");
    let line = match serde_json::to_string(trace) {
        Ok(line) => line,
        Err(err) => {
            tracing::debug!(error = %err, "could not encode a retrieval diagnostics record");
            return;
        }
    };
    let mut file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(err) => {
            tracing::debug!(error = %err, path = %path.display(), "could not open the retrieval diagnostics file");
            return;
        }
    };
    if let Err(err) = writeln!(file, "{line}") {
        tracing::debug!(error = %err, path = %path.display(), "could not append a retrieval diagnostics record");
    }
}

/// `trace` as one line of JSON, for `glasshouse memory search --explain` —
/// the same encoding [`append_diagnostics`] writes, so the two are
/// verifiably the same record.
pub fn explain_line(trace: &RetrievalTrace) -> String {
    serde_json::to_string_pretty(trace).unwrap_or_else(|err| {
        format!("{{\"error\": \"could not encode the retrieval trace: {err}\"}}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::{DecisionProvenance, MemoryId, MemoryKind, MemoryStatus};

    struct Fixed {
        reply: Result<String, ModelError>,
    }

    impl ExtractionModel for Fixed {
        fn describe(&self) -> String {
            "fixed/test-reranker".to_owned()
        }

        fn complete(&self, _prompt: &Prompt) -> Result<String, ModelError> {
            self.reply.clone()
        }
    }

    fn record(id: &str, subject: &str) -> MemoryRecord {
        MemoryRecord {
            id: MemoryId::new(id),
            project_id: "test-project".to_owned(),
            kind: MemoryKind::Finding,
            authority: None,
            status: MemoryStatus::Active,
            subject: Some(subject.to_owned()),
            body: format!("body of {subject}"),
            source_session_id: None,
            source_commit: None,
            extraction_trigger: None,
            source_events: None,
            provenance: DecisionProvenance::default(),
            superseded_by: None,
            superseded_reason: None,
            validity_conditions: None,
            invalidation_conditions: None,
            review_reason: None,
            review_marked_at: None,
            last_validated_at: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn no_model_is_not_configured_and_leaves_lexical_order() {
        let candidates = vec![record("a", "alpha"), record("b", "beta")];
        let (result, outcome) = rerank(candidates.clone(), None, "a task");
        assert_eq!(result, candidates);
        assert_eq!(outcome, RerankOutcome::NotConfigured);
    }

    #[test]
    fn one_candidate_calls_nothing() {
        let model = Fixed {
            reply: Ok(r#"["a"]"#.to_owned()),
        };
        let candidates = vec![record("a", "alpha")];
        let (result, outcome) = rerank(candidates.clone(), Some(&model), "a task");
        assert_eq!(result, candidates);
        assert_eq!(outcome, RerankOutcome::TooFew);
    }

    #[test]
    fn a_reversed_reply_reorders_the_candidates() {
        let model = Fixed {
            reply: Ok(r#"["b", "a"]"#.to_owned()),
        };
        let candidates = vec![record("a", "alpha"), record("b", "beta")];
        let (result, outcome) = rerank(candidates, Some(&model), "a task");
        assert_eq!(
            result.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["b", "a"]
        );
        assert_eq!(
            outcome,
            RerankOutcome::Reordered {
                resource: "fixed/test-reranker".to_owned(),
                returned: 2,
            }
        );
    }

    #[test]
    fn an_unknown_id_bypasses_the_whole_reply() {
        let model = Fixed {
            reply: Ok(r#"["a", "not-a-real-id"]"#.to_owned()),
        };
        let candidates = vec![record("a", "alpha"), record("b", "beta")];
        let (result, outcome) = rerank(candidates.clone(), Some(&model), "a task");
        assert_eq!(result, candidates, "an untrustworthy reply changes nothing");
        let RerankOutcome::Bypassed { resource, reason } = outcome else {
            panic!("expected a bypass");
        };
        assert_eq!(resource.as_deref(), Some("fixed/test-reranker"));
        assert!(reason.contains("not-a-real-id"), "{reason}");
    }

    #[test]
    fn a_call_that_errors_bypasses_with_the_model_error_as_the_reason() {
        let model = Fixed {
            reply: Err(ModelError::TimedOut),
        };
        let candidates = vec![record("a", "alpha"), record("b", "beta")];
        let (result, outcome) = rerank(candidates.clone(), Some(&model), "a task");
        assert_eq!(result, candidates);
        assert_eq!(
            outcome,
            RerankOutcome::Bypassed {
                resource: Some("fixed/test-reranker".to_owned()),
                reason: ModelError::TimedOut.to_string(),
            }
        );
    }

    #[test]
    fn omitted_ids_follow_in_lexical_order_and_the_window_bounds_what_is_sent() {
        let model = Fixed {
            reply: Ok(r#"["h"]"#.to_owned()),
        };
        let mut candidates: Vec<MemoryRecord> = (0..10)
            .map(|i| record(&format!("id{i}"), &format!("subject {i}")))
            .collect();
        // Give the ninth window slot an id the reply can name.
        candidates[7] = record("h", "the eighth");
        let (result, outcome) = rerank(candidates, Some(&model), "a task");
        assert_eq!(result[0].id.as_str(), "h", "the named id leads");
        assert_eq!(
            result[1].id.as_str(),
            "id0",
            "omitted ids follow in lexical order"
        );
        assert_eq!(
            result[8].id.as_str(),
            "id8",
            "beyond the sent window is untouched"
        );
        assert_eq!(result[9].id.as_str(), "id9");
        assert_eq!(
            outcome,
            RerankOutcome::Reordered {
                resource: "fixed/test-reranker".to_owned(),
                returned: 1,
            }
        );
    }

    #[test]
    fn diagnostics_render_the_same_record_whichever_caller_asks() {
        let candidates = diagnostics_candidates(
            &[record("inv-1", "an invariant")],
            &[record("o-1", "ordinary")],
        );
        let trace = RetrievalTrace::new(
            1_700_000_000,
            Some("session-1"),
            "fix the bug",
            candidates,
            &RerankOutcome::Reordered {
                resource: "fixed/test-reranker".to_owned(),
                returned: 1,
            },
            &["o-1".to_owned()],
            &[record("inv-1", "an invariant"), record("o-1", "ordinary")],
        );
        let file_line = serde_json::to_string(&trace).unwrap();
        let explain = explain_line(&trace);
        let reparsed_file: serde_json::Value = serde_json::from_str(&file_line).unwrap();
        let reparsed_explain: serde_json::Value = serde_json::from_str(&explain).unwrap();
        assert_eq!(reparsed_file, reparsed_explain);
        assert_eq!(reparsed_file["session"], "session-1");
        assert_eq!(reparsed_file["rerank"]["outcome"], "reordered");
    }
}
