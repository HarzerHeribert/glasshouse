//! Knowing which of this project's recorded sessions are still running, and
//! refusing to start a second one beside them.
//!
//! Three rules this module exists to keep:
//! - **Only sessions this project recorded.** No process enumeration
//!   anywhere in this file — a control plane that scans the machine for
//!   things that look like harnesses will eventually adopt somebody else's.
//! - **Alive-and-disowned is its own condition**,
//!   [`Supervision::Quarantined`]: neither stopped nor healthy.
//! - **This module reports and refuses; it never ends anything** — no
//!   `kill`, no signal, no `Child::wait`.
//!   `nothing_in_supervision_ends_a_process` is the guard.
//!
//! Identity is `(process id, start time, host)`, never a bare pid: pids are
//! reused, and a foreign host's pid means nothing here. [`observe`] reads
//! the start time from the kernel, normalised to milliseconds since the Unix
//! epoch (Linux's native unit is ticks since boot and repeats after every
//! reboot). The process recorded is the Glasshouse process that created the
//! session row — `std::process::id()` at
//! [`super::store::SessionStore::create`] — which for `glasshouse launch`
//! blocks in `session::attach` for the session's whole life. See
//! `docs/product/evidence/phase-10a.md`.
// History: design-decisions.md, "Trims: session module docs, second packet", session/supervision.rs module doc.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use super::store::{
    SessionId, SessionLifecycle, SessionRecord, SessionStore, SessionStoreError, SupervisionRecord,
};

/// How long a session may sit in `starting` before a later Glasshouse calls
/// the start failed.
///
/// The bound above is enforced by the process that did the starting. This one
/// covers the case that process never got to enforce anything — it was killed,
/// or the machine went down, mid-start — and it is deliberately far larger, so
/// that a slow start is never mistaken for a failed one by a `glasshouse`
/// invocation that happens to run beside it.
pub const NEVER_READY_AFTER: Duration = Duration::from_secs(300);

/// A process, identified by something a later process cannot inherit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// The kernel's start time for that process, in milliseconds since the
    /// Unix epoch. See the module doc for why this unit and not each
    /// platform's own.
    pub started_at_ms: i64,
    /// The machine the process id is meaningful on.
    pub host: String,
}

impl ProcessIdentity {
    /// This Glasshouse process, or `None` when the platform would not say.
    ///
    /// `None` is a real answer and is treated as one everywhere downstream: a
    /// session recorded without an identity is never adopted, never
    /// quarantined and never assumed dead. Inventing a fallback identity —
    /// the pid alone, or a placeholder host — would make exactly the stale
    /// match this type exists to prevent.
    pub fn of_this_process() -> Option<Self> {
        Self::of(std::process::id())
    }

    /// The process with this id, as the kernel currently describes it.
    pub fn of(pid: u32) -> Option<Self> {
        let observed = observe(pid)?;
        Some(Self {
            pid,
            started_at_ms: observed.started_at_ms,
            host: host_name()?,
        })
    }
}

impl fmt::Display for ProcessIdentity {
    /// How long ago it started, not when.
    ///
    /// The stored value is milliseconds since the Unix epoch, which is the
    /// right thing to compare and the wrong thing to read: printing
    /// `1787830430528` at somebody who is deciding whether to end a runaway
    /// process tells them nothing they can act on. Found by running the
    /// binary — the message was correct and unusable.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "process {} on {}", self.pid, self.host)?;
        if let Some(age) = age_of(self.started_at_ms) {
            write!(f, ", started {age} ago")?;
        }
        Ok(())
    }
}

/// How long ago a moment was, in words, or `None` if it is in the future or
/// the clock will not say.
fn age_of(started_at_ms: i64) -> Option<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let now_ms = i64::try_from(now.as_millis()).ok()?;
    let elapsed = now_ms.checked_sub(started_at_ms)?;
    (elapsed >= 0).then(|| human_gap(elapsed))
}

