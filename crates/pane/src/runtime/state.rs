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
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use crate::config::HelpersConfig;
use crate::contract::SessionId;
use crate::glasshouse::Glasshouse;
use crate::helpers::{HelperCall, HelperOutcome, HelperRecord};
use crate::runtime::handles::{HandleMeta, HandleTable, Provenance};
use crate::runtime::instructions::{InstructionContext, PendingInstructions};
use crate::runtime::outcome::{PlanItem, SourceEvidence};
use crate::runtime::preview::{self, Value};
use crate::sandbox::profile::{Access, Profile};
use crate::tools::invoke::CancellationToken;

/// Told when a helper call starts and again when it ends, with every call
/// this cell has made so far.
pub(crate) type HelperProgress = Rc<dyn Fn(&[HelperRecord])>;

thread_local! {
    static HELPER_PROGRESS: RefCell<Option<HelperProgress>> = const { RefCell::new(None) };
}

/// Installs the signal helper progress is reported through on this thread,
/// and answers with whatever it replaced.
///
/// **Thread-local, and it is the seam `session::ui`'s `OUTPUT` already is**: a
/// task's cells run on the session's own thread, so the terminal a running
/// helper must reach is the one belonging to the thread the call is made
/// from, and `runtime/**` names no terminal type of its own. A runtime built
/// without a session -- `agent.rs`'s subagents, every test -- finds none,
/// which is the absent case rather than a special one.
pub(crate) fn install_helper_progress(signal: Option<HelperProgress>) -> Option<HelperProgress> {
    HELPER_PROGRESS.with(|slot| std::mem::replace(&mut *slot.borrow_mut(), signal))
}

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
        if self.dropped_chars == 0 {
            return (self.buffer.clone(), 0);
        }
        // The omission must be visible in stdout itself: callers that have
        // not yet plumbed the numeric field still cannot mistake a tail for
        // complete output. Recompute after reserving the marker because that
        // reservation itself may drop a few more characters.
        for _ in 0..2 {
            let marker = format!(
                "[console: ~{} tokens omitted before this true tail]\n",
                self.dropped_chars.div_ceil(4)
            );
            self.trim_to(KEEP_CHARS.saturating_sub(marker.chars().count()));
        }
        let dropped = self.dropped_chars.div_ceil(4);
        let marker = format!("[console: ~{dropped} tokens omitted before this true tail]\n");
        let mut shown = marker;
        shown.push_str(&self.buffer);
        debug_assert!(shown.chars().count() <= KEEP_CHARS);
        (shown, dropped)
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
    /// Every helper call this cell completed, in call order — the one field
    /// `CellView.helpers` is built from.
    pub(crate) helpers: Vec<HelperRecord>,
    /// Helper calls **claimed** this cell, which is what the per-cell ceiling
    /// counts. A call in flight has claimed its slot and left no record yet,
    /// so counting the records instead would let a loop overrun the ceiling
    /// by whatever is outstanding.
    pub(crate) helper_calls: u32,
}

/// What every host callback can reach.
pub(crate) struct RuntimeState {
    pub(crate) messages: RefCell<Rc<RefCell<HashMap<String, crate::events::inbox::Message>>>>,
    pub(crate) handlers: Rc<crate::runtime::handlers::Handlers>,
    pub(crate) profile: Profile,
    /// Host-only suspension seam; absent in ordinary sessions and subagents.
    pub(crate) approval_gate: RefCell<Option<crate::approval::Gate>>,
    /// The current watchdog's host-visible flag. V8's termination query may
    /// stay false until a blocked Rust callback returns to an interrupt check.
    pub(crate) watchdog_fired: RefCell<Option<Arc<AtomicBool>>>,
    pub(crate) mcp: RefCell<crate::tools::mcp::Mcp>,
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
    pub(crate) instructions: RefCell<InstructionContext>,
    /// `[helpers]` as the session read it. The default carries no `model`,
    /// which is helpers **off**: a runtime nobody configured spends nothing
    /// on the user's behalf.
    helpers: RefCell<HelpersConfig>,
    /// Versions whose exact editing context has crossed a completed cell
    /// boundary and therefore reached the model.
    visible_sources: RefCell<HashSet<(PathBuf, String)>>,
    /// Context produced in the cell currently running. It becomes visible at
    /// the next cell boundary, never earlier merely because code holds it.
    pending_sources: RefCell<HashSet<(PathBuf, String)>>,
    pending_context_output: RefCell<Option<String>>,
}

