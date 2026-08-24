//! Terminal-backed child processes.
//!
//! Every harness Glasshouse manages is a real interactive program that expects
//! a terminal: it draws a TUI, reads keystrokes, and reacts to window resizes.
//! This module is the one place that knows how a pseudo-terminal is created and
//! how a child process is signalled, so the rest of Glasshouse can treat a
//! Claude Code session on macOS and a Codex session on native Windows
//! identically.
//!
//! Platform coverage comes from `portable-pty`, which uses Unix PTY primitives
//! on macOS and Linux and ConPTY on native Windows. Using that crate rather
//! than hand-rolling both backends is deliberate: it is an established
//! primitive that solves exactly this problem, and reimplementing ConPTY would
//! be complexity with no product benefit.
//!
//! There is intentionally no trait here. A single concrete [`PtyProcess`] is
//! already a common interface that hides every platform difference; adding a
//! trait for one implementation would be abstraction without a second
//! implementation to justify it.
//!
//! # Constraint for whatever renders this process's output
//!
//! On Windows, ConPTY emits a Device Status Report query (`ESC[6n`, "where is
//! the cursor?") on the pty's output as part of bringing the pseudo-console
//! up, and it blocks -- the child does not get to run, let alone produce any
//! output of its own -- until something on the other end writes back
//! `ESC[<row>;<col>R`. This is not a rendering glitch to clean up later: an
//! unanswered query is a full hang, indistinguishable from the harness
//! process itself having wedged, with nothing in the pty output to explain
//! why.
//!
//! This module is deliberately not where that gets answered. `PtyProcess`
//! only moves bytes in and out of the child; it has no notion of what a byte
//! *means*, and answering a control sequence requires exactly that. The
//! right place is Glasshouse's terminal-emulation layer (Phase 5 of the
//! capability map, which renders a harness's output and is the one place
//! that already parses the terminal control sequences flowing through this
//! module) -- whoever builds it needs to answer this query as part of
//! bringing a session up on Windows, or every Windows session will hang
//! silently before producing a single rendered byte.
//!
//! `crates/glasshouse/tests/pty_smoke.rs` answers this query itself (see
//! that file's module doc for how), which is the only reason its tests can
//! exercise a real child process on Windows at all -- without it, every test
//! needing child output or child exit hangs on exactly this handshake and
//! times out.

pub mod process;

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

pub use process::{ProcessSignal, SignalError};

/// Byte a terminal sends when the user presses Ctrl-C.
pub(crate) const ETX: u8 = 0x03;

/// Visible size of a terminal, in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub rows: u16,
    pub cols: u16,
}

impl TerminalSize {
    pub fn new(rows: u16, cols: u16) -> Self {
        // A zero dimension is rejected by the kernel on some platforms and
        // makes every TUI misbehave on the rest, so clamp to something usable.
        Self {
            rows: rows.max(1),
            cols: cols.max(1),
        }
    }
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self::new(24, 80)
    }
}