/// A span of milliseconds as a person would say it.
///
/// Coarse on purpose — nobody deciding what to do about a stray process needs
/// milliseconds, and the whole point of the incident this phase exists for was
/// that three processes had been running for *nineteen hours* without anyone
/// noticing.
fn human_gap(ms: i64) -> String {
    let seconds = ms / 1000;
    match seconds {
        ..0 => "no time".to_owned(),
        0 => format!("{ms}ms"),
        1..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {}s", seconds / 60, seconds % 60),
        _ => format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

/// Whether a process is running or merely has not been reaped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Live,
    /// Exited, and still in the process table because nobody has waited for
    /// it. Not alive: a zombie holds no resources, answers nothing, and is
    /// never a session.
    Zombie,
}

/// What the kernel says about one process id, right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedProcess {
    pub started_at_ms: i64,
    pub state: ProcessState,
}

impl ObservedProcess {
    pub fn is_live(self) -> bool {
        self.state == ProcessState::Live
    }
}

/// What comparing a recorded identity against the machine concluded.
///
/// Four answers rather than a bool, because "I could not tell" splits two ways
/// and collapsing them is how a supervisor ends up reporting an unverifiable
/// record as stopped.
///
/// There is no variant for *"no identity was recorded"*. That is the absence
/// of the argument, not one of its answers, and it is an `Option` at every
/// call site — `verify` cannot be asked the question without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The identity was recorded on another machine, where its process id
    /// means something this machine cannot check.
    ForeignHost { recorded: String, actual: String },
    /// Nothing is running under that process id.
    Gone,
    /// Alive, and the start time is the one that was recorded.
    Verified,
    /// Alive, and it is not what was recorded. The dangerous one.
    Mismatched {
        recorded_started_at_ms: i64,
        observed_started_at_ms: i64,
    },
}

/// Compare a recorded identity against the machine this Glasshouse is on.
///
/// The host is checked *first*, and that ordering is the point: a record from
/// another machine must be reported as unverifiable rather than as a pid
/// comparison that happened to succeed or fail. Comparing first and blaming
/// the host afterwards would report a coincidence as a mismatch.
pub fn verify(recorded: &ProcessIdentity, this_host: &str) -> Verdict {
    if recorded.host != this_host {
        return Verdict::ForeignHost {
            recorded: recorded.host.clone(),
            actual: this_host.to_owned(),
        };
    }
    let Some(observed) = observe(recorded.pid) else {
        return Verdict::Gone;
    };
    if !observed.is_live() {
        return Verdict::Gone;
    }
    if observed.started_at_ms == recorded.started_at_ms {
        Verdict::Verified
    } else {
        Verdict::Mismatched {
            recorded_started_at_ms: recorded.started_at_ms,
            observed_started_at_ms: observed.started_at_ms,
        }
    }
}

// ------------------------------------------------------------------
// What supervision concluded, and how it is written down.
// ------------------------------------------------------------------

/// What supervision concluded about one recorded session's process.
///
/// The four words the schema's `CHECK` allows, encoded through an exhaustive
/// match so that adding a fifth is a compile error here rather than a
/// constraint violation on a writer nobody is watching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Supervision {
    /// Started by this Glasshouse process, this run.
    Owned,
    /// Verified still running under a process this Glasshouse did not start,
    /// and taken back into supervision rather than replaced.
    Adopted,
    /// Alive and unaccounted for. Never reused, never replaced, never
    /// reported as stopped.
    Quarantined,
    /// The recorded process is no longer running.
    Lost,
}

impl Supervision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owned => "owned",
            Self::Adopted => "adopted",
            Self::Quarantined => "quarantined",
            Self::Lost => "lost",
        }
    }

    /// Not [`std::str::FromStr`], deliberately: that trait's `Err` would have
    /// to be something, and the something here is *"a word this build does not
    /// know"*, which the store reports as
    /// [`super::store::SessionStoreError::UnknownValue`] naming the column.
    /// The same choice the rest of this module's stored vocabularies make —
    /// see `session::store`'s `stored_vocabulary!`.
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "owned" => Some(Self::Owned),
            "adopted" => Some(Self::Adopted),
            "quarantined" => Some(Self::Quarantined),
            "lost" => Some(Self::Lost),
            _ => None,
        }
    }
}

