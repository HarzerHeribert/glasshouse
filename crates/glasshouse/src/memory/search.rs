//! Free-text search over project memory (Phase 23).
//!
//! Declared ahead of its implementation so that the module owning it never has
//! to edit `memory/mod.rs`, which another worker holds.
//!
//! # Free-form text is not FTS5 syntax
//!
//! FTS5's query language treats `"`, `*`, `:`, `^`, `-`, `(`, `)`, `NEAR`,
//! `AND`, `OR` and `NOT` as operators. A user is typing a question, not a
//! query language, so `sanitize_query` tokenizes on anything that is not a
//! letter or digit and wraps every token in double quotes — a quoted phrase
//! is FTS5's escape hatch for "treat this text literally" — doubling any
//! embedded `"` the way SQL string literals do. The result is passed to
//! `MATCH` as a bound parameter, never interpolated: the only SQL this module
//! ever builds from something other than a fixed literal is a column list it
//! wrote itself.
//!
//! # What the index covers
//!
//! `memories_fts` indexes `subject`, `body` and — from migration 6 —
//! `rationale`. The rationale is searchable because until that migration it
//! *was* the body: the extractor folded it in behind a marker precisely so a
//! search for the reason would find the decision. The eight other Phase 21B
//! provenance columns are deliberately not indexed; they describe a decision
//! somebody has already found rather than supplying the words they would
//! look for, and every indexed column shifts BM25's weighting of the ones
//! that matter.
//!
//! # BM25 direction
//!
//! SQLite's `bm25()` returns a *more negative* number for a *better* match.
//! `ORDER BY bm25(memories_fts) ASC` therefore puts the best match first —
//! this is asserted directly in the integration tests rather than trusted by
//! reading the manual once.

use std::cmp::Ordering;
use std::collections::HashMap;

use super::policy::retrieval_weight;
pub use super::policy::{LadderRung, ladder_rung};
use super::store::{
    MemoryAuthority, MemoryId, MemoryKind, MemoryRecord, MemoryStatus, MemoryStore,
    MemoryStoreError, normalize_observed_path, row_to_record,
};

/// How much of a project's memory a search is allowed to see.
///
/// A default search means [`SearchScope::Current`] — only
/// [`MemoryStatus::Active`] memories are current project knowledge, per the
/// module documentation on [`MemoryStatus`]. Everything else — superseded,
/// rejected, resolved, invalidated, needs-review, conflicted — is history,
/// and history is only ever returned when [`SearchScope::Historical`] is
/// asked for explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    /// Only [`MemoryStatus::Active`] memories. What every caller should use
    /// unless they specifically want history.
    Current,
    /// Every status, active or not. The explicit ask this module requires
    /// before a superseded, rejected, resolved, invalidated, needs-review or
    /// conflicted memory can be returned.
    Historical,
}

/// A sane number of results for a caller that has no particular limit in
/// mind. Never used implicitly — every call to [`MemoryStore::search`] states
/// its own limit — but it exists so no caller has to invent a number.
pub const DEFAULT_SEARCH_LIMIT: usize = 20;

/// A memory search's matches, split by whether they are currently active
/// invariants and constraints or not — Phase 21F line 929's *"retrieve
/// current active invariants and constraints separately from historical
/// decisions."*
///
/// Produced by [`MemoryStore::search_grouped`]. Narrower than
/// [`MemoryAuthority::is_binding`], which also counts an ordinary
/// [`MemoryAuthority::Decision`]: line 929 names exactly the two classes
/// Glasshouse treats as rules nobody may quietly work around, not every
/// class capable of directing current work.
///
/// # `Eq` is not derived, and the reason is the relevance
///
/// A relevance is an `f64`, and `f64` is [`PartialEq`] but not [`Eq`]. The
/// derive was dropped rather than the field hidden from equality: two
/// retrievals that returned the same memories at different relevances are
/// not the same retrieval, and pretending otherwise would be the only thing
/// worse than losing a trait nothing in this crate uses.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RetrievalResult {
    /// Currently active invariants and constraints, in the relevance/decay
    /// order [`MemoryStore::search`] produced them.
    pub invariants_and_constraints: Vec<MemoryRecord>,
    /// Everything else the search matched — ordinary current memories, and,
    /// under [`SearchScope::Historical`], history — including a memory whose
    /// authority is invariant or constraint but that is no longer active,
    /// which is history rather than a current rule.
    pub other: Vec<MemoryRecord>,
    /// What each returned memory scored on this query — see
    /// [`RetrievalResult::relevance`], which is the only way to read it.
    ///
    /// Private, unlike the two groups above, so that the one invariant this
    /// map has cannot be broken from outside: **every entry was produced by
    /// an actual retrieval.** A caller that could insert into it could
    /// manufacture a relevance for a memory no query ever matched, which is
    /// precisely the fabricated number [`super::inject::briefing`]'s refusal
    /// exists to prevent.
    ///
    /// The map is therefore **not** guaranteed to hold an entry for every
    /// record in the two groups above, and it never was: it holds an entry
    /// for every record some query *scored*. [`MemoryStore::for_path`]
    /// retrieves by an exact file-path match and runs no query at all, so a
    /// result it produced has both groups populated and this map empty —
    /// which is the invariant holding, not an omission. See [`Scored`] for
    /// why the absence is carried as an `Option` rather than filled in with
    /// a zero.
    relevances: HashMap<MemoryId, f64>,
}