impl RuntimeState {
    pub(crate) fn new(profile: &Profile, glasshouse: &Glasshouse, session: &SessionId) -> Self {
        Self {
            messages: RefCell::new(Rc::new(RefCell::new(HashMap::new()))),
            handlers: crate::runtime::handlers::Handlers::new(),
            profile: profile.clone(),
            approval_gate: RefCell::new(None),
            watchdog_fired: RefCell::new(None),
            mcp: RefCell::new(crate::tools::mcp::Mcp::default()),
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
            instructions: RefCell::new(InstructionContext::default()),
            helpers: RefCell::new(HelpersConfig::default()),
            visible_sources: RefCell::new(HashSet::new()),
            pending_sources: RefCell::new(HashSet::new()),
            pending_context_output: RefCell::new(None),
        }
    }

    pub(crate) fn enable_instruction_context(&self) {
        self.instructions.borrow_mut().enable(&self.profile);
    }

    pub(crate) fn instruction_boundary(
        &self,
        tool: &str,
        args: &crate::tools::invoke::Args,
    ) -> bool {
        self.instructions
            .borrow_mut()
            .gate(&self.profile, tool, args)
    }

    pub(crate) fn pending_instructions(&self) -> Option<PendingInstructions> {
        self.instructions.borrow().pending()
    }

    pub(crate) fn instruction_file_written(
        &self,
        tool: &str,
        args: &crate::tools::invoke::Args,
    ) -> bool {
        self.instructions
            .borrow_mut()
            .instruction_file_written(&self.profile, tool, args)
    }

    pub(crate) fn acknowledge_instructions(&self) {
        self.instructions.borrow_mut().acknowledge();
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
        self.visible_sources
            .borrow_mut()
            .extend(self.pending_sources.borrow_mut().drain());
        let cell = self.cell.get() + u64::from(!self.handlers.running.get());
        self.cell.set(cell);
        let mut current = self.current.borrow_mut();
        current.console.clear();
        current.captures.clear();
        current.freed.clear();
        current.helpers.clear();
        current.helper_calls = 0;
        cell
    }

    pub(crate) fn set_helpers(&self, helpers: HelpersConfig) {
        *self.helpers.borrow_mut() = helpers;
    }

    /// The model helpers run on, or the sentence saying why there is none.
    ///
    /// The invariant: **a helper never runs unasked.** `[helpers] model`
    /// unset is off, exactly as `[supervisor] model` unset is, because a
    /// helper spends money on the user's behalf and the fail-closed direction
    /// is *not configured, not run*.
    pub(crate) fn helper_model(&self) -> Result<String, String> {
        let helpers = self.helpers.borrow();
        if !helpers.enabled {
            return Err("helpers are off: `[helpers] enabled` is false in pane.toml".to_string());
        }
        helpers.model.clone().ok_or_else(|| {
            "helpers are not configured: set `[helpers] model` in .glasshouse/pane.toml".to_string()
        })
    }

    /// Takes one of this cell's helper-call slots, or says the ceiling is
    /// reached. `little-helpers.md`'s *cost is bounded per cell*: a program
    /// is code, so a helper call sits inside a loop, and the refusal is what
    /// the model catches instead of the loop running away.
    pub(crate) fn claim_helper_call(&self) -> Result<(), String> {
        let ceiling = self.helpers.borrow().calls_per_cell;
        let mut current = self.current.borrow_mut();
        if current.helper_calls >= ceiling {
            return Err(format!(
                "this cell has used its {ceiling} helper call(s); yield and start another cell"
            ));
        }
        current.helper_calls += 1;
        Ok(())
    }

