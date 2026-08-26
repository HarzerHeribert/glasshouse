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
                                 memories.source_excerpt";

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
    pub fn search(
        &self,
        text: &str,
        scope: SearchScope,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>, MemoryStoreError> {
        let Some(match_expr) = sanitize_query(text) else {
            return Ok(Vec::new());
        };

        let sql = format!(
            "SELECT {QUALIFIED_COLUMNS} \
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
        let rows = statement
            .query_map(
                rusqlite::params![
                    match_expr,
                    self.project_id(),
                    historical,
                    MemoryStatus::Active.as_str(),
                    i64::try_from(limit).unwrap_or(i64::MAX),
                ],
                row_to_record,
            )
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            .map_err(|source| MemoryStoreError::Sql {
                action: "search memories",
                source,
            })?;

        let mut records: Vec<MemoryRecord> = rows.into_iter().collect::<Result<_, _>>()?;
        demote_thin_decisions(&mut records);
        Ok(records)
    }
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
