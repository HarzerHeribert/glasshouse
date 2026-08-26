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
//! - **A message this API sends is never confused with a keystroke.** Every
//!   write goes through [`super::runtime::SessionRuntime::send_text_from`] and
//!   [`super::runtime::SessionRuntime::interrupt_from`] with
//!   [`crate::events::MessageOrigin::Machine`], not the plain `send_text` /
//!   `interrupt` a person's keyboard uses. The distinction is recorded in
//!   Glasshouse's own event log, not inferred later from context that will
//!   not exist by then.

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

    /// Send a line of text to a live session, as Glasshouse rather than as
    /// the user.
    ///
    /// A carriage return is appended, the same way `shell::send_session_text`
    /// sends a line typed at the shell's own prompt: this call delivers one
    /// line, not raw bytes, and `\r` is what a session's terminal expects to
    /// see for the harness's line editor to submit it.
    pub fn send_text(&mut self, id: &SessionId, text: &str) -> Result<(), ApiError> {
        self.resolve(id)?;
        if self.live.get(id).is_none() {
            return Err(ApiError::NotLive { id: id.clone() });
        }
        let mut line = String::with_capacity(text.len() + 1);
        line.push_str(text);
        line.push('\r');
        self.live
            .send_text_from(id, &line, MessageOrigin::Machine)?;
        Ok(())
    }

    /// Interrupt a live session, as Glasshouse rather than as the user.
    pub fn interrupt(&mut self, id: &SessionId) -> Result<(), ApiError> {
        self.resolve(id)?;
        if self.live.get(id).is_none() {
            return Err(ApiError::NotLive { id: id.clone() });
        }
        self.live.interrupt_from(id, MessageOrigin::Machine)?;
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

    /// A harness that just stays alive, for tests that only need a live
    /// process to write to or interrupt and do not care what it prints.
    #[cfg(unix)]
    fn install_sleepy_harness(bin_dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = bin_dir.join("sleepy-harness");
        std::fs::write(&path, "#!/bin/sh\nsleep 30\n").unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[cfg(windows)]
    fn install_sleepy_harness(bin_dir: &Path) -> PathBuf {
        let path = bin_dir.join("sleepy-harness.cmd");
        std::fs::write(&path, "@echo off\r\ntimeout /t 30 /nobreak >nul\r\n").unwrap();
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

    fn wait_for(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if condition() {
                return;
            }
            std::thread::sleep(POLL);
        }
        panic!("timed out waiting for a condition to become true");
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
            api.send_text(&record.id, "hello").unwrap();
        }

        wait_for(|| {
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
            install_sleepy_harness,
        );

        {
            let mut api = SessionApi::new(&store, &mut live);
            api.send_text(&record.id, "machine-line").unwrap();
        }
        assert!(live.write_to_focused(b"keystroke-line\r").unwrap());

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

        {
            let mut api = SessionApi::new(&store, &mut live);
            api.interrupt(&record.id).unwrap();
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
            .send_text(&SessionId::new("planted"), "hi")
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
                api.send_text(&id, "hi"),
                Err(ApiError::ForeignProject { .. })
            ),
            "send_text must refuse a foreign session"
        );
        assert!(
            matches!(api.interrupt(&id), Err(ApiError::ForeignProject { .. })),
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
            .send_text(&id, "hi")
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
