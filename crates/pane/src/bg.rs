//! Background jobs — `docs/product/pane/events-contract.md` §5.
//!
//! **A background job is a foreground tool call that nobody waits for.** Every
//! job runs through [`crate::tools::invoke::run_cancellable`], on a thread of
//! its own, so the grant, the argument check, the confinement, the process
//! group and the kill-and-reap are the ones a `bash()` call in a cell already
//! gets: there is no second spawn path here, no second confinement decision,
//! and no `Command` in this file. Nothing widens because it is asynchronous.
//!
//! The grant is asked **twice on purpose**, and the first ask is what §5's
//! "throws `PermissionDenied` at the call, before any handle exists" means:
//! [`run`] calls [`Profile::admits_command`] — the same function
//! `invoke::check_arguments` runs for `bash`'s command line — before it mints
//! a handle or starts a thread. The thread's own call asks again through
//! `invoke`, so a profile that refuses cannot be got past by reaching the
//! thread.
//!
//! One board per session id, in a process-wide registry. It is not a field of
//! [`crate::runtime::state::RuntimeState`] because a job outlives the cell
//! that started it and its completion has to be readable from the session's
//! turn loop, which holds no isolate; the session id keys it so two sessions
//! in one process never see each other's jobs.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::contract::SessionId;
use crate::events::{Event, Kind, PayloadRef, Priority, now};
use crate::glasshouse::Glasshouse;
use crate::sandbox::profile::{PermissionDenied, Profile};
use crate::tools::invoke::{self, Args, CancellationToken, ToolContext, ToolError};

/// How long [`cancel`] lets a job settle before it stops waiting for the
/// thread to notice — the token is already set by then, and
/// `invoke::spawn_confined`'s poll loop kills the whole process group at its
/// next tick.
const CANCEL_SETTLE: Duration = Duration::from_millis(50);

/// The slice a deadline thread sleeps in, so a timed job's timer does not
/// keep [`shutdown`] waiting for the whole timeout.
const DEADLINE_SLICE: Duration = Duration::from_millis(25);

/// The slice [`cancel`]'s settle polls the job's own `finished` flag in.
/// Finer than [`DEADLINE_SLICE`] because this one is on the exit path: a job
/// that dies in 22 ms (`invoke`'s cancel poll is 20 ms) should cost 25 ms,
/// not the whole of [`CANCEL_SETTLE`].
const SETTLE_SLICE: Duration = Duration::from_millis(5);

/// The fastest cadence `bg.watch` will accept, in milliseconds.
///
/// **A floor, not a default.** Every tick is a fresh confined `bash`, so the
/// cadence is a resource bound on model-chosen work rather than a
/// preference: measured on this machine, a watch runs 31 confined spawns a
/// second when it is asked for `{every: 1}` and 7.5 at this floor, and the
/// lifecycle verifier measured the unfloored version burning 13.20 s of CPU
/// in a 24.5 s session against 1.03 s at a second — about 54% of a core to
/// deliver, by §1's dedup, exactly the events a slow watch delivers. 100 ms
/// holds a watch's own polling near an eighth of a core, which is the most a
/// program should be able to spend on *looking* without asking for it.
///
/// It is enforced by refusing rather than by clamping, for the reason `cwd`
/// and `env` are refused: a program told nothing, and silently slowed, would
/// believe it had chosen a cadence it had not.
const WATCH_FLOOR_MS: u64 = 100;

/// How long [`shutdown`] waits for a cancelled job's thread to come back
/// before it stops waiting for it.
///
/// **A session must be able to end.** An unbounded `join` here is a session
/// that cannot exit at all, and reaching it needs only one job the kill could
/// not stop; a bounded wait leaves at worst a detached thread. This is not
/// hypothetical — the mutation that hands `cancel` a token nobody holds
/// reproduces it exactly, and hung the test that found it for twelve minutes
/// rather than failing it.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// The most characters of a job's own summary line a batch preview carries.
const SUMMARY_CHARS: usize = 160;

