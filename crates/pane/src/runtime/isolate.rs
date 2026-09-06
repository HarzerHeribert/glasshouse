//! The V8 isolate a task's cells run in — `runtime-contract.md` §1, §2 and §5.
//!
//! **A [`Runtime`] cannot exist without the session's compiled `Profile`.**
//! [`Runtime::new`] takes one and there is no other constructor, no
//! `Default`, and no builder; the profile is cloned from the session's, never
//! compiled here, so `sandbox-grants.md` §1.5 — computed once, immutable for
//! the session — survives this layer intact.
//!
//! **One isolate, one context, every cell.** The context's global object *is*
//! the persistent scope: a cell's top-level bindings are copied onto it when
//! the cell ends, so cell *n + 1* reads them as free variables (§2), and
//! redeclaring a name in a later cell is a fresh function scope rather than a
//! `SyntaxError`. Nothing is evicted — the only three operations that shrink
//! the table are `HandleTable`'s own, and none of them is reachable from
//! rendering.
//!
//! **There is no event loop.** Every host function is synchronous, `await` on
//! a non-promise is legal, and the only microtasks are the promise jobs the
//! cell's own `async` wrapper enqueues; one explicit checkpoint drains them.
//! A cell that awaits something nothing can settle is answered, not hung.

use std::ffi::c_void;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, Once, PoisonError};
use std::time::{Duration, Instant};

use crate::contract::SessionId;
use crate::glasshouse::Glasshouse;
use crate::runtime::bindings::{self, CellTrace};
use crate::runtime::cell::{self, CellError, CompiledCell, LINE_OFFSET};
use crate::runtime::handles::{self, HandleMeta};
use crate::runtime::marshal;
use crate::runtime::outcome::{
    CellOutcome, CellOutcomeKind, CellRecord, CellTurn, HandleRecord, TERMINAL_JSON_CAP, Terminal,
};
use crate::runtime::preview::{self, ErrorValue, PREVIEW_TOKEN_CAP, StackFrame, Value};
use crate::runtime::state::{HeapGuard, RuntimeState};
use crate::sandbox::profile::Profile;
use crate::tools::invoke::CancellationToken;

/// The isolate's heap ceiling until `pane.toml` supplies one — 61F owns the
/// setting, `runtime-contract.md` §7 says so, and this is the default it
/// takes until then. Crossing it fails the **cell** with
/// `RuntimeOutOfMemory`; it never frees a handle (§2).
pub const DEFAULT_HEAP_LIMIT_BYTES: usize = 256 * 1024 * 1024;

/// How long one cell may occupy the session before the runtime stops it —
/// the same shape as [`DEFAULT_HEAP_LIMIT_BYTES`], and 61F owns the
/// configurable value.
///
/// It exists because `while (true) {}` allocates nothing, so the heap
/// ceiling never sees it: without a wall clock a cell that computes forever
/// takes the whole session with it and no later cell ever runs. Thirty
/// seconds is far longer than any cell measured here and far shorter than a
/// person waiting on a hung session.
pub const DEFAULT_CELL_WALL_CLOCK_LIMIT: Duration = Duration::from_secs(30);

/// The most bytes a returned string may be before the cell yields with the
/// cap as its reason instead of returning — `runtime-contract.md` §9.2's
/// response cap. A constant here for the same reason as the two above: 61F
/// owns the `pane.toml` setting. It is never a truncation: over it the task
/// continues and the model is told the size.
pub const DEFAULT_RESPONSE_BYTE_CAP: usize = 16 * 1024;

/// How many microtask checkpoints a cell gets before its promise is declared
/// unsettleable. One drains the whole queue, including jobs the queue's own
/// jobs enqueue; the rest are slack.
const MICROTASK_CHECKPOINTS: usize = 8;

/// How deep a stack V8 captures for a throw. Deeper than the three frames
/// `runtime-contract.md` §3 renders, so the host-frame filter in
/// [`thrown_error`] still has something to filter after the model's own
/// frames run out.
const STACK_TRACE_FRAME_LIMIT: i32 = 10;

/// How often a watchdog that has already fired asks again, until the cell
/// actually stops.
///
/// A termination request V8 does not observe is otherwise never re-issued:
/// measured on this host, `while (true) { const x = new Array(100).fill("y"); }`
/// ran for minutes against a 500 ms limit because the request landed while
/// the thread was inside `Builtin_ArrayPrototypeFill`, which checks no
/// interrupt. At this interval a lost request costs 50 ms rather than the
/// session.
const TERMINATE_RETRY_INTERVAL: Duration = Duration::from_millis(50);

/// The multiple of the wall-clock limit past which a cell that is *still*
/// running has ignored every termination the watchdog issued, and the
/// runtime stops trusting its isolate: the cell is answered as a
/// `RuntimeTimeout` naming both deadlines and [`Runtime::poisoned`] becomes
/// true, so no later cell of the task runs code in it.
///
/// `runtime-contract.md` §7 owns the wall clock itself. This multiplier and
/// [`TERMINATE_RETRY_INTERVAL`] are the watchdog's own mechanics and move to
/// `pane.toml` beside the limit when 61F takes it.
const HARD_DEADLINE_MULTIPLE: u32 = 3;

/// The prefix every script a model authored is named with. A stack frame
/// from any other script is a host frame and is never shown (§5).
const CELL_SCRIPT_PREFIX: &str = "pane:cell:";

static V8_ONCE: Once = Once::new();

/// The V8 flags every isolate in this process is built under.
///
/// **A cell can always be stopped by a termination request, because no tier
/// that elides interrupt checks runs model code.** `terminate_execution` is
/// observed at V8's own interrupt checks, and TurboFan's code for an
/// allocating loop reaches none of them: measured on this host, with the
/// watchdog re-issuing the request every [`TERMINATE_RETRY_INTERVAL`],
/// `while (true) { const x = new Array(100).fill("y"); }` never stopped at
/// all, while `while (true) {}`, `while (true) { const x = new Array(100); }`,
/// a preallocated `a.fill("y")` and the same cell plus `Math.random()` each
/// stopped at ~305 ms against a 300 ms limit. Same binary, same cell, one
/// flag: no flag → never; `--no-maglev` → never; **`--no-turbofan` → 300 ms**;
/// `--jitless` → 311 ms.
///
/// **Maglev stays on**, so this gives up the top optimising tier and not the
/// JIT. A cell is a short program that calls tools, which that tier buys
/// almost nothing, and a cell that cannot be stopped is a session that cannot
/// be trusted — `runtime-contract.md` §2's wall clock is the whole reason
/// [`DEFAULT_CELL_WALL_CLOCK_LIMIT`] exists.
const V8_FLAGS: &str = "--no-turbofan";

fn initialize_v8() {
    V8_ONCE.call_once(|| {
        // Before the platform, because a flag read at initialisation is
        // ignored if it arrives after it.
        v8::V8::set_flags_from_string(V8_FLAGS);
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
    });
}

