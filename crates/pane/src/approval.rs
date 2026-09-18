//! A host-only, exact-call suspension seam. This is not a grant mechanism.
//!
//! A caller installs a gate on a runtime, receives requests on another thread,
//! and answers the one suspended Rust callback. The JavaScript cell stays on
//! its stack throughout; no source or continuation is returned for replay.
//! The immutable base profile must admit the call before a request is sent.
//! Neither a decision nor a remembered decision can add a sandbox capability.
//!
//! The live terminal can install this seam for explicit exact-call approval.
//! Interactive missing-grant approvals still require platform grants; see
//! `docs/product/pane/sandbox-grants.md` §8.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::tools::invoke::CheckedArgs;

/// Human confirmation is bounded independently of the cell compute clock.
pub const MAX_APPROVAL_WAIT: Duration = Duration::from_secs(10 * 60);

/// How long a cell has spent **not executing JavaScript**.
///
/// The cell's wall-clock limit exists to stop `while (true) {}`, which
/// allocates nothing and so is invisible to the heap ceiling. Time spent
/// inside a host callback -- a person deciding, a `cargo test` the cell was
/// granted, a helper answering -- is not that, and the runtime subtracts it
/// (`Watchdog::arm_pausing`). One clock serves every such wait: they differ
/// in what is being waited for and not in what the cell is doing, which is
/// nothing.
#[derive(Default)]
pub(crate) struct WaitClock(Mutex<WaitState>);
#[derive(Default)]
struct WaitState {
    accumulated: Duration,
    since: Option<Instant>,
}
impl WaitClock {
    pub(crate) fn elapsed(&self) -> Duration {
        let state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.accumulated + state.since.map(|since| since.elapsed()).unwrap_or_default()
    }
    /// Stops the clock until the returned guard is dropped. Re-entrant
    /// waits are not nested: the outer guard owns the span, because
    /// `since` is a single instant and a nested pause would end it early.
    pub(crate) fn pause(self: &Arc<Self>) -> Waiting {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .since = Some(Instant::now());
        Waiting(self.clone())
    }
}
pub(crate) struct Waiting(Arc<WaitClock>);
impl Drop for Waiting {
    fn drop(&mut self) {
        let mut state = self
            .0
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(since) = state.since.take() {
            state.accumulated += since.elapsed();
        }
    }
}

/// The answer to one exact action, never a pattern or a profile edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    AllowOnce,
    AllowForSession,
    Deny,
}

/// The decision model's answer to "does this call fit the request" (F4,
/// `decision-model.md`) -- informational only. It never changes [`Decision`],
/// and it arrives on its own thread, after the confirmation is already shown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hint {
    pub fits: f64,
    pub asked_ms: u64,
}

/// The `[decisions]` model and mode, attached to a [`Gate`] once at session
/// start (`with_decisions`). Shared by every clone of that gate, including
/// the per-task clone a `Runtime` holds, so the counts below are session-wide.
#[derive(Clone)]
struct Decisions {
    model: String,
    mode: crate::config::DecisionMode,
    /// This gate's cumulative count of approval hints that answered.
    asked: Arc<AtomicU32>,
    /// This gate's cumulative count of approval-hint requests that failed
    /// or timed out.
    failed: Arc<AtomicU32>,
}

/// A concrete canonical call. Full argument values are available only by an
/// explicit accessor for the confirmation surface; Debug and remembered-action
/// summaries omit them because command lines and file contents can be secrets.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Action {
    tool: String,
    root: String,
    arguments: CheckedArgs,
}

impl Action {
    pub(crate) fn new(tool: &str, root: &std::path::Path, arguments: CheckedArgs) -> Self {
        Self {
            tool: tool.into(),
            root: root.to_string_lossy().into_owned(),
            arguments,
        }
    }

    pub fn tool(&self) -> &str {
        &self.tool
    }

    pub fn root(&self) -> &str {
        &self.root
    }

    pub fn arguments(&self) -> &CheckedArgs {
        &self.arguments
    }

    /// A non-secret identifier for a session permissions view. Decisions are
    /// matched by the full action value, not by this short display hash.
    pub fn summary(&self) -> String {
        let bytes = serde_json::to_vec(&(&self.tool, &self.root, &self.arguments))
            .expect("canonical actions contain only strings");
        let hash = Sha256::digest(bytes);
        let hex = format!("{hash:x}");
        format!("{} · exact action {}", self.tool, &hex[..12])
    }