impl From<TerminalSize> for PtySize {
    fn from(size: TerminalSize) -> Self {
        PtySize {
            rows: size.rows,
            cols: size.cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

/// How a terminal-backed child process should be started.
///
/// `program` and `args` are already-resolved values: callers that start from a
/// bare command name run it through [`crate::platform::exec`] first, so this
/// module never has to know that a `.cmd` launcher on Windows needs an
/// interpreter. That keeps the PTY runtime independent of any harness adapter.
#[derive(Debug, Clone)]
pub struct TerminalCommand {
    program: PathBuf,
    args: Vec<OsString>,
    cwd: PathBuf,
    env: Vec<(OsString, OsString)>,
    /// Keys to strip from the child's inherited environment. Disjoint from
    /// `env` by construction: `env` and `env_remove` each remove a key from
    /// the other's list before recording their own operation, so a key is
    /// never pending in both at once. See [`TerminalCommand::env_remove`].
    env_removed: Vec<OsString>,
    size: TerminalSize,
}

impl TerminalCommand {
    /// Start describing a command.
    ///
    /// The working directory is required rather than optional: every harness
    /// process Glasshouse starts must run inside the active project root, and
    /// making that a mandatory argument means it cannot be forgotten.
    pub fn new(program: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            env: default_terminal_env(),
            env_removed: Vec::new(),
            size: TerminalSize::default(),
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Set an environment variable for the child process only.
    ///
    /// The child inherits the Glasshouse environment; these are overrides on
    /// top of it. Launch profiles rely on this to point a harness at an
    /// alternate provider without touching the user's global configuration.
    ///
    /// Ordering with [`TerminalCommand::env_remove`]: whichever of the two
    /// was called most recently for `key` wins. A key can never be both a
    /// pending override and a pending removal at once, so this `env` call
    /// always cancels out any earlier `env_remove` for the same key.
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        let key = key.into();
        self.env.retain(|(k, _)| k != &key);
        self.env_removed.retain(|k| k != &key);
        self.env.push((key, value.into()));
        self
    }

    /// Remove an environment variable the child would otherwise inherit.
    ///
    /// Launch profiles need this to route a session through a gateway: the
    /// child must not see a provider API key Glasshouse itself inherited,
    /// not merely a different value for it. Unlike [`TerminalCommand::env`],
    /// which only overrides what the child sees, this reaches into the
    /// child's inherited environment and removes the key outright.
    ///
    /// Ordering with [`TerminalCommand::env`]: whichever of the two was
    /// called most recently for `key` wins — see `env`'s doc comment for why
    /// that is the whole ordering rule.
    pub fn env_remove(mut self, key: impl Into<OsString>) -> Self {
        let key = key.into();
        self.env.retain(|(k, _)| k != &key);
        self.env_removed.retain(|k| k != &key);
        self.env_removed.push(key);
        self
    }

    pub fn size(mut self, size: TerminalSize) -> Self {
        self.size = size;
        self
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn args_slice(&self) -> &[OsString] {
        &self.args
    }

    /// The environment overrides applied to the child, in application order.
    pub fn env_overrides(&self) -> &[(OsString, OsString)] {
        &self.env
    }

    /// The environment variable names being stripped from the child's
    /// inherited environment. See [`TerminalCommand::env_remove`].
    pub fn env_removals(&self) -> &[OsString] {
        &self.env_removed
    }

    fn into_builder(self) -> (CommandBuilder, TerminalSize) {
        let mut builder = CommandBuilder::new(&self.program);
        builder.args(&self.args);
        builder.cwd(&self.cwd);
        for (key, value) in &self.env {
            builder.env(key, value);
        }
        // `env` and `env_removed` are disjoint (see the field doc comment),
        // so applying removals after overrides — or before, it does not
        // matter — never lets one undo the other for the same key.
        for key in &self.env_removed {
            builder.env_remove(key);
        }
        (builder, self.size)
    }
}

/// Environment every Glasshouse-started terminal process gets unless a caller
/// overrides it.
///
/// Harness TUIs render badly or refuse colour output when `TERM` is missing or
/// says the terminal is dumb, which happens whenever Glasshouse itself was
/// started from a context without a terminal.
fn default_terminal_env() -> Vec<(OsString, OsString)> {
    let term = std::env::var_os("TERM").filter(|t| {
        let t = t.to_string_lossy();
        !t.is_empty() && t != "dumb"
    });
    vec![(
        OsString::from("TERM"),
        term.unwrap_or_else(|| OsString::from("xterm-256color")),
    )]
}

/// How a child process finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitStatus {
    success: bool,
    code: u32,
    signal: Option<String>,
}

impl ExitStatus {
    pub fn success(&self) -> bool {
        self.success
    }

    pub fn code(&self) -> u32 {
        self.code
    }

    /// Name of the signal that killed the process, when it was signalled.
    pub fn signal(&self) -> Option<&str> {
        self.signal.as_deref()
    }
}

impl From<portable_pty::ExitStatus> for ExitStatus {
    fn from(status: portable_pty::ExitStatus) -> Self {
        Self {
            success: status.success(),
            code: status.exit_code(),
            signal: status.signal().map(str::to_owned),
        }
    }
}

impl std::fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.signal, self.success) {
            (Some(sig), _) => write!(f, "terminated by {sig}"),
            (None, true) => f.write_str("exited successfully"),
            (None, false) => write!(f, "exited with code {}", self.code),
        }
    }
}

/// The readable end of a terminal-backed process.
///
/// Handed out separately from [`PtyProcess`] so output can be streamed on its
/// own thread while the process is still being written to and signalled.
///
/// This holds its own duplicate of the pty master's read fd, so it must be
/// dropped with or before its `PtyProcess`, never after. On Unix, dropping
/// `PtyProcess` closes the pty only once every dup of the master fd is gone;
/// a `PtyOutput` kept alive past its `PtyProcess` is exactly such a dup, and
/// a probe confirmed the effect: the child is left running (state `S`, no
/// `SIGHUP`) rather than being torn down, because the hangup that would
/// otherwise close it never fires.
pub struct PtyOutput {
    inner: Box<dyn Read + Send>,
}