/// The near-heap-limit callback's own state, behind the one `*mut c_void` V8
/// hands that callback: the [`HeapGuard`] the isolate is signalled through,
/// and how often the ceiling has been raised.
///
/// **The ceiling is raised at most once per cell**, which is what makes it a
/// ceiling. The raise exists to buy the terminated cell room to unwind;
/// granting another `initial_heap_limit` on *every* callback turned a 256 MiB
/// ceiling into 7 GB of resident memory while a cell that ignored its
/// termination went on allocating.
struct HeapWatch {
    guard: Rc<HeapGuard>,
    /// Cleared at the start of every cell by [`Runtime::run_cell`]; set by
    /// the first callback that raises for that cell.
    raised_this_cell: AtomicBool,
    /// Raises since this runtime was built, which is what
    /// [`Runtime::heap_limit_raises`] answers with — the number that says
    /// the ceiling held rather than moved.
    raises: AtomicU32,
}

/// V8 hands this callback a `*mut c_void` and nothing else, so the isolate it
/// must stop travels in the [`HeapWatch`] behind that pointer.
///
/// The **first** call for a cell returns a raised limit on purpose:
/// returning the current one makes V8 abort the process, and this crate
/// answers an out-of-memory with a value. The raise buys just enough room for
/// the terminated cell to unwind. Every **later** call for the same cell is a
/// cell that took the room and kept allocating, so it is answered with the
/// limit it already has and terminated again instead.
unsafe extern "C" fn near_heap_limit(
    data: *mut c_void,
    current_heap_limit: usize,
    initial_heap_limit: usize,
) -> usize {
    // SAFETY: `data` is the address of a `HeapWatch` the `Runtime` owns and
    // declares *after* its isolate, so the watch outlives every callback the
    // isolate can make.
    let watch = unsafe { &*(data as *const HeapWatch) };
    watch.guard.hit.store(true, Ordering::SeqCst);
    if let Some(isolate) = watch.guard.isolate.get() {
        isolate.terminate_execution();
    }
    if watch.raised_this_cell.swap(true, Ordering::SeqCst) {
        return current_heap_limit;
    }
    watch.raises.fetch_add(1, Ordering::SeqCst);
    current_heap_limit.saturating_add(initial_heap_limit.max(8 * 1024 * 1024))
}

/// One task's isolate.
///
/// Field order is drop order and is load-bearing: the context handle is
/// released while its isolate is alive, and the heap guard the near-heap-limit
/// callback points at outlives the isolate that could call it.
pub struct Runtime {
    context: v8::Global<v8::Context>,
    isolate: v8::OwnedIsolate,
    state: Rc<RuntimeState>,
    heap: Rc<HeapWatch>,
    wall_clock_limit: Duration,
    /// The cell that ran past [`HARD_DEADLINE_MULTIPLE`] × the wall-clock
    /// limit while being terminated every [`TERMINATE_RETRY_INTERVAL`], if
    /// one did. Once it is set nothing in this type enters the isolate
    /// again — see [`Runtime::poisoned`].
    poisoned_by: Option<u64>,
}

impl Runtime {
    /// Builds a runtime for one task against the session's compiled profile.
    ///
    /// There is no constructor that does not take a [`Profile`], which is
    /// what makes "model-authored code never runs outside a sandbox" a
    /// property of the type rather than of a review:
    ///
    /// ```compile_fail
    /// use pane::runtime::isolate::Runtime;
    /// let _ = Runtime::new();
    /// ```
    ///
    /// and there is no `Default` to reach for either:
    ///
    /// ```compile_fail
    /// use pane::runtime::isolate::Runtime;
    /// let _: Runtime = Default::default();
    /// ```
    pub fn new(profile: &Profile, glasshouse: &Glasshouse, session: &SessionId) -> Self {
        Self::with_heap_limit(profile, glasshouse, session, DEFAULT_HEAP_LIMIT_BYTES)
    }

    /// [`Runtime::new`] with an explicit ceiling, so a test can reach the
    /// out-of-memory path without allocating 256 MiB.
    pub fn with_heap_limit(
        profile: &Profile,
        glasshouse: &Glasshouse,
        session: &SessionId,
        heap_limit_bytes: usize,
    ) -> Self {
        Self::with_limits(
            profile,
            glasshouse,
            session,
            heap_limit_bytes,
            DEFAULT_CELL_WALL_CLOCK_LIMIT,
        )
    }

    /// Both ceilings explicitly, so a test can reach the timeout path
    /// without waiting out the default.
    pub fn with_limits(
        profile: &Profile,
        glasshouse: &Glasshouse,
        session: &SessionId,
        heap_limit_bytes: usize,
        wall_clock_limit: Duration,
    ) -> Self {
        initialize_v8();
        let mut isolate =
            v8::Isolate::new(v8::CreateParams::default().heap_limits(0, heap_limit_bytes));
        isolate.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);
        // `Atomics.wait` blocks inside V8 where an interrupt may never land,
        // so the watchdog is not the answer to it: this line is, and measured
        // it is sufficient on its own — with both globals present, the
        // verifier's cell throws `TypeError: Atomics.wait cannot be called in
        // this context` at the model's own line in 665 µs. The bootstrap
        // deletes the two globals as well, because a single-threaded isolate
        // with no workers has nothing to share memory with and because
        // `the_isolate_has_no_ambient_authority` can then enumerate them.
        isolate.set_allow_atomics_wait(false);
        // Without this V8 captures no structured trace at all:
        // `v8::Exception::get_stack_trace` answers `None` for every throw, so
        // §5's "the top three in-program frames" was an empty list on every
        // error a model has ever been shown, and the two assertions that
        // looked like they checked it were quantifiers over that empty list.
        isolate.set_capture_stack_trace_for_uncaught_exceptions(true, STACK_TRACE_FRAME_LIMIT);

        let guard = HeapGuard::new();
        let _ = guard.isolate.set(isolate.thread_safe_handle());
        let heap = Rc::new(HeapWatch {
            guard,
            raised_this_cell: AtomicBool::new(false),
            raises: AtomicU32::new(0),
        });
        isolate.add_near_heap_limit_callback(
            near_heap_limit,
            Rc::as_ptr(&heap).cast_mut().cast::<c_void>(),
        );

        let state = Rc::new(RuntimeState::new(profile, glasshouse, session));
        isolate.set_slot(state.clone());

        let context = {
            v8::scope!(let handle_scope, &mut isolate);
            let context = v8::Context::new(handle_scope, v8::ContextOptions::default());
            let scope = &mut v8::ContextScope::new(handle_scope, context);
            bindings::install(scope);
            if let Some(source) = v8::String::new(scope, bindings::BOOTSTRAP)
                && let Some(script) = v8::Script::compile(scope, source, None)
            {
                script.run(scope);
            }
            v8::Global::new(scope, context)
        };

