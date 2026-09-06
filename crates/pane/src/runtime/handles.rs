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
    /// The whole entry, already rendered by its own producer, for a value
    /// whose preview `runtime-contract.md` §3's type rules do not describe.
    ///
    /// There is exactly one such value and it is the reason this field
    /// exists: `events-contract.md` §3 fixes the batch's preview — every
    /// interrupt in full, then counts by kind, then samples rarest-kind-first
    /// — which is not any of §3's types and would come out of the `string`
    /// rule as `len=…` and its first 200 characters. `Events.Batch` renders
    /// itself, already capped by the same estimator, and this carries it
    /// through unchanged. `None` for every other handle, which is every
    /// handle a model's own program ever binds.
    rendered: Option<String>,
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
            rendered: None,
        });
    }

    /// [`HandleTable::declare_with`] for a producer that rendered its own
    /// entry — today only the `batch` handle `events-contract.md` §4
    /// declares, whose preview is that document's §3 rather than
    /// `runtime-contract.md` §3's type rules.
    ///
    /// It is [`HandleTable::declare_with`] in every other respect, and
    /// deliberately so: the batch is a handle, so it is replaced, freed,
    /// counted and ordered exactly as one, and being **last** in the table is
    /// a consequence of `declare` appending rather than of a rule about
    /// batches.
    pub fn declare_rendered(
        &mut self,
        name: impl Into<String>,
        value: Value,
        cell: u64,
        meta: HandleMeta,
        entry: String,
    ) {
        let name = name.into();
        self.declare_with(name.clone(), value, cell, meta);
        if let Some(last) = self.entries.last_mut() {
            debug_assert_eq!(last.name, name);
            last.rendered = Some(entry);
        }
    }

    /// Takes a live handle's preview and size again, in place —
    /// `runtime-contract.md` §3's "the preview shows the value's type and
    /// size **as it now is**", for a handle the current cell never bound.
    ///
    /// The invariant this module is built on is untouched: this can neither
    /// add nor remove a live name, and it leaves the entry's position and its
    /// `(replaced at cell N)` annotation exactly as they were, so a refresh is
    /// invisible to everything but the preview and §2's ranking.
    /// [`HandleTable::declare_with`] cannot do this job — it removes and
    /// re-appends, which would reorder the table by refresh instead of by
    /// declaration, and it stamps `replaced_at_cell`, which means the model
    /// redeclared the name.
    pub fn refresh(&mut self, name: &str, value: Value, meta: HandleMeta) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.name == name) {
            entry.value = value;
            entry.meta = meta;
        }
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
                    entry
                        .rendered
                        .clone()
                        .unwrap_or_else(|| preview::render_preview(&entry.value, entry_cap)),
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
    let name_width = table
        .entries
        .iter()
        .map(|e| e.name.chars().count())
        .max()
        .unwrap_or(0)
        + NAME_COLUMN_GAP;
    let rendered: Vec<String> = table
        .entries
        .iter()
        .map(|entry| render_entry(entry, entry_cap, name_width))
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

/// Appends `annotation` to the first line of `text`, leaving every later
/// line alone — a pre-rendered entry's header is its first line.
fn annotate_first_line(text: &str, annotation: &str) -> String {
    match text.split_once('\n') {
        Some((head, rest)) => format!("{head}{annotation}\n{rest}"),
        None => format!("{text}{annotation}"),
    }
}

fn type_label(entry: &HandleEntry) -> &str {
    entry
        .meta
        .type_label
        .as_deref()
        .unwrap_or_else(|| preview::type_name(&entry.value))
}

/// Trailing spaces every header's name field carries beyond the widest live
/// name in the table — `model-contract.md` §7's `hits`/`adapter` column:
/// `hits` (4 chars) pads to width 9 alongside `adapter` (7 chars), so the
/// gap here is 2.
const NAME_COLUMN_GAP: usize = 2;

/// The fixed number of spaces separating a header's fields after the type
/// label. Not padded to a shared column across entries — unlike the name
/// field, type labels vary too widely in length (`File` vs `Grep.Match[]`)
/// for a shared column to read cleanly — `model-contract.md` §7.
const HEADER_FIELD_GAP: usize = 3;