impl fmt::Display for Supervision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One session supervision reached a conclusion about, and everything a person
/// needs to act on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisedSession {
    pub id: SessionId,
    pub harness: String,
    pub supervision: Supervision,
    /// Why, in a sentence. A quarantine with no stated reason is an
    /// accusation.
    pub reason: String,
    pub identity: Option<ProcessIdentity>,
    /// What the session still holds, and therefore what a replacement would
    /// collide with. Named rather than counted: "still holds the claude-code
    /// conversation d23ab938-…" is actionable, "holds 2 resources" is not.
    pub holds: Vec<String>,
}

/// What one pass of [`reconcile`] concluded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SupervisionReport {
    pub adopted: Vec<SupervisedSession>,
    pub quarantined: Vec<SupervisedSession>,
    pub lost: Vec<SupervisedSession>,
    /// Starts that never became ready, recorded as failures.
    pub never_ready: Vec<SupervisedSession>,
}

impl SupervisionReport {
    pub fn is_empty(&self) -> bool {
        self.adopted.is_empty()
            && self.quarantined.is_empty()
            && self.lost.is_empty()
            && self.never_ready.is_empty()
    }

    /// Everything the user is owed about what was found, or `None` when there
    /// is nothing to say.
    ///
    /// Only the two conditions a person can act on are printed. An adopted
    /// session is working as intended and a lost one has already been recorded
    /// as stopped; announcing either on every invocation would train people to
    /// ignore the line that matters.
    pub fn describe(&self) -> Option<String> {
        if self.quarantined.is_empty() && self.never_ready.is_empty() {
            return None;
        }
        let mut out = String::new();
        for session in &self.quarantined {
            out.push_str(&format!(
                "glasshouse: session {} ({}) is quarantined: {}\n",
                short(&session.id),
                session.harness,
                session.reason
            ));
            if let Some(identity) = &session.identity {
                out.push_str(&format!("  it was recorded as {identity}\n"));
            }

            for held in &session.holds {
                out.push_str(&format!("  it still holds {held}\n"));
            }
            out.push_str(
                "  Glasshouse will not reuse it, replace it, or end it. \
                 Decide what to do with that process yourself.\n",
            );
        }
        for session in &self.never_ready {
            out.push_str(&format!(
                "glasshouse: session {} ({}) never started: {}\n",
                short(&session.id),
                session.harness,
                session.reason
            ));
        }
        Some(out)
    }
}

/// The first eight characters, which is how this project shows a session id.
fn short(id: &SessionId) -> &str {
    let text = id.as_str();
    text.get(..8).unwrap_or(text)
}

/// Why a start was refused.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SupervisionRefusal {
    #[error(
        "session `{id}` is still running as {identity}; refusing to start a second \
         session beside it"
    )]
    Duplicate {
        id: SessionId,
        identity: ProcessIdentity,
    },
    #[error(
        "session `{id}` is quarantined and a process Glasshouse cannot account for \
         still holds {holds}; refusing to start a replacement. {reason}"
    )]
    Quarantined {
        id: SessionId,
        holds: String,
        reason: String,
    },
}

// ------------------------------------------------------------------
// Discovery, verification, adoption, quarantine.
// ------------------------------------------------------------------

/// Every session this project recorded that still claims to be live.
///
/// The starting point for everything else here, and the place the first
/// architectural requirement is enforced: the candidate set comes from the
/// project's own session table and from nowhere else. A finished record is not
/// a candidate — supervision has nothing to say about a session that already
/// ended.
pub fn discover(store: &SessionStore<'_>) -> Result<Vec<SessionRecord>, SessionStoreError> {
    Ok(store
        .list()?
        .into_iter()
        .filter(|record| record.project_id == store.project_id())
        .filter(|record| record.lifecycle.is_live())
        .collect())
}