    /// A bounded, terminal-safe description. Approval is disabled when the
    /// complete action does not fit the display budget.
    pub fn confirmation(&self) -> Confirmation {
        Confirmation::new(&self.tool, &self.root, &self.arguments)
    }
}

#[derive(Debug, Clone)]
pub struct Confirmation {
    pub text: String,
    pub complete: bool,
}

impl Confirmation {
    pub fn new(tool: &str, root: &str, arguments: &CheckedArgs) -> Self {
        let text = serde_json::to_string_pretty(&serde_json::json!({
            "tool": tool, "workspace": root, "arguments": arguments,
        }))
        .expect("checked arguments contain strings");
        // JSON escapes ASCII controls; escape Unicode formatting controls as
        // well so bidirectional text cannot reorder the confirmation surface.
        let escaped: String = text.chars().flat_map(|c| {
            if c != '\n' && (c.is_control() || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')) {
                c.escape_unicode().collect::<Vec<_>>()
            } else { vec![c] }
        }).collect();
        let complete = escaped.len() <= 16 * 1024;
        let text = if complete {
            escaped
        } else {
            "Action exceeds the 16 KiB confirmation limit. Approval is disabled; deny this call and ask for a smaller action.".into()
        };
        Self { text, complete }
    }
}

impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.summary())
    }
}

/// The reply sender belongs to this request and is consumed by `respond`.
/// A late reply or a dropped request cannot approve another call.
pub struct Request {
    action: Action,
    reply: mpsc::SyncSender<Decision>,
    pending: Arc<AtomicBool>,
    /// The approval hint, once the decision model has answered. Populated by
    /// a background thread `Gate::admit` spawns; `None` before it answers,
    /// on failure, or when no decision model is configured.
    hint: Arc<Mutex<Option<Hint>>>,
    /// Whether [`Self::hint_line`] may surface the hint at all -- `mode =
    /// shadow` still fills [`Self::hint`] above, but never this gate.
    show_hint: bool,
}

impl Request {
    pub fn is_pending(&self) -> bool {
        self.pending.load(Ordering::SeqCst)
    }
    pub fn action(&self) -> &Action {
        &self.action
    }

    /// The recorded hint regardless of `mode` -- telemetry and tests read
    /// this; the confirmation surface reads [`Self::hint_line`] instead.
    pub fn hint(&self) -> Option<Hint> {
        *self
            .hint
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The hint the confirmation surface may show: `None` before an answer
    /// arrives, on failure, or with `mode = shadow` (recorded, never shown).
    pub fn hint_line(&self) -> Option<Hint> {
        if self.show_hint { self.hint() } else { None }
    }

    /// Returns false when the waiting callback has ended. A queued reply may
    /// still be denied if cancellation is observed before it is consumed.
    pub fn respond(self, decision: Decision) -> bool {
        self.is_pending() && self.reply.send(decision).is_ok()
    }
}

struct Pending(Arc<AtomicBool>);
impl Drop for Pending {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// A session-scoped host channel. Clones share exact session decisions, but
/// the gate is never inherited by `agent.run` or background jobs.
#[derive(Clone)]
pub struct Gate {
    requests: mpsc::Sender<Request>,
    remembered: Arc<Mutex<BTreeSet<Action>>>,
    wait_clock: Option<Arc<WaitClock>>,
    decisions: Option<Decisions>,
    /// The current task's request text, attached to the clone a `Runtime`
    /// holds for one task (`with_task`) -- `Gate` itself is session-scoped
    /// and outlives any one task.
    task: Option<String>,
}

impl Gate {
    pub fn channel() -> (Self, mpsc::Receiver<Request>) {
        let (requests, receiver) = mpsc::channel();
        (
            Self {
                requests,
                remembered: Arc::new(Mutex::new(BTreeSet::new())),
                wait_clock: None,
                decisions: None,
                task: None,
            },
            receiver,
        )
    }

    /// Attaches the `[decisions]` model and mode once, at session start
    /// (`decision-model.md`). `None` leaves every clone of this gate exactly
    /// as it is today: no thread, no request.
    pub(crate) fn with_decisions(
        mut self,
        model: Option<String>,
        mode: crate::config::DecisionMode,
    ) -> Self {
        self.decisions = model.map(|model| Decisions {
            model,
            mode,
            asked: Arc::new(AtomicU32::new(0)),
            failed: Arc::new(AtomicU32::new(0)),
        });
        self
    }

    /// Attaches the current task's request text to the clone a `Runtime`
    /// holds for one task -- the approval hint's one `noul` question asks
    /// whether a call fits this text.
    pub(crate) fn with_task(mut self, task: String) -> Self {
        self.task = Some(task);
        self
    }