/// What one emission of a job produced. `status` is small enough to render;
/// `stdout` and `stderr` are not, which is the whole reason §5 makes them
/// handles of their own — a job that printed 40 MB costs a status line until
/// the model's own program asks for the bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobResult {
    pub stdout: String,
    pub stderr: String,
    pub status: String,
}

/// `bg.run`'s options object.
///
/// `cwd` and `env` are **refused rather than ignored**: `invoke` runs every
/// child in the project root with the session's own environment, and honouring
/// either here would mean a second spawn path — which is exactly what this
/// module exists not to have. A silent ignore would be worse: a program would
/// believe it had chosen a directory it had not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOptions {
    pub cwd: Option<String>,
    pub env: Option<String>,
    pub timeout_ms: Option<u64>,
}

/// `bg.watch`'s options object. `until` matches when the run's stdout
/// contains it — a substring, not a pattern, because a pattern language here
/// would be a second grammar for the model to learn and §5 names none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchOptions {
    pub every_ms: u64,
    pub until: Option<String>,
    pub timeout_ms: Option<u64>,
}

/// One live job: the token its call is cancellable through, whether a
/// cancellation was already asked for (so [`cancel`] is idempotent), and
/// whether the thread has finished.
struct JobEntry {
    token: CancellationToken,
    cancelled: bool,
    finished: bool,
    thread: Option<JoinHandle<()>>,
}

/// One session's jobs, the events they have raised and the payloads those
/// events name.
#[derive(Default)]
struct Board {
    next: u64,
    jobs: HashMap<String, JobEntry>,
    pending: Vec<Event>,
    payloads: HashMap<String, JobResult>,
}

static BOARDS: OnceLock<Mutex<HashMap<String, Board>>> = OnceLock::new();

fn boards() -> &'static Mutex<HashMap<String, Board>> {
    BOARDS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Runs `f` against this session's board.
///
/// A poisoned mutex is recovered from rather than propagated: a panicking job
/// thread must not take the session's whole event channel with it, and every
/// field here is a plain collection with no invariant a panic could have left
/// half-applied.
fn with_board<T>(session: &SessionId, f: impl FnOnce(&mut Board) -> T) -> T {
    let mut boards = boards().lock().unwrap_or_else(|e| e.into_inner());
    f(boards.entry(session.as_str().to_string()).or_default())
}

/// One line, bounded, with no newline that could forge a second row of a
/// batch preview.
fn summary(text: &str) -> String {
    let one_line = text.lines().collect::<Vec<_>>().join(" ");
    one_line.chars().take(SUMMARY_CHARS).collect()
}

/// A short digest of one emission's output, so §1's `bg.done` dedup key
/// (`bg/<handle> + emission`) collapses two identical outputs inside one
/// window into one event — which is `bg.watch`'s whole "identical output in
/// one window is one event".
fn emission_of(stdout: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(stdout.as_bytes());
    format!("{digest:x}")[..16].to_string()
}

/// Refuses the two options `invoke` gives this module no way to honour.
fn honourable(options: &RunOptions) -> Result<(), PermissionDenied> {
    let refuse = |name: &str, rule: &str| PermissionDenied {
        tool: "bg.run".to_string(),
        path: name.to_string(),
        rule: rule.to_string(),
    };
    if options.cwd.is_some() {
        return Err(refuse(
            "cwd",
            "a background job runs in the project root, the same directory a foreground tool call \
             runs in; choosing another would need a second spawn path outside the one confinement \
             (events-contract.md §5)",
        ));
    }
    if options.env.is_some() {
        return Err(refuse(
            "env",
            "a background job runs with the session's own environment; adding to it is a grant \
             question sandbox-grants.md has not been asked",
        ));
    }
    Ok(())
}

/// Refuses a watch cadence whose own polling would be the load.
///
/// [`WATCH_FLOOR_MS`] carries the measurement; this is where a program is
/// told, and the message names the floor so a program can retry at it.
fn fast_enough_to_be_free(every_ms: u64) -> Result<(), PermissionDenied> {
    if every_ms >= WATCH_FLOOR_MS {
        return Ok(());
    }
    Err(PermissionDenied {
        tool: "bg.watch".to_string(),
        path: "every".to_string(),
        rule: format!(
            "a watch runs a fresh confined shell every tick, so `every` has a floor of \
             {WATCH_FLOOR_MS} ms and {every_ms} ms is under it; ask for {WATCH_FLOOR_MS} or more \
             (events-contract.md §5)"
        ),
    })
}

