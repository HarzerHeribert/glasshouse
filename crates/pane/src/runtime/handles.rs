//! The table of a task's live handles — `runtime-contract.md` §2.
//!
//! **A handle is freed only by redeclaration, an explicit [`HandleTable::free`],
//! or [`HandleTable::end_task`].** That is not a rule this module remembers to
//! follow; it is the only three `&mut self` operations [`HandleTable`] has that
//! can shrink its set of live names. [`render_table`] — the operation that may
//! drop entries from a turn's rendering when the table is over budget — takes
//! `&HandleTable`, a shared reference, so there is no path by which rendering
//! could free anything: the borrow checker refuses it, not a comment.

use serde::Serialize;
use std::collections::BTreeMap;

use crate::runtime::preview::{self, Value};

/// Which recorded tool call produced a handle — `runtime-contract.md` §4's
/// `provenance` object.
///
/// `pure` is copied from the tool's own declaration
/// ([`crate::tools::registry::Purity`]) and is never inferred from the call,
/// the arguments or the result: §4 makes re-materialising a stale handle
/// depend on it, so a guess here would silently re-run something with an
/// effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Provenance {
    pub tool: String,
    pub args: BTreeMap<String, String>,
    pub sha256: String,
    pub pure: bool,
}

/// What the runtime knows about a handle beyond its preview.
///
/// Kept beside the value rather than inside it because none of it is
/// rendered by `preview.rs`'s type rules: the type label replaces the
/// structural name in the table header (`Grep.Match[]`, not `Array`), the
/// size estimate only ever answers §2's "five largest live handles" question
/// when the heap ceiling is hit, and the provenance is §4's rollout field.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HandleMeta {
    /// The declared type as the model's own tool signature spells it, when
    /// the producer had one. `None` falls back to
    /// [`preview::type_name`].
    pub type_label: Option<String>,
    /// A cheap marshalled estimate of the live object's size, in bytes.
    /// **Not a retained size** — V8 offers no per-object retained size
    /// without a heap snapshot, and taking one to answer an out-of-memory
    /// error would allocate at exactly the wrong moment.
    pub size_estimate: u64,
    pub provenance: Option<Provenance>,
}

struct HandleEntry {
    name: String,
    value: Value,
    meta: HandleMeta,
    /// Set when this declaration replaced a live handle of the same name;
    /// carries the cell the replacement happened in, for `render_table`'s
    /// `(replaced at cell N)` annotation — `runtime-contract.md` §2.
    replaced_at_cell: Option<u64>,
}

/// A task's live handles, in declaration order. Redeclaring a name removes
/// its old entry and appends a new one, so "declaration order" always means
/// "most recent declaration or redeclaration order" — which is also the
/// order [`render_table`] drops from, oldest first.
#[derive(Default)]
pub struct HandleTable {
    entries: Vec<HandleEntry>,
}