    /// This gate's cumulative approval-hint counts: `(answered, failed)`,
    /// `(0, 0)` when no decision model is configured.
    pub(crate) fn hint_counts(&self) -> (u32, u32) {
        self.decisions.as_ref().map_or((0, 0), |decisions| {
            (
                decisions.asked.load(Ordering::Relaxed),
                decisions.failed.load(Ordering::Relaxed),
            )
        })
    }

    /// Attaches the clock the cell's watchdog subtracts, so a confirmation
    /// the person is still reading does not spend the cell's compute budget.
    /// The runtime passes **its own** clock, so an approval wait and a
    /// granted child process accrue into one span rather than two the
    /// watchdog would have to add up.
    pub(crate) fn with_wait_clock(mut self, clock: Arc<WaitClock>) -> Self {
        self.wait_clock = Some(clock);
        self
    }

    /// Values that can be displayed without leaking command arguments,
    /// file contents, or MCP parameters. This seam has no persisted state.
    pub fn session_actions(&self) -> Vec<String> {
        self.remembered
            .lock()
            .map(|actions| actions.iter().map(Action::summary).collect())
            .unwrap_or_default()
    }

    pub(crate) fn admit(&self, action: Action, stopped: impl Fn() -> bool) -> bool {
        if stopped() {
            return false;
        }
        let Ok(remembered) = self.remembered.lock() else {
            return false;
        };
        if remembered.contains(&action) {
            return !stopped();
        }
        drop(remembered);
        let _waiting = self.wait_clock.as_ref().map(WaitClock::pause);
        let waiting_started = Instant::now();
        let pending = Arc::new(AtomicBool::new(true));
        let _pending = Pending(pending.clone());
        let (reply, response) = mpsc::sync_channel(1);
        let hint = Arc::new(Mutex::new(None));
        let show_hint = self
            .decisions
            .as_ref()
            .is_some_and(|decisions| decisions.mode == crate::config::DecisionMode::On);
        if self
            .requests
            .send(Request {
                action: action.clone(),
                reply,
                pending,
                hint: hint.clone(),
                show_hint,
            })
            .is_err()
        {
            return false;
        }
        // The confirmation above is already sent to the human; this thread
        // never delays it. Human approval waits up to ten minutes (line 23),
        // and the decision model answers in ~0.5-1s (phase-66.md's Provider
        // facts) or times out at its own two-second bound (`decide.rs`), so
        // the hint is almost always ready before anyone reads the prompt.
        if let Some(decisions) = self.decisions.clone()
            && decisions.mode != crate::config::DecisionMode::Off
        {
            let task = self.task.clone().unwrap_or_default();
            let tool = action.tool().to_string();
            let summary = action.summary();
            thread::spawn(move || {
                let state = serde_json::json!({
                    "request": task,
                    "tool": tool,
                    "summary": summary,
                });
                let questions = [(
                    "fits".to_string(),
                    crate::decide::Question::Noul {
                        instructions: "The tool call fits what the request asked for \
                                       and does nothing beyond it."
                            .to_string(),
                    },
                )];
                match crate::decide::decide(&decisions.model, state, &questions) {
                    Ok(answers) => {
                        let answer = answers
                            .decisions
                            .into_iter()
                            .find(|decision| decision.key == "fits");
                        match answer {
                            Some(crate::decide::Decision {
                                answer: crate::decide::Answer::Noul(fits),
                                latency_ms,
                                ..
                            }) => {
                                decisions.asked.fetch_add(1, Ordering::Relaxed);
                                if let Ok(mut slot) = hint.lock() {
                                    *slot = Some(Hint {
                                        fits,
                                        asked_ms: latency_ms,
                                    });
                                }
                            }
                            _ => {
                                decisions.failed.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    Err(_) => {
                        decisions.failed.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        }
        loop {
            if stopped() || waiting_started.elapsed() >= MAX_APPROVAL_WAIT {
                return false;
            }
            match response.recv_timeout(Duration::from_millis(20)) {
                Ok(decision) => {
                    // A response queued before a cancellation is still denied
                    // if the call has stopped before it can consume that answer.
                    if stopped() || waiting_started.elapsed() >= MAX_APPROVAL_WAIT {
                        return false;
                    }
                    return match decision {
                        Decision::AllowOnce => true,
                        Decision::AllowForSession => {
                            let Ok(mut remembered) = self.remembered.lock() else {
                                return false;
                            };
                            remembered.insert(action);
                            true
                        }
                        Decision::Deny => false,
                    };
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return false,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}
