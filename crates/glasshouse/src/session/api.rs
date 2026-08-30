//! The internal API for driving and inspecting a live session.
//!
//! [`SessionApi`] is the one surface that sends text to, interrupts, or
//! inspects a session by identifier — the seam an orchestrator, the MCP
//! surface, or anything else internal to Glasshouse goes through instead of
//! reaching into [`super::store::SessionStore`] and [`super::runtime::SessionRuntime`]
//! directly. Two things make that worth a seam of its own:
//!
//! - **Project scope is checked once, here, for every entry point.** Every
//!   method resolves the identifier through the store first and compares its
//!   `project_id` against the active project before doing anything else —
//!   including before asking whether the session is even live. A foreign
//!   session that also happens to be stopped is still refused as foreign,
//!   never as merely not running, because "you asked about someone else's
//!   session" is the true answer and the only one worth giving.
//! - **Who sent a message is recorded, never inferred.** Every write goes
//!   through [`super::runtime::SessionRuntime::send_text_from`] and
//!   [`super::runtime::SessionRuntime::interrupt_from`] with an origin its
//!   **caller** supplies, not the plain `send_text` / `interrupt` that assume
//!   a person's keyboard. The distinction is recorded in Glasshouse's own
//!   event log, not inferred later from context that will not exist by then.
//!
//!   This seam used to hard-wire [`crate::events::MessageOrigin::Machine`],
//!   on the reasoning that everything reaching it was Glasshouse or an
//!   orchestrator. That stopped being true when `glasshouse api send` and
//!   `glasshouse api interrupt` shipped: a person's keystrokes now arrive
//!   here, over the control door, and hard-wiring made their intervention
//!   equal field for field to an orchestrator's own message. A seam that
//!   *decides* the origin can only be right while it has one kind of caller,
//!   so this one asks instead. Callers that are Glasshouse still pass
//!   `Machine` and are unchanged; the control door passes what its request
//!   said, defaulting to `Machine` when it said nothing.

use crate::events::MessageOrigin;

use super::{
    RuntimeError, SessionId, SessionLifecycle, SessionRecord, SessionRuntime, SessionStore,
    SessionStoreError,
};

/// Why a call into [`SessionApi`] could not be carried out.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("no session `{id}` in this project")]
    NotFound { id: SessionId },
    #[error(
        "session `{id}` belongs to project `{actual}`, not to the active \
         project `{expected}`; refusing to act on another project's session"
    )]
    ForeignProject {
        id: SessionId,
        expected: String,
        actual: String,
    },
    #[error("session `{id}` is not live in this Glasshouse")]
    NotLive { id: SessionId },
    #[error(transparent)]
    Store(#[from] SessionStoreError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}

/// The internal surface for sending to, interrupting, and inspecting one
/// project's sessions.
///
/// Borrows both halves it coordinates rather than owning either: the store
/// is the project's durable record, the runtime is whichever live sessions
/// this Glasshouse process actually holds, and neither belongs to this type.
pub struct SessionApi<'a> {
    store: &'a SessionStore<'a>,
    live: &'a mut SessionRuntime,
}

impl<'a> SessionApi<'a> {
    pub fn new(store: &'a SessionStore<'a>, live: &'a mut SessionRuntime) -> Self {
        Self { store, live }
    }

    /// Look a session up and confirm it belongs to the active project.
    ///
    /// The one check every other method starts with. It is deliberately
    /// unconcerned with liveness — that is a separate question a caller asks
    /// afterwards — so that a foreign session is always refused for being
    /// foreign, never for whatever else might also be true about it.
    fn resolve(&self, id: &SessionId) -> Result<SessionRecord, ApiError> {
        let record = self
            .store
            .get(id)?
            .ok_or_else(|| ApiError::NotFound { id: id.clone() })?;

        if record.project_id != self.store.project_id() {
            return Err(ApiError::ForeignProject {
                id: id.clone(),
                expected: self.store.project_id().to_owned(),
                actual: record.project_id,
            });
        }

        Ok(record)
    }

