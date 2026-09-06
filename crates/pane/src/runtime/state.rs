//! What the host keeps while a task's cells run.
//!
//! One [`RuntimeState`] per [`crate::runtime::isolate::Runtime`], reachable
//! from every host callback through the isolate's slot. It holds the
//! session's own `Profile`, `Glasshouse` seam and `SessionId` — **cloned from
//! the session's, never compiled here**: nothing in `runtime/**` calls
//! `Profile::compile`, which is `sandbox-grants.md` §1.5's guarantee that a
//! program cannot widen the sandbox it runs in — plus the live handle table,
//! the cell's captured `console` output, and the calls whose results became
//! objects.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::contract::SessionId;
use crate::glasshouse::Glasshouse;
use crate::runtime::handles::{HandleMeta, HandleTable, Provenance};
use crate::runtime::outcome::PlanItem;
use crate::runtime::preview::{self, Value};
use crate::sandbox::profile::Profile;
use crate::tools::invoke::CancellationToken;

/// A `console` capture bounded ahead of rendering.
///
/// It keeps at most twice [`preview::STDOUT_TOKEN_CAP`]'s worth of characters
/// and drops from the front, so a program logging in a loop costs a bounded
/// amount of memory rather than a growing one, and `runtime-contract.md`
/// §3's "the rest is dropped with a count" is a number this already has.
#[derive(Debug, Default)]
pub(crate) struct ConsoleCapture {
    buffer: String,
    kept_chars: usize,
    dropped_chars: usize,
}

/// The characters [`preview::STDOUT_TOKEN_CAP`] tokens are worth, by the
/// `chars / 4` estimate the whole crate shares.
const KEEP_CHARS: usize = preview::STDOUT_TOKEN_CAP * 4;

impl ConsoleCapture {
    pub(crate) fn write_line(&mut self, line: &str) {
        self.buffer.push_str(line);
        self.buffer.push('\n');
        self.kept_chars += line.chars().count() + 1;
        // Trim in one move per KEEP_CHARS appended rather than on every
        // write, so a program logging a million short lines pays O(1) per
        // line amortised.
        if self.kept_chars > 2 * KEEP_CHARS {
            self.trim_to(KEEP_CHARS);
        }
    }

    fn trim_to(&mut self, keep: usize) {
        if self.kept_chars <= keep {
            return;
        }
        let drop = self.kept_chars - keep;
        let byte = self
            .buffer
            .char_indices()
            .nth(drop)
            .map_or(self.buffer.len(), |(index, _)| index);
        self.buffer.drain(..byte);
        self.kept_chars -= drop;
        self.dropped_chars += drop;
    }

    /// The tail the turn shows, and how many tokens were dropped ahead of it.
    pub(crate) fn tail(&mut self) -> (String, usize) {
        self.trim_to(KEEP_CHARS);
        (self.buffer.clone(), self.dropped_chars.div_ceil(4))
    }

    pub(crate) fn clear(&mut self) {
        self.buffer.clear();
        self.kept_chars = 0;
        self.dropped_chars = 0;
    }
}

/// One tool call whose result became a live object, kept so the binding that
/// holds that object can be given its provenance and its already-built
/// preview instead of being marshalled a second time.
#[derive(Debug, Clone)]
pub(crate) struct RecordedCall {
    pub(crate) preview: Value,
    pub(crate) meta: HandleMeta,
}

/// A value a cell captured, in the order it was captured.
#[derive(Debug, Clone)]
pub(crate) struct Capture {
    pub(crate) name: String,
    pub(crate) value: Value,
    pub(crate) meta: HandleMeta,
}

/// The per-cell half of [`RuntimeState`], reset at the start of every cell.
#[derive(Debug, Default)]
pub(crate) struct CellState {
    pub(crate) console: ConsoleCapture,
    pub(crate) captures: Vec<Capture>,
    /// Names the model's own `free` released during this cell. A capture of
    /// one is skipped: `runtime-contract.md` §2 makes `free` a lifetime
    /// event, and re-capturing at the end of the cell would undo it.
    pub(crate) freed: Vec<String>,
}