/// Builds one handle's header and body — `model-contract.md` §7's exact
/// shape, the only place that shape is built (box line 2465). The header
/// always carries the name (padded to `name_width`, the widest live name in
/// the table) and the type label; what follows depends on the value:
///
/// - an `Array` carries `n=<len>` then `inline cost ~<N> tok · preview <M>
///   tok` on the header itself, and the element rows are the body;
/// - a `File` carries `<path>   <bytes> B · <lines> lines · <mtime>` on the
///   header — its own byte count already states the size, so no `inline
///   cost` repeats it — and the preview's own token count is appended to
///   the last `L#` line of the body instead, padded by [`HEADER_FIELD_GAP`];
/// - every other type carries `inline cost ~<N> tok · preview <M> tok` on
///   the header, and its body is unchanged from [`preview::render_preview`].
///
/// `inline cost` is [`preview::tokens_for_bytes`] over
/// [`HandleMeta::size_estimate`] — the bytes the isolate recorded for the
/// call that produced this handle, before anything was parsed into a
/// structured value. `preview` is [`preview::estimate_tokens`] over the
/// rendered preview body itself, computed after rendering so it reflects
/// whatever cap-driven shrinking already happened.
fn render_entry(entry: &HandleEntry, cap: usize, name_width: usize) -> String {
    // A producer that rendered its own entry has already produced the whole
    // shape -- header line included -- so nothing is built around it here;
    // the replacement annotation is the one thing this table knows and the
    // producer cannot (`runtime-contract.md` §2).
    if let Some(rendered) = &entry.rendered {
        return match entry.replaced_at_cell {
            Some(cell) => annotate_first_line(rendered, &format!("  (replaced at cell {cell})")),
            None => rendered.clone(),
        };
    }
    let name = preview::escape_line(&entry.name);
    let type_name = preview::escape_line(type_label(entry));
    let gap = " ".repeat(HEADER_FIELD_GAP);

    let mut header = format!("{name:<name_width$}{type_name}");
    if let Some(cell) = entry.replaced_at_cell {
        header.push_str(&format!("  (replaced at cell {cell})"));
    }

    let inline_cost = preview::tokens_for_bytes(entry.meta.size_estimate);

    match &entry.value {
        Value::Array(array) => {
            let body = preview::render_preview(&entry.value, cap);
            let preview_tokens = preview::estimate_tokens(&body);
            header.push_str(&gap);
            header.push_str(&format!(
                "n={}{gap}inline cost ~{} tok · preview {} tok",
                array.len(),
                preview::thousands(inline_cost as u64),
                preview::thousands(preview_tokens as u64),
            ));
            // `array_elements_only` prefixes every element with its own
            // `\n`, the same convention `render_preview`'s `n=`+elements
            // join relies on -- no extra separator here, or a blank line
            // would appear between the header and `[0]`.
            let elements = preview::array_elements_only(array, cap);
            format!("{header}{elements}")
        }
        Value::File(file) => {
            header.push_str(&gap);
            header.push_str(&format!(
                "{}{gap}{} B · {} lines · {}",
                preview::escape_line(&file.path),
                preview::thousands(file.byte_len),
                preview::thousands(file.line_count),
                preview::escape_line(&file.mtime),
            ));
            let body = preview::render_preview(&entry.value, cap);
            let preview_tokens = preview::estimate_tokens(&body);
            let annotation = format!(
                "{gap}preview {} tok",
                preview::thousands(preview_tokens as u64)
            );
            let mut lines = preview::file_lines_only(file);
            match lines.last_mut() {
                Some(last) => last.push_str(&annotation),
                None => header.push_str(&annotation),
            }
            if lines.is_empty() {
                header
            } else {
                format!("{header}\n{}", lines.join("\n"))
            }
        }
        _ => {
            let body = preview::render_preview(&entry.value, cap);
            let preview_tokens = preview::estimate_tokens(&body);
            header.push_str(&gap);
            header.push_str(&format!(
                "inline cost ~{} tok · preview {} tok",
                preview::thousands(inline_cost as u64),
                preview::thousands(preview_tokens as u64),
            ));
            if body.is_empty() {
                header
            } else {
                format!("{header}\n{body}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::preview::Value;

    /// The header carries the declared type, the array's own length, and
    /// both token figures — `model-contract.md` §7's `hits` line shape.
    #[test]
    fn an_entry_header_carries_the_type_the_length_and_both_token_figures() {
        let mut table = HandleTable::new();
        table.declare_with(
            "hits",
            Value::array(vec![Value::Number(1.0), Value::Number(2.0)]),
            1,
            HandleMeta {
                type_label: Some("Grep.Match[]".into()),
                size_estimate: 40,
                ..HandleMeta::default()
            },
        );
        let rendered = render_table(&table, preview::PREVIEW_TOKEN_CAP, preview::TABLE_TOKEN_CAP);
        let header = rendered.lines().next().unwrap();
        assert!(
            header.starts_with("hits  Grep.Match[]   n=2   "),
            "{header}"
        );
        assert!(header.contains("inline cost ~10 tok"), "{header}");
        assert!(header.contains("· preview "), "{header}");
        assert!(header.ends_with("tok"), "{header}");
    }

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
