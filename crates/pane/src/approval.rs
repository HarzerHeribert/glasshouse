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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::tools::invoke::CheckedArgs;

/// Human confirmation is bounded independently of the cell compute clock.
pub const MAX_APPROVAL_WAIT: Duration = Duration::from_secs(10 * 60);

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
    fn pause(self: &Arc<Self>) -> Waiting {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .since = Some(Instant::now());
        Waiting(self.clone())
    }
}
struct Waiting(Arc<WaitClock>);
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
}

impl Request {
    pub fn is_pending(&self) -> bool {
        self.pending.load(Ordering::SeqCst)
    }
    pub fn action(&self) -> &Action {
        &self.action
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
}

impl Gate {
    pub fn channel() -> (Self, mpsc::Receiver<Request>) {
        let (requests, receiver) = mpsc::channel();
        (
            Self {
                requests,
                remembered: Arc::new(Mutex::new(BTreeSet::new())),
                wait_clock: None,
            },
            receiver,
        )
    }

    pub(crate) fn with_wait_clock(mut self) -> Self {
        self.wait_clock = Some(Arc::new(WaitClock::default()));
        self
    }

    pub(crate) fn wait_clock(&self) -> Option<Arc<WaitClock>> {
        self.wait_clock.clone()
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
        if self
            .requests
            .send(Request {
                action: action.clone(),
                reply,
                pending,
            })
            .is_err()
        {
            return false;
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