/// Verify every discovered session, adopt what is verified, quarantine what is
/// not, and record a start that never became ready as a failure.
///
/// Called once from [`super::store::ProjectSessions::open`], which is the door
/// every `glasshouse` invocation that touches sessions comes through — the
/// command line, the shell, and the hook handler alike. That is deliberate:
/// supervision that only ran in the interactive interface would have missed
/// the 2026-08-26 processes entirely, because nobody was in the interface.
///
/// `now` is the store's own clock in seconds since the epoch, and
/// `session_dir` names where a session's own directory lives so a quarantine
/// can say what is still held.
pub fn reconcile(
    store: &SessionStore<'_>,
    this: Option<&ProcessIdentity>,
    now: i64,
    session_dir: &dyn Fn(&SessionId) -> PathBuf,
) -> Result<SupervisionReport, SessionStoreError> {
    let mut report = SupervisionReport::default();
    let Some(this_host) = host_name() else {
        // Without a host name nothing here can be verified, and guessing is
        // the one thing this module must not do.
        return Ok(report);
    };

    for record in discover(store)? {
        let holds = holdings(&record, session_dir);
        let recorded = store.supervision_of(&record.id)?;
        let Some(identity) = recorded.identity else {
            // Recorded by a build that stored no identity. Nothing may be
            // concluded — except that a session which has sat in `starting`
            // far past any plausible start did not start.
            if record.lifecycle == SessionLifecycle::Starting
                && now.saturating_sub(record.created_at) > NEVER_READY_AFTER.as_secs() as i64
            {
                let reason = format!(
                    "it has been starting for {}s and no process identity was ever \
                     recorded for it, so it never became ready",
                    now.saturating_sub(record.created_at)
                );
                store.record_supervision(
                    &record.id,
                    Supervision::Lost,
                    &reason,
                    Some(SessionLifecycle::Failed),
                )?;
                report.never_ready.push(SupervisedSession {
                    id: record.id,
                    harness: record.harness,
                    supervision: Supervision::Lost,
                    reason,
                    identity: None,
                    holds,
                });
            }
            continue;
        };

        match verify(&identity, &this_host) {
            Verdict::Verified => {
                // Ours, this run. Nothing to adopt and nothing to say.
                if this.is_some_and(|this| *this == identity) {
                    continue;
                }
                let reason = format!(
                    "verified still running as {identity}; adopted rather than started \
                     a second time"
                );
                store.record_supervision(&record.id, Supervision::Adopted, &reason, None)?;
                report.adopted.push(SupervisedSession {
                    id: record.id,
                    harness: record.harness,
                    supervision: Supervision::Adopted,
                    reason,
                    identity: Some(identity),
                    holds,
                });
            }
            Verdict::Mismatched {
                recorded_started_at_ms,
                observed_started_at_ms,
            } => {
                let reason = format!(
                    "process {} is running, but it started {} {} the process this \
                     session recorded, so the id was reused and what is running under \
                     it now cannot be accounted for",
                    identity.pid,
                    human_gap((observed_started_at_ms - recorded_started_at_ms).abs()),
                    if observed_started_at_ms >= recorded_started_at_ms {
                        "after"
                    } else {
                        "before"
                    },
                );
                store.record_supervision(&record.id, Supervision::Quarantined, &reason, None)?;
                report.quarantined.push(SupervisedSession {
                    id: record.id,
                    harness: record.harness,
                    supervision: Supervision::Quarantined,
                    reason,
                    identity: Some(identity),
                    holds,
                });
            }
            Verdict::ForeignHost { recorded, actual } => {
                let reason = format!(
                    "it was recorded on `{recorded}` and this is `{actual}`, so its \
                     process id cannot be checked here and it is neither alive nor \
                     stopped as far as this machine can tell"
                );
                store.record_supervision(&record.id, Supervision::Quarantined, &reason, None)?;
                report.quarantined.push(SupervisedSession {
                    id: record.id,
                    harness: record.harness,
                    supervision: Supervision::Quarantined,
                    reason,
                    identity: Some(identity),
                    holds,
                });
            }
            Verdict::Gone => {
                // The process is provably not running. Recording that is
                // reporting, not ending: there is nothing left to end.
                let starting = record.lifecycle == SessionLifecycle::Starting;
                let (state, reason) = if starting {
                    (
                        SessionLifecycle::Failed,
                        format!(
                            "its process ({}) is gone and it never left `starting`, so \
                             the start never became ready",
                            identity.pid
                        ),
                    )
                } else {
                    (
                        SessionLifecycle::Stopped,
                        format!("its process ({}) is no longer running", identity.pid),
                    )
                };
                store.record_supervision(&record.id, Supervision::Lost, &reason, Some(state))?;
                let found = SupervisedSession {
                    id: record.id,
                    harness: record.harness,
                    supervision: Supervision::Lost,
                    reason,
                    identity: Some(identity),
                    holds,
                };
                if starting {
                    report.never_ready.push(found);
                } else {
                    report.lost.push(found);
                }
            }
        }
    }

    Ok(report)
}