impl RetrievalResult {
    /// What `id` scored on the query that produced this result, or `None` if
    /// `id` was not one of the memories it returned.
    ///
    /// `None` is a real answer and the only honest one for a memory this
    /// retrieval never saw: there is no relevance to report, and a zero would
    /// be a fabrication that reads as "matched as badly as possible" rather
    /// than "was not asked about". A search that matched nothing therefore
    /// answers `None` to every question, rather than `Some(0.0)` to some of
    /// them.
    ///
    /// It is also the answer for **every** memory in a result
    /// [`MemoryStore::for_path`] produced, and for the same reason one step
    /// further out: that door retrieves by an exact file-path match and asks
    /// no question, so none of the memories it returns was scored by
    /// anything. "Was not asked about" is exactly what happened to them.
    ///
    /// # This is a relevance, and it is deliberately not a confidence
    ///
    /// SQLite's `bm25()` scores how well one memory's indexed text matched
    /// one query against **this project's own corpus statistics** — term
    /// frequency, document length, and how many other memories in this table
    /// contain the same terms. More negative is a better match (see the
    /// module documentation), so the scale is unbounded below and has no
    /// natural zero.
    ///
    /// Three consequences, and each one is a reason not to threshold it:
    ///
    /// - **It is not calibrated.** The same number means different things for
    ///   two different queries, and for the same query against two different
    ///   projects. There is no constant of which *"below this, the retrieval
    ///   was poor"* is a true statement, so a threshold would be a number
    ///   somebody picked rather than a fact about the retrieval.
    /// - **It is not the order the results came back in.**
    ///   [`MemoryStore::search`] ranks by [`LadderRung`] first, breaks ties
    ///   *within* one rung by this number multiplied by a decay weight, and
    ///   then `demote_thin_decisions` permutes again. Reading it as "why this
    ///   memory came first" is wrong across rungs.
    /// - **It measures the match, not the memory.** Whether a memory is worth
    ///   putting into a session's context is a question about the memory's
    ///   authority, currency and scope. None of those is in here.
    ///
    /// So map line 1129 — *"avoid injecting memory when retrieval confidence
    /// is low"* — is **not** satisfied by comparing this against a constant,
    /// and [`super::inject::briefing`] still refuses it. That function's
    /// documentation carries the three objections that survive this method
    /// existing.
    ///
    /// # Why the raw match and not the blended ranking score
    ///
    /// [`MemoryStore::search`] also computes `relevance × retrieval_weight` —
    /// the number it actually sorts on inside a rung. That one is not offered
    /// here, and the difference is the whole reason this method is worth
    /// having: `super::policy::retrieval_weight` reads a memory's authority,
    /// age, validation state and project phase and **never sees the query
    /// text**. Blending it in yields a number that is high for an ancient
    /// invariant no matter what was asked — exactly the query-blind signal
    /// `inject.rs` refuses to build a gate from. It is also wall-clock
    /// dependent, so the same store and the same query yield a different
    /// value tomorrow.
    ///
    /// The raw match is the one quantity in this module that varies with the
    /// query and with nothing else. Anything inside this module that
    /// genuinely wants the blend can compute it: the record carries its own
    /// authority, timestamps and phase, and `retrieval_weight` is the same
    /// function [`MemoryStore::search`] calls.
    pub fn relevance(&self, id: &MemoryId) -> Option<f64> {
        self.relevances.get(id).copied()
    }
}

/// One retrieval hit and the BM25 relevance the query gave it, kept together
/// from the moment the row is decoded until the moment the two groups of
/// [`RetrievalResult`] are built.
///
/// A pair rather than a field on [`MemoryRecord`], because a relevance is a
/// property of *this retrieval* and not of the memory: the same record scores
/// differently for a different query, and a record read by
/// [`MemoryStore::get`] has no relevance at all. Putting it on the record
/// would make that absence unrepresentable except as a lie.
///
/// # `None` is *"was not asked about"*, and it is why this is an `Option`
///
/// [`MemoryStore::for_path`] retrieves by an exact `memory_files.path` match.
/// **It runs no query, so there is no relevance for it to supply** — and the
/// alternative was to hand `group` a `0.0`, which would put a manufactured
/// number into [`RetrievalResult`]'s private relevance map for a memory no
/// query ever matched. That is precisely what the map is private to prevent,
/// and [`RetrievalResult::relevance`] already says a zero there *"would be a
/// fabrication that reads as 'matched as badly as possible' rather than 'was
/// not asked about'"*.
///
/// Making the absence representable **strengthens** that invariant rather
/// than piercing it: the map still holds only relevances an actual query
/// produced, because `group` inserts nothing for a `None`, and the third door
/// still gets the one grouping and the one ranking the other two get.
#[derive(Debug, Clone)]
struct Scored {
    record: MemoryRecord,
    /// The BM25 score this hit earned, or `None` for a retrieval that asked
    /// no question. Never `Some(0.0)` standing in for the absence.
    relevance: Option<f64>,
}

/// Whether a memory is a *currently binding* invariant or constraint — see
/// [`RetrievalResult`] for why this is narrower than
/// [`MemoryAuthority::is_binding`].
fn is_current_invariant_or_constraint(record: &MemoryRecord) -> bool {
    record.is_current()
        && matches!(
            record.authority,
            Some(MemoryAuthority::Invariant) | Some(MemoryAuthority::Constraint)
        )
}

