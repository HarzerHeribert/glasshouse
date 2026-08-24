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
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        let key = key.into();
        self.env.retain(|(k, _)| k != &key);
        self.env.push((key, value.into()));
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

    fn into_builder(self) -> (CommandBuilder, TerminalSize) {
        let mut builder = CommandBuilder::new(&self.program);
        builder.args(&self.args);
        builder.cwd(&self.cwd);
        for (key, value) in &self.env {
            builder.env(key, value);
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
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    size: TerminalSize,
    /// Cached once observed, because a process can only be reaped once and
    /// signalling a reaped pid could reach an unrelated process.
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

        // The slave handle must be dropped here. While Glasshouse holds it open
        // the pty never reaches end-of-file, so the output reader would block
        // forever after the child exits instead of seeing EOF.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .context("could not read from the pseudo-terminal")?;
        let writer = pair
            .master
            .take_writer()
            .context("could not write to the pseudo-terminal")?;
        let killer = child.clone_killer();

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
                killer,
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
        if self.exit_status.is_some() {
            return Err(SignalError::AlreadyExited);
        }
        process::signal_process(
            signal,
            self.child.as_ref(),
            self.master.as_ref(),
            self.killer.as_mut(),
        )
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