        Self {
            context,
            isolate,
            state,
            heap,
            wall_clock_limit,
            poisoned_by: None,
        }
    }

    /// [`DEFAULT_RESPONSE_BYTE_CAP`] replaced, so a test can reach the
    /// over-cap yield without building sixteen kilobytes.
    #[must_use]
    pub fn with_response_byte_cap(self, bytes: usize) -> Self {
        let mut runtime = self;
        runtime.trace().response_byte_cap.set(bytes);
        runtime
    }

    /// §9's trajectory, yield request and response cap, in the isolate's
    /// second slot -- keyed by its type, beside the state rather than in it,
    /// and installed on first use so the constructor is unchanged.
    fn trace(&mut self) -> Rc<CellTrace> {
        if let Some(trace) = self.isolate.get_slot::<Rc<CellTrace>>() {
            return trace.clone();
        }
        let trace = CellTrace::new();
        self.isolate.set_slot(trace.clone());
        trace
    }

    /// The token every tool call this runtime makes becomes cancellable
    /// through — a builder, so [`Runtime::new`]'s signature is unchanged and
    /// a caller that has nothing to cancel goes on writing what it wrote.
    ///
    /// A cancelled call is `runtime-contract.md` §5's throw, class
    /// `Cancelled`, in the turn slot a yield would have used. Setting the
    /// token widens nothing and starts nothing: `tools::invoke` either
    /// returns before a child exists or kills one that does.
    #[must_use]
    pub fn with_token(self, token: CancellationToken) -> Self {
        // `let mut runtime = self` rather than `mut self` in the signature,
        // the same shape `with_response_byte_cap` above uses: it keeps this a
        // builder over an already-built runtime, which is the property
        // `runtime_cells::the_runtime_cannot_be_built_without_a_profile`
        // reads off this line.
        let mut runtime = self;
        runtime.set_token(token);
        runtime
    }

    /// The token the **next** cell's calls are cancellable through, replacing
    /// whatever [`with_token`](Self::with_token) or an earlier call installed.
    ///
    /// **A cancelled token has no way back, so a task hands each cell a fresh
    /// one.** [`CancellationToken::cancel`] is deliberately one-way — a token
    /// is one call's decision, not a reusable switch — so a session that kept
    /// one token for the whole task would answer every cell after the first
    /// cancellation with an instant `Cancelled` throw. This is how one Ctrl-C
    /// cancels one cell's calls; `session.rs` is its only caller.
    pub fn set_token(&mut self, token: CancellationToken) {
        *self.state.token.borrow_mut() = token;
    }

    /// The number of the cell that ran last; 0 before the first.
    pub fn cell(&self) -> u64 {
        self.state.cell.get()
    }

    /// Whether this runtime has stopped trusting its isolate.
    ///
    /// It is set by exactly one event: a cell that went on running past
    /// [`HARD_DEADLINE_MULTIPLE`] × the wall-clock limit while the watchdog
    /// terminated it every [`TERMINATE_RETRY_INTERVAL`]. A cell that ignored
    /// that many requests is running code the runtime cannot stop, so from
    /// then on [`Runtime::run_cell`] answers a throw naming the cell that did
    /// it **without entering the isolate**, and [`Runtime::end_task`] drops
    /// the handles without entering it either.
    pub fn poisoned(&self) -> bool {
        self.poisoned_by.is_some()
    }

    /// How many times the near-heap-limit callback has raised this isolate's
    /// ceiling since it was built — at most once per cell, which is what
    /// makes `DEFAULT_HEAP_LIMIT_BYTES` a ceiling rather than a first
    /// instalment.
    pub fn heap_limit_raises(&self) -> u32 {
        self.heap.raises.load(Ordering::SeqCst)
    }

    pub fn is_live(&self, name: &str) -> bool {
        self.state.table.borrow().is_live(name)
    }

    pub fn handle_names(&self) -> Vec<String> {
        self.state
            .table
            .borrow()
            .names()
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    /// The turn's whole rendering of the handle table.
    pub fn render_handles(&self) -> String {
        handles::render_table(
            &self.state.table.borrow(),
            preview::PREVIEW_TOKEN_CAP,
            preview::TABLE_TOKEN_CAP,
        )
    }

    /// The task ending — `runtime-contract.md` §2's third and last lifetime
    /// event, and the only one this type performs itself.
    pub fn end_task(&mut self) {
        // A poisoned runtime's isolate ignored every termination the watchdog
        // issued, and [`Runtime::poisoned`] promises nothing re-enters it.
        // The handles go with the task either way: the table below is the
        // host's own record, and the persistent scope dies with the isolate.
        if !self.poisoned() {
            let names = self.handle_names();
            v8::scope!(let handle_scope, &mut self.isolate);
            let context = v8::Local::new(handle_scope, &self.context);
            let scope = &mut v8::ContextScope::new(handle_scope, context);
            let global = context.global(scope);
            for name in &names {
                if let Some(key) = v8::String::new(scope, name) {
                    global.delete(scope, key.into());
                }
            }
        }
        self.state.table.borrow_mut().end_task();
        self.state.forget_calls();
    }

    /// Runs one cell and answers with what it produced.
    ///
    /// Never a `Result`: a throw is a result (§5), a refusal is a throw
    /// (`sandbox-grants.md` §1.4), and a program that will not compile is a
    /// throw in the same turn slot. Nothing about a cell is an error of the
    /// runtime's.
    pub fn run_cell(&mut self, source: &str) -> CellOutcome {
        let started = Instant::now();
        let cell = self.state.begin_cell();
        self.trace().begin_cell();
        // Before anything touches V8: an isolate that ignored every
        // termination is one this runtime no longer runs code in, and the
        // model is told which cell did it rather than being handed a hang.
        if let Some(by) = self.poisoned_by {
            return self.finish(
                cell,
                source,
                started,
                Ending::Threw(poisoned_error(by)),
                Stopped::none(),
            );
        }
        // A hit the previous cell did not consume — one raised while its own
        // previews were being taken — is not this cell's reason for stopping.
        self.heap.guard.take_hit();
        // The ceiling may be raised once for *this* cell, whatever the last
        // one needed.
        self.heap.raised_this_cell.store(false, Ordering::SeqCst);

        let compiled = match cell::compile(source, cell) {
            Ok(compiled) => compiled,
            Err(error) => {
                let value = compile_error_value(&error);
                return self.finish(cell, source, started, Ending::Threw(value), Stopped::none());
            }
        };

        let watchdog = Watchdog::arm(
            self.heap.guard.isolate.get().cloned(),
            self.wall_clock_limit,
        );
        let ending = self.execute(&compiled);
        // Both halves are read here, before anything below allocates: taking
        // a preview can itself raise the heap callback, and a hit raised by
        // the runtime's own bookkeeping is not why the cell stopped.
        let disarmed = watchdog.disarm();
        let stopped = Stopped {
            timed_out: disarmed.fired,
            heap_hit: self.heap.guard.take_hit(),
            yielded: self.trace().take_yield(),
        };
        // Unconditionally, and it is not tidiness. Until a termination is
        // cancelled every V8 call below it bails out, so the previews would
        // stay the ones the declaration lines took and the out-of-memory list
        // would rank by them. Unconditional because the watchdog can also
        // fire in the instant between `execute` returning and `disarm` taking
        // the lock: the cell ends normally and a termination nobody asked for
        // is left pending on the isolate, which the *next* cell would run
        // into. A cancel with nothing pending is a no-op that returns false.
        self.isolate.cancel_terminate_execution();

        self.forget_freed();
        self.refresh_previews();
        // Once more, because the refresh above re-reads the model's own
        // values and a getter is the model's own code: one that calls
        // `yieldNow` or fills the heap there requests a termination after the
        // cancel above, and a request nobody cancels is honoured by the next
        // cell's first statement. The refresh already saw every effect of it
        // (the read answered with nothing); this is only the flag.
        self.isolate.cancel_terminate_execution();
        // The cell did stop, or this line would not be running — but it took
        // more than the hard deadline of terminations to do it, so this is
        // the last cell that runs in this isolate. Reported ahead of the heap
        // hit: a ceiling that fired is recoverable and this is not.
        if disarmed.gave_up {
            self.poisoned_by = Some(cell);
            let error = gave_up(
                started.elapsed(),
                self.wall_clock_limit,
                self.wall_clock_limit.saturating_mul(HARD_DEADLINE_MULTIPLE),
            );
            return self.finish(cell, source, started, Ending::Threw(error), Stopped::none());
        }
        self.finish(cell, source, started, ending, stopped)
    }

    /// Everything that happens inside the isolate, so every V8 handle is
    /// released before the turn is assembled.
    fn execute(&mut self, compiled: &CompiledCell) -> Ending {
        let response_byte_cap = self.trace().response_byte_cap.get();
        v8::scope!(let handle_scope, &mut self.isolate);
        let context = v8::Local::new(handle_scope, &self.context);
        let scope = &mut v8::ContextScope::new(handle_scope, context);
        v8::tc_scope!(let try_catch, scope);

        let Some(source) = v8::String::new(try_catch, &compiled.javascript) else {
            return Ending::Threw(plain_error(
                "RangeError",
                "the cell is too large to compile",
            ));
        };
        let Some(name) = v8::String::new(try_catch, &compiled.script_name) else {
            return Ending::Threw(plain_error("RangeError", "the cell could not be named"));
        };
        let origin = v8::ScriptOrigin::new(
            try_catch,
            name.into(),
            0,
            0,
            false,
            -1,
            None,
            false,
            false,
            false,
            None,
        );

        let Some(script) = v8::Script::compile(try_catch, source, Some(&origin)) else {
            // A terminated cell has no exception to read, and reading one
            // while V8 is unwinding a termination is not safe.
            if try_catch.has_terminated() {
                return Ending::Terminated;
            }
            let exception = try_catch.exception();
            return Ending::Threw(caught_error(try_catch, exception));
        };
        let Some(wrapper) = script.run(try_catch) else {
            // A terminated cell has no exception to read, and reading one
            // while V8 is unwinding a termination is not safe.
            if try_catch.has_terminated() {
                return Ending::Terminated;
            }
            let exception = try_catch.exception();
            return Ending::Threw(caught_error(try_catch, exception));
        };
        let Ok(wrapper) = v8::Local::<v8::Function>::try_from(wrapper) else {
            return Ending::Threw(plain_error(
                "TypeError",
                "the cell did not compile to a callable",
            ));
        };

        let host = bindings::host_object(try_catch);
        let receiver: v8::Local<v8::Value> = v8::undefined(try_catch).into();
        let Some(promise) = wrapper.call(try_catch, receiver, &[host.into()]) else {
            // A terminated cell has no exception to read, and reading one
            // while V8 is unwinding a termination is not safe.
            if try_catch.has_terminated() {
                return Ending::Terminated;
            }
            let exception = try_catch.exception();
            return Ending::Threw(caught_error(try_catch, exception));
        };

        for _ in 0..MICROTASK_CHECKPOINTS {
            try_catch.perform_microtask_checkpoint();
            if !matches!(promise_state(promise), Some(v8::PromiseState::Pending)) {
                break;
            }
        }

        if try_catch.has_terminated() {
            return Ending::Terminated;
        }

        let Ok(promise) = v8::Local::<v8::Promise>::try_from(promise) else {
            // An `async function` always answers with a promise; a value
            // here would mean the wrapper was not the one this module wrote.
            return Ending::Threw(plain_error(
                "TypeError",
                "the cell did not answer with a promise",
            ));
        };
        let ending = match promise.state() {
            v8::PromiseState::Fulfilled => {
                let value = promise.result(try_catch);
                // §1's two endings, decided by the value rather than by a
                // flag: the generated body ends `return __pane_cell.e()`, and
                // only the host can mint what that answers with.
                if bindings::is_end_marker(try_catch, value, self.state.cell.get()) {
                    Ending::Yielded { reason: None }
                } else {
                    // Inside the watchdog on purpose: reading a result walks
                    // the model's own getters and proxy traps. A read that
                    // did not answer leaves its exception or termination on
                    // the `TryCatch`, and nothing may run the model's code
                    // again until that has been read off -- measured: a
                    // second read re-entered a getter the watchdog had just
                    // stopped, with no watchdog left to stop it.
                    match returned(try_catch, &self.state, response_byte_cap, value) {
                        Ok(ending) => ending,
                        Err(ReadFailed) => {
                            if try_catch.has_terminated() {
                                return Ending::Terminated;
                            }
                            let exception = try_catch.exception();
                            return Ending::Threw(caught_error(try_catch, exception));
                        }
                    }
                }
            }
            v8::PromiseState::Rejected => {
                let value = promise.result(try_catch);
                Ending::Threw(thrown_error(try_catch, value))
            }
            v8::PromiseState::Pending => Ending::Threw(plain_error(
                "RuntimeStalled",
                "the cell awaited a promise nothing can settle: pane's isolate has no timers, no \
                 sockets and no event loop, and every tool call is synchronous",
            )),
        };
        // Reading the result runs the model's getters, and a getter the
        // watchdog stopped would otherwise hand back a partial value as a
        // completed return: a termination raised while the ending was being
        // read is the ending.
        if try_catch.has_terminated() {
            return Ending::Terminated;
        }
        ending
    }

    /// `free("name")` is one of §2's three lifetime events, and the epilogue
    /// that re-reads every binding for the value it ended with must not undo
    /// one the cell itself performed: a program that declares a name and then
    /// frees it would otherwise leave the object on the persistent scope,
    /// live to the next cell and absent from the table that is supposed to
    /// list everything live.
    fn forget_freed(&mut self) {
        let freed = self.state.current.borrow().freed.clone();
        if freed.is_empty() {
            return;
        }
        v8::scope!(let handle_scope, &mut self.isolate);
        let context = v8::Local::new(handle_scope, &self.context);
        let scope = &mut v8::ContextScope::new(handle_scope, context);
        let global = context.global(scope);
        for name in &freed {
            if let Some(key) = v8::String::new(scope, name) {
                global.delete(scope, key.into());
            }
        }
    }

    /// Takes every live capture's preview again from the value the name
    /// holds now that the cell has ended.
    ///
    /// `capture()` marshals where `s(…)` runs, which is the end of the
    /// declaration's line — so `const arr = []; arr.push(1,2,3,4,5)` showed
    /// the model `n=0` for an array of five, and the `RuntimeOutOfMemory`
    /// list ranked the array that filled the heap last, at `~0 B`, because
    /// it was empty when it was declared. §3's preview is of the handle, and
    /// §2's five largest are the five largest now.
    ///
    /// The persistent scope is where the value is read from: `capture()` has
    /// already put every captured name on the global object, so this needs no
    /// handle of its own and reads exactly what the next cell will see. The
    /// epilogue's own re-capture covers the same ground for a binding the
    /// `finally` can reach; this is what covers a `class`, a `keep`, and the
    /// two endings where the `finally` never runs at all.
    ///
    /// **Every live name, not only the ones this cell bound.** The captures
    /// alone are the cell that has just ended, so `const arr = []` in cell 1
    /// and `arr.push(1,2,3,4,5)` in cell 2 left the model reading `n=0` for an
    /// array of five for the rest of the task. A captured name is written back
    /// into `current.captures`, which [`Runtime::finish`] drains into the
    /// table; a live-but-uncaptured one goes straight to the table through
    /// [`handles::HandleTable::refresh`], which is the only write that does
    /// not reorder it or claim the model redeclared the name.
    fn refresh_previews(&mut self) {
        let mut names: Vec<String> = self
            .state
            .table
            .borrow()
            .names()
            .into_iter()
            .map(str::to_string)
            .collect();
        let captured: Vec<String> = self
            .state
            .current
            .borrow()
            .captures
            .iter()
            .map(|capture| capture.name.clone())
            .collect();
        for name in &captured {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        if names.is_empty() {
            return;
        }
        let state = self.state.clone();
        let mut refreshed: Vec<(String, Value, HandleMeta)> = Vec::with_capacity(names.len());
        {
            v8::scope!(let handle_scope, &mut self.isolate);
            let context = v8::Local::new(handle_scope, &self.context);
            let scope = &mut v8::ContextScope::new(handle_scope, context);
            let global = context.global(scope);
            for name in names {
                let Some(key) = v8::String::new(scope, &name) else {
                    continue;
                };
                // A name the scope no longer has keeps the preview it had:
                // replacing it with `undefined` would report a handle the
                // table still lists as having lost its value.
                if global.has(scope, key.into()) != Some(true) {
                    continue;
                }
                let Some(value) = global.get(scope, key.into()) else {
                    continue;
                };
                let (preview, meta) = bindings::preview_of(scope, &state, value);
                refreshed.push((name, preview, meta));
            }
        }
        let mut current = state.current.borrow_mut();
        for (name, preview, meta) in refreshed {
            if let Some(capture) = current
                .captures
                .iter_mut()
                .find(|capture| capture.name == name)
            {
                capture.value = preview;
                capture.meta = meta;
            } else {
                // Live, and this cell did not bind it: the table's own entry
                // is the only copy there is.
                state.table.borrow_mut().refresh(&name, preview, meta);
            }
        }
    }

    /// Turns a cell's ending into the turn the session loop reads.
    fn finish(
        &mut self,
        cell: u64,
        source: &str,
        started: Instant,
        ending: Ending,
        stopped: Stopped,
    ) -> CellOutcome {
        let captures = std::mem::take(&mut self.state.current.borrow_mut().captures);
        for capture in captures {
            self.state.table.borrow_mut().declare_with(
                capture.name,
                capture.value,
                cell,
                capture.meta,
            );
        }

        // After the captures are in the table, because both of these read it:
        // the out-of-memory list names the five largest live handles, and a
        // timeout's own message promises the cell's bindings are still there.
        let ending = match ending {
            Ending::Terminated if stopped.heap_hit => Ending::Threw(self.out_of_memory()),
            Ending::Terminated if stopped.timed_out => {
                Ending::Threw(timed_out(started.elapsed(), self.wall_clock_limit))
            }
            // The two ceilings above win over the flag; the flag wins over
            // nothing else terminating — `yieldNow` is a yield, never an
            // error (§9.3).
            Ending::Terminated => match stopped.yielded {
                Some(reason) => Ending::Yielded { reason },
                // Nothing else in this crate terminates execution, so this
                // is reported as what it is rather than as one of the three.
                None => Ending::Threw(plain_error(
                    "RuntimeTerminated",
                    "the isolate was terminated before the cell finished",
                )),
            },
            other => other,
        };

        let table = self.render_handles();
        let (stdout_tail, stdout_dropped_tokens) = self.state.current.borrow_mut().console.tail();
        let kind = match &ending {
            Ending::Yielded { .. } => CellOutcomeKind::Yielded,
            Ending::Returned(..) => CellOutcomeKind::Returned,
            Ending::Threw(_) | Ending::Terminated => CellOutcomeKind::Threw,
        };
        let yield_reason = match &ending {
            Ending::Yielded { reason } => reason.clone(),
            _ => None,
        };
        let calls = self.trace().take_calls();
        let record = CellRecord {
            cell,
            source: source.to_string(),
            outcome: kind,
            handles: self
                .state
                .table
                .borrow()
                .rows(preview::PREVIEW_TOKEN_CAP)
                .into_iter()
                .map(|(name, type_name, preview, provenance)| HandleRecord {
                    name,
                    type_name,
                    preview,
                    provenance,
                })
                .collect(),
            calls,
        };
        let turn = CellTurn {
            elapsed_ms: started.elapsed().as_millis() as u64,
            table,
            stdout_tail,
            stdout_dropped_tokens,
            yield_reason,
            record,
        };

        match ending {
            Ending::Yielded { .. } => CellOutcome::Yielded { turn },
            Ending::Returned(value, terminal) => CellOutcome::Returned {
                value,
                terminal,
                turn,
            },
            Ending::Threw(error) => CellOutcome::Threw { error, turn },
            // Normalised above. An arm rather than an `unreachable!` so a
            // future ending cannot silently become a yield.
            Ending::Terminated => CellOutcome::Threw {
                error: plain_error(
                    "RuntimeTerminated",
                    "the isolate was terminated before the cell finished",
                ),
                turn,
            },
        }
    }

    /// The `n` largest live handles **as they are now**, largest first, each
    /// measured off the persistent scope at the moment the error is built.
    ///
    /// `HandleMeta::size_estimate` is taken where a handle is captured, so
    /// for a handle the *current* cell never bound it is the size that
    /// handle had in some earlier cell. That is the common shape here — a
    /// model declares `const acc = []` in one cell and fills it in the next
    /// — and it ranked the array that filled the heap last, at `~0 B`,
    /// behind a 44-character string. §2 promises "the five largest live
    /// handles by retained size" so that the model can choose what to free,
    /// and a ranking by stale sizes tells it to free the wrong thing.
    ///
    /// Measuring here rather than refreshing every cell keeps the cost on
    /// the path that has already lost: `marshal::size_estimate` is the
    /// deliberately shallow one written for this moment (it never walks an
    /// array's elements or asks an array for its property names), and it
    /// runs after `run_cell`'s unconditional `cancel_terminate_execution`,
    /// so V8 answers rather than bailing. A name the scope no longer holds
    /// keeps the size the table recorded for it.
    fn largest_live_now(&mut self, n: usize) -> Vec<(String, u64)> {
        let mut sized: Vec<(String, u64)> = {
            let table = self.state.table.borrow();
            table
                .names()
                .into_iter()
                .map(|name| {
                    (
                        name.to_string(),
                        table.meta(name).map_or(0, |meta| meta.size_estimate),
                    )
                })
                .collect()
        };
        {
            v8::scope!(let handle_scope, &mut self.isolate);
            let context = v8::Local::new(handle_scope, &self.context);
            let scope = &mut v8::ContextScope::new(handle_scope, context);
            let global = context.global(scope);
            for (name, size) in &mut sized {
                let Some(key) = v8::String::new(scope, name) else {
                    continue;
                };
                if global.has(scope, key.into()) != Some(true) {
                    continue;
                }
                let Some(value) = global.get(scope, key.into()) else {
                    continue;
                };
                *size = marshal::size_estimate(scope, value);
            }
        }
        sized.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        sized.truncate(n);
        sized
    }

    /// The `RuntimeOutOfMemory` preview: the five largest live handles, so
    /// the *model* can choose what to free. Nothing is evicted here — §2 is
    /// explicit that a handle vanishing under a program that still names it
    /// is the one failure that would make the channel untrustworthy.
    fn out_of_memory(&mut self) -> ErrorValue {
        let largest = self.largest_live_now(5);
        let listed = if largest.is_empty() {
            "no live handles".to_string()
        } else {
            largest
                .iter()
                .map(|(name, size)| format!("{name} (~{size} B)"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        ErrorValue {
            class: "RuntimeOutOfMemory".to_string(),
            message: format!(
                "the isolate reached its heap ceiling; nothing was freed. Largest live handles: \
                 {listed}. Call free(\"name\") on what you no longer need."
            ),
            line: None,
            column: None,
            stack: Vec::new(),
        }
    }
}

/// How a cell ended, before the turn around it is assembled.
enum Ending {
    /// A fall-off (`reason: None`), or a yield on purpose with `yieldNow`'s
    /// reason or the response cap's sentence (§9.3, §9.2).
    Yielded {
        reason: Option<String>,
    },
    Returned(Value, Terminal),
    Threw(ErrorValue),
    /// V8 stopped the cell. [`Stopped`] says which of the two ceilings did
    /// it, or that `yieldNow` asked, because the model is told a different
    /// thing by each.
    Terminated,
}

/// Which ceiling stopped the cell, read the instant it stopped.
///
/// Every flag is consumed once, before any of the bookkeeping that follows
/// a cell allocates: taking a preview can raise the heap callback itself,
/// and a hit raised there would report a timeout as an out-of-memory.
#[derive(Debug, Clone)]
struct Stopped {
    heap_hit: bool,
    timed_out: bool,
    /// `Some` when `yieldNow` was called, with the reason it gave.
    yielded: Option<Option<String>>,
}

impl Stopped {
    /// A cell that never reached V8 — one that did not compile.
    fn none() -> Self {
        Self {
            heap_hit: false,
            timed_out: false,
            yielded: None,
        }
    }
}

/// A read of the model's value that did not answer: the getter or trap it
/// ran threw, or was stopped. The exception is on the `TryCatch` and the
/// reader stops at once, because the next read would run that code again.
struct ReadFailed;

/// A top-level `return`'s value, read for what §9.2 makes of it.
///
/// A string is read **in full** — the terminal response is never `marshal`'s
/// sample — unless it is over the response cap, in which case the cell
/// yields with the cap as its reason and nothing of the string is rendered
/// (§9.2: a response is never silently truncated). Any other value becomes
/// its JSON under [`TERMINAL_JSON_CAP`].
fn returned(
    scope: &mut v8::PinScope,
    state: &Rc<RuntimeState>,
    response_byte_cap: usize,
    value: v8::Local<v8::Value>,
) -> Result<Ending, ReadFailed> {
    if value.is_string() {
        let string: v8::Local<v8::String> = value.try_into().expect("is_string");
        let bytes = string.utf8_length(scope);
        if bytes > response_byte_cap {
            return Ok(Ending::Yielded {
                reason: Some(format!(
                    "the response is {} bytes, over the cap of {} bytes; return less or yield",
                    preview::thousands(bytes as u64),
                    preview::thousands(response_byte_cap as u64)
                )),
            });
        }
        let text = string.to_rust_string_lossy(scope);
        return Ok(Ending::Returned(
            marshal::marshal(scope, value),
            Terminal::Text(text),
        ));
    }
    // The walk first: it reads every property, so a getter that throws or
    // never returns is found here, and `marshal` -- which would read the
    // same getters again -- runs only once every read has answered.
    let terminal = terminal_json(scope, state, value, TERMINAL_JSON_CAP)?;
    Ok(Ending::Returned(marshal::marshal(scope, value), terminal))
}

/// §9.2's rendering of a non-string result: its JSON with values.
///
/// Written by the host rather than by `JSON.stringify` because only the host
/// can read a tool object's private tag, and §4 says such an object
/// contributes its preview and never its payload. The walk stops once the
/// cap is passed, so a large value the program built costs the cap and not
/// its size. `JSON.stringify`'s shape otherwise: an `undefined`, a function
/// or a symbol is skipped in an object and `null` in an array, a
/// non-finite number is `null`; a `toJSON` method is not consulted.
fn terminal_json(
    scope: &mut v8::PinScope,
    state: &Rc<RuntimeState>,
    value: v8::Local<v8::Value>,
    cap: usize,
) -> Result<Terminal, ReadFailed> {
    let mut json = JsonText {
        out: String::new(),
        cap,
        over: false,
    };
    write_json(scope, state, value, &mut json, 0)?;
    let cut = json.over || json.out.len() > cap;
    let mut text = json.out;
    if text.len() > cap {
        let mut end = cap;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    Ok(Terminal::Json { text, cut })
}

/// The JSON being written, and whether the walk stopped short of the value.
struct JsonText {
    out: String,
    cap: usize,
    over: bool,
}

impl JsonText {
    fn full(&self) -> bool {
        self.out.len() > self.cap
    }

    fn push(&mut self, text: &str) {
        if self.full() {
            self.over = true;
            return;
        }
        self.out.push_str(text);
    }
}

/// Deeper than this and the walk stops: with the byte cap it is not what
/// ends a cycle, only what bounds the stack while the cap does.
const JSON_MAX_DEPTH: u32 = 64;

/// A value that `JSON.stringify` leaves out of an object.
fn json_skips(value: v8::Local<v8::Value>) -> bool {
    value.is_undefined() || value.is_function() || value.is_symbol()
}

fn json_string(text: &str) -> String {
    serde_json::Value::String(text.to_string()).to_string()
}

fn write_json(
    scope: &mut v8::PinScope,
    state: &Rc<RuntimeState>,
    value: v8::Local<v8::Value>,
    json: &mut JsonText,
    depth: u32,
) -> Result<(), ReadFailed> {
    if json.full() || depth > JSON_MAX_DEPTH {
        json.over = true;
        return Ok(());
    }
    if value.is_null() {
        json.push("null");
        return Ok(());
    }
    if json_skips(value) {
        json.push(if depth == 0 { "undefined" } else { "null" });
        return Ok(());
    }
    if value.is_boolean() {
        json.push(if value.boolean_value(scope) {
            "true"
        } else {
            "false"
        });
        return Ok(());
    }
    if value.is_number() {
        let number = value.number_value(scope).unwrap_or(f64::NAN);
        if number.is_finite() {
            json.push(&value.to_rust_string_lossy(scope));
        } else {
            json.push("null");
        }
        return Ok(());
    }
    if value.is_string() {
        let string: v8::Local<v8::String> = value.try_into().expect("is_string");
        let room = json.cap.saturating_sub(json.out.len()) + 4;
        let (text, whole) = bounded_utf8(scope, string, room);
        json.push(&json_string(&text));
        if !whole {
            json.over = true;
        }
        return Ok(());
    }
    if value.is_native_error() {
        let error = marshal::error_of(scope, value);
        json.push(&format!(
            "{{\"name\":{},\"message\":{}}}",
            json_string(&error.class),
            json_string(&error.message)
        ));
        return Ok(());
    }
    // Before the array and object arms: a tool result is one of those, and
    // its tag is what says it renders as its preview.
    if let Some(call) = bindings::recorded_call(scope, state, value) {
        json.push(&json_string(&preview::render_preview(
            &call.preview,
            PREVIEW_TOKEN_CAP,
        )));
        return Ok(());
    }
    if value.is_array() {
        let array: v8::Local<v8::Array> = value.try_into().expect("is_array");
        json.push("[");
        for index in 0..array.length() {
            if json.full() {
                json.over = true;
                break;
            }
            if index > 0 {
                json.push(",");
            }
            let element = array.get_index(scope, index).ok_or(ReadFailed)?;
            if json_skips(element) {
                json.push("null");
            } else {
                write_json(scope, state, element, json, depth + 1)?;
            }
        }
        json.push("]");
        return Ok(());
    }
    if value.is_object() {
        let object: v8::Local<v8::Object> = value.try_into().expect("is_object");
        json.push("{");
        let mut first = true;
        let names = object
            .get_own_property_names(scope, v8::GetPropertyNamesArgs::default())
            .ok_or(ReadFailed)?;
        for index in 0..names.length() {
            if json.full() {
                json.over = true;
                break;
            }
            let key = names.get_index(scope, index).ok_or(ReadFailed)?;
            let property = object.get(scope, key).ok_or(ReadFailed)?;
            if json_skips(property) {
                continue;
            }
            if !first {
                json.push(",");
            }
            first = false;
            json.push(&json_string(&key.to_rust_string_lossy(scope)));
            json.push(":");
            write_json(scope, state, property, json, depth + 1)?;
        }
        json.push("}");
        return Ok(());
    }
    // A bigint: `JSON.stringify` throws on one; its canonical spelling,
    // quoted, is the honest rendering of a value a person asked to see.
    json.push(&json_string(&value.to_rust_string_lossy(scope)));
    Ok(())
}

/// At most `max_bytes` of `string`, whole characters only, and whether that
/// was all of it — so a string the program built out of a payload is read
/// to the cap and not to its length.
fn bounded_utf8(
    scope: &mut v8::PinScope,
    string: v8::Local<v8::String>,
    max_bytes: usize,
) -> (String, bool) {
    if string.utf8_length(scope) <= max_bytes {
        return (string.to_rust_string_lossy(scope), true);
    }
    let mut buffer = vec![0u8; max_bytes];
    let written = string.write_utf8_v2(
        scope,
        &mut buffer,
        v8::WriteFlags::kReplaceInvalidUtf8,
        None,
    );
    buffer.truncate(written);
    (String::from_utf8_lossy(&buffer).into_owned(), false)
}

/// What a [`Watchdog`] did while its cell ran.
#[derive(Debug, Clone, Copy)]
struct Disarmed {
    /// The wall-clock limit passed and the watchdog terminated the cell.
    fired: bool,
    /// The cell went on running past [`HARD_DEADLINE_MULTIPLE`] × the limit
    /// while being terminated every [`TERMINATE_RETRY_INTERVAL`]. It stopped
    /// in the end — nothing downstream of `disarm` could run otherwise — but
    /// the isolate is no longer one this runtime will re-enter.
    gave_up: bool,
}

/// The wall clock, as one thread per cell.
///
/// It holds a clone of the isolate's `IsolateHandle` — the same `Send` handle
/// the near-heap-limit callback uses — waits on a condition variable for the
/// cell to finish, and calls `terminate_execution` if the wait times out
/// first. The condition variable rather than a sleep loop is what makes
/// [`Watchdog::disarm`] return immediately for the overwhelming majority of
/// cells, which finish in milliseconds.
///
/// **It terminates until the cell stops, not once.** V8 observes a
/// termination request at its own interrupt checks, and there are stretches
/// with none: measured on this host, a cell allocating through
/// `Array.prototype.fill` sat inside `Builtin_ArrayPrototypeFill` and never
/// saw the single request the watchdog used to issue, so a 500 ms limit
/// became minutes and a 30 s one became 1:59 through the shipped binary.
/// Asking again every [`TERMINATE_RETRY_INTERVAL`] turns a lost request into
/// a 50 ms delay.
///
/// **And it never stops asking**, even past the hard deadline, because the
/// thread blocked inside V8 is the one that must return: giving up on the
/// request would be giving up on `run_cell` ever answering. What the hard
/// deadline does is record that this isolate stopped being stoppable, which
/// is what [`Runtime::poisoned`] then acts on.
struct Watchdog {
    done: Arc<(Mutex<bool>, Condvar)>,
    fired: Arc<AtomicBool>,
    gave_up: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watchdog {
    fn arm(isolate: Option<v8::IsolateHandle>, limit: Duration) -> Self {
        let done = Arc::new((Mutex::new(false), Condvar::new()));
        let fired = Arc::new(AtomicBool::new(false));
        let gave_up = Arc::new(AtomicBool::new(false));
        let armed = Instant::now();
        let hard = limit.saturating_mul(HARD_DEADLINE_MULTIPLE);
        let thread = isolate.map(|isolate| {
            let done = Arc::clone(&done);
            let fired = Arc::clone(&fired);
            let gave_up = Arc::clone(&gave_up);
            std::thread::spawn(move || {
                let (lock, finished) = &*done;
                let guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
                let (mut guard, timeout) = finished
                    .wait_timeout_while(guard, limit, |done| !*done)
                    .unwrap_or_else(PoisonError::into_inner);
                if !timeout.timed_out() || *guard {
                    return;
                }
                // Ordered before the terminate so the flag is visible to
                // `disarm`, which cannot run until this thread releases the
                // lock it is still holding.
                fired.store(true, Ordering::SeqCst);
                isolate.terminate_execution();
                loop {
                    let (next, _) = finished
                        .wait_timeout_while(guard, TERMINATE_RETRY_INTERVAL, |done| !*done)
                        .unwrap_or_else(PoisonError::into_inner);
                    guard = next;
                    // Read before the cell's own ending is: a cell that
                    // stopped at last, but only after the hard deadline, is
                    // exactly the one this flag is about. Checking it after
                    // the `return` below would answer "false" for every cell
                    // that eventually stopped, which is all of them.
                    if armed.elapsed() >= hard {
                        gave_up.store(true, Ordering::SeqCst);
                    }
                    if *guard {
                        return;
                    }
                    isolate.terminate_execution();
                }
            })
        });
        Self {
            done,
            fired,
            gave_up,
            thread,
        }
    }

    /// Stops the watch and answers what it did.
    fn disarm(mut self) -> Disarmed {
        self.stop();
        Disarmed {
            fired: self.fired.load(Ordering::SeqCst),
            gave_up: self.gave_up.load(Ordering::SeqCst),
        }
    }

    /// Tells the thread the cell finished and waits for it to notice.
    /// Idempotent, so [`Watchdog::disarm`] and the `Drop` below can both run.
    fn stop(&mut self) {
        {
            let (lock, finished) = &*self.done;
            *lock.lock().unwrap_or_else(PoisonError::into_inner) = true;
            finished.notify_all();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A panic between arming and disarming would otherwise leave a thread that
/// terminates whatever the isolate is running when its deadline arrives —
/// which, by then, is a later cell.
impl Drop for Watchdog {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The `RuntimeTimeout` a stopped cell is answered with.
///
/// It is a throw and not a runtime error for the reason §5 gives for every
/// other one: the bindings the cell completed are in the table, the session
/// is intact, and the model gets the next turn to decide what to do about it.
fn timed_out(elapsed: Duration, limit: Duration) -> ErrorValue {
    ErrorValue {
        class: "RuntimeTimeout".to_string(),
        message: format!(
            "the cell ran for {} ms without finishing and was stopped at pane's wall-clock limit \
             of {} ms; nothing was freed and the bindings it completed are still live",
            elapsed.as_millis(),
            limit.as_millis()
        ),
        line: None,
        column: None,
        stack: Vec::new(),
    }
}

/// The `RuntimeTimeout` a cell that ignored the whole ladder of
/// terminations is answered with — the wall-clock limit it passed, and the
/// hard deadline past which this isolate stopped being trusted.
///
/// Still a throw, and still `RuntimeTimeout`: from the model's side this is
/// the same event as [`timed_out`], only worse, and §5's shape does not
/// change because the host lost confidence in its isolate.
fn gave_up(elapsed: Duration, limit: Duration, hard: Duration) -> ErrorValue {
    ErrorValue {
        class: "RuntimeTimeout".to_string(),
        message: format!(
            "the cell ran for {} ms, ignoring every termination pane issued from its wall-clock \
             limit of {} ms onwards and past its hard deadline of {} ms; nothing was freed, and \
             no later cell runs in this isolate",
            elapsed.as_millis(),
            limit.as_millis(),
            hard.as_millis()
        ),
        line: None,
        column: None,
        stack: Vec::new(),
    }
}

/// What every cell after [`gave_up`] is answered with, naming the cell that
/// cost the isolate its trust. Built without touching V8, which is the whole
/// promise of [`Runtime::poisoned`].
fn poisoned_error(by: u64) -> ErrorValue {
    ErrorValue {
        class: "RuntimePoisoned".to_string(),
        message: format!(
            "cell {by} did not stop when pane terminated it, so this isolate is no longer trusted \
             and no later cell runs in it; the task is over"
        ),
        line: None,
        column: None,
        stack: Vec::new(),
    }
}

fn plain_error(class: &str, message: &str) -> ErrorValue {
    ErrorValue {
        class: class.to_string(),
        message: message.to_string(),
        line: None,
        column: None,
        stack: Vec::new(),
    }
}

fn compile_error_value(error: &CellError) -> ErrorValue {
    let (line, column) = error
        .position()
        .map_or((None, None), |(l, c)| (Some(l), Some(c)));
    ErrorValue {
        class: error.class().to_string(),
        message: error.message(),
        line,
        column,
        stack: Vec::new(),
    }
}

fn promise_state(value: v8::Local<v8::Value>) -> Option<v8::PromiseState> {
    v8::Local::<v8::Promise>::try_from(value)
        .ok()
        .map(|promise| promise.state())
}

/// The error a `TryCatch` was holding — a compile failure or a synchronous
/// throw before the cell's first `await`. The exception is read out of the
/// `TryCatch` by the caller so this takes an ordinary scope.
fn caught_error(scope: &mut v8::PinScope, exception: Option<v8::Local<v8::Value>>) -> ErrorValue {
    match exception {
        Some(exception) => thrown_error(scope, exception),
        None => plain_error("Error", "the cell failed without an exception"),
    }
}

/// An exception, read for exactly what `runtime-contract.md` §5 lists: the
/// class, the message, the position inside the model's own program, and only
/// the frames that are inside it.
fn thrown_error(scope: &mut v8::PinScope, exception: v8::Local<v8::Value>) -> ErrorValue {
    let mut error = marshal::error_of(scope, exception);

    if let Some(trace) = v8::Exception::get_stack_trace(scope, exception) {
        for index in 0..trace.get_frame_count() {
            let Some(frame) = trace.get_frame(scope, index) else {
                continue;
            };
            let script = frame
                .get_script_name(scope)
                .map(|name| name.to_rust_string_lossy(scope))
                .unwrap_or_default();
            if !script.starts_with(CELL_SCRIPT_PREFIX) {
                // A host frame, and §5 says the model never sees one.
                continue;
            }
            let line = (frame.get_line_number() as u32).saturating_sub(LINE_OFFSET);
            let column = (frame.get_column() as u32).saturating_sub(1);
            if error.line.is_none() {
                error.line = Some(line);
                error.column = Some(column);
            }
            let function = frame
                .get_function_name(scope)
                .map(|name| name.to_rust_string_lossy(scope))
                .unwrap_or_default();
            let cell = script.trim_start_matches(CELL_SCRIPT_PREFIX);
            error.stack.push(StackFrame {
                description: if function.is_empty() {
                    format!("cell {cell}, line {line}, column {column}")
                } else {
                    format!("{function} (cell {cell}, line {line}, column {column})")
                },
            });
        }
    }

    if error.line.is_none()
        && let Some(message) = v8::Exception::create_message(scope, exception).into()
    {
        let message: v8::Local<v8::Message> = message;
        let script = message
            .get_script_resource_name(scope)
            .map(|name| name.to_rust_string_lossy(scope))
            .unwrap_or_default();
        if script.starts_with(CELL_SCRIPT_PREFIX)
            && let Some(line) = message.get_line_number(scope)
        {
            error.line = Some((line as u32).saturating_sub(LINE_OFFSET));
            error.column = Some(message.get_start_column() as u32);
        }
    }

    error
}

/// The one property of this module that is a property of its *source*: no
/// expression under `runtime/` compiles a sandbox profile, so the profile a
/// cell runs against can only be the one the session compiled at start-up
/// (`sandbox-grants.md` §1.5).
#[cfg(test)]
mod tests {
    const ISOLATE_SOURCE: &str = include_str!("isolate.rs");
    const BINDINGS_SOURCE: &str = include_str!("bindings.rs");
    const STATE_SOURCE: &str = include_str!("state.rs");
    const CELL_SOURCE: &str = include_str!("cell.rs");
    const MARSHAL_SOURCE: &str = include_str!("marshal.rs");

    const SOURCES: [(&str, &str); 5] = [
        ("isolate.rs", ISOLATE_SOURCE),
        ("bindings.rs", BINDINGS_SOURCE),
        ("state.rs", STATE_SOURCE),
        ("cell.rs", CELL_SOURCE),
        ("marshal.rs", MARSHAL_SOURCE),
    ];

    /// The production half of a file: everything before its first
    /// `#[cfg(test)]`, with comment lines dropped so a sentence *about* a
    /// forbidden call is not mistaken for one.
    fn production(source: &str) -> String {
        source
            .split_once("#[cfg(test)]")
            .map_or(source, |(before, _)| before)
            .lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                !trimmed.starts_with("//")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A scan that scanned nothing would pass every assertion below, so each
    /// file's production half is checked to still contain the item that
    /// makes it that file.
    #[test]
    fn the_scan_has_something_to_scan() {
        for (name, source) in SOURCES {
            let production = production(source);
            assert!(
                production.len() > 500,
                "{name}'s production half is only {} bytes",
                production.len()
            );
        }
        assert!(production(ISOLATE_SOURCE).contains("pub fn run_cell"));
        assert!(production(BINDINGS_SOURCE).contains("invoke::run"));
        assert!(production(CELL_SOURCE).contains("fn compile"));
    }

    #[test]
    fn nothing_in_the_runtime_compiles_a_profile() {
        for (name, source) in SOURCES {
            assert!(
                !production(source).contains("Profile::compile"),
                "{name} compiles a profile; sandbox-grants.md §1.5 says the session's is the only \
                 one"
            );
        }
    }

    /// The other half of the same claim: the only child-spawning path out of
    /// this module is `invoke::run`, which is confined before it spawns.
    #[test]
    fn the_runtime_spawns_nothing_of_its_own() {
        for (name, source) in SOURCES {
            let production = production(source);
            for forbidden in [
                "Command::new",
                "std::fs::",
                "TcpStream",
                "std::net",
                "File::open",
            ] {
                assert!(
                    !production.contains(forbidden),
                    "{name} reaches for `{forbidden}`; every effect this package has goes through \
                     tools::invoke::run"
                );
            }
        }
    }
}