/// §5's `bg.run(cmd, {cwd, env, timeout})` — answers with the job's handle
/// **before the process has done anything**.
///
/// The refusal is the profile's own and it happens here, at the call: a
/// command outside the grant leaves no job on the board, no thread running
/// and no handle for the model to hold.
pub fn run(
    profile: &Profile,
    glasshouse: &Glasshouse,
    session: &SessionId,
    command: &str,
    options: &RunOptions,
) -> Result<String, PermissionDenied> {
    honourable(options)?;
    profile.admits_command(command)?;
    Ok(start(
        profile,
        glasshouse,
        session,
        command,
        options.timeout_ms,
        None,
    ))
}

/// §5's `bg.watch(cmd, {every, until})`, built on [`run`]'s machinery rather
/// than a second spawn path: the same thread, the same call, in a loop.
///
/// A cadence under [`WATCH_FLOOR_MS`] is refused here, before a handle
/// exists, exactly as a command outside the grant is.
pub fn watch(
    profile: &Profile,
    glasshouse: &Glasshouse,
    session: &SessionId,
    command: &str,
    options: &WatchOptions,
) -> Result<String, PermissionDenied> {
    fast_enough_to_be_free(options.every_ms)?;
    profile.admits_command(command)?;
    Ok(start(
        profile,
        glasshouse,
        session,
        command,
        options.timeout_ms,
        Some(options.clone()),
    ))
}

/// Mints the handle, registers the job and starts its thread — the one place
/// a job comes into existence, so `run` and `watch` cannot drift about what a
/// job is.
fn start(
    profile: &Profile,
    glasshouse: &Glasshouse,
    session: &SessionId,
    command: &str,
    timeout_ms: Option<u64>,
    watching: Option<WatchOptions>,
) -> String {
    let token = CancellationToken::new();
    let handle = with_board(session, |board| {
        board.next += 1;
        let handle = format!("job{}", board.next);
        board.jobs.insert(
            handle.clone(),
            JobEntry {
                token: token.clone(),
                cancelled: false,
                finished: false,
                thread: None,
            },
        );
        handle
    });

    let job = JobThread {
        handle: handle.clone(),
        profile: profile.clone(),
        glasshouse: glasshouse.clone(),
        session: session.clone(),
        command: command.to_string(),
        token: token.clone(),
        watching,
    };
    let thread = std::thread::spawn(move || job.serve());

    if let Some(ms) = timeout_ms {
        arm_deadline(session.clone(), handle.clone(), token, ms);
    }

    with_board(session, |board| {
        if let Some(entry) = board.jobs.get_mut(&handle) {
            entry.thread = Some(thread);
        }
    });
    handle
}

/// The deadline half of `{timeout}` and of `bg.watch`'s `until` that never
/// matches: a thread that sleeps in [`DEADLINE_SLICE`] slices so `shutdown`
/// is never held up by a long timeout, and that cancels the job when the
/// deadline arrives.
///
/// §5: "the deadline expiring emits a `timer`" — and the job's own thread
/// then emits the `bg.done` with `status: "cancelled"`, so nothing waits for
/// a dead result.
fn arm_deadline(session: SessionId, handle: String, token: CancellationToken, ms: u64) {
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            if token.is_cancelled() || finished(&session, &handle) {
                return;
            }
            std::thread::sleep(DEADLINE_SLICE);
        }
        if finished(&session, &handle) {
            return;
        }
        raise(
            &session,
            Event::pending(
                Kind::Timer {
                    deadline: ms.to_string(),
                },
                format!("bg/{handle}"),
                now(),
                PayloadRef::new(format!("{handle}#timer")),
                Priority::Batch,
                summary(&format!("{handle} reached its {ms} ms deadline")),
            ),
        );
        token.cancel();
    });
}