    /// Every session in the active project, most recently active first.
    ///
    /// Filtered by project here as well as trusting the store, so that a row
    /// which should never exist — one bearing another project's identifier,
    /// however it got into this database — cannot surface in a listing even
    /// though [`SessionStore::list`] itself has nothing to filter by; see
    /// that module's doc comment for why the store does not filter.
    pub fn list(&self) -> Result<Vec<SessionRecord>, ApiError> {
        Ok(self
            .store
            .list()?
            .into_iter()
            .filter(|record| record.project_id == self.store.project_id())
            .collect())
    }

    /// The lifecycle state of one session, as the store recorded it.
    pub fn state(&self, id: &SessionId) -> Result<SessionLifecycle, ApiError> {
        Ok(self.resolve(id)?.lifecycle)
    }

    /// Send a line of text to a live session, on behalf of `origin`.
    ///
    /// A carriage return is appended, the same way `shell::send_session_text`
    /// sends a line typed at the shell's own prompt: this call delivers one
    /// line, not raw bytes, and `\r` is what a session's terminal expects to
    /// see for the harness's line editor to submit it.
    ///
    /// `origin` is the caller's to state and this method's to record — see
    /// the module doc comment for why it is no longer decided here. Pass
    /// [`MessageOrigin::Machine`] for anything Glasshouse itself originates,
    /// which is what every caller inside this process does; only the control
    /// door has a caller it did not write, and only that door can know
    /// whether a person is on the other end of it.
    pub fn send_text(
        &mut self,
        id: &SessionId,
        text: &str,
        origin: MessageOrigin,
    ) -> Result<(), ApiError> {
        self.resolve(id)?;
        if self.live.get(id).is_none() {
            return Err(ApiError::NotLive { id: id.clone() });
        }
        let mut line = String::with_capacity(text.len() + 1);
        line.push_str(text);
        line.push('\r');
        self.live.send_text_from(id, &line, origin)?;
        Ok(())
    }

    /// Interrupt a live session, on behalf of `origin`.
    ///
    /// An interrupt is an intervention like any other line, and carries the
    /// same attribution for the same reason: `origin` is the caller's to
    /// state. See [`SessionApi::send_text`].
    pub fn interrupt(&mut self, id: &SessionId, origin: MessageOrigin) -> Result<(), ApiError> {
        self.resolve(id)?;
        if self.live.get(id).is_none() {
            return Err(ApiError::NotLive { id: id.clone() });
        }
        self.live.interrupt_from(id, origin)?;
        Ok(())
    }

    /// The most recent terminal output of a session, at most `max_bytes`,
    /// cut at a character boundary.
    ///
    /// Glasshouse does not persist terminal output yet, so a session with no
    /// live process has none to give: returning an empty string would be a
    /// lie the caller has no way to detect, so this refuses with
    /// [`ApiError::NotLive`] instead.
    pub fn recent_output(&self, id: &SessionId, max_bytes: usize) -> Result<String, ApiError> {
        self.resolve(id)?;
        let session = self
            .live
            .get(id)
            .ok_or_else(|| ApiError::NotLive { id: id.clone() })?;
        Ok(session.with_scrollback(|scrollback| tail(&scrollback.text(), max_bytes)))
    }
}