/// What a session still holds, named so a refusal can say what it collided
/// with.
fn holdings(record: &SessionRecord, session_dir: &dyn Fn(&SessionId) -> PathBuf) -> Vec<String> {
    let mut holds = Vec::new();
    if let Some(native) = &record.native_session_id {
        holds.push(format!(
            "the {} conversation `{native}`",
            record.harness.as_str()
        ));
    }
    holds.push(format!(
        "its session directory `{}`",
        session_dir(&record.id).display()
    ));
    holds
}

/// Whether a record's recorded supervision forbids starting for it.
///
/// The refusal half of adoption and of quarantine, and the reason both boxes
/// are one function: a caller must not be able to check one and forget the
/// other. Consulted by [`super::store::SessionStore::open_for_resume`], which
/// is the production path a replacement is started through.
pub fn guard_start(
    record: &SessionRecord,
    recorded: &SupervisionRecord,
    this: Option<&ProcessIdentity>,
    session_dir: &dyn Fn(&SessionId) -> PathBuf,
) -> Result<(), SupervisionRefusal> {
    // Quarantine refuses whatever the record's lifecycle says, and that is the
    // point of it: a quarantined session is neither running nor stopped, so
    // "it reads as stopped, therefore it is free to replace" is exactly the
    // reasoning that produces a second process over the top of a first.
    if recorded.supervision == Some(Supervision::Quarantined) {
        return Err(SupervisionRefusal::Quarantined {
            id: record.id.clone(),
            holds: holdings(record, session_dir).join(", and "),
            reason: recorded
                .reason
                .clone()
                .unwrap_or_else(|| "no reason was recorded".to_owned()),
        });
    }

    // The duplicate refusal is about a **live** session — the line says so —
    // and about a process that is **not this one**.
    //
    // Both halves matter. A record that has stopped is not duplicated by
    // starting again however alive the Glasshouse that recorded it still is;
    // that Glasshouse is very often this one, still running, having recorded
    // the exit itself a moment ago. And a live record whose process *is* this
    // process is not a duplicate either — it is this process's own session,
    // and the caller is told so by the disposition it already computes,
    // which says "still running" and is the true and useful answer.
    if !record.lifecycle.is_live() {
        return Ok(());
    }
    let Some(identity) = recorded.identity.clone() else {
        return Ok(());
    };
    if this == Some(&identity) {
        return Ok(());
    }
    let Some(host) = host_name() else {
        return Ok(());
    };
    if verify(&identity, &host) == Verdict::Verified {
        return Err(SupervisionRefusal::Duplicate {
            id: record.id.clone(),
            identity,
        });
    }
    Ok(())
}

// ------------------------------------------------------------------
// The platform layer: three ways to ask a kernel when a process started.
// ------------------------------------------------------------------

