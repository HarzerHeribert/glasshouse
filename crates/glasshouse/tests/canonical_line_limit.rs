//! A delivery that would wedge a session's input is refused, not written.
//!
//! # The defect
//!
//! `SessionRuntime::send_text_from` used to write a caller's text straight
//! into the session's pseudo-terminal. A terminal in **canonical mode** —
//! every harness that has not yet put its own tty into raw mode, and every
//! shell — assembles input one line at a time in a bounded kernel buffer.
//! Overflow that buffer and the caller's data is lost, silently, having been
//! answered `ok`.
//!
//! **How it is lost is a per-kernel fact, and this file no longer assumes
//! one.** On macOS and the BSDs the excess is discarded together with the
//! line's terminator, so the line never reaches the reader, the buffer stays
//! full, and every byte written to that terminal afterwards is discarded too:
//! the session is deaf for the rest of its life. On Linux the line is
//! delivered *truncated* and the terminal keeps working — quieter, and not
//! obviously better, because a shell handed a truncated command runs it. Both
//! are silent data loss, which is why one refusal is the right answer to
//! both; see `glasshouse::pty::CanonicalOverflow`.
//!
//! # What is proven here, and in what order
//!
//! Practice §59: reproduce the *state*, not the trigger. The state is a live
//! canonical-mode terminal with a reader on the other end, so every test
//! below drives a real pty.
//!
//! 1. [`the_compiled_limit_is_the_terminals_own_and_one_byte_over_it_loses_data`]
//!    — the hazard itself, at the `PtyProcess` layer that sits *below* the fix,
//!    and the compiled ceiling measured against the kernel in both directions.
//! 2. [`a_line_over_the_terminals_limit_is_refused_and_the_session_survives`]
//!    — the fix: refused by name, and the session still takes input.
//! 3. [`a_line_at_the_terminals_limit_still_arrives_intact`] — the refusal
//!    starts exactly one byte late.
//! 4. [`a_raw_mode_terminal_takes_a_line_far_over_the_canonical_limit`] — the
//!    limit is conditional, and a raw-mode harness is not regressed.
//! 5. [`a_refused_line_is_never_recorded_as_delivered`] — the event log does
//!    not claim a delivery that did not happen.
//! 6. [`the_memory_injection_ceiling_still_clears_the_terminals_own`] — the
//!    900-byte belt-and-braces bound in `memory::inject` still holds.
//!
//! # Why this file is Unix-only
//!
//! There is no canonical mode to overflow on Windows: ConPTY is a screen
//! buffer rather than a tty (practice §21) and has no line discipline at all.
//! `PtyProcess::line_discipline` reports `LineDiscipline::Unknown` there and
//! nothing is enforced, which is the honest platform-specific answer §25 asks
//! for rather than a Unix ceiling applied where it means nothing.

#![cfg(unix)]

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glasshouse::Project;
use glasshouse::events::{EventBus, LifecycleEvent, MessageOrigin, Subscription};
use glasshouse::launch::HarnessLaunch;
use glasshouse::memory::inject::MAX_INJECTED_BYTES;
use glasshouse::platform::exec;
use glasshouse::pty::{
    CanonicalLine, CanonicalOverflow, LineDiscipline, ProcessSignal, PtyOutput, PtyProcess,
    TerminalCommand,
};
use glasshouse::session::{RuntimeError, SessionId, SessionPresentation, SessionRuntime};

/// Upper bound for any single wait. Generous enough for a loaded machine,
/// short enough that a genuine wedge fails instead of stalling.
const TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(10);

/// How long a delivery that is *expected to vanish* is waited for before
/// concluding it vanished.
///
/// Much shorter than [`TIMEOUT`], because this is the negative case and every
/// test that uses it pays it in full. A line that is going to arrive arrives
/// in single-digit milliseconds on the same machine — the positive waits below
/// measure that — so half a second is two orders of magnitude of headroom for
/// an absence that would otherwise be indistinguishable from slowness.
const ABSENCE: Duration = Duration::from_millis(500);

/// How many times the boundary cases are run.
///
/// Practice §60: a single trial cannot separate "fixed" from "fixed most of
/// the time". This defect is not a race — the boundary reproduced identically
/// on every trial — and a count is recorded rather than inferred so the next
/// reader sees `0 in 20` instead of guessing `0` from `ok`.
const TRIALS: usize = 20;

