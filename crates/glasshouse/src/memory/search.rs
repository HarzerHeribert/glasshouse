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

use super::policy::retrieval_weight;
use super::store::{
    MemoryAuthority, MemoryKind, MemoryRecord, MemoryStatus, MemoryStore, MemoryStoreError,
    row_to_record,
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetrievalResult {
    /// Currently active invariants and constraints, in the relevance/decay
    /// order [`MemoryStore::search`] produced them.
    pub invariants_and_constraints: Vec<MemoryRecord>,
    /// Everything else the search matched — ordinary current memories, and,
    /// under [`SearchScope::Historical`], history — including a memory whose
    /// authority is invariant or constraint but that is no longer active,
    /// which is history rather than a current rule.
    pub other: Vec<MemoryRecord>,
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
                                 memories.review_marked_at, memories.last_validated_at";

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
    pub fn search(
        &self,
        text: &str,
        scope: SearchScope,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>, MemoryStoreError> {
        let Some(match_expr) = sanitize_query(text) else {
            return Ok(Vec::new());
        };
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

        let mut scored: Vec<(MemoryRecord, f64)> = rows.into_iter().collect::<Result<_, _>>()?;

        self.flag_contradictions(&mut scored)?;

        let now = self.now();
        scored.sort_by(|(a, a_relevance), (b, b_relevance)| {
            let a_weight = retrieval_weight(
                a.authority,
                now,
                a.created_at,
                a.last_validated_at,
                a.provenance.project_phase,
            );
            let b_weight = retrieval_weight(
                b.authority,
                now,
                b.created_at,
                b.last_validated_at,
                b.provenance.project_phase,
            );
            let a_score = *a_relevance * a_weight;
            let b_score = *b_relevance * b_weight;
            a_score
                .partial_cmp(&b_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut records: Vec<MemoryRecord> = scored.into_iter().map(|(record, _)| record).collect();
        demote_thin_decisions(&mut records);
        records.truncate(limit);
        Ok(records)
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
        let records = self.search(text, scope, limit)?;
        let mut grouped = RetrievalResult::default();
        for record in records {
            if is_current_invariant_or_constraint(&record) {
                grouped.invariants_and_constraints.push(record);
            } else {
                grouped.other.push(record);
            }
        }
        Ok(grouped)
    }

    /// Phase 22 line 1063: detect mutually contradictory current memories
    /// among `scored` and flag them, rather than returning either silently.
    ///
    /// Conservative by construction — see [`contradicts`] — and scoped to
    /// pairs within this one result set. Marking a pair
    /// [`MemoryStatus::Conflicted`] is exactly [`MemoryStore::mark_conflicted`],
    /// so both halves of Phase 22's requirement — flagging, and detecting —
    /// go through the one mechanism the store already exposes for it.
    fn flag_contradictions(
        &self,
        scored: &mut [(MemoryRecord, f64)],
    ) -> Result<(), MemoryStoreError> {
        for i in 0..scored.len() {
            for j in (i + 1)..scored.len() {
                let already_flagged = scored[i].0.status != MemoryStatus::Active
                    || scored[j].0.status != MemoryStatus::Active;
                if already_flagged || !contradicts(&scored[i].0, &scored[j].0) {
                    continue;
                }

                let one = scored[i].0.id.clone();
                let other = scored[j].0.id.clone();
                let (updated_one, updated_other) = self.mark_conflicted(&one, &other)?;
                scored[i].0 = updated_one;
                scored[j].0 = updated_other;
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
fn demote_thin_decisions(records: &mut [MemoryRecord]) {
    let classes: Vec<Option<MemoryAuthority>> = {
        let mut seen: Vec<Option<MemoryAuthority>> = Vec::new();
        for record in records.iter() {
            if !seen.contains(&record.authority) {
                seen.push(record.authority);
            }
        }
        seen
    };

    for class in classes {
        let slots: Vec<usize> = records
            .iter()
            .enumerate()
            .filter(|(_, record)| record.authority == class && record.kind == MemoryKind::Decision)
            .map(|(index, _)| index)
            .collect();
        if slots.len() < 2 {
            continue;
        }

        let mut ordered = slots.clone();
        ordered.sort_by_key(|&index| records[index].is_lower_confidence_decision());
        let moved: Vec<MemoryRecord> = ordered
            .into_iter()
            .map(|index| records[index].clone())
            .collect();
        for (slot, record) in slots.into_iter().zip(moved) {
            records[slot] = record;
        }
    }
}