impl HandleTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// A top-level binding, a cell's yielded value, or an explicit `keep` —
    /// `runtime-contract.md` §2's three ways a value becomes a handle. If
    /// `name` already names a live handle, that handle is freed immediately
    /// and the new one's next rendering carries `(replaced at cell N)`.
    pub fn declare(&mut self, name: impl Into<String>, value: Value, cell: u64) {
        self.declare_with(name, value, cell, HandleMeta::default());
    }

    /// [`HandleTable::declare`], carrying what the isolate knows about the
    /// live object behind the preview.
    pub fn declare_with(
        &mut self,
        name: impl Into<String>,
        value: Value,
        cell: u64,
        meta: HandleMeta,
    ) {
        let name = name.into();
        let replaced_at_cell = if let Some(pos) = self.entries.iter().position(|e| e.name == name) {
            self.entries.remove(pos);
            Some(cell)
        } else {
            None
        };
        self.entries.push(HandleEntry {
            name,
            value,
            meta,
            replaced_at_cell,
        });
    }

    /// The model's own `free("name")`. Freeing a name that is not live is a
    /// no-op: there is nothing for the model to have gotten wrong.
    pub fn free(&mut self, name: &str) {
        self.entries.retain(|e| e.name != name);
    }

    /// The task ending frees every handle no earlier event already freed —
    /// `runtime-contract.md` §2's third and last lifetime event.
    pub fn end_task(&mut self) {
        self.entries.clear();
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.entries
            .iter()
            .find(|e| e.name == name)
            .map(|e| &e.value)
    }

    pub fn meta(&self, name: &str) -> Option<&HandleMeta> {
        self.entries
            .iter()
            .find(|e| e.name == name)
            .map(|e| &e.meta)
    }

    pub fn is_live(&self, name: &str) -> bool {
        self.entries.iter().any(|e| e.name == name)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every live name, in declaration order — what the model's `handles()`
    /// answers, and what §3's drop note promises is still there.
    pub fn names(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.name.as_str()).collect()
    }

    /// The `n` live handles with the largest [`HandleMeta::size_estimate`],
    /// largest first. `runtime-contract.md` §2 shows these in the
    /// `RuntimeOutOfMemory` preview so the *model* can choose what to free;
    /// **this function frees nothing**, and it takes `&self` so it cannot.
    pub fn largest(&self, n: usize) -> Vec<(&str, u64)> {
        let mut sized: Vec<(&str, u64)> = self
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.meta.size_estimate))
            .collect();
        sized.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        sized.truncate(n);
        sized
    }

    /// Every live handle as `(name, type label, preview body, provenance)`,
    /// in declaration order — the rollout's `handles` array
    /// (`runtime-contract.md` §4), assembled by the caller that owns the
    /// record's shape.
    pub fn rows(&self, entry_cap: usize) -> Vec<(String, String, String, Option<Provenance>)> {
        self.entries
            .iter()
            .map(|entry| {
                (
                    entry.name.clone(),
                    type_label(entry).to_string(),
                    preview::render_preview(&entry.value, entry_cap),
                    entry.meta.provenance.clone(),
                )
            })
            .collect()
    }
}

/// Renders every live handle as one entry in declaration order, each entry's
/// body shrunk to fit `entry_cap` tokens by [`preview::render_preview`]. If
/// the whole table would exceed `table_cap` tokens, entries are dropped from
/// the *rendering* oldest-first — this takes `&HandleTable`, so nothing it
/// does can free a handle — and one line names how many were not shown.
pub fn render_table(table: &HandleTable, entry_cap: usize, table_cap: usize) -> String {
    let rendered: Vec<String> = table
        .entries
        .iter()
        .map(|entry| render_entry(entry, entry_cap))
        .collect();

    // Try showing every entry, then all but the oldest, then all but the two
    // oldest, and so on, until what remains fits -- or nothing does, in
    // which case every entry is dropped from the rendering.
    for dropped in 0..=rendered.len() {
        let visible = rendered[dropped..].join("\n\n");
        if dropped == rendered.len() || preview::estimate_tokens(&visible) <= table_cap {
            return with_drop_note(dropped, &visible);
        }
    }
    unreachable!("the loop always returns at dropped == rendered.len()")
}

fn with_drop_note(dropped: usize, visible: &str) -> String {
    if dropped == 0 {
        return visible.to_string();
    }
    let note = format!("…{dropped} older handles not shown; call handles() for the full list");
    if visible.is_empty() {
        note
    } else {
        format!("{note}\n\n{visible}")
    }
}

fn type_label(entry: &HandleEntry) -> &str {
    entry
        .meta
        .type_label
        .as_deref()
        .unwrap_or_else(|| preview::type_name(&entry.value))
}

