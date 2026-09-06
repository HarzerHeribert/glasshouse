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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Once, PoisonError};
use std::time::{Duration, Instant};

use crate::contract::SessionId;
use crate::glasshouse::Glasshouse;
use crate::runtime::bindings;
use crate::runtime::cell::{self, CellError, CompiledCell, LINE_OFFSET};
use crate::runtime::handles::{self, HandleMeta};
use crate::runtime::marshal;
use crate::runtime::outcome::{CellOutcome, CellOutcomeKind, CellRecord, CellTurn, HandleRecord};
use crate::runtime::preview::{self, ErrorValue, StackFrame, Value};
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

/// How many microtask checkpoints a cell gets before its promise is declared
/// unsettleable. One drains the whole queue, including jobs the queue's own
/// jobs enqueue; the rest are slack.
const MICROTASK_CHECKPOINTS: usize = 8;

/// The prefix every script a model authored is named with. A stack frame
/// from any other script is a host frame and is never shown (§5).
const CELL_SCRIPT_PREFIX: &str = "pane:cell:";

static V8_ONCE: Once = Once::new();

fn initialize_v8() {
    V8_ONCE.call_once(|| {
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
    });
}

/// V8 hands this callback a `*mut c_void` and nothing else, so the isolate it
/// must stop travels in the [`HeapGuard`] behind that pointer.
///
/// It returns a **raised** limit on purpose: returning the current one makes
/// V8 abort the process, and this crate answers an out-of-memory with a
/// value. The raise buys just enough room for the terminated cell to unwind.
unsafe extern "C" fn near_heap_limit(
    data: *mut c_void,
    current_heap_limit: usize,
    initial_heap_limit: usize,
) -> usize {
    // SAFETY: `data` is the address of a `HeapGuard` the `Runtime` owns and
    // declares *after* its isolate, so the guard outlives every callback the
    // isolate can make.
    let guard = unsafe { &*(data as *const HeapGuard) };
    guard.hit.store(true, Ordering::SeqCst);
    if let Some(isolate) = guard.isolate.get() {
        isolate.terminate_execution();
    }
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
    heap: Rc<HeapGuard>,
    wall_clock_limit: Duration,
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

        let heap = HeapGuard::new();
        let _ = heap.isolate.set(isolate.thread_safe_handle());
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
        }
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
        *self.state.token.borrow_mut() = token;
        self
    }

    /// The number of the cell that ran last; 0 before the first.
    pub fn cell(&self) -> u64 {
        self.state.cell.get()
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
        let names = self.handle_names();
        {
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
        // A hit the previous cell did not consume — one raised while its own
        // previews were being taken — is not this cell's reason for stopping.
        self.heap.take_hit();

        let compiled = match cell::compile(source, cell) {
            Ok(compiled) => compiled,
            Err(error) => {
                let value = compile_error_value(&error);
                return self.finish(cell, source, started, Ending::Threw(value), Stopped::none());
            }
        };

        let watchdog = Watchdog::arm(self.heap.isolate.get().cloned(), self.wall_clock_limit);
        let ending = self.execute(&compiled);
        // Both halves are read here, before anything below allocates: taking
        // a preview can itself raise the heap callback, and a hit raised by
        // the runtime's own bookkeeping is not why the cell stopped.
        let stopped = Stopped {
            timed_out: watchdog.disarm(),
            heap_hit: self.heap.take_hit(),
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
        self.finish(cell, source, started, ending, stopped)
    }

    /// Everything that happens inside the isolate, so every V8 handle is
    /// released before the turn is assembled.
    fn execute(&mut self, compiled: &CompiledCell) -> Ending {
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
        match promise.state() {
            v8::PromiseState::Fulfilled => {
                let value = promise.result(try_catch);
                // §1's two endings, decided by the value rather than by a
                // flag: the generated body ends `return __pane_cell.e()`, and
                // only the host can mint what that answers with.
                if bindings::is_end_marker(try_catch, value, self.state.cell.get()) {
                    Ending::Yielded
                } else {
                    Ending::Returned(marshal::marshal(try_catch, value))
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
        }
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
    fn refresh_previews(&mut self) {
        let names: Vec<String> = self
            .state
            .current
            .borrow()
            .captures
            .iter()
            .map(|capture| capture.name.clone())
            .collect();
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
            // Neither: nothing else in this crate terminates execution, so
            // this is reported as what it is rather than as one of the two.
            Ending::Terminated => Ending::Threw(plain_error(
                "RuntimeTerminated",
                "the isolate was terminated before the cell finished",
            )),
            other => other,
        };

        let table = self.render_handles();
        let (stdout_tail, stdout_dropped_tokens) = self.state.current.borrow_mut().console.tail();
        let kind = match &ending {
            Ending::Yielded => CellOutcomeKind::Yielded,
            Ending::Returned(_) => CellOutcomeKind::Returned,
            Ending::Threw(_) | Ending::Terminated => CellOutcomeKind::Threw,
        };
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
        };
        let turn = CellTurn {
            elapsed_ms: started.elapsed().as_millis() as u64,
            table,
            stdout_tail,
            stdout_dropped_tokens,
            record,
        };

        match ending {
            Ending::Yielded => CellOutcome::Yielded { turn },
            Ending::Returned(value) => CellOutcome::Returned { value, turn },
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

    /// The `RuntimeOutOfMemory` preview: the five largest live handles, so
    /// the *model* can choose what to free. Nothing is evicted here — §2 is
    /// explicit that a handle vanishing under a program that still names it
    /// is the one failure that would make the channel untrustworthy.
    fn out_of_memory(&mut self) -> ErrorValue {
        let table = self.state.table.borrow();
        let largest = table.largest(5);
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
    Yielded,
    Returned(Value),
    Threw(ErrorValue),
    /// V8 stopped the cell. [`Stopped`] says which of the two ceilings did
    /// it, because the model is told a different thing by each.
    Terminated,
}

/// Which ceiling stopped the cell, read the instant it stopped.
///
/// Both flags are consumed once, before any of the bookkeeping that follows
/// a cell allocates: taking a preview can raise the heap callback itself,
/// and a hit raised there would report a timeout as an out-of-memory.
#[derive(Debug, Clone, Copy)]
struct Stopped {
    heap_hit: bool,
    timed_out: bool,
}

impl Stopped {
    /// A cell that never reached V8 — one that did not compile.
    fn none() -> Self {
        Self {
            heap_hit: false,
            timed_out: false,
        }
    }
}

/// The wall clock, as one thread per cell.
///
/// It holds a clone of the isolate's `IsolateHandle` — the same `Send` handle
/// the near-heap-limit callback uses — waits on a condition variable for the
/// cell to finish, and calls `terminate_execution` if the wait times out
/// first. The condition variable rather than a sleep loop is what makes
/// [`Watchdog::disarm`] return immediately for the overwhelming majority of
/// cells, which finish in milliseconds.
struct Watchdog {
    done: Arc<(Mutex<bool>, Condvar)>,
    fired: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watchdog {
    fn arm(isolate: Option<v8::IsolateHandle>, limit: Duration) -> Self {
        let done = Arc::new((Mutex::new(false), Condvar::new()));
        let fired = Arc::new(AtomicBool::new(false));
        let thread = isolate.map(|isolate| {
            let done = Arc::clone(&done);
            let fired = Arc::clone(&fired);
            std::thread::spawn(move || {
                let (lock, finished) = &*done;
                let guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
                let (done, timeout) = finished
                    .wait_timeout_while(guard, limit, |done| !*done)
                    .unwrap_or_else(PoisonError::into_inner);
                if timeout.timed_out() && !*done {
                    // Ordered before the terminate so the flag is visible to
                    // `disarm`, which cannot run until this thread releases
                    // the lock it is still holding.
                    fired.store(true, Ordering::SeqCst);
                    isolate.terminate_execution();
                }
            })
        });
        Self {
            done,
            fired,
            thread,
        }
    }

    /// Stops the watch and answers whether it fired.
    fn disarm(mut self) -> bool {
        self.stop();
        self.fired.load(Ordering::SeqCst)
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