fn finished(session: &SessionId, handle: &str) -> bool {
    with_board(session, |board| {
        board.jobs.get(handle).is_none_or(|job| job.finished)
    })
}

/// Puts one event on the session's board for the turn loop to drain.
fn raise(session: &SessionId, event: Event) {
    with_board(session, |board| board.pending.push(event));
}

/// Everything one job's thread needs, owned — a thread cannot borrow the
/// session's profile, and cloning it is what `RuntimeState` already does.
/// Cloning cannot widen anything: [`Profile`] has no method that mutates it
/// and no constructor outside `Profile::compile`.
struct JobThread {
    handle: String,
    profile: Profile,
    glasshouse: Glasshouse,
    session: SessionId,
    command: String,
    token: CancellationToken,
    watching: Option<WatchOptions>,
}

impl JobThread {
    fn serve(self) {
        match &self.watching {
            None => self.serve_once(),
            Some(options) => self.serve_watch(options),
        }
        with_board(&self.session, |board| {
            if let Some(entry) = board.jobs.get_mut(&self.handle) {
                entry.finished = true;
            }
        });
    }

    /// One call through `invoke`, then one `bg.done`. A job completes once,
    /// so the emission is the constant §1's dedup table needs to make that
    /// true.
    fn serve_once(&self) {
        let result = self.call();
        self.emit("exit", result);
    }

    /// §5's watch: `cmd` every `every` ms, one `bg.done` per match, until
    /// `until` matches or the job is cancelled.
    ///
    /// A match is a run that produced output — a still-running tick raises
    /// nothing, which is §1's "polling raises no event, only a transition
    /// does". Two identical outputs inside one window collapse to one event
    /// by the emission digest, without this loop knowing anything about
    /// windows.
    fn serve_watch(&self, options: &WatchOptions) {
        // Unreachable by construction -- [`watch`] refuses anything under the
        // floor before a job exists -- so this is what makes the floor a
        // property of the loop rather than only of its one caller.
        let every = Duration::from_millis(options.every_ms.max(WATCH_FLOOR_MS));
        loop {
            if self.token.is_cancelled() {
                self.emit("cancelled", Err(cancelled()));
                return;
            }
            let result = self.call();
            let matched = match &result {
                Ok(job) => !job.stdout.trim().is_empty(),
                Err(_) => true,
            };
            let stop = match (&result, &options.until) {
                (Ok(job), Some(until)) => job.stdout.contains(until.as_str()),
                (Err(_), _) => true,
                (Ok(_), None) => false,
            };
            if matched {
                let emission = match &result {
                    Ok(job) => emission_of(&job.stdout),
                    Err(_) => "cancelled".to_string(),
                };
                self.emit(&emission, result);
            }
            if stop {
                return;
            }
            // Slept in slices so a cancellation is noticed within one slice
            // rather than within `every`, which a model may have set to
            // minutes.
            let until_next = Instant::now() + every;
            while Instant::now() < until_next {
                if self.token.is_cancelled() {
                    self.emit("cancelled", Err(cancelled()));
                    return;
                }
                std::thread::sleep(DEADLINE_SLICE.min(every));
            }
        }
    }

    /// The one call this module makes, and the only path by which a job
    /// reaches a process: `invoke`'s own `bash`, checked, confined, spawned
    /// as its own process group leader and killed as one.
    fn call(&self) -> Result<JobResult, ToolError> {
        let context = ToolContext {
            profile: &self.profile,
            glasshouse: &self.glasshouse,
            session: &self.session,
        };
        let args = Args::new().with("command", self.command.clone());
        invoke::run_cancellable(&context, &self.token, "bash", &args).map(|result| JobResult {
            stdout: result.stdout,
            stderr: result.stderr,
            status: result
                .exit_code
                .map_or_else(|| "signal".to_string(), |code| code.to_string()),
        })
    }