impl Read for PtyOutput {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl std::fmt::Debug for PtyOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PtyOutput")
    }
}

/// A running child process attached to a pseudo-terminal.
pub struct PtyProcess {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Used only on Windows: Unix signals the child's process group directly
    /// (see [`process::signal_process`]) and has no use for portable-pty's
    /// killer handle at all.
    #[cfg(windows)]
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    /// A Windows Job Object the child was placed in so that killing it also
    /// reaches processes it spawned, not just itself. `None` when placing it
    /// failed (see [`process::JobHandle::assign`]) — signalling then falls
    /// back to the direct killer, which cannot reach grandchildren.
    #[cfg(windows)]
    job: Option<process::JobHandle>,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    size: TerminalSize,
    /// Cached once observed, because a process can only be reaped once and
    /// signalling a reaped pid could reach an unrelated process.
    ///
    /// The invariant this whole module depends on: nothing other than this
    /// `PtyProcess` ever reaps this child (no background reaper, nothing
    /// else calling `waitpid`/`GetExitCodeProcess` on it). That is what
    /// makes an *unreaped* exited child still pin its pid — the operating
    /// system cannot recycle it for an unrelated process until something
    /// reaps it — which is the only reason `signal`'s `kill(-pgid, ...)` is
    /// safe to call at all between the liveness check and the actual
    /// syscall. If a background reaper is ever introduced, this reasoning
    /// breaks and `signal` needs to be revisited.
    exit_status: Option<ExitStatus>,
}

impl std::fmt::Debug for PtyProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyProcess")
            .field("pid", &self.process_id())
            .field("size", &self.size)
            .field("exit_status", &self.exit_status)
            .finish()
    }
}