/// Split one ranked result list the way [`RetrievalResult`] describes. A
/// stable partition — neither group is re-sorted.
///
/// The relevance travels sideways rather than into either group: a memory
/// keeps the score it earned whichever group it lands in, and the grouping
/// cannot change a number it does not touch.
///
/// A hit whose relevance is `None` — see [`Scored`] — contributes **no entry
/// at all** to the relevance map. It is not stored as a zero and not stored
/// as a sentinel, so [`RetrievalResult::relevance`] answers `None` for it,
/// which is the true answer: nothing scored that memory, because nothing was
/// asked. This is the one place the invariant could have been broken, and it
/// is where it is kept.
fn group(hits: Vec<Scored>) -> RetrievalResult {
    let mut grouped = RetrievalResult::default();
    for Scored { record, relevance } in hits {
        if let Some(relevance) = relevance {
            grouped.relevances.insert(record.id.clone(), relevance);
        }
        if is_current_invariant_or_constraint(&record) {
            grouped.invariants_and_constraints.push(record);
        } else {
            grouped.other.push(record);
        }
    }
    grouped
}

/// The one ordering in this crate, applied by every door before
/// [`demote_thin_decisions`] permutes within it.
///
/// # Phase 21E: the ladder rung is the primary key
///
/// See [`ladder_rung`]'s own documentation for why an idea must never
/// outrank an invariant regardless of how well it matched. Only within the
/// same rung does the weight below decide the order.
///
/// # Within a rung, and why the query-less door is not a second ranking
///
/// A queried hit is ordered by `relevance × retrieval_weight`, ascending.
/// SQLite's `bm25()` is *more negative* for a better match (see the module
/// documentation) and [`retrieval_weight`] is strictly positive, so ascending
/// puts the best-matching, highest-weighted memory first — exactly the
/// comparison [`MemoryStore::search`] has always made.
///
/// A hit with no relevance ([`MemoryStore::for_path`]) is ordered by
/// `retrieval_weight` alone, descending, which is the **same** comparison
/// with the one factor it does not have left out rather than replaced.
/// Substituting a number for the missing factor is what this whole change
/// exists to avoid: a `0.0` would collapse every product to zero and order
/// the results by nothing at all, while still looking like a ranking.
/// `retrieval_weight` never sees the query text — that is stated at
/// [`RetrievalResult::relevance`] as the reason the blend is not offered to
/// callers — so it remains an honest key when there is no query.
///
/// The mixed case cannot arise: a retrieval either ran a `MATCH` or did not,
/// and both doors build every one of their hits the same way. [`Ordering::Equal`]
/// is the answer that adds no claim if it ever does.
fn rank(hits: &mut [Scored], now: i64) {
    let weight = |record: &MemoryRecord| {
        retrieval_weight(
            record.authority,
            now,
            record.created_at,
            record.last_validated_at,
            record.provenance.project_phase,
        )
    };

    hits.sort_by(|a, b| {
        let a_rung = ladder_rung(&a.record);
        let b_rung = ladder_rung(&b.record);
        b_rung.cmp(&a_rung).then_with(|| {
            let a_weight = weight(&a.record);
            let b_weight = weight(&b.record);
            match (a.relevance, b.relevance) {
                (Some(a_relevance), Some(b_relevance)) => (a_relevance * a_weight)
                    .partial_cmp(&(b_relevance * b_weight))
                    .unwrap_or(Ordering::Equal),
                (None, None) => b_weight.partial_cmp(&a_weight).unwrap_or(Ordering::Equal),
                (Some(_), None) | (None, Some(_)) => Ordering::Equal,
            }
        })
    });
}

/// Every column of `memories`, qualified by table name.
///
/// [`super::store::ALL_COLUMNS`] cannot be reused here unqualified: this
/// query joins `memories` against `memories_fts`, which has its own `subject`
/// and `body` columns, and an unqualified `SELECT subject, body, ...` would
/// be ambiguous between the two tables. [`row_to_record`] reads columns by
/// name rather than position, so qualifying them here changes nothing about
/// how the row is decoded.
const QUALIFIED_COLUMNS: &str = "memories.id, memories.project_id, memories.kind, \
                                 memories.authority, memories.status, memories.subject, \
                                 memories.body, memories.source_session_id, \
                                 memories.source_commit, memories.source_event_first, \
                                 memories.source_event_last, memories.superseded_by, \
                                 memories.created_at, memories.updated_at, \
                                 memories.rationale, memories.project_phase, \
                                 memories.problem, memories.assumptions, \
                                 memories.scale_assumptions, memories.security_assumptions, \
                                 memories.compatibility_assumptions, \
                                 memories.operational_assumptions, memories.evidence, \
                                 memories.source_excerpt, memories.validity_conditions, \
                                 memories.invalidation_conditions, memories.review_reason, \
                                 memories.review_marked_at, memories.last_validated_at, \
                                 memories.superseded_reason";

/// How many extra candidates a search fetches beyond `limit`, before decay
/// reordering runs.
///
/// Phase 21D's decay is applied in Rust, after the SQL query, because it
/// depends on the wall clock and on per-authority policy — see
/// [`retrieval_weight`]. If the SQL `LIMIT` were the caller's own `limit`,
/// decay could never promote a fresh, high-authority memory that ranked
/// outside the raw BM25 top-`limit` back into the returned set: the row would
/// already be gone. Over-fetching a wider candidate pool and truncating after
/// reordering is what keeps decay able to change *which* memories come back,
/// not only their order among a fixed set.
const DECAY_OVERFETCH_FACTOR: usize = 5;

