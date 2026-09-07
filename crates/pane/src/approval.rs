//! A host-only, exact-call suspension seam. This is not a grant mechanism.
//!
//! A caller installs a gate on a runtime, receives requests on another thread,
//! and answers the one suspended Rust callback. The JavaScript cell stays on
//! its stack throughout; no source or continuation is returned for replay.
//! The immutable base profile must admit the call before a request is sent.
//! Neither a decision nor a remembered decision can add a sandbox capability.
//!
//! The shipped session does not install this seam. Interactive missing-grant
//! approvals require exact platform grants and deny rendering first; see
//! `docs/product/pane/sandbox-grants.md` §8.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::tools::invoke::CheckedArgs;

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
}

impl Request {
    pub fn action(&self) -> &Action {
        &self.action
    }

    /// Returns false when the waiting callback has ended. A queued reply may
    /// still be denied if cancellation is observed before it is consumed.
    pub fn respond(self, decision: Decision) -> bool {
        self.reply.send(decision).is_ok()
    }
}

/// A session-scoped host channel. Clones share exact session decisions, but
/// the gate is never inherited by `agent.run` or background jobs.
#[derive(Clone)]
pub struct Gate {
    requests: mpsc::Sender<Request>,
    remembered: Arc<Mutex<BTreeSet<Action>>>,
}

impl Gate {
    pub fn channel() -> (Self, mpsc::Receiver<Request>) {
        let (requests, receiver) = mpsc::channel();
        (
            Self {
                requests,
                remembered: Arc::new(Mutex::new(BTreeSet::new())),
            },
            receiver,
        )
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
        let (reply, response) = mpsc::sync_channel(1);
        if self
            .requests
            .send(Request {
                action: action.clone(),
                reply,
            })
            .is_err()
        {
            return false;
        }
        loop {
            if stopped() {
                return false;
            }
            match response.recv_timeout(Duration::from_millis(20)) {
                Ok(decision) => {
                    // A response queued before a cancellation is still denied
                    // if the call has stopped before it can consume that answer.
                    if stopped() {
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
