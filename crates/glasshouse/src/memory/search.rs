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
//! # BM25 direction
//!
//! SQLite's `bm25()` returns a *more negative* number for a *better* match.
//! `ORDER BY bm25(memories_fts) ASC` therefore puts the best match first —
//! this is asserted directly in the integration tests rather than trusted by
//! reading the manual once.

use super::store::{MemoryRecord, MemoryStatus, MemoryStore, MemoryStoreError, row_to_record};

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
                                 memories.source_commit, memories.superseded_by, \
                                 memories.created_at, memories.updated_at";

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

        rows.into_iter().collect()
    }
}