/// The most candidates a search ever pulls from SQLite before truncating to
/// `limit`, regardless of how small `limit` is. Bounds the cost of a search
/// over a project with a very large memory table.
const DECAY_OVERFETCH_CAP: usize = 500;

/// The SQL `LIMIT` for a search asking for `limit` final results.
fn overfetch_limit(limit: usize) -> usize {
    let scaled = limit.saturating_mul(DECAY_OVERFETCH_FACTOR);
    scaled.clamp(limit, DECAY_OVERFETCH_CAP.max(limit))
}

/// Turn free-form text into a safe FTS5 `MATCH` expression, or `None` if
/// nothing in it could be searched for.
///
/// Splits on anything that is not alphanumeric, so punctuation such as `:`,
/// `-`, `(` and `)` becomes a token boundary rather than an operator, and
/// wraps each surviving token in double quotes — doubling any embedded `"` —
/// so FTS5 reads it as a literal phrase rather than syntax. Bare tokens are
/// implicitly ANDed by FTS5, which is the right default for a search box: a
/// query for several words should narrow, not enumerate every operator
/// combination.
///
/// A query with nothing alphanumeric in it at all — empty, whitespace, or
/// pure punctuation like `"` or `*` — sanitizes to nothing, and `None` is the
/// caller's signal to return no results without ever reaching SQLite.
fn sanitize_query(text: &str) -> Option<String> {
    let tokens: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect();

    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" "))
    }
}

/// The `memories_fts` column that holds a memory's own statement of what it
/// is about — see `injection_query`, which uses it as line 930's *scope*.
const SUBJECT_COLUMN: &str = "subject";

/// Turn a routed task into the FTS5 `MATCH` expression **context injection**
/// uses, or `None` if nothing in it could be searched for.
///
/// The shape is
///
/// ```text
/// ("a" "b" "c") OR ({subject} : ("a" OR "b" OR "c"))
/// ```
///
/// — today's conjunctive query, unchanged, `OR`ed with a disjunctive one
/// restricted to the `subject` column.
///
/// # The left half: nothing that is retrieved today stops being retrieved
///
/// `sanitize_query` joins its quoted tokens with spaces, which FTS5 reads as
/// implicit `AND`: every word must appear in the same memory. That is right
/// for a search box, where a person adds a word to narrow the result set, and
/// it is wrong for a routed task, which is prose. *"Please look at the kestrel
/// export and make sure it cannot write a partial file"* demands that one
/// memory contain `please` and `look` and `sure` and `up`, so injection
/// retrieved **nothing** for any task written as a sentence — the limit Phase
/// 27 closed line 1126 with, named rather than hidden.
///
/// That expression is kept verbatim as the left disjunct, so the result set
/// here is a **superset** of the one the search box gets, by construction
/// rather than by test: whatever a keyword-shaped task retrieves today it
/// still retrieves. This step only ever adds recall.
///
/// # The right half is line 930, and it is in the query rather than after it
///
/// *"Inject only memories whose scope overlaps the current task."* Joining
/// prose with a bare `OR` makes membership almost free — one incidental word
/// and a memory is a candidate — and `MemoryStore::search` ranks by
/// [`LadderRung`] **before** relevance, so the top of a wide candidate set is
/// this project's highest-authority memories whatever the task was about.
/// Measured on a fifteen-memory corpus: a bare `OR` answered *"update the
/// README with the new installation instructions"* with three binding
/// invariants about pseudo-terminals, secrets and project isolation, matched
/// on the word `the` alone.
///
/// So the added disjunct is restricted to the `subject` column — the field
/// where a memory records what it is *about*, and the field
/// [`contradicts`] already treats as a memory's identity when deciding that
/// two memories concern the same thing. A memory joins the candidate set on
/// prose only if the task names its subject.
///
/// **Why this is not a relevance threshold wearing a different name.** It
/// reads no score, sorts nothing, and cannot be satisfied by matching the
/// same word harder; a memory whose body mentions the task's words a hundred
/// times is still out if its subject is about something else. More to the
/// point, a relevance threshold would not have worked: in the measurement
/// above the noise was selected by *rung*, not by score, so no cut on `bm25()`
/// could have removed it, and a stop-word or corpus-frequency filter could
/// not either — for the task *"make sure it is up to date"* no term matched
/// more than 47% of that corpus and every one of the three injected memories
/// was still irrelevant.
///
/// A memory that records **no** subject cannot be judged this way and is not
/// judged: it matches only through the left disjunct, which is exactly the
/// behaviour it has today. That is the direction this project's requirement
/// points — injection is strictly more recall, never less — and it is a real
/// limit, recorded in `phase-27.md` rather than papered over.
///
/// # This is a second expression, not a second retrieval
///
/// Phase 27 refused line 1129 partly because a second BM25 query issued from
/// `inject.rs` *"would be a second retrieval implementation ranking
/// differently from the one that chose the memories it was scoring."* That
/// objection is about **ranking**, and nothing here ranks: this function
/// returns a `MATCH` expression and `MemoryStore::search_matching` — the same
/// table, the same `bm25()`, the same ladder, the same decay weighting, the
/// same thin-decision demotion — does the rest for both doors.
///
/// # The quoting is inherited, not re-implemented
///
/// Every token is built by `sanitize_query` itself and only the join is
/// changed. A token is alphanumeric-only by construction there, so no quoted
/// token can contain a space and splitting that output on spaces recovers
/// exactly the tokens it produced. A task containing `OR`, `NEAR`, `*`, `"`
/// or `-` is therefore quoted here by the same code that quotes it for the
/// search box, and the containment property has one home rather than two.
fn injection_query(text: &str) -> Option<String> {
    let conjunctive = sanitize_query(text)?;
    let scoped = conjunctive.split(' ').collect::<Vec<&str>>().join(" OR ");
    Some(format!(
        "({conjunctive}) OR ({{{SUBJECT_COLUMN}}} : ({scoped}))"
    ))
}