    /// A helper call starting: the record is kept with its outcome unfilled
    /// and the progress signal fires, so the lane can show the call **while
    /// it is in flight**. Answers with the slot [`finish_helper`] resolves.
    ///
    /// The invariant: a call is visible from the moment it starts. Its wire
    /// call blocks this thread, so a record kept only on return can never be
    /// rendered as running -- which is the whole of `little-helpers.md`'s
    /// lane. `record.outcome` and `record.turns` are what the call has
    /// produced so far, which at the start is nothing: leave them at their
    /// defaults rather than at the spec's ceiling.
    ///
    /// [`finish_helper`]: RuntimeState::finish_helper
    pub(crate) fn begin_helper(&self, record: HelperRecord) -> usize {
        let slot = {
            let mut current = self.current.borrow_mut();
            current.helpers.push(record);
            current.helpers.len() - 1
        };
        self.report_helper_progress();
        slot
    }

    /// The call in `slot` resolving: what came back and the turns it actually
    /// took replace the unfilled ones, and the progress signal fires again.
    ///
    /// It takes the whole [`HelperCall`] rather than its outcome because
    /// `turns` is only known once the call has answered, and a record left at
    /// the turns it was allowed is exactly what the inspector section exists
    /// to make visible.
    pub(crate) fn finish_helper(&self, slot: usize, call: HelperCall) {
        if let Some(record) = self.current.borrow_mut().helpers.get_mut(slot) {
            record.outcome = call.outcome;
            record.turns = call.turns;
        }
        self.report_helper_progress();
    }

    /// One call that has already resolved, for a caller with nothing to show
    /// in flight. It is [`begin_helper`] and [`finish_helper`] back to back,
    /// so there is one path a record reaches the lane by rather than two.
    ///
    /// [`begin_helper`]: RuntimeState::begin_helper
    /// [`finish_helper`]: RuntimeState::finish_helper
    pub(crate) fn record_helper(&self, record: HelperRecord) {
        let call = HelperCall {
            outcome: record.outcome.clone(),
            turns: record.turns,
        };
        let slot = self.begin_helper(HelperRecord {
            outcome: HelperOutcome::default(),
            turns: 0,
            ..record
        });
        self.finish_helper(slot, call);
    }

    /// Hands this cell's calls to the installed signal, if there is one. The
    /// signal is cloned out before it runs, so it may install another.
    fn report_helper_progress(&self) {
        let Some(signal) = HELPER_PROGRESS.with(|slot| slot.borrow().clone()) else {
            return;
        };
        let records = self.current.borrow().helpers.clone();
        signal(&records);
    }

    /// Every helper call the cell that just ran completed, in call order.
    pub(crate) fn helper_records(&self) -> Vec<HelperRecord> {
        self.current.borrow().helpers.clone()
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
        self.visible_sources.borrow_mut().clear();
        self.pending_sources.borrow_mut().clear();
        self.pending_context_output.borrow_mut().take();
    }

    pub(crate) fn note_source_context(&self, evidence: &SourceEvidence, text: String) {
        let path = self.absolute_source_path(Path::new(&evidence.path));
        if evidence.complete {
            self.pending_sources
                .borrow_mut()
                .insert((path, evidence.sha256.clone()));
        }
        *self.pending_context_output.borrow_mut() = Some(text);
    }

    pub(crate) fn has_pending_source_context(&self) -> bool {
        self.pending_context_output.borrow().is_some()
    }

    /// Appends editing context after model-authored console output so the
    /// bounded true tail always retains the complete target it certifies.
    pub(crate) fn flush_source_context(&self) {
        if let Some(text) = self.pending_context_output.borrow_mut().take() {
            self.current.borrow_mut().console.write_line(&text);
        }
    }