/// What every host callback can reach.
pub(crate) struct RuntimeState {
    pub(crate) profile: Profile,
    pub(crate) glasshouse: Glasshouse,
    pub(crate) session: SessionId,
    pub(crate) cell: std::cell::Cell<u64>,
    pub(crate) table: RefCell<HandleTable>,
    pub(crate) current: RefCell<CellState>,
    /// The token every tool call this runtime makes is cancellable through.
    /// It is replaceable because [`crate::runtime::isolate::Runtime::with_token`]
    /// is a builder over an already-constructed runtime, and a cell only ever
    /// reads it, so there is no path by which a program can reach it.
    pub(crate) token: RefCell<CancellationToken>,
    /// Every call whose result became a live object, by the id the object is
    /// tagged with.
    ///
    /// **Task-scoped, not cell-scoped.** A tag minted in cell *n* is read
    /// again when a later cell rebinds the object it names — by an
    /// assignment, by `keep`, or by the end-of-cell re-marshal — so a
    /// per-cell store would answer with whatever call happened to sit at the
    /// same position in the later cell, and the handle would be shown
    /// another call's preview and provenance. The map is cleared by
    /// [`RuntimeState::forget_calls`] when the task ends, which is the
    /// lifetime `runtime-contract.md` §2 gives a handle.
    calls: RefCell<HashMap<u64, RecordedCall>>,
    next_call: std::cell::Cell<u64>,
    /// The model's own plan, replaced whole by each `todo.write`.
    ///
    /// **Task-scoped, like [`calls`](Self::calls) and for the same reason**:
    /// a plan is the shape of the task in hand, so it is cleared with the
    /// task rather than carried into the next one.
    plan: RefCell<Vec<PlanItem>>,
    /// Whether this runtime belongs to a subagent, which may not start one.
    pub(crate) subagent: std::cell::Cell<bool>,
    /// What the parent task has left to spend, in tokens, refreshed each turn.
    /// `0` means unknown rather than exhausted -- a runtime nobody told is not
    /// a runtime that must refuse.
    pub(crate) budget_remaining: std::cell::Cell<u64>,
    /// The model the parent task is using, so a subagent inherits it rather
    /// than silently falling back to the compiled-in default.
    pub(crate) model: RefCell<String>,
}

impl RuntimeState {
    pub(crate) fn new(profile: &Profile, glasshouse: &Glasshouse, session: &SessionId) -> Self {
        Self {
            profile: profile.clone(),
            glasshouse: glasshouse.clone(),
            session: session.clone(),
            cell: std::cell::Cell::new(0),
            table: RefCell::new(HandleTable::new()),
            current: RefCell::new(CellState::default()),
            token: RefCell::new(CancellationToken::new()),
            calls: RefCell::new(HashMap::new()),
            next_call: std::cell::Cell::new(0),
            plan: RefCell::new(Vec::new()),
            subagent: std::cell::Cell::new(false),
            budget_remaining: std::cell::Cell::new(0),
            model: RefCell::new(crate::wire::MODEL.to_string()),
        }
    }

    /// Replaces the plan whole — `todo.write`'s only effect.
    pub(crate) fn set_plan(&self, items: Vec<PlanItem>) {
        *self.plan.borrow_mut() = items;
    }

    /// The plan as it stands, for `todo.read` and for the turn the cell ends.
    pub(crate) fn plan(&self) -> Vec<PlanItem> {
        self.plan.borrow().clone()
    }

    pub(crate) fn begin_cell(&self) -> u64 {
        let cell = self.cell.get() + 1;
        self.cell.set(cell);
        let mut current = self.current.borrow_mut();
        current.console.clear();
        current.captures.clear();
        current.freed.clear();
        cell
    }

    /// Records a call whose result became a live object, and answers with the
    /// **task-scoped** id the object is tagged with. Ids start at 1, so a
    /// zero read back from a tag is not a call.
    pub(crate) fn record_call(&self, call: RecordedCall) -> u64 {
        let id = self.next_call.get() + 1;
        self.next_call.set(id);
        self.calls.borrow_mut().insert(id, call);
        id
    }