impl<'a> MemoryStore<'a> {
    /// Search this project's memory by free text, ranked by BM25 relevance.
    ///
    /// `text` is never interpreted as FTS5 syntax — see the module
    /// documentation and `sanitize_query` — so a user typing `what does
    /// "foo" do?`, a bare `AND`, or `a: b` gets a search rather than a
    /// `SqliteFailure`. A query that sanitizes to nothing returns an empty
    /// result rather than an error.
    ///
    /// `scope` decides whether history is visible at all; see
    /// [`SearchScope`]. `limit` bounds how many results come back — there is
    /// no way to ask this method for the whole table.
    ///
    /// Every result already carries its own provenance
    /// ([`MemoryRecord::source_session_id`], [`MemoryRecord::source_commit`])
    /// as `Option`, so a memory recorded without one reports it absent
    /// instead of inventing an empty string.
    ///
    /// # Phase 21E: the ladder ranks before the weight does
    ///
    /// Every candidate is first placed on a [`LadderRung`] ([`ladder_rung`]),
    /// and results are ordered by rung before anything else — a validated
    /// current constraint outranks an older ordinary decision, and a
    /// binding invariant outranks everything, regardless of how well any of
    /// them matched the query text. The weight described below is only ever
    /// a tie-breaker *within* one rung; it never lets a memory cross into a
    /// rung its own authority and currency do not earn it.
    ///
    /// # Phase 21D: decay is applied here, after the match
    ///
    /// The raw BM25 relevance of every candidate is multiplied by
    /// `retrieval_weight` before the final ordering, so an old, low-
    /// authority memory that happens to match the query text well still
    /// ranks below a fresh, high-authority memory that matches it poorly —
    /// line 904's *"avoid resurfacing low-authority stale memories merely
    /// because of high lexical similarity."* This has to run in Rust rather
    /// than in the `ORDER BY`: the weight depends on the wall clock and on a
    /// per-authority policy (`super::policy::retrieval_weight`), neither of
    /// which SQLite's `bm25()` has access to. See `overfetch_limit` for why
    /// the SQL `LIMIT` is not simply `limit`.
    ///
    /// # Phase 22 line 1063: conflicts are detected here too
    ///
    /// Before decay runs, every pair of still-[`MemoryStatus::Active`]
    /// candidates in *this* result set is checked for contradiction — see
    /// `contradicts` — and a contradicting pair is moved to
    /// [`MemoryStatus::Conflicted`] via [`MemoryStore::mark_conflicted`]
    /// before being returned, so a caller never receives two mutually
    /// contradictory memories presented as equally settled. Detection is
    /// scoped to the memories this query actually matched, not the whole
    /// project: Phase 22 asks that a conflict be flagged, not that every
    /// memory be compared against every other one on every search.
    ///
    /// # The relevance is no longer thrown away
    ///
    /// This method still returns bare records, because a caller that wanted a
    /// list of memories before wants one now. The BM25 relevance every hit
    /// earned survives the call on the other door:
    /// [`MemoryStore::search_grouped`] returns a [`RetrievalResult`], and
    /// [`RetrievalResult::relevance`] reads it back by
    /// [`super::store::MemoryId`]. **Read that method before using the
    /// number** — it is a within-query match score, not a confidence, and it
    /// must not be thresholded.
    pub fn search(
        &self,
        text: &str,
        scope: SearchScope,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>, MemoryStoreError> {
        Ok(self
            .search_scored(text, scope, limit)?
            .into_iter()
            .map(|hit| hit.record)
            .collect())
    }

    /// [`MemoryStore::search`] with each hit's BM25 relevance still attached.
    ///
    /// The one line of [`MemoryStore::search`] that is not shared: turning
    /// free text into a `MATCH` expression. Everything after it — the SQL,
    /// the project scoping, the conflict flagging, the ladder, the decay
    /// weighting, the thin-decision demotion and the truncation — is
    /// `search_matching`, so this door and the injection door cannot rank
    /// differently.
    ///
    /// Private because [`RetrievalResult::relevance`] is the supported way to
    /// read a relevance and carries the reasons it must not be thresholded.
    /// A bare `Vec<Scored>` carries no such warning, and a second public door
    /// returning one would be a second place for the next reader to find the
    /// number without finding the caveat.
    fn search_scored(
        &self,
        text: &str,
        scope: SearchScope,
        limit: usize,
    ) -> Result<Vec<Scored>, MemoryStoreError> {
        let Some(match_expr) = sanitize_query(text) else {
            return Ok(Vec::new());
        };
        self.search_matching(&match_expr, scope, limit)
    }

    /// The whole of [`MemoryStore::search`] except the one line that turns
    /// free text into a `MATCH` expression.
    ///
    /// Split out so that the injection path can vary **only** that line. The
    /// SQL, the project scoping, the conflict flagging, the ladder, the decay
    /// weighting, the thin-decision demotion and the truncation are shared
    /// verbatim — there is exactly one ranking in this crate and both callers
    /// get it. See `injection_query` for why a second *expression* is not a
    /// second *retrieval*.
    ///
    /// [`MemoryStore::for_path`] does not come through here, because it has
    /// no `MATCH` expression to be given: it queries a different table by an
    /// exact path. It shares the parts that are about *ordering* rather than
    /// about matching — [`rank`], [`demote_thin_decisions`], [`group`] — so
    /// the "one ranking" property survives the third door.
    fn search_matching(
        &self,
        match_expr: &str,
        scope: SearchScope,
        limit: usize,
    ) -> Result<Vec<Scored>, MemoryStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let sql = format!(
            "SELECT {QUALIFIED_COLUMNS}, bm25(memories_fts) AS relevance \
             FROM memories_fts \
             JOIN memories ON memories.rowid = memories_fts.rowid \
             WHERE memories_fts MATCH ?1 \
               AND memories.project_id = ?2 \
               AND (?3 OR memories.status = ?4) \
             ORDER BY bm25(memories_fts) ASC \
             LIMIT ?5"
        );

        let mut statement =
            self.connection()
                .prepare(&sql)
                .map_err(|source| MemoryStoreError::Sql {
                    action: "prepare a memory search",
                    source,
                })?;

        let historical = matches!(scope, SearchScope::Historical);
        let fetch_limit = overfetch_limit(limit);
        let rows = statement
            .query_map(
                rusqlite::params![
                    match_expr,
                    self.project_id(),
                    historical,
                    MemoryStatus::Active.as_str(),
                    i64::try_from(fetch_limit).unwrap_or(i64::MAX),
                ],
                |row| {
                    let relevance: f64 = row.get("relevance")?;
                    Ok(row_to_record(row)?.map(|record| (record, relevance)))
                },
            )
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            .map_err(|source| MemoryStoreError::Sql {
                action: "search memories",
                source,
            })?;

        let scored: Vec<(MemoryRecord, f64)> = rows.into_iter().collect::<Result<_, _>>()?;

        // Phase 51 / producer P7: the relevance stops being discarded here.
        // Until that change the map below was `|(record, _)| record` — the
        // sort read the score, the sort threw it away, and nothing
        // downstream could ever see how well anything had matched. Pairing
        // it onto `Scored` instead changes no ordering: `rank` and
        // `demote_thin_decisions` permute the same slots by the same record
        // fields, and the relevance is carried along by the element it
        // belongs to rather than looked up afterwards.
        //
        // `Some`, not a bare number, because `Scored` also carries the hits
        // of a retrieval that asked no question — see `Scored` and
        // `MemoryStore::for_path`. Every hit here came from a `MATCH`, so
        // every one of them has a real score to carry.
        let mut hits: Vec<Scored> = scored
            .into_iter()
            .map(|(record, relevance)| Scored {
                record,
                relevance: Some(relevance),
            })
            .collect();

        self.flag_contradictions(&mut hits)?;
        rank(&mut hits, self.now());
        demote_thin_decisions(&mut hits);
        hits.truncate(limit);
        Ok(hits)
    }