    pub(crate) fn source_version_is_visible(&self, args: &crate::tools::invoke::Args) -> bool {
        let (Some(path), Some(hash)) = (args.get("path"), args.get("expected_sha256")) else {
            return false;
        };
        let path = self.absolute_source_path(Path::new(path));
        self.visible_sources
            .borrow()
            .contains(&(path, hash.to_ascii_lowercase()))
    }

    /// Supplies the hash when exactly one complete version of this path has
    /// crossed a cell boundary. This keeps the common edit call small while
    /// refusing to guess after two different versions were inspected.
    pub(crate) fn sole_visible_source_hash(
        &self,
        args: &crate::tools::invoke::Args,
    ) -> Option<String> {
        let path = self.absolute_source_path(Path::new(args.get("path")?));
        let visible = self.visible_sources.borrow();
        let mut hashes = visible
            .iter()
            .filter_map(|(candidate, hash)| (candidate == &path).then_some(hash.clone()));
        let hash = hashes.next()?;
        hashes.next().is_none().then_some(hash)
    }

    fn absolute_source_path(&self, path: &Path) -> PathBuf {
        self.profile
            .check("edit", Access::Read, path)
            .unwrap_or_else(|_| {
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    self.profile.root().join(path)
                }
            })
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

    fn state() -> RuntimeState {
        RuntimeState::new(
            &Profile::compile(
                std::env::temp_dir(),
                Some(r#"{"permissions":{"allow":[]}}"#),
            ),
            &Glasshouse::Command {
                glasshouse: PathBuf::from("glasshouse"),
            },
            &SessionId::new("progress"),
        )
    }

    fn asked(name: &str) -> HelperRecord {
        HelperRecord {
            helper: name.to_string(),
            verb: "reducing".to_string(),
            asked: "4118 lines".to_string(),
            ..HelperRecord::default()
        }
    }

    /// The lane exists to say a helper is running, so the signal must arrive
    /// **before** the answer does: one call, two reports, the first carrying a
    /// record nothing has resolved yet.
    #[test]
    fn a_helper_call_reports_its_start_and_then_its_end() {
        let seen: Rc<RefCell<Vec<Vec<HelperRecord>>>> = Rc::new(RefCell::new(Vec::new()));
        let recorder = Rc::clone(&seen);
        let previous = install_helper_progress(Some(Rc::new(move |records: &[HelperRecord]| {
            recorder.borrow_mut().push(records.to_vec());
        })));

        let state = state();
        let slot = state.begin_helper(asked("reduce"));
        state.finish_helper(
            slot,
            HelperCall {
                outcome: HelperOutcome {
                    text: "3 distinct root failures".to_string(),
                    ok: true,
                    elapsed_ms: 1_100,
                },
                turns: 1,
            },
        );
        install_helper_progress(previous);

        let seen = seen.borrow();
        assert_eq!(seen.len(), 2, "one call must report a start and an end");
        assert_eq!(seen[0].len(), 1);
        assert!(
            !seen[0][0].outcome.ok && seen[0][0].outcome.text.is_empty(),
            "the first report must carry an unresolved call: {:?}",
            seen[0][0].outcome
        );
        assert_eq!(seen[0][0].helper, "reduce");
        assert!(seen[1][0].outcome.ok, "the second report must be resolved");
        assert_eq!(seen[1][0].outcome.text, "3 distinct root failures");
        assert_eq!(state.helper_records().len(), 1, "one call, one record");
    }

    /// Every runtime built without a session -- `agent.rs`'s subagents and
    /// every test -- finds no signal, and that is the ordinary case rather
    /// than a special one.
    #[test]
    fn a_helper_call_with_no_terminal_installed_still_records() {
        let previous = install_helper_progress(None);
        let state = state();
        let slot = state.begin_helper(asked("reduce"));
        state.finish_helper(
            slot,
            HelperCall {
                outcome: HelperOutcome {
                    text: "nothing failed".to_string(),
                    ok: true,
                    elapsed_ms: 40,
                },
                turns: 1,
            },
        );
        install_helper_progress(previous);
        let records = state.helper_records();
        assert_eq!(records.len(), 1);
        assert!(records[0].outcome.ok);
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