impl PtyProcess {
    /// Open a pseudo-terminal and spawn the command into it.
    pub fn spawn(command: TerminalCommand) -> Result<(Self, PtyOutput)> {
        let program = command.program().to_path_buf();
        let cwd = command.cwd().to_path_buf();

        if !cwd.is_dir() {
            anyhow::bail!(
                "working directory `{}` for `{}` does not exist",
                cwd.display(),
                program.display()
            );
        }

        let (builder, size) = command.into_builder();

        let pair = native_pty_system()
            .openpty(size.into())
            .context("could not open a pseudo-terminal")?;

        let child = pair.slave.spawn_command(builder).with_context(|| {
            format!(
                "could not start `{}` in `{}`",
                program.display(),
                cwd.display()
            )
        })?;

        // The slave handle must be dropped here.
        //
        // On Unix this is what makes the output reader see EOF: the pty only
        // reports end-of-file once every open fd for the slave side is
        // closed, and this was the last one Glasshouse itself would
        // otherwise still be holding.
        //
        // On Windows this line does *not* produce EOF. `ConPtySlavePty`
        // shares an `Arc<Mutex<Inner>>` with the master, so dropping it
        // closes no descriptor; the pipe's write end lives inside conhost
        // and is released only by `ClosePseudoConsole`, which runs when the
        // *master* — the `MasterPty` this `PtyProcess` owns — is dropped,
        // not the slave. Dropping the slave here is still correct and
        // necessary (a `SlavePty` has nothing left to do once
        // `spawn_command` has returned, and Unix genuinely needs it gone),
        // it just does not give Windows callers an EOF-based way to notice
        // the child is done.
        //
        // Constraint this implies for a future interactive reader thread
        // (Phase 4): it must not treat "no more bytes" as its stop
        // condition, because on Windows that may never come while the pty is
        // still held open. Treat "the process was observed to have exited"
        // (`try_wait`/`wait`) as the authoritative stop condition on every
        // platform instead. And because releasing the pty — dropping this
        // `PtyProcess` — is what eventually lets `ClosePseudoConsole` run,
        // and that call can block until buffered output has drained to a
        // reader, that drop must never happen on a UI thread.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .context("could not read from the pseudo-terminal")?;
        let writer = pair
            .master
            .take_writer()
            .context("could not write to the pseudo-terminal")?;

        #[cfg(windows)]
        let killer = child.clone_killer();

        // Put the child in a Job Object so that killing it also reaches
        // whatever it spawned (Glasshouse's harnesses are npm `.cmd` shims,
        // so the direct child is `cmd.exe` and the real process is a
        // grandchild `node.exe`). This can legitimately fail — see
        // `process::JobHandle::assign` — in which case signalling falls back
        // to the direct killer, which is worse but not fatal, so a failure
        // here must never fail the spawn itself.
        #[cfg(windows)]
        let job = match child.as_raw_handle() {
            Some(handle) => match process::JobHandle::assign(handle) {
                Ok(job) => Some(job),
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "could not put the child process in a Windows Job Object; \
                         terminating it will not reach anything it spawns"
                    );
                    None
                }
            },
            None => None,
        };

        tracing::debug!(
            program = %program.display(),
            cwd = %cwd.display(),
            pid = ?child.process_id(),
            rows = size.rows,
            cols = size.cols,
            "spawned terminal process"
        );

        Ok((
            Self {
                child,
                #[cfg(windows)]
                killer,
                #[cfg(windows)]
                job,
                master: pair.master,
                writer,
                size,
                exit_status: None,
            },
            PtyOutput { inner: reader },
        ))
    }

    /// Operating-system process identifier, while the process is alive.
    pub fn process_id(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// Write raw bytes to the terminal, exactly as if they had been typed.
    pub fn write_input(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    /// Send text to the session without the user having to focus it.
    ///
    /// This is the transport orchestration uses to hand a task to a worker.
    pub fn send_text(&mut self, text: &str) -> std::io::Result<()> {
        self.write_input(text.as_bytes())
    }

    /// Tell the child that the window changed size.
    pub fn resize(&mut self, size: TerminalSize) -> Result<()> {
        self.master
            .resize(size.into())
            .context("could not resize the pseudo-terminal")?;
        self.size = size;
        Ok(())
    }

    /// The size Glasshouse last asked for.
    pub fn size(&self) -> TerminalSize {
        self.size
    }

    /// The size the operating system reports for the pseudo-terminal.
    ///
    /// Used to confirm a resize actually reached the kernel rather than only
    /// updating Glasshouse's own bookkeeping.
    pub fn os_size(&self) -> Result<TerminalSize> {
        let size = self
            .master
            .get_size()
            .context("could not read the pseudo-terminal size")?;
        Ok(TerminalSize::new(size.rows, size.cols))
    }

    /// Interrupt the running task the way a user pressing Ctrl-C would.
    ///
    /// This writes the interrupt character into the terminal rather than
    /// sending a signal directly. That matters: harness TUIs run the terminal
    /// in raw mode and handle Ctrl-C themselves to cancel the current turn
    /// while staying alive. Sending SIGINT out of band would bypass that
    /// handling. Use [`PtyProcess::signal`] when the process itself must go
    /// away.
    pub fn interrupt(&mut self) -> std::io::Result<()> {
        self.write_input(&[ETX])
    }

    /// Send a real process signal to the child and its descendants.
    pub fn signal(&mut self, signal: ProcessSignal) -> Result<(), SignalError> {
        // `exit_status` only reflects what a *previous* `wait`/`try_wait`
        // observed; nothing forces a poll on its own, so a session that
        // exited on its own since the last check would otherwise still look
        // signallable and this would return `Ok(())` without having
        // terminated anything. Poll first so that never happens.
        if self.try_wait().map_err(SignalError::Os)?.is_some() {
            return Err(SignalError::AlreadyExited);
        }

        #[cfg(unix)]
        {
            process::signal_process(signal, self.child.as_ref())
        }
        #[cfg(windows)]
        {
            process::signal_process(signal, self.job.as_ref(), self.killer.as_mut())
        }
    }

    /// Check whether the process has finished, without blocking.
    ///
    /// Exit is detected from the process itself rather than from anything in
    /// the terminal output, so a quiet session is never mistaken for a finished
    /// one.
    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        if let Some(status) = &self.exit_status {
            return Ok(Some(status.clone()));
        }
        match self.child.try_wait()? {
            Some(status) => {
                let status = ExitStatus::from(status);
                self.exit_status = Some(status.clone());
                Ok(Some(status))
            }
            None => Ok(None),
        }
    }

    /// Block until the process finishes.
    pub fn wait(&mut self) -> std::io::Result<ExitStatus> {
        if let Some(status) = &self.exit_status {
            return Ok(status.clone());
        }
        let status = ExitStatus::from(self.child.wait()?);
        self.exit_status = Some(status.clone());
        Ok(status)
    }

    /// The exit status if it has already been observed.
    pub fn exit_status(&self) -> Option<&ExitStatus> {
        self.exit_status.as_ref()
    }
}