    /// [`MemoryStore::search`], grouped the way Phase 21F line 929 asks:
    /// currently active invariants and constraints apart from everything
    /// else the search matched. Both groups keep `search`'s own relevance
    /// and decay order.
    ///
    /// A type, not a sort a caller has to notice: two memories can both be
    /// [`SearchScope::Current`] while one is a binding
    /// [`MemoryAuthority::Invariant`] or [`MemoryAuthority::Constraint`] and
    /// the other an ordinary [`MemoryAuthority::Decision`], and a reader
    /// must be able to tell those apart without re-deriving it from a
    /// rendered string. This is the shared core `main.rs`'s `memory_report`
    /// (the CLI's `glasshouse memory search`) and the control API's
    /// `query_memory` both render from — see that door's own doc comment for
    /// why the two must never disagree.
    pub fn search_grouped(
        &self,
        text: &str,
        scope: SearchScope,
        limit: usize,
    ) -> Result<RetrievalResult, MemoryStoreError> {
        Ok(group(self.search_scored(text, scope, limit)?))
    }

    /// [`MemoryStore::search_grouped`] for a routed task — the retrieval
    /// behind context injection, and its only caller is
    /// [`super::inject::briefing`].
    ///
    /// Identical to [`MemoryStore::search_grouped`] in every respect except
    /// the `MATCH` expression, which comes from `injection_query` rather than
    /// `sanitize_query`: prose is joined with `OR` so that a task written as a
    /// sentence retrieves at all. See `injection_query` for why the two doors
    /// differ and why this is not a second ranking.
    pub fn search_grouped_for_injection(
        &self,
        task: &str,
        scope: SearchScope,
        limit: usize,
    ) -> Result<RetrievalResult, MemoryStoreError> {
        let Some(match_expr) = injection_query(task) else {
            return Ok(RetrievalResult::default());
        };
        Ok(group(self.search_matching(&match_expr, scope, limit)?))
    }