// ---------------------------------------------------------------------------
// A pty with a reader on the other end.
// ---------------------------------------------------------------------------

struct Collector {
    buffer: Arc<Mutex<Vec<u8>>>,
    ended: Arc<AtomicBool>,
}

impl Collector {
    fn start(mut output: PtyOutput) -> Self {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let ended = Arc::new(AtomicBool::new(false));
        let thread_buffer = Arc::clone(&buffer);
        let thread_ended = Arc::clone(&ended);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            loop {
                match output.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => thread_buffer.lock().unwrap().extend_from_slice(&chunk[..n]),
                }
            }
            thread_ended.store(true, Ordering::SeqCst);
        });
        Self { buffer, ended }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(
            &self
                .buffer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .into_owned()
    }

    fn forget(&self) {
        self.buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    fn saw(&self, needle: &str, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        loop {
            if self.text().contains(needle) {
                return true;
            }
            if Instant::now() >= deadline || self.ended.load(Ordering::SeqCst) {
                return self.text().contains(needle);
            }
            std::thread::sleep(POLL);
        }
    }
}

/// The child every test here talks to: it reads whole lines and reports each
/// one's **length**, never its content.
///
/// Reporting the length is what makes an assertion possible without echoing a
/// thousand `x`s through the assertion message, and it is also what proves
/// arrival *intact* rather than merely truncated — a line cut anywhere would
/// report a different number.
///
/// `extra` runs before the loop, which is how the raw-mode case puts the tty
/// into the state under test using the tty's own tooling rather than
/// Glasshouse's.
fn line_length_reporter_script(extra: &str) -> String {
    format!("{extra}while IFS= read -r line; do printf 'GOT:%s\\n' \"${{#line}}\"; done")
}

fn line_length_reporter(extra: &str) -> TerminalCommand {
    TerminalCommand::new("/bin/sh", std::env::temp_dir())
        .arg("-c")
        .arg(line_length_reporter_script(extra))
}

/// A pty running the reporter, already known to be reading.
struct Pty {
    process: PtyProcess,
    collector: Collector,
}

impl Pty {
    fn start(extra: &str) -> Self {
        let (mut process, output) = PtyProcess::spawn(line_length_reporter(extra)).expect("spawn");
        let collector = Collector::start(output);
        // Established, not assumed: until a line has made the round trip the
        // child may not have reached its `read` (nor, in the raw case, run
        // its `stty`), and a later absence would prove nothing.
        process.write_input(b"ready\r").expect("write");
        assert!(
            collector.saw("GOT:5", TIMEOUT),
            "the child never started reading lines; output so far:\n{}",
            collector.text()
        );
        collector.forget();
        Self { process, collector }
    }

    /// This terminal's ceiling *and* what it does above it, from the same
    /// reading of the same kernel — so a test cannot end up asserting one
    /// platform's hazard against another platform's number.
    fn line(&self) -> CanonicalLine {
        match self.process.line_discipline() {
            LineDiscipline::Canonical(line) => line,
            other => panic!("this test needs a canonical-mode terminal; got {other:?}"),
        }
    }

    fn limit(&self) -> usize {
        self.line().max_bytes()
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.process.signal(ProcessSignal::Kill);
    }
}

/// A line of `payload` bytes plus the carriage return `SessionApi::send_text`
/// appends, so `total` is what the kernel's line buffer must hold.
fn line_of(total: usize) -> String {
    let mut line = "x".repeat(total - 1);
    line.push('\r');
    assert_eq!(line.len(), total);
    line
}

// ---------------------------------------------------------------------------
// 1. The hazard, below the fix.
// ---------------------------------------------------------------------------