    /// Records one emission's payload and raises its `bg.done`.
    ///
    /// A cancelled or timed-out job emits one too, with `status:
    /// "cancelled"` — §5's "nothing waits for a dead result".
    fn emit(&self, emission: &str, result: Result<JobResult, ToolError>) {
        let job = match result {
            Ok(job) => job,
            Err(ToolError::Cancelled { .. }) => JobResult {
                stdout: String::new(),
                stderr: String::new(),
                status: "cancelled".to_string(),
            },
            Err(other) => JobResult {
                stdout: String::new(),
                stderr: other.to_string(),
                status: "failed".to_string(),
            },
        };
        let payload = format!("{}#{emission}", self.handle);
        let line = summary(&format!(
            "{} → {} ({} B out)",
            self.command,
            job.status,
            job.stdout.len()
        ));
        with_board(&self.session, |board| {
            board.payloads.insert(payload.clone(), job);
            board.pending.push(Event::pending(
                Kind::BgDone {
                    emission: emission.to_string(),
                },
                format!("bg/{}", self.handle),
                now(),
                PayloadRef::new(payload.clone()),
                Priority::Batch,
                line.clone(),
            ));
        });
    }
}

fn cancelled() -> ToolError {
    ToolError::Cancelled {
        tool: "bash".to_string(),
    }
}

/// §5's `bg.cancel(handle)`: idempotent, and it stops everything the job
/// started.
///
/// Setting the token is the whole mechanism, and it is deliberately the only
/// one: `invoke::spawn_confined`'s poll loop reads it and calls
/// `kill_and_reap`, which signals the **process group** the call created
/// before it reaps the child — so a job whose command backgrounded something
/// leaves nothing spinning, and a job that ignores a polite signal dies
/// anyway.
pub fn cancel(session: &SessionId, handle: &str) {
    cancel_by(session, handle, Instant::now() + CANCEL_SETTLE);
}

/// [`cancel`], with the settle's own end named by the caller so a shutdown
/// pays one grace for a whole board rather than one per job.
///
/// **The settle waits for the job rather than for the clock.** It is there so
/// `invoke::spawn_confined`'s poll loop reaches its next tick, and a job that
/// has already finished has no tick left to reach: sleeping through
/// [`CANCEL_SETTLE`] for it charged every caller 50 ms for work that was over
/// before it was asked for. `shutdown` calls this for every handle on the
/// board, so the bill was 50 ms × every job the session ever started, paid at
/// the end of every task with the user watching — 12.34 s for 200 short jobs,
/// measured against the shipped binary. A model's own `bg.cancel` paid it
/// too.
fn cancel_by(session: &SessionId, handle: &str, deadline: Instant) {
    let token = with_board(session, |board| {
        let entry = board.jobs.get_mut(handle)?;
        if entry.cancelled {
            return None;
        }
        entry.cancelled = true;
        Some(entry.token.clone())
    });
    let Some(token) = token else { return };
    token.cancel();
    while !finished(session, handle) && Instant::now() < deadline {
        std::thread::sleep(SETTLE_SLICE);
    }
}

/// Every event raised since the last drain, oldest first.
pub fn drain(session: &SessionId) -> Vec<Event> {
    with_board(session, |board| std::mem::take(&mut board.pending))
}

/// One emission's output, by the `PayloadRef` its event carries — §1's
/// "materialised on first access".
pub fn payload(session: &SessionId, id: &str) -> Option<JobResult> {
    with_board(session, |board| board.payloads.get(id).cloned())
}

/// How many of this session's jobs have not finished.
pub fn live(session: &SessionId) -> usize {
    with_board(session, |board| {
        board.jobs.values().filter(|job| !job.finished).count()
    })
}

/// Cancels every job of this session and waits for its thread, then forgets
/// the board.
///
/// **A background job outlives no session.** This is called where
/// `session::run_task` ends a task and again where `session::run` returns,
/// and [`shutdown_within`] is called where `Interrupter::end_the_session`
/// exits from the signal watcher — so an end of input, a task that returns,
/// a task that failed mid-flight and the double Ctrl-C all reach it; every
/// job dies through [`cancel`]'s ladder, which is `invoke`'s own group kill.
pub fn shutdown(session: &SessionId) {
    shutdown_within(session, SHUTDOWN_GRACE);
}