    /// Every memory this project learned while `path` was being worked on —
    /// the read door onto migration 17's `memory_files` rows, grouped the
    /// same way [`MemoryStore::search_grouped`] groups a query's answer.
    ///
    /// `path` is repo-relative and `/`-separated; it is put through
    /// [`super::store::normalize_observed_path`] — **the same function
    /// [`MemoryStore::record_observed_files`] put the column through** — so a
    /// caller may spell it `./src//a.rs` or `src\a.rs` and still match the
    /// row the writer stored. A path that function refuses is a path no row
    /// can hold, and the answer is an empty result rather than an error:
    /// nothing was observed against a file that cannot be named here.
    ///
    /// # There is no relevance here, and none is invented
    ///
    /// This runs no `MATCH`, so [`RetrievalResult::relevance`] answers `None`
    /// for every memory it returns, and that is the true answer — the memory
    /// was not asked about. See `Scored` for why the alternative, a `0.0`,
    /// would have been a fabricated number in the one map this module keeps
    /// private to stop exactly that.
    ///
    /// # Ordering is `rank`'s, not this function's
    ///
    /// The hits go through the same `rank` and the same
    /// `demote_thin_decisions` the other two doors go through, so a memory
    /// cannot rank one way when a query found it and another way when a path
    /// did. Within a rung the ordering falls back to `retrieval_weight`
    /// alone, which is the query-blind half of the comparison a search makes
    /// — see `rank`.
    ///
    /// # What this door deliberately does not do
    ///
    /// It does not flag contradictions. That is a **write**
    /// ([`MemoryStore::mark_conflicted`]), and Phase 22 line 1063 scopes
    /// detection to *"the memories this query actually matched"* — a path
    /// lookup matched no query, and a read door that mutates the table on
    /// behalf of a caller that only asked what a file is associated with is
    /// a larger claim than this package makes. A consumer that needs
    /// conflict flagging should say so, and the argument belongs where that
    /// consumer is built.
    ///
    /// It also does not narrow by [`super::store::FileAssociation`]. Every
    /// row this build can write is `observed`; a door that filtered on the
    /// one existing value would have to be revisited by the producer that
    /// adds the second, and would read as a promise this build cannot keep.
    pub fn for_path(
        &self,
        path: &str,
        scope: SearchScope,
        limit: usize,
    ) -> Result<RetrievalResult, MemoryStoreError> {
        if limit == 0 {
            return Ok(RetrievalResult::default());
        }
        let Some(canonical) = normalize_observed_path(path) else {
            return Ok(RetrievalResult::default());
        };

        // `DISTINCT` because `memory_files` carries no uniqueness constraint
        // — migration 17 argued one would be an index on speculation — so a
        // memory associated with the same path twice must still be returned
        // once. Both `project_id` predicates are deliberate: the association
        // row's scoping is what the triggers maintain, and the memory row's
        // is what a row that reached the file by some other route would have
        // to defeat as well.
        //
        // The SQL `ORDER BY` decides only which candidates survive the
        // overfetch, never the order returned: `rank` runs in Rust for the
        // same reason `MemoryStore::search` needs it to — `retrieval_weight`
        // reads the wall clock and a per-authority policy, neither of which
        // SQLite has. Newest memory first is the honest candidate rule when
        // there is no relevance to rank candidates by.
        let sql = format!(
            "SELECT DISTINCT {QUALIFIED_COLUMNS} \
             FROM memories \
             JOIN memory_files ON memory_files.memory_id = memories.id \
             WHERE memories.project_id = ?1 \
               AND memory_files.project_id = ?1 \
               AND memory_files.path = ?2 \
               AND (?3 OR memories.status = ?4) \
             ORDER BY memories.created_at DESC, memories.id ASC \
             LIMIT ?5"
        );

        let mut statement =
            self.connection()
                .prepare(&sql)
                .map_err(|source| MemoryStoreError::Sql {
                    action: "prepare a memory path lookup",
                    source,
                })?;

        let historical = matches!(scope, SearchScope::Historical);
        let fetch_limit = overfetch_limit(limit);
        let rows = statement
            .query_map(
                rusqlite::params![
                    self.project_id(),
                    canonical,
                    historical,
                    MemoryStatus::Active.as_str(),
                    i64::try_from(fetch_limit).unwrap_or(i64::MAX),
                ],
                row_to_record,
            )
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            .map_err(|source| MemoryStoreError::Sql {
                action: "look up memories by file path",
                source,
            })?;

        let mut hits: Vec<Scored> = rows
            .into_iter()
            .map(|record| {
                record.map(|record| Scored {
                    record,
                    relevance: None,
                })
            })
            .collect::<Result<_, _>>()?;

        rank(&mut hits, self.now());
        demote_thin_decisions(&mut hits);
        hits.truncate(limit);
        Ok(group(hits))
    }