/// The machine this process is on, or `None` when it cannot be read.
///
/// `None` rather than a placeholder, for the reason
/// [`ProcessIdentity::of_this_process`] gives: a placeholder host would match
/// another machine's placeholder, which is the stale match the host column
/// exists to prevent.
pub fn host_name() -> Option<String> {
    #[cfg(unix)]
    {
        let mut buffer = [0i8; 256];
        // SAFETY: `buffer` is a live, correctly sized array and the length
        // passed is its own. `gethostname` writes at most that many bytes and
        // NUL-terminates within them on success.
        let rc = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
        if rc != 0 {
            return None;
        }
        let bytes: Vec<u8> = buffer
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as u8)
            .collect();
        let name = String::from_utf8_lossy(&bytes).into_owned();
        (!name.is_empty()).then_some(name)
    }
    #[cfg(windows)]
    {
        std::env::var("COMPUTERNAME")
            .ok()
            .filter(|name| !name.is_empty())
    }
}

/// What the kernel says about one process id, or `None` if there is nothing
/// there.
///
/// The one place in the crate that asks an operating system about a process it
/// did not spawn, and it asks about exactly one id at a time. There is no
/// enumeration here on purpose — see this module's doc comment.
#[cfg(target_os = "linux")]
pub fn observe(pid: u32) -> Option<ObservedProcess> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is in parentheses and may itself contain spaces and
    // parentheses, so the fields are read from after the *last* `)`. Splitting
    // on whitespace from the front is the classic way to misparse this file.
    let tail = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = tail.split_whitespace().collect();
    // `state` is the first field after the name; `starttime` is the
    // twentieth, in clock ticks since boot.
    let state = *fields.first()?;
    let ticks: u64 = fields.get(19)?.parse().ok()?;

    // SAFETY: `sysconf` takes an integer and returns one; there are no
    // pointers and no allocation involved.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let hz = if hz > 0 { hz as u64 } else { 100 };
    let boot_ms = boot_time_ms()?;

    Some(ObservedProcess {
        started_at_ms: boot_ms.saturating_add((ticks.saturating_mul(1000) / hz) as i64),
        state: if state == "Z" {
            ProcessState::Zombie
        } else {
            ProcessState::Live
        },
    })
}

/// When this machine booted, in milliseconds since the Unix epoch.
///
/// Linux reports a process's start time relative to boot, so this is what
/// turns it into something that survives a reboot — without it, a process on
/// one boot and a process on the next would carry the same recorded start
/// time, which is precisely the collision the start time exists to prevent.
#[cfg(target_os = "linux")]
fn boot_time_ms() -> Option<i64> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let seconds: i64 = stat
        .lines()
        .find_map(|line| line.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    Some(seconds.saturating_mul(1000))
}