impl Drop for PtyProcess {
    /// Never leave a session running or a pid unreaped just because nobody
    /// remembered to call `signal` and `wait` before letting a `PtyProcess`
    /// go out of scope. Without this, probes showed two separate leaks: an
    /// exited-but-unwaited child stays a zombie forever (five spawn+drop
    /// cycles left five permanent zombies), and a still-running child simply
    /// keeps running, unreachable, once nothing references it any more.
    ///
    /// This cannot panic: `signal` and `wait` both return `Result`, and both
    /// results are discarded rather than unwrapped. It cannot block
    /// indefinitely either: `signal(Kill)` sends an unmaskable termination
    /// (`SIGKILL` to the group on Unix, `TerminateJobObject`/
    /// `TerminateProcess` on Windows) which a well-behaved kernel honours
    /// promptly, and `wait` afterward only blocks for however long that
    /// takes to land — it does not wait on anything Glasshouse controls. If
    /// the process already exited, `signal` returns `AlreadyExited`
    /// immediately (no signal sent) and `wait` returns just as fast, since
    /// waiting on an already-terminated child never blocks.
    fn drop(&mut self) {
        if self.exit_status.is_none() {
            let _ = self.signal(ProcessSignal::Kill);
            let _ = self.child.wait();
        }
    }
}

/// Convenience for building an argument list from string-ish values.
pub fn os_args<I, S>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter()
        .map(|a| a.as_ref().to_os_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_size_never_has_a_zero_dimension() {
        let size = TerminalSize::new(0, 0);
        assert_eq!(size.rows, 1);
        assert_eq!(size.cols, 1);
    }

    #[test]
    fn env_overrides_replace_rather_than_duplicate() {
        let cmd = TerminalCommand::new("/bin/sh", "/tmp")
            .env("FOO", "one")
            .env("FOO", "two");
        let foo: Vec<_> = cmd
            .env_overrides()
            .iter()
            .filter(|(k, _)| k == "FOO")
            .collect();
        assert_eq!(foo.len(), 1);
        assert_eq!(foo[0].1, OsString::from("two"));
    }

    #[test]
    fn a_later_env_call_wins_over_an_earlier_env_remove() {
        let cmd = TerminalCommand::new("/bin/sh", "/tmp")
            .env("FOO", "one")
            .env_remove("FOO")
            .env("FOO", "two");

        assert!(cmd.env_removals().is_empty(), "{:?}", cmd.env_removals());
        let foo: Vec<_> = cmd
            .env_overrides()
            .iter()
            .filter(|(k, _)| k == "FOO")
            .collect();
        assert_eq!(foo.len(), 1);
        assert_eq!(foo[0].1, OsString::from("two"));
    }

    #[test]
    fn a_later_env_remove_call_wins_over_an_earlier_env() {
        let cmd = TerminalCommand::new("/bin/sh", "/tmp")
            .env("FOO", "one")
            .env_remove("FOO");

        assert!(cmd.env_overrides().iter().all(|(k, _)| k != "FOO"));
        assert_eq!(cmd.env_removals(), &[OsString::from("FOO")]);
    }

    #[test]
    fn env_remove_reaches_an_inherited_variable_not_just_overrides() {
        // `into_builder`'s `CommandBuilder` starts from the real process
        // environment. `PATH` is about as close to a universal inherited
        // variable as exists, so removing it proves `env_remove` reaches
        // into inherited state rather than only ever undoing a prior `.env`.
        let cmd = TerminalCommand::new("/bin/sh", "/tmp").env_remove("PATH");
        let (builder, _) = cmd.into_builder();
        assert_eq!(builder.get_env("PATH"), None);
    }

    #[test]
    fn a_usable_term_is_always_set() {
        let cmd = TerminalCommand::new("/bin/sh", "/tmp");
        let term = cmd
            .env_overrides()
            .iter()
            .find(|(k, _)| k == "TERM")
            .expect("TERM override");
        assert!(!term.1.is_empty());
        assert_ne!(term.1, OsString::from("dumb"));
    }

    #[test]
    fn spawning_into_a_missing_working_directory_fails_clearly() {
        let err = PtyProcess::spawn(TerminalCommand::new(
            "/bin/sh",
            "/definitely/not/a/real/directory",
        ))
        .unwrap_err();
        assert!(err.to_string().contains("does not exist"), "{err}");
    }
}