    /// Phase 22 line 1063: detect mutually contradictory current memories
    /// among `scored` and flag them, rather than returning either silently.
    ///
    /// Conservative by construction — see [`contradicts`] — and scoped to
    /// pairs within this one result set. Marking a pair
    /// [`MemoryStatus::Conflicted`] is exactly [`MemoryStore::mark_conflicted`],
    /// so both halves of Phase 22's requirement — flagging, and detecting —
    /// go through the one mechanism the store already exposes for it.
    fn flag_contradictions(&self, hits: &mut [Scored]) -> Result<(), MemoryStoreError> {
        for i in 0..hits.len() {
            for j in (i + 1)..hits.len() {
                let already_flagged = hits[i].record.status != MemoryStatus::Active
                    || hits[j].record.status != MemoryStatus::Active;
                if already_flagged || !contradicts(&hits[i].record, &hits[j].record) {
                    continue;
                }

                let one = hits[i].record.id.clone();
                let other = hits[j].record.id.clone();
                let (updated_one, updated_other) = self.mark_conflicted(&one, &other)?;
                hits[i].record = updated_one;
                hits[j].record = updated_other;
            }
        }
        Ok(())
    }
}

/// Phase 22 line 1063's conservative contradiction test: the same subject,
/// with one memory recording the subject as adopted and the other recording
/// it as abandoned.
///
/// The map's own vocabulary for this is "same subject, opposite
/// disposition," but `disposition` is a field [`super::extract::schema`]
/// reads from a model's reply and this table never persists it — see that
/// module's documentation. [`MemoryKind::Decision`] and
/// [`MemoryKind::Constraint`] are the stored proxy for "adopted," and
/// [`MemoryKind::FailedAttempt`] is the stored proxy for "abandoned": the
/// same distinction `memory::extract::authority`'s disposition ceilings
/// exist to enforce, expressed in the vocabulary this table actually keeps.
/// A memory with no subject can never be compared this way — silence is not
/// a contradiction.
fn contradicts(a: &MemoryRecord, b: &MemoryRecord) -> bool {
    let (Some(subject_a), Some(subject_b)) = (a.subject.as_deref(), b.subject.as_deref()) else {
        return false;
    };
    if normalize_subject(subject_a) != normalize_subject(subject_b) {
        return false;
    }

    matches!(
        (a.kind, b.kind),
        (
            MemoryKind::Decision | MemoryKind::Constraint,
            MemoryKind::FailedAttempt
        ) | (
            MemoryKind::FailedAttempt,
            MemoryKind::Decision | MemoryKind::Constraint
        )
    )
}

/// A subject compared for contradiction, case- and whitespace-insensitively.
/// Conservative in the same direction [`sanitize_query`] is: two subjects
/// that differ in wording are never treated as the same one.
fn normalize_subject(subject: &str) -> String {
    subject.trim().to_lowercase()
}

/// Phase 21B: *"treat a decision with missing rationale and missing
/// assumptions as lower-confidence than a well-proven decision of the same
/// authority class"*.
///
/// # Why this is a permutation and not an `ORDER BY`
///
/// The obvious implementation — sorting thin decisions to the bottom of the
/// whole result set — reads the line as *"lower-confidence than
/// everything"*, which is not what it says and would be a real search
/// regression: a perfectly relevant decision would fall behind a
/// barely-relevant memory of some unrelated kind. The line has two
/// qualifiers and both are load-bearing. It compares a decision against **a
/// decision**, and against one **of the same authority class**.
///
/// So the relevance order BM25 produced is left almost entirely alone: every
/// record that is not a [`MemoryKind::Decision`] keeps its position exactly,
/// and so does every authority class as a whole. The only thing that moves
/// is the order of the decisions *within* one authority class, where a
/// decision that recorded neither why it was made nor what it assumed is put
/// behind one that did.
///
/// A search returning one decision is therefore unchanged, and so is a
/// search returning a decision and a finding. A search returning two
/// `decision`-class decisions puts the better-proven one first however the
/// text happened to match.
///
/// Unclassified memories (`authority IS NULL`) form their own group, because
/// `None` is a distinct fact from every class and not a class to merge into.
///
/// The sort is stable, so two decisions that are both thin, or both
/// well-proven, keep their BM25 order relative to each other.
///
/// Operates on [`Scored`] rather than bare records so that a memory keeps the
/// relevance it earned when this permutation moves it. The permutation reads
/// only `authority`, `kind` and [`MemoryRecord::is_lower_confidence_decision`]
/// — never the relevance — so attaching the score changed no ordering.
fn demote_thin_decisions(hits: &mut [Scored]) {
    let classes: Vec<Option<MemoryAuthority>> = {
        let mut seen: Vec<Option<MemoryAuthority>> = Vec::new();
        for hit in hits.iter() {
            if !seen.contains(&hit.record.authority) {
                seen.push(hit.record.authority);
            }
        }
        seen
    };

    for class in classes {
        let slots: Vec<usize> = hits
            .iter()
            .enumerate()
            .filter(|(_, hit)| {
                hit.record.authority == class && hit.record.kind == MemoryKind::Decision
            })
            .map(|(index, _)| index)
            .collect();
        if slots.len() < 2 {
            continue;
        }

        let mut ordered = slots.clone();
        ordered.sort_by_key(|&index| hits[index].record.is_lower_confidence_decision());
        // The whole `Scored` moves, not the record out of it. A permutation
        // that reassigned scores to positions would leave every memory
        // holding the relevance of whichever memory used to sit where it
        // landed — a number that is real, plausible, and about a different
        // query result.
        let moved: Vec<Scored> = ordered
            .into_iter()
            .map(|index| hits[index].clone())
            .collect();
        for (slot, hit) in slots.into_iter().zip(moved) {
            hits[slot] = hit;
        }
    }
}