#[cfg(target_os = "macos")]
pub fn observe(pid: u32) -> Option<ObservedProcess> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` is a live, zeroed value of exactly the type this flavor
    // fills, and the size passed is its own. `proc_pidinfo` writes at most
    // that many bytes and returns how many it wrote.
    let written = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            std::ptr::from_mut(&mut info).cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    let started_at_ms = i64::try_from(info.pbi_start_tvsec)
        .ok()?
        .saturating_mul(1000)
        .saturating_add(i64::try_from(info.pbi_start_tvusec / 1000).ok()?);
    Some(ObservedProcess {
        started_at_ms,
        state: if info.pbi_status == libc::SZOMB {
            ProcessState::Zombie
        } else {
            ProcessState::Live
        },
    })
}

#[cfg(windows)]
pub fn observe(pid: u32) -> Option<ObservedProcess> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// `STILL_ACTIVE`, which is what `GetExitCodeProcess` reports for a
    /// process that has not exited. A process really exiting with 259 is
    /// indistinguishable from a running one through this call, which is a
    /// documented Windows wart; it costs a false "alive", which supervision
    /// answers with quarantine rather than with a replacement, so it fails in
    /// the safe direction.
    const STILL_ACTIVE: u32 = 259;

    // SAFETY: a plain call taking integers and returning a handle. Every exit
    // from this function below closes it.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return None;
    }

    let mut creation = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exit = creation;
    let mut kernel = creation;
    let mut user = creation;
    // SAFETY: `handle` is open, and all four out-parameters are live values of
    // the right type.
    let times_ok = unsafe {
        GetProcessTimes(
            handle,
            std::ptr::from_mut(&mut creation),
            std::ptr::from_mut(&mut exit),
            std::ptr::from_mut(&mut kernel),
            std::ptr::from_mut(&mut user),
        )
    } != 0;

    let mut code: u32 = 0;
    // SAFETY: as above.
    let code_ok = unsafe { GetExitCodeProcess(handle, std::ptr::from_mut(&mut code)) } != 0;

    // SAFETY: `handle` came from `OpenProcess` above and is closed exactly
    // once, here, on every path that opened it.
    unsafe { CloseHandle(handle) };

    if !times_ok {
        return None;
    }

    let ticks = (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
    // FILETIME counts 100-nanosecond intervals from 1601-01-01; the Unix epoch
    // is 11644473600 seconds later.
    const EPOCH_DIFFERENCE_100NS: u64 = 11_644_473_600 * 10_000_000;
    let since_epoch = ticks.checked_sub(EPOCH_DIFFERENCE_100NS)?;

    Some(ObservedProcess {
        started_at_ms: i64::try_from(since_epoch / 10_000).ok()?,
        state: if code_ok && code != STILL_ACTIVE {
            ProcessState::Zombie
        } else {
            ProcessState::Live
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Production source of a module: everything before its
    /// `#[cfg(test)] mod tests` block, comments removed. The same helper
    /// `session::lifecycle` uses, and for the same reason — anchoring on the
    /// attribute that actually introduces `mod tests` rather than on the first
    /// `#[cfg(test)]` in the file.
    fn production_code(source: &str) -> String {
        let lines: Vec<&str> = source.lines().collect();
        let end = lines
            .windows(2)
            .position(|pair| {
                pair[0].trim_end() == "#[cfg(test)]" && pair[1].trim_end().starts_with("mod tests")
            })
            .unwrap_or(lines.len());
        lines[..end]
            .iter()
            .filter(|line| !line.trim_start().starts_with("//"))
            .filter(|line| !line.trim_start().starts_with("///"))
            .copied()
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The third architectural requirement, as a guard rather than a promise.
    ///
    /// *"Glasshouse reports and refuses; it never ends a session the user did
    /// not ask it to end."* A supervisor that kills is a different product and
    /// is easier to build, which is exactly why this is a test: the day
    /// somebody decides quarantine ought to reap, they have to delete this
    /// first and explain themselves.
    #[test]
    fn nothing_in_supervision_ends_a_process() {
        let code = production_code(include_str!("supervision.rs"));
        for forbidden in [
            "kill(",
            "TerminateProcess",
            "SIGKILL",
            "SIGTERM",
            "libc::signal",
            ".wait()",
        ] {
            assert!(
                !code.contains(forbidden),
                "supervision names `{forbidden}`, so it can end a process it was only \
                 ever allowed to report on"
            );
        }
    }

    /// The first architectural requirement, as a guard.
    ///
    /// *"Supervision covers only sessions this project recorded."* The way
    /// that stops being true is a process enumeration: once a list of every
    /// process on the machine is in hand, filtering it by name looks
    /// reasonable and adopts a stranger on the first coincidence.
    #[test]
    fn supervision_never_enumerates_processes() {
        let code = production_code(include_str!("supervision.rs"));
        for forbidden in [
            "proc_listpids",
            "proc_listallpids",
            "CreateToolhelp32Snapshot",
            "read_dir(\"/proc",
        ] {
            assert!(
                !code.contains(forbidden),
                "supervision names `{forbidden}`, so it can find processes this project \
                 never started"
            );
        }
    }

    #[test]
    fn this_process_has_an_identity_and_it_is_stable() {
        let first = ProcessIdentity::of_this_process().expect("this platform names its processes");
        let second = ProcessIdentity::of_this_process().expect("and does so twice");
        assert_eq!(first, second, "a process's identity must not drift");
        assert_eq!(first.pid, std::process::id());
        assert!(
            first.started_at_ms > 1_600_000_000_000,
            "a start time of {}ms is not a plausible wall clock, so the platform unit \
             was not normalised",
            first.started_at_ms
        );
    }

    #[test]
    fn this_process_verifies_against_its_own_record() {
        let identity = ProcessIdentity::of_this_process().expect("this platform names processes");
        let host = host_name().expect("and its host");
        assert_eq!(verify(&identity, &host), Verdict::Verified);
    }

    /// The whole reason the start time is recorded.
    ///
    /// A record holding only this process's id would verify against this
    /// process. One holding the id *and a start time that is not this
    /// process's* must not — that is a reused identifier, and treating it as
    /// the session would report a stranger as this project's work.
    #[test]
    fn a_reused_process_id_does_not_match_a_stale_record() {
        let mut stale = ProcessIdentity::of_this_process().expect("this platform names processes");
        let real = stale.started_at_ms;
        stale.started_at_ms -= 60_000;
        let host = host_name().expect("and its host");
        assert_eq!(
            verify(&stale, &host),
            Verdict::Mismatched {
                recorded_started_at_ms: real - 60_000,
                observed_started_at_ms: real,
            },
            "a live process id with a different start time is a reused id, never a match"
        );
    }

    #[test]
    fn a_record_from_another_machine_is_never_verified_and_never_assumed_dead() {
        let mut foreign =
            ProcessIdentity::of_this_process().expect("this platform names processes");
        foreign.host = format!("{}-somewhere-else", foreign.host);
        let host = host_name().expect("and its host");
        match verify(&foreign, &host) {
            Verdict::ForeignHost { recorded, actual } => {
                assert_ne!(recorded, actual);
            }
            other => panic!("a foreign host must not be verified or called gone; got {other:?}"),
        }
    }

    #[test]
    fn the_four_supervision_words_round_trip_through_the_schemas_vocabulary() {
        for word in [
            Supervision::Owned,
            Supervision::Adopted,
            Supervision::Quarantined,
            Supervision::Lost,
        ] {
            assert_eq!(Supervision::from_str(word.as_str()), Some(word));
        }
        assert_eq!(Supervision::from_str("reaped"), None);
    }

    #[test]
    fn a_report_with_nothing_actionable_says_nothing() {
        let report = SupervisionReport::default();
        assert!(report.is_empty());
        assert_eq!(report.describe(), None);

        // An adopted session is working as intended, so it is not news either.
        let adopted_only = SupervisionReport {
            adopted: vec![SupervisedSession {
                id: SessionId::new("abcdef0123456789"),
                harness: "claude-code".to_owned(),
                supervision: Supervision::Adopted,
                reason: "verified".to_owned(),
                identity: None,
                holds: Vec::new(),
            }],
            ..SupervisionReport::default()
        };
        assert!(!adopted_only.is_empty());
        assert_eq!(adopted_only.describe(), None);
    }

    #[test]
    fn a_quarantine_says_what_it_saw_and_what_is_still_held() {
        let report = SupervisionReport {
            quarantined: vec![SupervisedSession {
                id: SessionId::new("abcdef0123456789"),
                harness: "claude-code".to_owned(),
                supervision: Supervision::Quarantined,
                reason: "the process id was reused".to_owned(),
                identity: Some(ProcessIdentity {
                    pid: 4711,
                    started_at_ms: 1_700_000_000_000,
                    host: "somewhere".to_owned(),
                }),
                holds: vec!["the claude-code conversation `d23ab938`".to_owned()],
            }],
            ..SupervisionReport::default()
        };
        let described = report.describe().expect("a quarantine is always news");
        assert!(described.contains("abcdef01"), "{described}");
        assert!(described.contains("claude-code"), "{described}");
        assert!(described.contains("4711"), "{described}");
        assert!(described.contains("d23ab938"), "{described}");
        assert!(
            described.contains("end it"),
            "the user must be told Glasshouse will not end it: {described}"
        );
    }
}