/// The last `max_bytes` of `text`, advanced to the next UTF-8 character
/// boundary so the result never opens with a severed character.
///
/// Advancing forward — never backward — means the returned string can be
/// shorter than `max_bytes` when the cut point lands inside a character, but
/// never longer, and never invalid.
fn tail(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let cut = text.len() - max_bytes;
    let start = (cut..=text.len())
        .find(|&index| text.is_char_boundary(index))
        .unwrap_or(text.len());
    text[start..].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::LifecycleEvent;
    use crate::launch::HarnessLaunch;
    use crate::platform::exec;
    use crate::session::{NewSession, SessionPresentation};
    use crate::{Cli, Runtime};
    use clap::Parser;
    use rusqlite::Connection;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::time::{Duration, Instant};

    const TIMEOUT: Duration = Duration::from_secs(15);
    const POLL: Duration = Duration::from_millis(20);

    /// A bootstrapped project with an open connection to its database.
    ///
    /// Mirrors `session::store`'s own test fixture: this module needs
    /// `crate::database::open`, which is crate-private, so the pattern is
    /// reproduced here rather than imported from a private test module.
    struct Fixture {
        base: PathBuf,
        runtime: Runtime,
        conn: Connection,
    }

    impl Fixture {
        fn new(base: &Path, name: &str) -> Self {
            let root = base.join("workspace").join(name);
            std::fs::create_dir_all(root.join(".git")).unwrap();
            let root = std::fs::canonicalize(&root).unwrap();
            let runtime = bootstrap_at(base, &root);
            let conn = crate::database::open(&runtime).unwrap();
            Self {
                base: base.to_path_buf(),
                runtime,
                conn,
            }
        }

        fn store(&self) -> SessionStore<'_> {
            SessionStore::new(&self.conn).unwrap()
        }

        fn store_with_ticking_clock(&self, start: i64, step: i64) -> SessionStore<'_> {
            let next = AtomicI64::new(start);
            let clock: crate::session::store::Clock =
                std::sync::Arc::new(move || next.fetch_add(step, Ordering::SeqCst));
            SessionStore::with_clock(&self.conn, clock).unwrap()
        }

        fn project_id(&self) -> &str {
            self.runtime.project().id().as_str()
        }

        /// A second project sharing this machine's data/config root.
        fn sibling(&self, name: &str) -> Runtime {
            let root = self.base.join("workspace").join(name);
            std::fs::create_dir_all(root.join(".git")).unwrap();
            let root = std::fs::canonicalize(&root).unwrap();
            bootstrap_at(&self.base, &root)
        }
    }

    fn bootstrap_at(base: &Path, root: &Path) -> Runtime {
        let cli = Cli::try_parse_from([
            "glasshouse",
            "--data-dir",
            base.join("data").to_str().unwrap(),
            "--config-dir",
            base.join("config").to_str().unwrap(),
        ])
        .unwrap();
        crate::bootstrap(&cli, root).unwrap()
    }

    /// Insert a row directly, bypassing [`SessionStore`] entirely, to model a
    /// session bearing another project's identifier however it got there.
    /// See `session::store`'s own test module for the fuller explanation.
    fn plant_foreign_row(conn: &Connection, id: &str, project_id: &str, native: Option<&str>) {
        conn.execute_batch("DROP TRIGGER sessions_reject_foreign_project_insert;")
            .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, project_id, harness, native_session_id, role, \
             lifecycle, presentation, created_at, last_activity_at) \
             VALUES (?1, ?2, 'claude-code', ?3, 'normal', 'stopped', 'embedded', 10, 20)",
            rusqlite::params![id, project_id, native],
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER sessions_reject_foreign_project_insert
             BEFORE INSERT ON sessions
             FOR EACH ROW
             WHEN NEW.project_id IS NOT (
                 SELECT value FROM project_metadata WHERE key = 'project_id'
             )
             BEGIN
                 SELECT RAISE(ABORT, 'session belongs to a different project');
             END;",
        )
        .unwrap();
    }

    /// A harness that reads one line and echoes it back, so a real send can
    /// be observed reaching a real terminal.
    #[cfg(unix)]
    fn install_echo_harness(bin_dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = bin_dir.join("echo-harness");
        std::fs::write(&path, "#!/bin/sh\nread line\necho \"got:$line\"\n").unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[cfg(windows)]
    fn install_echo_harness(bin_dir: &Path) -> PathBuf {
        let path = bin_dir.join("echo-harness.cmd");
        std::fs::write(
            &path,
            "@echo off\r\nsetlocal enabledelayedexpansion\r\nset /p line=\r\necho got:!line!\r\n",
        )
        .unwrap();
        path
    }

    /// A harness that announces itself and then stays alive, for tests that
    /// interrupt it or write to it.
    ///
    /// The `READY` line matters: on Windows the child does not start until
    /// something answers ConPTY's `ESC[6n` query (see `drive`'s doc comment),
    /// so a harness that produced no output could still be sitting unstarted
    /// at the handshake when a test acts on it. `READY` in the scrollback is
    /// proof the process the test is about to interrupt is the real one.
    ///
    /// The sleep is minutes, not seconds, and that is load-bearing, found by
    /// a mutation that should have failed and did not: a shorter sleep can
    /// finish **on its own** inside a test's own wait window, and a test
    /// that only checks "is it dead yet" cannot tell that apart from a real
    /// interrupt. Dropping the interrupt delivery entirely (see this
    /// module's mutation record) still passed against a 30-second sleep,
    /// because the process outlived the interrupt but not the test's own
    /// patience. A sleep long enough to outlast any reasonable wait removes
    /// that escape.
    #[cfg(unix)]
    fn install_sleepy_harness(bin_dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = bin_dir.join("sleepy-harness");
        std::fs::write(&path, "#!/bin/sh\necho READY\nsleep 300\n").unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[cfg(windows)]
    fn install_sleepy_harness(bin_dir: &Path) -> PathBuf {
        let path = bin_dir.join("sleepy-harness.cmd");
        std::fs::write(
            &path,
            "@echo off\r\necho READY\r\ntimeout /t 300 /nobreak >nul\r\n",
        )
        .unwrap();
        path
    }

    /// A harness that echoes every line it reads, forever, so more than one
    /// send can be observed reaching a real, still-running child.
    #[cfg(unix)]
    fn install_looping_echo_harness(bin_dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = bin_dir.join("looping-echo-harness");
        std::fs::write(
            &path,
            "#!/bin/sh\nwhile IFS= read -r line; do echo \"got:$line\"; done\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[cfg(windows)]
    fn install_looping_echo_harness(bin_dir: &Path) -> PathBuf {
        let path = bin_dir.join("looping-echo-harness.cmd");
        std::fs::write(
            &path,
            "@echo off\r\nsetlocal enabledelayedexpansion\r\n:loop\r\nset \"line=\"\r\nset /p line=\r\necho got:!line!\r\ngoto loop\r\n",
        )
        .unwrap();
        path
    }

    fn start_live_session(
        fixture: &Fixture,
        live: &mut SessionRuntime,
        id: SessionId,
        bin_dir: &Path,
        install: impl FnOnce(&Path) -> PathBuf,
    ) {
        let harness_path = install(bin_dir);
        let resolved = exec::resolve_explicit(&harness_path).unwrap();
        let launch = HarnessLaunch::new(resolved, fixture.runtime.project());
        live.start(id, SessionPresentation::Embedded, &launch)
            .unwrap();
    }

    /// Drive the runtime the way an owner of one does, until `done` is
    /// satisfied, and fail with what was actually seen.
    ///
    /// `answer_terminal_queries` is in the loop because it is in every
    /// production tick — `shell::run`'s and the headless launch loop in
    /// `main.rs` both call it — and on Windows it is not a nicety. ConPTY
    /// asks `ESC[6n` while bringing the pseudo-console up and **does not let
    /// the child start** until something replies; Glasshouse is the terminal
    /// for an embedded session, so nothing else can. Probed on the Windows
    /// ARM64 CI VM: a harness whose first act was to write a file outside the
    /// pty had still not written it three seconds after spawn, and the entire
    /// scrollback was the one unanswered query. Answering produced the file,
    /// the child's echo, and the buffered input, in that order.
    ///
    /// So a wait loop that only accumulates output is modelling an owner of a
    /// [`SessionRuntime`] that cannot exist. This mirrors
    /// `tests/events_lifecycle.rs`'s `drive` for the same reason.
    fn drive(
        live: &mut SessionRuntime,
        what: &str,
        mut done: impl FnMut(&mut SessionRuntime) -> bool,
    ) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            live.answer_terminal_queries();
            live.poll_exits();
            if done(live) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; sessions: {live:?}"
            );
            std::thread::sleep(POLL);
        }
    }

    #[test]
    fn listing_returns_every_session_in_this_project_most_recent_first() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store_with_ticking_clock(500, 10);

        let first = store.create(NewSession::embedded("claude-code")).unwrap();
        let second = store.create(NewSession::embedded("codex")).unwrap();
        store.touch(&first.id).unwrap();

        let mut live = SessionRuntime::new();
        let api = SessionApi::new(&store, &mut live);

        let listed: Vec<_> = api.list().unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(listed, vec![first.id, second.id]);
    }

    #[test]
    fn state_reports_what_the_store_recorded() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let store = fixture.store();
        let record = store.create(NewSession::embedded("claude-code")).unwrap();

        store
            .set_lifecycle(&record.id, SessionLifecycle::WaitingForUser)
            .unwrap();

        let mut live = SessionRuntime::new();
        let api = SessionApi::new(&store, &mut live);

        assert_eq!(
            api.state(&record.id).unwrap(),
            SessionLifecycle::WaitingForUser,
            "waiting for the user and idle are different states"
        );

        store
            .set_lifecycle(&record.id, SessionLifecycle::Idle)
            .unwrap();
        assert_eq!(api.state(&record.id).unwrap(), SessionLifecycle::Idle);
    }

    #[test]
    fn sending_text_to_a_live_session_reaches_its_terminal() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let store = fixture.store();
        let record = store.create(NewSession::embedded("claude-code")).unwrap();

        let mut live = SessionRuntime::new();
        start_live_session(
            &fixture,
            &mut live,
            record.id.clone(),
            &bin_dir,
            install_echo_harness,
        );

        {
            let mut api = SessionApi::new(&store, &mut live);
            api.send_text(&record.id, "hello", MessageOrigin::Machine)
                .unwrap();
        }

        drive(&mut live, "the harness to echo the line back", |live| {
            live.get(&record.id)
                .map(|session| session.scrollback().contains("got:hello"))
                .unwrap_or(false)
        });
    }

    /// The load-bearing test of the batch: a machine-sent line and a typed
    /// one must never be merged in Glasshouse's own event log, even though
    /// the harness on the other end cannot tell them apart at all.
    #[test]
    fn a_machine_sent_line_is_recorded_separately_from_a_keystroke() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let store = fixture.store();
        let record = store.create(NewSession::embedded("claude-code")).unwrap();

        let mut live = SessionRuntime::new();
        start_live_session(
            &fixture,
            &mut live,
            record.id.clone(),
            &bin_dir,
            install_looping_echo_harness,
        );

        {
            let mut api = SessionApi::new(&store, &mut live);
            api.send_text(&record.id, "machine-line", MessageOrigin::Machine)
                .unwrap();
        }
        // A dead-at-handshake child (never past ConPTY's `ESC[6n`, see
        // `drive`'s doc comment) would never echo this: this line is proof
        // the machine send reached a real, running process, not just
        // Glasshouse's own event log.
        drive(&mut live, "the harness to echo the machine line", |live| {
            live.get(&record.id)
                .map(|session| session.scrollback().contains("got:machine-line"))
                .unwrap_or(false)
        });

        assert!(live.write_to_focused(b"keystroke-line\r").unwrap());
        drive(
            &mut live,
            "the harness to echo the keystroke line",
            |live| {
                live.get(&record.id)
                    .map(|session| session.scrollback().contains("got:keystroke-line"))
                    .unwrap_or(false)
            },
        );

        let history = live.events().history_for(&record.id);
        let origins: Vec<MessageOrigin> = history
            .iter()
            .filter_map(|recorded| match recorded.event() {
                LifecycleEvent::TextDelivered { origin, .. } => Some(*origin),
                _ => None,
            })
            .collect();

        assert_eq!(
            origins,
            vec![MessageOrigin::Machine, MessageOrigin::UserKeystroke],
            "a machine-sent line and a keystroke must be recorded with distinct origins, in order"
        );
    }

    /// The seam records the origin it is told, and no longer decides one.
    ///
    /// The distinction from
    /// [`a_machine_sent_line_is_recorded_separately_from_a_keystroke`] is the
    /// whole point: that test writes one line through this API and one
    /// through the keyboard, so it would pass just as well against the old
    /// hard-wired `MessageOrigin::Machine`. **Both** lines here go through
    /// [`SessionApi::send_text`], on the same session, against the same
    /// running process — so the only thing that can make their recorded
    /// origins differ is the argument, which is exactly the property the
    /// control door needs and did not have.
    ///
    /// Ordered, not merely set-equal: the second row is the person's, and a
    /// seam that stamped every write with its first caller's origin would
    /// produce two identical rows here.
    #[test]
    fn the_seam_records_whichever_origin_its_caller_states() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let store = fixture.store();
        let record = store.create(NewSession::embedded("claude-code")).unwrap();

        let mut live = SessionRuntime::new();
        start_live_session(
            &fixture,
            &mut live,
            record.id.clone(),
            &bin_dir,
            install_looping_echo_harness,
        );

        // Deliberately the same length, so `bytes` cannot be what tells the
        // two rows apart — the same care `tests/worker_access.rs` takes at
        // the door.
        const BY_GLASSHOUSE: &str = "sent-by-machine";
        const BY_A_PERSON: &str = "sent-by-persons";
        assert_eq!(BY_GLASSHOUSE.len(), BY_A_PERSON.len());

        for (text, origin) in [
            (BY_GLASSHOUSE, MessageOrigin::Machine),
            (BY_A_PERSON, MessageOrigin::UserKeystroke),
        ] {
            {
                let mut api = SessionApi::new(&store, &mut live);
                api.send_text(&record.id, text, origin).unwrap();
            }
            // Echoed by a real process before the next line is sent: the
            // rows below are about two deliveries that demonstrably
            // happened, and the wait also fixes their order.
            drive(&mut live, "the harness to echo the line back", |live| {
                live.get(&record.id)
                    .map(|session| session.scrollback().contains(&format!("got:{text}")))
                    .unwrap_or(false)
            });
        }

        let history = live.events().history_for(&record.id);
        let delivered: Vec<(MessageOrigin, usize)> = history
            .iter()
            .filter_map(|recorded| match recorded.event() {
                LifecycleEvent::TextDelivered { origin, bytes } => Some((*origin, *bytes)),
                _ => None,
            })
            .collect();

        assert_eq!(
            delivered,
            vec![
                (MessageOrigin::Machine, BY_GLASSHOUSE.len() + 1),
                (MessageOrigin::UserKeystroke, BY_A_PERSON.len() + 1),
            ],
            "both lines went through `SessionApi::send_text` and differ only in \
             the origin their caller stated, so the recorded origins must \
             differ too — a seam that decides the origin itself cannot record \
             a person's intervention through the control door: {history:?}"
        );
    }

    #[test]
    fn interrupting_through_the_api_is_recorded_as_machine_initiated() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let store = fixture.store();
        let record = store.create(NewSession::embedded("claude-code")).unwrap();

        let mut live = SessionRuntime::new();
        start_live_session(
            &fixture,
            &mut live,
            record.id.clone(),
            &bin_dir,
            install_sleepy_harness,
        );

        // Wait for `READY`, not just for the runtime to say the session
        // started: on Windows the child does not run until something
        // answers ConPTY's handshake (see `drive`'s doc comment), so without
        // this a dead-at-handshake child would still "pass" the interrupt
        // below — nothing would be there to receive it, and nothing here
        // would notice.
        drive(&mut live, "the harness to announce itself", |live| {
            live.get(&record.id)
                .map(|session| session.scrollback().contains("READY"))
                .unwrap_or(false)
        });
        assert!(
            live.get(&record.id).unwrap().is_running(),
            "the harness must be a real, still-running process before it is interrupted"
        );

        {
            let mut api = SessionApi::new(&store, &mut live);
            api.interrupt(&record.id, MessageOrigin::Machine).unwrap();
        }

        // The sleeping child installs no handler, so a real interrupt has a
        // real, platform-specific effect on it -- proof a dead child could
        // never produce, as opposed to the event log entry alone, which the
        // API writes whether or not anything was listening.
        //
        // On Unix the shell's `sleep` simply dies: no trap, default action.
        // On Windows the sleeping harness is a `.cmd` script, and `cmd.exe`
        // itself intercepts Ctrl-C rather than dying -- verified on the ARM64
        // CI VM: the console's own `^C` echo appears immediately, but neither
        // process death nor a "Terminate batch job (Y/N)?" prompt reliably
        // follows within a test's timeout when the batch job is unattended
        // (no one is there to answer Y or N). `^C` in the scrollback is proof
        // enough on its own -- the console only prints it in response to a
        // genuine console control event reaching a live process, and a dead
        // child could not produce it either.
        // A longer deadline than `drive`'s shared `TIMEOUT`, not a shorter
        // one: this waits on a console's own reaction to a control event
        // under whatever load the rest of the suite is putting on the same
        // machine, and that reaction has been observed to take longer than
        // 15s here specifically when many pty-heavy tests are running at
        // once (§34/§40's standing timing debt, not a defect in the wait
        // condition itself).
        {
            let deadline = Instant::now() + Duration::from_secs(45);
            loop {
                live.answer_terminal_queries();
                live.poll_exits();
                let reacted = live
                    .get(&record.id)
                    .map(|session| {
                        if cfg!(windows) {
                            session.scrollback().contains("^C")
                        } else {
                            !session.is_running()
                        }
                    })
                    .unwrap_or(false);
                if reacted {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for the interrupted harness to react to the interrupt; \
                     sessions: {live:?}"
                );
                std::thread::sleep(POLL);
            }
        }

        let history = live.events().history_for(&record.id);
        assert!(
            history.iter().any(|recorded| matches!(
                recorded.event(),
                LifecycleEvent::InterruptDelivered {
                    origin: MessageOrigin::Machine
                }
            )),
            "expected a machine-initiated interrupt in the history: {history:?}"
        );
    }

    #[test]
    fn messaging_a_session_from_another_project_is_refused_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let other = fixture.sibling("beta");
        let other_id = other.project().id().as_str().to_owned();
        assert_ne!(
            other_id,
            fixture.project_id(),
            "fixture must use two projects"
        );

        plant_foreign_row(&fixture.conn, "planted", &other_id, None);

        let store = fixture.store();
        let mut live = SessionRuntime::new();
        let mut api = SessionApi::new(&store, &mut live);

        let error = api
            .send_text(&SessionId::new("planted"), "hi", MessageOrigin::Machine)
            .expect_err("a session from another project must never be messaged");

        match &error {
            ApiError::ForeignProject {
                id,
                expected,
                actual,
            } => {
                assert_eq!(id.as_str(), "planted");
                assert_eq!(expected, fixture.project_id());
                assert_eq!(actual, &other_id);
            }
            other => panic!("expected ForeignProject, got {other:?}"),
        }
    }

    #[test]
    fn every_api_call_refuses_a_foreign_session() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let other = fixture.sibling("beta");
        let other_id = other.project().id().as_str().to_owned();

        plant_foreign_row(&fixture.conn, "planted", &other_id, None);
        let id = SessionId::new("planted");

        let store = fixture.store();
        let mut live = SessionRuntime::new();
        let mut api = SessionApi::new(&store, &mut live);

        assert!(
            matches!(api.state(&id), Err(ApiError::ForeignProject { .. })),
            "state must refuse a foreign session"
        );
        assert!(
            matches!(
                api.send_text(&id, "hi", MessageOrigin::Machine),
                Err(ApiError::ForeignProject { .. })
            ),
            "send_text must refuse a foreign session"
        );
        assert!(
            matches!(
                api.interrupt(&id, MessageOrigin::Machine),
                Err(ApiError::ForeignProject { .. })
            ),
            "interrupt must refuse a foreign session"
        );
        assert!(
            matches!(
                api.recent_output(&id, 100),
                Err(ApiError::ForeignProject { .. })
            ),
            "recent_output must refuse a foreign session"
        );

        let listed = api.list().unwrap();
        assert!(
            listed.iter().all(|record| record.id != id),
            "list must never surface a session from another project"
        );
    }

    #[test]
    fn a_foreign_session_that_is_not_running_is_refused_as_foreign_not_as_dead() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(tmp.path(), "alpha");
        let other = fixture.sibling("beta");
        let other_id = other.project().id().as_str().to_owned();

        // Planted with no native session id and lifecycle 'stopped': not
        // live in this runtime either, so a check that asked liveness first
        // would answer NotLive instead of the true answer.
        plant_foreign_row(&fixture.conn, "planted", &other_id, None);
        let id = SessionId::new("planted");

        let store = fixture.store();
        let mut live = SessionRuntime::new();
        let mut api = SessionApi::new(&store, &mut live);

        let error = api
            .send_text(&id, "hi", MessageOrigin::Machine)
            .expect_err("a foreign, non-running session must still be refused");
        assert!(
            matches!(error, ApiError::ForeignProject { .. }),
            "expected ForeignProject, got {error:?} — scope must be checked before liveness"
        );
    }

    #[test]
    fn recent_output_returns_the_tail_and_never_splits_a_character() {
        let text = "héllo";
        assert_eq!(
            tail(text, 100),
            "héllo",
            "shorter than max_bytes returns everything"
        );
        assert_eq!(tail(text, 5), "éllo");
        assert_eq!(
            tail(text, 4),
            "llo",
            "a boundary that would split é drops the whole character rather than mangling it"
        );
        assert_eq!(tail(text, 0), "");
        for cut in 0..=text.len() {
            assert!(
                std::str::from_utf8(tail(text, cut).as_bytes()).is_ok(),
                "tail({cut}) produced invalid UTF-8"
            );
        }
    }
}
