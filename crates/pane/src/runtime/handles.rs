//! The table of a task's live handles — `runtime-contract.md` §2.
//!
//! **A handle is freed only by redeclaration, an explicit [`HandleTable::free`],
//! or [`HandleTable::end_task`].** That is not a rule this module remembers to
//! follow; it is the only three `&mut self` operations [`HandleTable`] has that
//! can shrink its set of live names. [`render_table`] — the operation that may
//! drop entries from a turn's rendering when the table is over budget — takes
//! `&HandleTable`, a shared reference, so there is no path by which rendering
//! could free anything: the borrow checker refuses it, not a comment.

use crate::runtime::preview::{self, Value};

struct HandleEntry {
    name: String,
    value: Value,
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

    pub fn is_live(&self, name: &str) -> bool {
        self.entries.iter().any(|e| e.name == name)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
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

fn render_entry(entry: &HandleEntry, cap: usize) -> String {
    let name = preview::escape_line(&entry.name);
    let type_name = preview::type_name(&entry.value);
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
            table.declare(format!("h{i}"), Value::String(long.clone()), i);
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
}
