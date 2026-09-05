//! The concise current-project snapshot agents ask for (Phase 26).
//!
//! Only [`MemoryStatus::Active`] memories are current: a todo whose status
//! is [`MemoryStatus::NeedsReview`] or [`MemoryStatus::Conflicted`] is still
//! open work by [`MemoryStatus::is_open_work`], but this snapshot is what an
//! agent treats as settled project knowledge, so every section here holds
//! `Active` memories and nothing else — every other status stays in the
//! database, queryable by id or by [`super::MemoryStore::with_status`], and
//! simply does not appear.
//!
//! [`snapshot`] takes a [`SnapshotBudget`] and honours it on every section
//! independently — a per-section entry cap and a per-entry body length, so a
//! project with five thousand memories and one with fifty produce
//! same-sized output — and nothing is silently dropped: a section that hit
//! its cap reports how many entries it left out
//! ([`SnapshotSection::omitted`]), and a cut body records that it was
//! ([`SnapshotEntry::body_truncated`]).
//!
//! History: design-decisions.md, "Trims: memory and session module docs", memory/snapshot.rs module doc.

use rusqlite::OptionalExtension;

use super::store::{
    ALL_COLUMNS, MemoryAuthority, MemoryId, MemoryKind, MemoryRecord, MemoryStatus, MemoryStore,
    MemoryStoreError, row_to_record,
};

/// How much a [`snapshot`] may return, per section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotBudget {
    /// The most entries any one [`SnapshotSection`] returns.
    pub per_section_limit: usize,
    /// The most characters any one [`SnapshotEntry::body`] carries. Measured
    /// in `char`s, not bytes, so a multi-byte character is never split.
    pub max_body_chars: usize,
}

impl SnapshotBudget {
    pub fn new(per_section_limit: usize, max_body_chars: usize) -> Self {
        Self {
            per_section_limit,
            max_body_chars,
        }
    }
}

impl Default for SnapshotBudget {
    /// Fits comfortably in an agent's context regardless of how many
    /// memories the project has accumulated: six sections of ten entries
    /// each, each entry's body capped well short of a paragraph.
    fn default() -> Self {
        Self {
            per_section_limit: 10,
            max_body_chars: 280,
        }
    }
}

/// One current memory, presented with its budget applied and its provenance
/// intact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotEntry {
    pub id: MemoryId,
    pub subject: Option<String>,
    /// The body, cut to [`SnapshotBudget::max_body_chars`] if it was longer.
    pub body: String,
    /// Whether [`Self::body`] is shorter than what was actually stored.
    pub body_truncated: bool,
    /// `None` means unclassified — a distinct fact from every authority
    /// class, and never rendered as one. See [`MemoryAuthority`].
    pub authority: Option<MemoryAuthority>,
    pub source_session_id: Option<String>,
    pub source_commit: Option<String>,
}

impl SnapshotEntry {
    fn from_record(record: MemoryRecord, budget: &SnapshotBudget) -> Self {
        let body_truncated = record.body.chars().count() > budget.max_body_chars;
        let body = if body_truncated {
            record.body.chars().take(budget.max_body_chars).collect()
        } else {
            record.body
        };
        Self {
            id: record.id,
            subject: record.subject,
            body,
            body_truncated,
            authority: record.authority,
            source_session_id: record.source_session_id,
            source_commit: record.source_commit,
        }
    }
}

/// One [`MemoryKind`]'s current entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotSection {
    pub kind: MemoryKind,
    /// Most recently updated first, capped at
    /// [`SnapshotBudget::per_section_limit`].
    pub entries: Vec<SnapshotEntry>,
    /// How many additional current entries of this kind exist beyond
    /// [`Self::entries`]. Zero means nothing was left out.
    pub omitted: usize,
}

/// The concise current-project snapshot — Phase 26's `memory.snapshot`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// One section per [`MemoryKind`], in the kind's schema order. Every
    /// kind is present even when its section is empty, so a caller never has
    /// to guess whether a missing section means "empty" or "not queried".
    pub sections: Vec<SnapshotSection>,
}

impl Snapshot {
    /// The section for one kind.
    ///
    /// Never `None` in practice — [`snapshot`] builds one section per
    /// [`MemoryKind::ALL`] entry — but a caller should not have to `unwrap`
    /// to ask a question this type already answers structurally.
    pub fn section(&self, kind: MemoryKind) -> Option<&SnapshotSection> {
        self.sections.iter().find(|section| section.kind == kind)
    }
}

/// Group every current memory in this project by kind, most recently
/// updated first within each kind, honouring `budget`.
///
/// Runs entirely on the connection `store` already holds open, which
/// [`super::ProjectMemory::open`] bound to the active project — there is no
/// path argument and no project id argument to reach anywhere else.
pub fn snapshot(
    store: &MemoryStore<'_>,
    budget: &SnapshotBudget,
) -> Result<Snapshot, MemoryStoreError> {
    let sections = MemoryKind::ALL
        .iter()
        .copied()
        .map(|kind| section_for(store, kind, budget))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Snapshot { sections })
}

fn section_for(
    store: &MemoryStore<'_>,
    kind: MemoryKind,
    budget: &SnapshotBudget,
) -> Result<SnapshotSection, MemoryStoreError> {
    // A full match, not a lookup table keyed by string: a `MemoryKind`
    // variant that reaches here without a case is a compile error, not a
    // section that silently never appears.
    match kind {
        MemoryKind::Decision
        | MemoryKind::Constraint
        | MemoryKind::Feature
        | MemoryKind::Finding
        | MemoryKind::FailedAttempt
        | MemoryKind::Todo => {}
    }

    let total = count_active(store, kind)?;
    let records = fetch_active(store, kind, budget.per_section_limit)?;
    let omitted = total.saturating_sub(records.len());
    let entries = records
        .into_iter()
        .map(|record| SnapshotEntry::from_record(record, budget))
        .collect();

    Ok(SnapshotSection {
        kind,
        entries,
        omitted,
    })
}

fn count_active(store: &MemoryStore<'_>, kind: MemoryKind) -> Result<usize, MemoryStoreError> {
    let count: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE project_id = ?1 AND kind = ?2 AND status = ?3",
            rusqlite::params![
                store.project_id(),
                kind.as_str(),
                MemoryStatus::Active.as_str()
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(|source| MemoryStoreError::Sql {
            action: "count current memories for a snapshot section",
            source,
        })?
        .unwrap_or(0);
    Ok(usize::try_from(count).unwrap_or(0))
}

fn fetch_active(
    store: &MemoryStore<'_>,
    kind: MemoryKind,
    limit: usize,
) -> Result<Vec<MemoryRecord>, MemoryStoreError> {
    let mut statement = store
        .connection()
        .prepare(&format!(
            "SELECT {ALL_COLUMNS} FROM memories \
             WHERE project_id = ?1 AND kind = ?2 AND status = ?3 \
             ORDER BY updated_at DESC, id ASC LIMIT ?4"
        ))
        .map_err(|source| MemoryStoreError::Sql {
            action: "prepare a snapshot section query",
            source,
        })?;
    let rows = statement
        .query_map(
            rusqlite::params![
                store.project_id(),
                kind.as_str(),
                MemoryStatus::Active.as_str(),
                i64::try_from(limit).unwrap_or(i64::MAX),
            ],
            row_to_record,
        )
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        .map_err(|source| MemoryStoreError::Sql {
            action: "read a snapshot section",
            source,
        })?;
    rows.into_iter().collect()
}