fn render_entry(entry: &HandleEntry, cap: usize) -> String {
    let name = preview::escape_line(&entry.name);
    let type_name = preview::escape_line(type_label(entry));
    let mut header = format!("{name}  {type_name}");
    if let Some(cell) = entry.replaced_at_cell {
        header.push_str(&format!("  (replaced at cell {cell})"));
    }
    let body = preview::render_preview(&entry.value, cap);
    if body.is_empty() {
        header
    } else {
        format!("{header}\n{body}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::preview::Value;

    #[test]
    fn a_handle_is_freed_only_by_redeclaration_free_or_task_end() {
        let mut table = HandleTable::new();

        table.declare("x", Value::Number(1.0), 1);
        assert!(table.is_live("x"));

        // Redeclaration replaces: still exactly one "x", carrying the new value.
        table.declare("x", Value::Number(2.0), 2);
        assert_eq!(table.len(), 1);
        assert_eq!(table.get("x"), Some(&Value::Number(2.0)));

        table.declare("y", Value::Boolean(true), 3);
        table.free("y");
        assert!(!table.is_live("y"));

        table.declare("z", Value::Null, 4);
        table.end_task();
        assert!(!table.is_live("x"));
        assert!(!table.is_live("z"));
        assert!(table.is_empty());
    }

    #[test]
    fn a_redeclaration_announces_the_cell_it_happened_in() {
        let mut table = HandleTable::new();
        table.declare("x", Value::Number(1.0), 1);
        table.declare("x", Value::Number(2.0), 5);

        let rendered = render_table(&table, preview::PREVIEW_TOKEN_CAP, preview::TABLE_TOKEN_CAP);
        assert!(rendered.contains("x  number  (replaced at cell 5)"));
    }

    #[test]
    fn a_table_over_the_cap_drops_renderings_and_frees_nothing() {
        let mut table = HandleTable::new();
        let long = "y".repeat(100);
        for i in 0..5u64 {
            table.declare(format!("h{i}"), Value::string(&long), i);
        }

        // Each entry alone is well under the per-preview cap, but five of
        // them together are not under a small table cap.
        let rendered = render_table(&table, preview::PREVIEW_TOKEN_CAP, 60);

        assert!(rendered.starts_with("…"));
        assert!(rendered.contains("older handles not shown"));
        // The newest handle survives the drop; an old one does not appear.
        assert!(rendered.contains("h4"));
        assert!(!rendered.contains("h0  string"));

        // Nothing was freed: every handle the rendering dropped is still live.
        for i in 0..5u64 {
            assert!(table.is_live(&format!("h{i}")), "h{i} must still be live");
        }
    }

    #[test]
    fn a_handle_table_with_no_entries_renders_empty() {
        let table = HandleTable::new();
        let rendered = render_table(&table, preview::PREVIEW_TOKEN_CAP, preview::TABLE_TOKEN_CAP);
        assert_eq!(rendered, "");
    }

    /// The declared type replaces the structural one in the header, so the
    /// model reads the name its own tool signature used.
    #[test]
    fn a_declared_type_label_is_what_the_header_shows() {
        let mut table = HandleTable::new();
        table.declare_with(
            "hits",
            Value::array(vec![Value::Number(1.0)]),
            1,
            HandleMeta {
                type_label: Some("Grep.Match[]".into()),
                ..HandleMeta::default()
            },
        );
        let rendered = render_table(&table, preview::PREVIEW_TOKEN_CAP, preview::TABLE_TOKEN_CAP);
        assert!(rendered.starts_with("hits  Grep.Match[]"), "{rendered}");
    }

    #[test]
    fn the_largest_handles_are_ranked_and_nothing_is_freed_to_find_them() {
        let mut table = HandleTable::new();
        for (name, size) in [("a", 10u64), ("b", 900), ("c", 50)] {
            table.declare_with(
                name,
                Value::Number(0.0),
                1,
                HandleMeta {
                    size_estimate: size,
                    ..HandleMeta::default()
                },
            );
        }
        assert_eq!(table.largest(2), vec![("b", 900), ("c", 50)]);
        assert_eq!(table.len(), 3);
        assert_eq!(table.names(), vec!["a", "b", "c"]);
    }
}