/// The platform fact the refusal exists for, and the compiled constant that
/// names it, measured against a real terminal in both directions.
///
/// # Why this brackets rather than only reproducing
///
/// The one-byte-over half is the hazard. That alone would pass with any
/// ceiling at all, because a line over *any* ceiling still overflows a real
/// one. So the at-the-limit half is here too, and it is what ties
/// `CanonicalLine::max_bytes` to the kernel: a compiled value larger than the
/// platform's real ceiling makes this half fail, because the line it says
/// should arrive does not.
///
/// Together they say the constant is neither too high (data would still be
/// lost) nor too low (deliveries would be refused that work), on whichever
/// platform the test runs on — which is the claim `CanonicalLine::max_bytes`
/// makes in prose and could not otherwise back.
///
/// # The hazard is not the same on every platform, so it is asked, not assumed
///
/// This test used to assert one hazard everywhere: the over-long line is gone
/// **and so is the terminal**. That is macOS's answer. On Linux the line
/// arrives *truncated* and the terminal keeps working — so the wedge half
/// failed there, correctly, saying the hazard it was written for does not
/// exist on that kernel.
///
/// The repair is not to weaken the assertion into one that passes everywhere
/// — that would leave this file asserting a refusal against nothing. It is to
/// take the platform's own answer from
/// [`glasshouse::pty::CanonicalLine::overflow`], which is production code's
/// claim about the kernel, and demand that the kernel back **that** claim.
/// Each branch below is as strong as the single one it replaced, and a
/// platform whose behaviour is misdeclared fails here rather than passing
/// vacuously.
///
/// # Why it goes under the fix
///
/// Through `PtyProcess::write_input`, beneath `SessionRuntime`'s check, on
/// purpose. It is not testing Glasshouse; it is establishing that the thing
/// Glasshouse now refuses genuinely loses the caller's data, so the refusal
/// is not a limit invented for its own sake. If this ever stops reproducing,
/// the refusal above it should be re-argued rather than kept out of habit.
///
/// A fresh pty per case, because a wedged canonical buffer never recovers and
/// would contaminate everything after it. [`TRIALS`] of each.
#[test]
fn the_compiled_limit_is_the_terminals_own_and_one_byte_over_it_loses_data() {
    for trial in 1..=TRIALS {
        {
            let mut pty = Pty::start("");
            let limit = pty.limit();
            let at = line_of(limit);
            pty.process.write_input(at.as_bytes()).expect("write");
            assert!(
                pty.collector.saw(&format!("GOT:{}", limit - 1), TIMEOUT),
                "trial {trial}/{TRIALS}: a line of exactly {limit} bytes did not \
                 arrive whole, so {limit} is above this terminal's real ceiling \
                 and the compiled constant is wrong; output:\n{}",
                pty.collector.text()
            );
            pty.collector.forget();
            pty.process.write_input(b"abcd\r").expect("write");
            assert!(
                pty.collector.saw("GOT:4", TIMEOUT),
                "trial {trial}/{TRIALS}: a line of exactly {limit} bytes left the \
                 terminal deaf, so {limit} is above its real ceiling"
            );
        }
        {
            let mut pty = Pty::start("");
            let line = pty.line();
            let limit = line.max_bytes();
            let over = line_of(limit + 1);
            pty.process.write_input(over.as_bytes()).expect("write");

            // Both platforms agree on this much, and it is the half that
            // catches a ceiling compiled too low: a line one byte over must
            // not arrive whole. If the constant were below the terminal's
            // real ceiling this line would sail through intact and report its
            // full length.
            let whole = format!("GOT:{limit}");

            match line.overflow() {
                CanonicalOverflow::WedgesTheTerminal => {
                    assert!(
                        !pty.collector.saw(&whole, ABSENCE),
                        "trial {trial}/{TRIALS}: a {}-byte line was expected to be \
                         discarded by the line discipline, so {limit} is above this \
                         terminal's real ceiling",
                        limit + 1
                    );
                    pty.collector.forget();
                    pty.process.write_input(b"abcd\r").expect("write");
                    assert!(
                        !pty.collector.saw("GOT:4", ABSENCE),
                        "trial {trial}/{TRIALS}: the over-long line did not wedge the \
                         terminal, so every assertion in this file about the refusal is \
                         about a hazard that no longer exists on this platform; output:\n{}",
                        pty.collector.text()
                    );
                }
                CanonicalOverflow::TruncatesTheLine => {
                    // The hazard here is quieter and needs a *positive*
                    // assertion to have any force: the line arrives, and it
                    // arrives short. Asserting only that it did not arrive
                    // whole would also pass if the child had died.
                    assert!(
                        pty.collector.saw(&format!("GOT:{}", limit - 1), TIMEOUT),
                        "trial {trial}/{TRIALS}: a {}-byte line was expected to arrive \
                         truncated to this terminal's ceiling of {limit} bytes, \
                         reporting {} bytes of payload; output:\n{}",
                        limit + 1,
                        limit - 1,
                        pty.collector.text()
                    );
                    assert!(
                        !pty.collector.saw(&whole, ABSENCE),
                        "trial {trial}/{TRIALS}: a {}-byte line arrived whole, so \
                         {limit} is below this terminal's real ceiling and the compiled \
                         constant refuses deliveries that would have worked",
                        limit + 1
                    );
                    pty.collector.forget();
                    pty.process.write_input(b"abcd\r").expect("write");
                    assert!(
                        pty.collector.saw("GOT:4", TIMEOUT),
                        "trial {trial}/{TRIALS}: the over-long line left the terminal \
                         deaf, so this platform wedges rather than truncates and \
                         `CanonicalOverflow::TruncatesTheLine` is the wrong declaration \
                         for it; output:\n{}",
                        pty.collector.text()
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 2. The fix.
// ---------------------------------------------------------------------------

/// A line the terminal would discard is refused by name, and — the half that
/// is the whole point — **the session still works afterwards**.
///
/// A fix that refuses and still wedges has fixed nothing, so the surviving
/// short line is asserted every trial, not once.
#[test]
fn a_line_over_the_terminals_limit_is_refused_and_the_session_survives() {
    for trial in 0..TRIALS {
        let harness = LiveHarness::start("");
        let limit = harness.limit();
        let over = line_of(limit + 1);

        let refused = harness
            .runtime()
            .send_text(harness.id(), &over)
            .expect_err("an over-long line must be refused");

        assert!(
            matches!(
                refused,
                RuntimeError::LineTooLong { bytes, limit: reported, overflow, .. }
                    if bytes == limit + 1
                        && reported == limit
                        && overflow == harness.line().overflow()
            ),
            "trial {trial}: wrong refusal: {refused:?}"
        );
        // The refusal must describe the hazard this kernel actually has. The
        // sentence used to be macOS's, unconditionally, which made it a false
        // statement on every Linux session it was ever shown on.
        let hazard = harness.line().overflow().to_string();
        assert!(
            refused.to_string().contains(&hazard),
            "trial {trial}: the refusal must say what this terminal would really \
             have done ({hazard}): {refused}"
        );
        assert!(
            refused.to_string().contains(harness.id().as_str()),
            "the refusal must name the session: {refused}"
        );
        // The security invariant: length and limit, never the caller's text.
        assert!(
            !refused.to_string().contains("xxxx"),
            "the refusal must not echo the caller's line back: {refused}"
        );

        harness
            .runtime()
            .send_text(harness.id(), "abcd\r")
            .expect("a short line after a refusal");
        assert!(
            harness.saw("GOT:4", TIMEOUT),
            "trial {trial}: the session was wedged by a line that was supposedly \
             never written; output:\n{}",
            harness.output()
        );
    }
}

/// The refusal starts exactly one byte late: a line *at* the terminal's limit
/// still arrives, whole.
///
/// Asserted on the reported length rather than on arrival alone — a truncated
/// line would still produce a `GOT:` line, and this is the test that would
/// catch a fix that silently cut the caller's text instead of refusing it.
#[test]
fn a_line_at_the_terminals_limit_still_arrives_intact() {
    for trial in 0..TRIALS {
        let harness = LiveHarness::start("");
        let limit = harness.limit();
        let at = line_of(limit);

        harness
            .runtime()
            .send_text(harness.id(), &at)
            .expect("a line at the limit must not be refused");
        assert!(
            harness.saw(&format!("GOT:{}", limit - 1), TIMEOUT),
            "trial {trial}: a {limit}-byte line did not arrive whole; output:\n{}",
            harness.output()
        );
    }
}

/// A raw-mode terminal has no canonical buffer and no ceiling, and Glasshouse
/// must not invent one for it.
///
/// This is the regression the conditional exists to avoid: every harness TUI
/// Glasshouse drives runs its tty in raw mode, and a flat unconditional
/// refusal would have stopped long tasks reaching all of them. Twice the
/// canonical limit, delivered through the same seam, arriving whole.
#[test]
fn a_raw_mode_terminal_takes_a_line_far_over_the_canonical_limit() {
    let harness = LiveHarness::start("stty -icanon -echo\n");
    assert_eq!(
        harness.discipline(),
        LineDiscipline::Raw,
        "this test is vacuous unless the child's tty is really in raw mode"
    );

    let canonical_limit = Pty::start("").limit();
    let long = line_of(canonical_limit * 2 + 1);
    harness
        .runtime()
        .send_text(harness.id(), &long)
        .expect("a raw-mode terminal must not be refused a long line");
    assert!(
        harness.saw(&format!("GOT:{}", canonical_limit * 2), TIMEOUT),
        "the long line did not arrive whole; output:\n{}",
        harness.output()
    );
}

/// `LifecycleEvent::TextDelivered` records a delivery's size, so a refused
/// delivery must publish nothing at all.
///
/// The successful send that follows is what makes the absence meaningful: it
/// proves the log was reachable and recording the whole time, so the missing
/// event is a refusal rather than a bus nobody was listening to.
#[test]
fn a_refused_line_is_never_recorded_as_delivered() {
    let events = EventBus::new();
    let log = events.subscribe();
    let harness = LiveHarness::with_events("", events);
    let limit = harness.limit();

    // The harness's own startup line, and proof the bus was recording before
    // any of the three deliveries below — which is what makes the *absence*
    // asserted at the end mean something.
    let startup = delivered_sizes(&log);
    assert_eq!(startup, vec!["ready\r".len()]);

    let refused = harness
        .runtime()
        .send_text_from(harness.id(), &line_of(limit + 1), MessageOrigin::Machine)
        .expect_err("an over-long line must be refused");
    assert!(matches!(refused, RuntimeError::LineTooLong { .. }));

    harness
        .runtime()
        .send_text_from(harness.id(), "abcd\r", MessageOrigin::Machine)
        .expect("a short line after a refusal");
    assert!(harness.saw("GOT:4", TIMEOUT));

    assert_eq!(
        delivered_sizes(&log),
        vec!["abcd\r".len()],
        "the refused line must leave no trace in the event log; the only \
         delivery recorded may be the one that happened"
    );
}

/// The byte counts of every `TextDelivered` waiting on `log`, draining it.
fn delivered_sizes(log: &Subscription) -> Vec<usize> {
    log.drain()
        .into_iter()
        .filter_map(|recorded| match recorded.event() {
            LifecycleEvent::TextDelivered { bytes, .. } => Some(*bytes),
            _ => None,
        })
        .collect()
}

/// `memory::inject`'s own 900-byte ceiling is a belt-and-braces bound that
/// this fix does not license removing, and it must still clear the terminal's.
///
/// Two claims, and the second is the one worth having: the constant is
/// unchanged, *and* a block of exactly that size still reaches a
/// canonical-mode session through the same seam. If a future platform's
/// `MAX_CANON` ever dropped below 900 this would fail here rather than in
/// production.
#[test]
fn the_memory_injection_ceiling_still_clears_the_terminals_own() {
    assert_eq!(MAX_INJECTED_BYTES, 900);

    let harness = LiveHarness::start("");
    assert!(
        MAX_INJECTED_BYTES < harness.limit(),
        "memory injection's ceiling ({MAX_INJECTED_BYTES}) no longer clears this \
         platform's canonical line limit ({})",
        harness.limit()
    );

    // A full-size injection, sent exactly the way `deliver_memory` sends one.
    let block = "m".repeat(MAX_INJECTED_BYTES);
    harness
        .runtime()
        .send_text_from(harness.id(), &format!("{block}\r"), MessageOrigin::Machine)
        .expect("a full-size injection must still be delivered");
    assert!(
        harness.saw(&format!("GOT:{MAX_INJECTED_BYTES}"), TIMEOUT),
        "the injection did not arrive whole; output:\n{}",
        harness.output()
    );
}

// ---------------------------------------------------------------------------
// The same child, reached the way production reaches it.
// ---------------------------------------------------------------------------

/// A `SessionRuntime` holding one live session running the line-length
/// reporter, entered through `HarnessLaunch` — the sanctioned seam — so these
/// tests exercise the production path rather than a pty the test made itself.
///
/// Output is read back from the session's own scrollback, which is where the
/// runtime's reader thread puts it. That is deliberate: it is the same buffer
/// `SessionApi::output` serves to the machine door, so a line these tests
/// call "arrived" is a line a caller could actually have observed arriving.
struct LiveHarness {
    _tmp: tempfile::TempDir,
    runtime: Mutex<SessionRuntime>,
    id: SessionId,
    /// Everything the session has printed up to the last [`LiveHarness::saw`]
    /// that ran, so [`LiveHarness::forget`] can draw a line under it.
    ignore_before: Mutex<usize>,
}

impl LiveHarness {
    fn start(extra: &str) -> Self {
        Self::with_events(extra, EventBus::new())
    }

    fn with_events(extra: &str, events: EventBus) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_dir = tmp.path().join("proj");
        std::fs::create_dir_all(project_dir.join(".git")).expect("create project");
        let project = Project::discover(&project_dir, None, false).expect("discover project");

        let harness_path = tmp.path().join("reporter");
        std::fs::write(
            &harness_path,
            format!("#!/bin/sh\n{}\n", line_length_reporter_script(extra)),
        )
        .expect("write reporter harness");
        let mut perms = std::fs::metadata(&harness_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&harness_path, perms).unwrap();

        let resolved = exec::resolve_explicit(&harness_path).expect("resolve reporter");
        let launch = HarnessLaunch::new(resolved, &project);

        let mut runtime = SessionRuntime::with_event_bus(256 * 1024, events);
        let id = SessionId::new("canonical-line-limit");
        runtime
            .start(id.clone(), SessionPresentation::Embedded, &launch)
            .expect("start reporter session");

        let harness = Self {
            _tmp: tmp,
            runtime: Mutex::new(runtime),
            id,
            ignore_before: Mutex::new(0),
        };
        // Established, not assumed: until a line has made the round trip the
        // child may not have reached its `read` — nor, in the raw case, run
        // its `stty` — and a later absence would prove nothing.
        harness
            .runtime()
            .send_text(&harness.id, "ready\r")
            .expect("the reporter session must accept its first line");
        assert!(
            harness.saw("GOT:5", TIMEOUT),
            "the reporter session never started reading lines; output:\n{}",
            harness.output()
        );
        harness.forget();
        harness
    }

    /// Poisoning is treated as ownership, never as a reason to give up.
    ///
    /// Every assertion below holds this guard across the call it is asserting
    /// on — `harness.runtime().send_text(..).expect(..)` keeps the temporary
    /// alive through the `expect` — so a failing assertion poisons the mutex
    /// on its way out. `Drop` then runs during unwinding and would panic a
    /// second time on `.unwrap()`, which aborts the process and turns a
    /// legible one-line failure into a dead test binary. Recovering the inner
    /// value keeps the first panic the only panic, and it is the same ruling
    /// `SessionRuntime::resize` and the machine door's `lock` already make.
    fn runtime(&self) -> std::sync::MutexGuard<'_, SessionRuntime> {
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn id(&self) -> &SessionId {
        &self.id
    }

    fn discipline(&self) -> LineDiscipline {
        self.runtime()
            .get(&self.id)
            .expect("session held")
            .line_discipline()
    }

    fn line(&self) -> CanonicalLine {
        match self.discipline() {
            LineDiscipline::Canonical(line) => line,
            other => panic!("this test needs a canonical-mode terminal; got {other:?}"),
        }
    }

    fn limit(&self) -> usize {
        self.line().max_bytes()
    }

    /// Everything the session has printed since the last [`LiveHarness::forget`].
    fn output(&self) -> String {
        let all = self
            .runtime()
            .get(&self.id)
            .expect("session held")
            .scrollback();
        let from = *self
            .ignore_before
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        all.get(from..).unwrap_or(&all).to_owned()
    }

    /// Draw a line under the output so far, so the next `GOT:` assertion
    /// cannot be satisfied by an earlier round's answer.
    fn forget(&self) {
        let len = self
            .runtime()
            .get(&self.id)
            .expect("session held")
            .scrollback()
            .len();
        *self
            .ignore_before
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = len;
    }

    fn saw(&self, needle: &str, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        loop {
            if self.output().contains(needle) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(POLL);
        }
    }
}

impl Drop for LiveHarness {
    fn drop(&mut self) {
        let _ = self.runtime().close(&self.id);
    }
}