/// [`shutdown`], bounded by the caller's own grace.
///
/// **The whole shutdown is bounded by `grace`, not each job by its own.** The
/// exit that needs this is the second Ctrl-C, which is a person asking the
/// program to stop *now*: it passes its own reap grace, and a board of two
/// hundred jobs must not turn that into two hundred settles. Cancelling is
/// what kills a job — the token is set for every handle whatever the clock
/// says — so the deadline only ever shortens the *waiting*.
pub fn shutdown_within(session: &SessionId, grace: Duration) {
    let deadline = Instant::now() + grace;
    let handles: Vec<String> = with_board(session, |board| board.jobs.keys().cloned().collect());
    for handle in &handles {
        cancel_by(
            session,
            handle,
            deadline.min(Instant::now() + CANCEL_SETTLE),
        );
    }
    // Waited for by the flag rather than by the join, and bounded: a thread
    // sets `finished` as the last thing it does, so joining only the finished
    // ones is a join that returns, and a job the kill could not stop costs a
    // detached thread instead of a session that cannot exit.
    while Instant::now() < deadline && live(session) > 0 {
        std::thread::sleep(DEADLINE_SLICE);
    }
    // The threads are taken out from under the lock and joined outside it:
    // a job thread takes the same lock to raise its `bg.done`, so joining
    // while holding it would deadlock every time.
    let threads: Vec<JoinHandle<()>> = with_board(session, |board| {
        board
            .jobs
            .values_mut()
            .filter(|job| job.finished)
            .filter_map(|job| job.thread.take())
            .collect()
    });
    for thread in threads {
        let _ = thread.join();
    }
    let mut boards = boards().lock().unwrap_or_else(|e| e.into_inner());
    boards.remove(session.as_str());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cwd_and_env_are_refused_rather_than_ignored() {
        let denied = honourable(&RunOptions {
            cwd: Some("/tmp".to_string()),
            ..RunOptions::default()
        })
        .unwrap_err();
        assert_eq!(denied.path, "cwd");
        let denied = honourable(&RunOptions {
            env: Some("A=1".to_string()),
            ..RunOptions::default()
        })
        .unwrap_err();
        assert_eq!(denied.path, "env");
        assert!(honourable(&RunOptions::default()).is_ok());
    }

    /// §1's `bg.done` key is `bg/<handle> + emission`, so two identical
    /// outputs of one watch produce one emission and two different ones do
    /// not.
    #[test]
    fn identical_output_has_the_same_emission() {
        assert_eq!(
            emission_of("still building\n"),
            emission_of("still building\n")
        );
        assert_ne!(emission_of("still building\n"), emission_of("built\n"));
    }

    /// **A session must be able to end**, and this is the property in the
    /// form the world cannot produce: a job whose thread never sets
    /// `finished` at all.
    ///
    /// The lifecycle verifier could not build one — a confined `bash` cannot
    /// `setsid` out of the group `cancel` kills, so every job a model can
    /// write dies — and a bound nothing can exercise is a bound nobody has
    /// checked. Reaching into the board is the only way to hold the exit the
    /// way a bug in this module would: the thread outlives the shutdown,
    /// which is the point, because [`shutdown_within`] joins only what has
    /// finished and detaches the rest.
    #[test]
    fn shutdown_is_bounded_by_its_grace_when_a_job_never_finishes() {
        let session = SessionId::new("bg-never-finishes");
        let stuck = std::thread::spawn(|| std::thread::sleep(Duration::from_secs(3)));
        with_board(&session, |board| {
            board.jobs.insert(
                "job1".to_string(),
                JobEntry {
                    token: CancellationToken::new(),
                    cancelled: false,
                    finished: false,
                    thread: Some(stuck),
                },
            );
        });

        let started = Instant::now();
        shutdown_within(&session, Duration::from_millis(200));
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(1),
            "a job that never finishes held the exit for {elapsed:?}; the grace is not a bound"
        );
    }

    #[test]
    fn a_summary_is_one_bounded_line() {
        let line = summary(&format!("{}\nsecond", "x".repeat(400)));
        assert_eq!(line.chars().count(), SUMMARY_CHARS);
        assert!(!line.contains('\n'));
    }
}