    pub(crate) fn recorded(&self, id: u64) -> Option<RecordedCall> {
        self.calls.borrow().get(&id).cloned()
    }

    /// The task ending. Every handle is gone, so every call recorded for one
    /// is too.
    pub(crate) fn forget_calls(&self) {
        self.calls.borrow_mut().clear();
        self.next_call.set(0);
        // The plan is the shape of the task that just ended, so it goes with
        // it: a next task inheriting the last one's checklist would be
        // reporting work it never did.
        self.plan.borrow_mut().clear();
    }

    /// A name is captured twice in the ordinary case — once where the
    /// binding is made, once by the epilogue that reads the value it ended
    /// with — so the second capture **overwrites in place**. Removing and
    /// re-appending would order the table by the epilogue instead of by the
    /// model's own declarations, and a name the epilogue cannot read (a
    /// `class`) would then sort ahead of every name it can.
    pub(crate) fn capture(&self, name: &str, value: Value, meta: HandleMeta) {
        let mut current = self.current.borrow_mut();
        if current.freed.iter().any(|freed| freed == name) {
            return;
        }
        if let Some(existing) = current
            .captures
            .iter_mut()
            .find(|existing| existing.name == name)
        {
            existing.value = value;
            existing.meta = meta;
            return;
        }
        current.captures.push(Capture {
            name: name.to_string(),
            value,
            meta,
        });
    }

    pub(crate) fn note_free(&self, name: &str) {
        let mut current = self.current.borrow_mut();
        current.captures.retain(|existing| existing.name != name);
        if !current.freed.iter().any(|freed| freed == name) {
            current.freed.push(name.to_string());
        }
    }
}

/// The provenance of one call, assembled where both the tool's declaration
/// and the call's own arguments are in hand.
pub(crate) fn provenance(
    tool: &str,
    args: &crate::tools::invoke::Args,
    stdout: &str,
    pure: bool,
) -> Provenance {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(stdout.as_bytes());
    Provenance {
        tool: tool.to_string(),
        args: args
            .names()
            .map(|name| {
                (
                    name.to_string(),
                    args.get(name).unwrap_or_default().to_string(),
                )
            })
            .collect(),
        sha256: format!("{digest:x}"),
        pure,
    }
}

/// Signals an out-of-memory across V8's near-heap-limit callback, which is
/// handed a `*mut c_void` and nothing else.
///
/// [`AtomicBool`] and [`OnceLock`] rather than a `RefCell`: the callback runs
/// inside a garbage collection, where a `RefCell` this crate might already
/// have borrowed would panic.
pub(crate) struct HeapGuard {
    pub(crate) hit: AtomicBool,
    pub(crate) isolate: OnceLock<v8::IsolateHandle>,
}

impl HeapGuard {
    pub(crate) fn new() -> Rc<Self> {
        Rc::new(Self {
            hit: AtomicBool::new(false),
            isolate: OnceLock::new(),
        })
    }

    pub(crate) fn take_hit(&self) -> bool {
        self.hit.swap(false, Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_output_is_bounded_and_says_how_much_it_dropped() {
        let mut console = ConsoleCapture::default();
        for i in 0..2_000 {
            console.write_line(&format!("line {i} with some padding to make it wide"));
        }
        let (tail, dropped) = console.tail();
        assert!(
            preview::estimate_tokens(&tail) <= preview::STDOUT_TOKEN_CAP,
            "tail was {} tokens",
            preview::estimate_tokens(&tail)
        );
        assert!(dropped > 0, "nothing was reported dropped");
        // The tail is the *end* of the output, which is what a model needs.
        assert!(tail.contains("line 1999"), "{tail}");
        assert!(!tail.contains("line 0 "), "{tail}");
    }

    #[test]
    fn short_console_output_is_kept_whole_with_nothing_dropped() {
        let mut console = ConsoleCapture::default();
        console.write_line("hello");
        let (tail, dropped) = console.tail();
        assert_eq!(tail, "hello\n");
        assert_eq!(dropped, 0);
    }
}
