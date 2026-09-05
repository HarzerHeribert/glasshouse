//! Terminal-backed child processes.
//!
//! This module is the one place that knows how a pseudo-terminal is created
//! and a child process is signalled, via `portable-pty` (Unix PTY
//! primitives, ConPTY on Windows) and one concrete [`PtyProcess`] rather
//! than a trait with one impl, so the rest of Glasshouse can treat any
//! harness identically. Production code derives the working directory
//! through `TerminalCommand::for_harness` from the active [`Project`] alone
//! (`pub(crate)`, so no external caller can pass an arbitrary directory);
//! [`TerminalCommand::new`] and [`PtyProcess::spawn`] stay public for
//! generic PTY work, but the project-root guarantee holds only for
//! [`crate::launch::HarnessLaunch`]. On Windows, ConPTY blocks the child on
//! an unanswered cursor-position
//! query (`ESC[6n`) — a full hang, not a rendering glitch — which this
//! module deliberately does not answer (`PtyProcess` moves bytes, it does
//! not parse them); Glasshouse's terminal-emulation layer (Phase 5) must,
//! or every Windows session hangs silently. `pty_smoke.rs` answers it itself
//! to exercise a real child process on Windows at all. History:
//! design-decisions.md, "Trims: the remaining module docs, second packet",
//! pty/mod.rs module doc.

pub mod process;

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, MasterPty, PtyPair, PtySize, native_pty_system};

use crate::Project;

pub use process::{ProcessSignal, SignalError};

/// Byte a terminal sends when the user presses Ctrl-C.
pub(crate) const ETX: u8 = 0x03;

/// How many times allocating a pseudo-terminal is attempted before the
/// failure is reported to the caller. See [`open_pty`].
const PTY_ALLOCATION_ATTEMPTS: u32 = 5;

/// How long to wait between pseudo-terminal allocation attempts. Long enough
/// for a racing allocation elsewhere on the host to finish, short enough that
/// the worst case (every attempt failing) adds well under a tenth of a second
/// to a launch that was going to fail anyway.
const PTY_ALLOCATION_RETRY_DELAY: Duration = Duration::from_millis(20);

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
            env: Vec::new(),
            env_removed: Vec::new(),
            size: TerminalSize::default(),
        }
    }

    /// Describe a command for a harness process, tied to the active project.
    ///
    /// Crate-only by design: the sanctioned production caller is
    /// [`crate::launch::HarnessLaunch`], which derives everything from the
    /// resolved executable and the active project. The working directory
    /// comes from `project` and nothing on this path exposes a way to set or
    /// mutate it; an external caller cannot reach this seam at all, and
    /// in-crate callers can only escape its rule by using the generic
    /// [`TerminalCommand::new`] instead.
    ///
    /// The directory is [`Project::display_root`]: it denotes exactly the
    /// canonical project root that access control and identity are built on,
    /// but with Windows' verbatim `\\?\` prefix stripped, because
    /// `CreateProcessW`'s `lpCurrentDirectory` does not reliably accept the
    /// verbatim form (and `cmd.exe` refuses it outright). On every other
    /// platform it *is* [`Project::root`].
    pub(crate) fn for_harness(program: impl Into<PathBuf>, project: &Project) -> Self {
        Self::new(program, project.display_root())
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
        // `portable-pty` adds platform defaults to its base environment. On
        // Windows that includes registry values which can replace variables
        // from this process, including PATH. Start from an exact snapshot of
        // Glasshouse's own environment instead, then layer only the recorded
        // child changes over it.
        builder.env_clear();
        for (key, value) in std::env::vars_os() {
            builder.env(key, value);
        }
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

#[cfg(test)]
impl PtyOutput {
    /// A [`PtyOutput`] over something that is not a pseudo-terminal.
    ///
    /// Test-only, and deliberately so: the readers above this type are the
    /// crate's only consumers of it, and the failures they must survive —
    /// an interrupted read, a hangup mid-stream — cannot be produced on
    /// demand from a real pty. Production builds one place only, in
    /// [`PtyProcess::spawn_with`].
    pub(crate) fn from_reader(reader: impl Read + Send + 'static) -> Self {
        Self {
            inner: Box::new(reader),
        }
    }
}

/// Read the next chunk from a terminal, retrying a read that a signal
/// interrupted.
///
/// `None` means nothing more is coming — end-of-file, or an error that is
/// not retryable. `Some(n)` is always a non-zero count.
///
/// [`std::io::Read::read`] does not retry `EINTR` itself, so a signal
/// arriving while a reader thread is blocked surfaces as
/// [`std::io::ErrorKind::Interrupted`] and says nothing about the far end.
/// Every reader above this one is the *only* thing draining its terminal and
/// is never restarted, so folding an interrupted read in with a hangup
/// would end that drain for good and leave the pseudo-terminal to fill
/// until the harness blocks on its own `write` — worse than a gap in the
/// history. Every other error kind still ends the loop: a pty reports a
/// session's end as EOF on some platforms and a read error on others, and
/// neither is a fault to report. Not `#[cfg(unix)]`-gated: `Interrupted` is
/// a Unix concept in practice, and Windows reads essentially never produce
/// it, so the extra branch there is never taken but costs nothing to keep.
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `next_chunk`.
pub(crate) fn next_chunk(reader: &mut impl Read, buffer: &mut [u8]) -> Option<usize> {
    loop {
        match reader.read(buffer) {
            Ok(0) => return None,
            Ok(read) => return Some(read),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
}

/// What a session's terminal is doing with the bytes written to it.
///
/// **Canonical mode** assembles input one line at a time in a kernel buffer
/// with a hard ceiling; a write past it loses data, and **the kernels do
/// not lose it the same way** — see [`CanonicalOverflow`]. Measured, 20
/// trials of each:
///
/// ```text
/// macOS 25.5   1023 + CR = 1024 -> arrives, terminal still works
///              1024 + CR = 1025 -> discarded, terminal wedged forever
/// Linux 7.0.11 4095 + CR = 4096 -> arrives, terminal still works
///              4096 + CR = 4097 -> arrives TRUNCATED to 4095, terminal fine
/// ```
///
/// Both are data loss the writing side is not told about, so one refusal is
/// the right answer to both; only the wreckage differs. In **raw** mode
/// there is no such ceiling — a harness TUI puts its own tty into raw mode
/// as it starts, a plain shell does not — so [`PtyProcess::line_discipline`]
/// obtains the mode rather than assuming it. History: design-decisions.md,
/// "Trims: the remaining module docs, second packet", `LineDiscipline`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineDiscipline {
    /// `ICANON` is set, and this platform's ceiling is known.
    ///
    /// A terminal whose `ICANON` is set on a platform with no known ceiling
    /// is [`LineDiscipline::Unknown`] instead, so that this variant always
    /// carries a limit that can actually be enforced.
    Canonical(CanonicalLine),
    /// `ICANON` is clear: bytes reach the child as they arrive, unbounded.
    Raw,
    /// The mode could not be read, or no ceiling is known for this platform.
    ///
    /// Windows has no line discipline at all — ConPTY is a screen buffer,
    /// not a tty (practice §21) — and on Unix a master that answers neither
    /// `as_raw_fd` nor `tcgetattr` lands here too.
    ///
    /// **Treated as unbounded**, deliberately. Enforcing a Unix ceiling where
    /// there is none would refuse deliveries that work, and the defect this
    /// enum exists for cannot occur where there is no canonical buffer to
    /// overflow.
    Unknown,
}

impl LineDiscipline {
    /// The ceiling one line of input must stay under, or `None` where none
    /// applies.
    pub const fn max_line_bytes(self) -> Option<usize> {
        match self {
            Self::Canonical(line) => Some(line.max_bytes),
            Self::Raw | Self::Unknown => None,
        }
    }
}

/// What a canonical-mode terminal does with a line that overflows its buffer.
///
/// # This is not the same hazard on every platform, and that was measured
///
/// The defect this module's refusal exists for was found on macOS, where an
/// over-long line takes the terminal with it. Setting Linux's ceiling from
/// its documented buffer size and inheriting that description was wrong:
/// Linux's `n_tty` is not BSD's, and one byte over its ceiling is survivable
/// in a way one byte over BSD's is not.
///
/// Both variants are silent data loss, so both justify refusing the write —
/// but a message, a doc comment, or a test that claims the wrong one on the
/// wrong platform is claiming a hazard it cannot demonstrate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalOverflow {
    /// The line's excess **and its terminator** are discarded, so the line
    /// never reaches the reader, the buffer stays full, and every byte
    /// written to that terminal afterwards is discarded too. The session's
    /// input is deaf for the rest of its life.
    ///
    /// macOS and the BSDs. Measured on macOS 25.5 (arm64): a 1025-byte line
    /// never arrived and neither did the four-byte line after it, 20 trials
    /// out of 20.
    WedgesTheTerminal,
    /// The excess is discarded and the line is **delivered short**,
    /// terminator intact; the terminal keeps working and the next line
    /// arrives normally.
    ///
    /// Linux. Measured on 7.0.11 (arm64) inside the gate's own `rust:1.98.0`
    /// image: a 4097-byte line arrived as 4095 bytes, 20 trials out of 20,
    /// with the following four-byte line arriving every time. The discarded
    /// tail does **not** come back as a second line — a 65536-byte line
    /// produces exactly one 4095-byte line and nothing else.
    ///
    /// Quieter than [`Self::WedgesTheTerminal`] and not obviously better: a
    /// wedged session visibly stops, whereas a shell handed a truncated
    /// command runs the truncated command.
    TruncatesTheLine,
}

impl std::fmt::Display for CanonicalOverflow {
    /// The consequence clause of a refusal sentence, in the present
    /// conditional: *"a 1025-byte line would `…`"*.
    ///
    /// Written to slot into [`crate::session::RuntimeError::LineTooLong`]
    /// rather than to stand alone, because that is the only sentence that has
    /// to be true on both platforms and it is the one a caller is shown.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::WedgesTheTerminal => {
                "be discarded along with every byte written to that terminal afterwards"
            }
            Self::TruncatesTheLine => {
                "arrive truncated to that ceiling, silently losing everything past it"
            }
        })
    }
}

/// A canonical-mode terminal's limit on one line, what it counts as the
/// end of one, and what it does to a line that will not fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanonicalLine {
    max_bytes: usize,
    cr_ends_a_line: bool,
    overflow: CanonicalOverflow,
}

impl CanonicalLine {
    /// The most bytes one line may carry, **its terminator included**.
    ///
    /// **macOS/BSD**: `MAX_CANON`, `1024`. **Linux**: `4096`, the `n_tty`
    /// line discipline's own buffer (`N_TTY_BUF_SIZE`) — not the `255` POSIX
    /// minimum `<linux/limits.h>` and `fpathconf` report, which would refuse
    /// 256-byte lines that demonstrably arrive; this number was originally
    /// compiled in with BSD's wedge description, since corrected by
    /// measurement to [`CanonicalOverflow::TruncatesTheLine`]. **Everything
    /// else**, Windows included: no limit, [`LineDiscipline::Unknown`]
    /// rather than [`LineDiscipline::Canonical`]. Each is compiled in per
    /// target, and `tests/canonical_line_limit.rs` measures it back on
    /// whichever platform runs, in both directions.
    ///
    /// History: design-decisions.md, "Trims: the remaining module docs,
    /// second packet", `CanonicalLine::max_bytes`.
    pub const fn max_bytes(self) -> usize {
        self.max_bytes
    }

    /// What this terminal does to a line that will not fit in [`Self::max_bytes`].
    ///
    /// Carried on the value rather than looked up by the caller, so the one
    /// place that knows the platform — [`PtyProcess::line_discipline`], which
    /// read this terminal's mode a statement ago — is also the only place
    /// that has to.
    pub const fn overflow(self) -> CanonicalOverflow {
        self.overflow
    }

    /// Whether this terminal treats `byte` as the end of a line.
    ///
    /// `NL` always. `CR` only when `ICRNL` is set — which it is by default,
    /// and which is why a carriage return is what
    /// `SessionApi::send_text` appends — because with `ICRNL` clear a
    /// carriage return is an ordinary character that ends nothing, and
    /// counting it as a terminator would let a 3000-byte CR-separated block
    /// through to wedge the terminal.
    ///
    /// `VEOF` and `VEOL` also end a canonical line and are deliberately not
    /// listed: leaving a terminator out can only make a line look *longer*
    /// than it is, which refuses a delivery that would have worked. Adding
    /// one wrongly does the opposite, and the opposite is the defect.
    pub const fn ends_a_line(self, byte: u8) -> bool {
        byte == b'\n' || (self.cr_ends_a_line && byte == b'\r')
    }

    /// The longest run of bytes in `input` that this terminal's line buffer
    /// would have to hold at once, terminator included.
    ///
    /// The run after the last terminator counts, and counts as if it were
    /// terminated: it stays in the buffer waiting for one, and it is the
    /// *next* write's terminator that gets discarded for want of room.
    /// Measured on both platforms, because the accounting is the claim and
    /// not the number — writing `max_bytes` bytes and then a bare `CR` as two
    /// separate calls costs exactly what one `max_bytes + 1` write costs:
    /// a macOS pty wedges either way, and a Linux pty truncates either way.
    pub fn longest_line(self, input: &[u8]) -> usize {
        input
            .split(|&byte| self.ends_a_line(byte))
            .map(|segment| segment.len() + 1)
            .max()
            .unwrap_or(0)
    }

    /// `Some(bytes)` when `input` carries a line this terminal would
    /// discard, naming that line's length; `None` when every line fits.
    ///
    /// # What this cannot see
    ///
    /// Only `input`. Bytes already sitting in the kernel's line buffer from
    /// an earlier unterminated delivery are invisible from the master side —
    /// there is no ioctl that reports them — so two 600-byte unterminated
    /// deliveries each pass this check and together wedge the terminal. Every
    /// caller Glasshouse has today ends its delivery with a terminator (see
    /// `SessionApi::send_text`), which empties the buffer and makes the check
    /// exact for them. Reconstructing the buffer's depth across deliveries
    /// would mean modelling the kernel's line editor — erase, kill, werase,
    /// `tcflush` — and a model that drifts high refuses a healthy session
    /// forever, which is a worse failure than the one being prevented.
    pub fn would_discard(self, input: &[u8]) -> Option<usize> {
        let longest = self.longest_line(input);
        (longest > self.max_bytes).then_some(longest)
    }

    /// This platform's canonical-mode ceiling and the hazard one byte over
    /// it, or `None` where no canonical ceiling applies. See
    /// [`CanonicalLine::max_bytes`] for the numbers and [`CanonicalOverflow`]
    /// for the two behaviours, both measured against a real pty. Public
    /// because it is a fact about the **platform**, not one terminal — a
    /// caller sizing a delivery before it has a session (the memory
    /// injection ceiling) needs the number without a pty in hand — and
    /// because `PtyProcess::line_discipline`, its only consumer, is
    /// `#[cfg(unix)]`, so a private constant was dead code on Windows.
    ///
    /// History: design-decisions.md, "Trims: the remaining module docs,
    /// second packet", `CanonicalLine::PLATFORM`.
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    pub const PLATFORM: Option<(usize, CanonicalOverflow)> =
        Some((1024, CanonicalOverflow::WedgesTheTerminal));

    /// See [`CanonicalLine::PLATFORM`].
    #[cfg(target_os = "linux")]
    pub const PLATFORM: Option<(usize, CanonicalOverflow)> =
        Some((4096, CanonicalOverflow::TruncatesTheLine));

    /// See [`CanonicalLine::PLATFORM`].
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly",
        target_os = "linux"
    )))]
    pub const PLATFORM: Option<(usize, CanonicalOverflow)> = None;
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

/// Allocate a pseudo-terminal, retrying a bounded number of times.
///
/// macOS's `openpty(3)` has a race under concurrent allocation that
/// intermittently fails even nowhere near `kern.tty.ptmx_max` (reproduced
/// with 16 concurrent processes holding four pseudo-terminals each; a
/// single process churning the same total produced none); `errno` comes
/// back `-6`, not a valid errno at all, so the condition cannot be
/// classified and must be handled by retrying. This covers exactly one
/// call — the allocation — and nothing has been started when it fails, so
/// retrying is side-effect free by construction, unlike retrying a spawn
/// would be. A genuinely exhausted host still fails, just
/// [`PTY_ALLOCATION_ATTEMPTS`] times over roughly
/// [`PTY_ALLOCATION_RETRY_DELAY`] each. The allocator is a parameter so a
/// test can inject transient failures; production passes
/// `native_pty_system()`.
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `open_pty`.
fn open_pty(
    size: PtySize,
    mut allocate: impl FnMut(PtySize) -> Result<PtyPair>,
) -> Result<PtyPair> {
    let mut last_error = None;
    for attempt in 1..=PTY_ALLOCATION_ATTEMPTS {
        match allocate(size) {
            Ok(pair) => {
                if attempt > 1 {
                    tracing::debug!(attempt, "pseudo-terminal allocated after a retry");
                }
                return Ok(pair);
            }
            Err(err) => {
                tracing::debug!(
                    attempt,
                    attempts = PTY_ALLOCATION_ATTEMPTS,
                    error = %err,
                    "could not allocate a pseudo-terminal"
                );
                last_error = Some(err);
                if attempt < PTY_ALLOCATION_ATTEMPTS {
                    std::thread::sleep(PTY_ALLOCATION_RETRY_DELAY);
                }
            }
        }
    }

    Err(last_error.expect("the loop runs at least once and only exits early on success")).context(
        format!("could not open a pseudo-terminal after {PTY_ALLOCATION_ATTEMPTS} attempts"),
    )
}

/// End and reap a child that a failed spawn is about to abandon.
///
/// `portable_pty::Child` is `std::process::Child` on Unix, whose `Drop`
/// neither kills nor reaps, so dropping one on an error path leaves the
/// harness running unreaped — the two leaks [`PtyProcess::drop`] exists to
/// prevent, on the one path where there is no `PtyProcess` yet to run it.
/// `wait` is unconditional: a child already exited is exactly the case that
/// leaves a permanent zombie. On Unix this reaches the whole process group
/// like [`PtyProcess::signal`]; on Windows it can only reach the direct
/// child, since the Job Object that reaches grandchildren is created later
/// in `spawn` — strictly better than leaking both, and the most this point
/// can do.
///
/// History: design-decisions.md, "Trims: the remaining module docs, second
/// packet", `end_abandoned_child`.
fn end_abandoned_child(child: &mut Box<dyn portable_pty::Child + Send + Sync>) {
    #[cfg(unix)]
    let _ = process::signal_process(ProcessSignal::Kill, child.as_ref());
    #[cfg(windows)]
    {
        let mut killer = child.clone_killer();
        // `None` for the job: there is not one yet at any point this is
        // called. `signal_process` treats the direct killer's result as
        // advisory, for the inverted-result reason recorded there.
        let _ = process::signal_process(ProcessSignal::Kill, None, killer.as_mut());
    }
    let _ = child.wait();
}

impl PtyProcess {
    /// Open a pseudo-terminal and spawn the command into it.
    pub fn spawn(command: TerminalCommand) -> Result<(Self, PtyOutput)> {
        Self::spawn_with(command, |size| native_pty_system().openpty(size))
    }

    /// [`PtyProcess::spawn`], with the pseudo-terminal allocator injected.
    ///
    /// Same seam and same reason as [`open_pty`]'s: production passes
    /// `native_pty_system()`, and a test passes an allocator whose master
    /// fails in the narrow window between the child starting and this
    /// function owning it — the window this function must not leak in.
    fn spawn_with(
        command: TerminalCommand,
        allocate: impl FnMut(PtySize) -> Result<PtyPair>,
    ) -> Result<(Self, PtyOutput)> {
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

        let pair = open_pty(size.into(), allocate)?;

        let mut child = pair.slave.spawn_command(builder).with_context(|| {
            format!(
                "could not start `{}` in `{}`",
                program.display(),
                cwd.display()
            )
        })?;

        // The slave handle must be dropped here: on Unix this is what makes
        // the output reader see EOF (the last open fd for the slave side).
        // On Windows it does *not* produce EOF — `ConPtySlavePty` shares the
        // master's `Arc<Mutex<Inner>>`, and the pipe is released only by
        // `ClosePseudoConsole`, which runs when the *master* is dropped —
        // but dropping the slave here is still correct and necessary. So a
        // reader must treat "the process was observed to have exited"
        // (`try_wait`/`wait`), never EOF, as the authoritative stop
        // condition on every platform, and dropping this `PtyProcess` (which
        // can block on `ClosePseudoConsole` until output drains) must never
        // happen on a UI thread.
        //
        // History: design-decisions.md, "Trims: the remaining module docs,
        // second packet", `PtyProcess::spawn_with` (slave-drop comment).
        drop(pair.slave);

        // Not `?`. The harness is already running at this point and is not
        // yet owned by a `PtyProcess`, so a bare return here abandons it:
        // still running, holding a terminal nothing references, and never
        // reaped. `try_clone_reader` is a `dup()` of the master fd, so
        // `EMFILE`/`ENFILE` under many concurrent sessions reaches this by
        // ordinary resource pressure — the same class of failure `open_pty`
        // above already retries five times for.
        let reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(error) => {
                end_abandoned_child(&mut child);
                return Err(error).context("could not read from the pseudo-terminal");
            }
        };
        let writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(error) => {
                end_abandoned_child(&mut child);
                return Err(error).context("could not write to the pseudo-terminal");
            }
        };

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

    /// What the session's terminal is doing with input **right now**.
    ///
    /// Never cached: a child may change its tty's mode at any instant (a
    /// harness enters raw mode as it draws its first frame, leaves it while
    /// it shells out), so this asks the kernel on every call and callers are
    /// expected to call it immediately before the write it governs — the
    /// residual race is one syscall wide (see `SessionRuntime::send_text_from`).
    /// Reads `tcgetattr` on the **master** fd: a pty pair shares one
    /// `termios`, so the master's answer *is* the slave's line discipline,
    /// verified (not assumed) in `tests/canonical_line_limit.rs`. The fd
    /// (`MasterPty::as_raw_fd`) is borrowed for the call and never stored,
    /// closed, or duplicated.
    ///
    /// History: design-decisions.md, "Trims: the remaining module docs,
    /// second packet", `PtyProcess::line_discipline`.
    #[cfg(unix)]
    pub fn line_discipline(&self) -> LineDiscipline {
        let (Some(fd), Some((max_bytes, overflow))) =
            (self.master.as_raw_fd(), CanonicalLine::PLATFORM)
        else {
            return LineDiscipline::Unknown;
        };
        let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: `fd` is the pty master's descriptor, owned by
        // `self.master`, which outlives this call; nothing here takes
        // ownership of it or closes it. `termios` is a correctly sized and
        // aligned allocation for the one struct `tcgetattr` writes.
        if unsafe { libc::tcgetattr(fd, termios.as_mut_ptr()) } != 0 {
            return LineDiscipline::Unknown;
        }
        // SAFETY: `tcgetattr` returned 0, so it initialised the struct.
        let termios = unsafe { termios.assume_init() };
        if termios.c_lflag & libc::ICANON == 0 {
            return LineDiscipline::Raw;
        }
        LineDiscipline::Canonical(CanonicalLine {
            max_bytes,
            cr_ends_a_line: termios.c_iflag & libc::ICRNL != 0,
            overflow,
        })
    }

    /// What the session's terminal is doing with input right now.
    ///
    /// Always [`LineDiscipline::Unknown`] here: Windows has no line
    /// discipline to read. See that variant's doc comment.
    #[cfg(not(unix))]
    pub fn line_discipline(&self) -> LineDiscipline {
        LineDiscipline::Unknown
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
    use crate::platform::paths as platform_paths;

    /// A terminal shaped like the pty every test in
    /// `tests/canonical_line_limit.rs` measures: `MAX_CANON` 1024, `ICRNL`
    /// set, which is a pty's default on both platforms Glasshouse ships on.
    fn macos_default() -> CanonicalLine {
        CanonicalLine {
            max_bytes: 1024,
            cr_ends_a_line: true,
            overflow: CanonicalOverflow::WedgesTheTerminal,
        }
    }

    /// `payload` bytes followed by the carriage return `SessionApi::send_text`
    /// appends.
    fn terminated(payload: usize) -> Vec<u8> {
        let mut line = vec![b'x'; payload];
        line.push(b'\r');
        line
    }

    #[test]
    fn a_lines_length_counts_its_terminator() {
        let line = macos_default();
        // 1023 payload + CR = 1024 total, which is exactly what a real macOS
        // pty accepts; one more byte is what wedges it.
        assert_eq!(line.longest_line(b"abc\r"), 4);
        assert_eq!(line.would_discard(&terminated(1023)), None);
        assert_eq!(line.would_discard(&terminated(1024)), Some(1025));
    }

    #[test]
    fn an_unterminated_run_counts_as_if_it_were_terminated() {
        // It is the *next* write's terminator that gets discarded for want of
        // room, so a trailing run needs the same byte of headroom a
        // terminated one does. Measured: 1024 bytes then a bare CR, as two
        // writes, wedges a macOS pty exactly as one 1025-byte write does.
        let line = macos_default();
        assert_eq!(line.longest_line(b"abc"), 4);
        assert_eq!(line.would_discard(&[b'x'; 1023]), None);
        assert_eq!(line.would_discard(&[b'x'; 1024]), Some(1025));
    }

    /// Two 999-byte lines separated and terminated by carriage returns —
    /// 2000 bytes that a real pty delivers whole when `ICRNL` is set.
    fn cr_separated() -> Vec<u8> {
        [&b"a".repeat(999)[..], b"\r", &b"b".repeat(999)[..], b"\r"].concat()
    }

    #[test]
    fn a_long_block_of_short_lines_is_not_a_long_line() {
        // The regression a whole-text ceiling would have caused: 3000 bytes
        // of 999-byte lines all arrive on a real pty, so refusing them would
        // refuse something that works.
        let line = macos_default();
        let block = cr_separated();
        assert_eq!(block.len(), 2000);
        assert_eq!(line.longest_line(&block), 1000);
        assert_eq!(line.would_discard(&block), None);
    }

    #[test]
    fn a_carriage_return_ends_a_line_only_when_icrnl_says_so() {
        let with_icrnl = macos_default();
        let without = CanonicalLine {
            cr_ends_a_line: false,
            ..macos_default()
        };
        let block = cr_separated();
        // Same bytes, two terminals, two honest answers: with ICRNL the CRs
        // end lines and this is fine; without it they are ordinary characters
        // and the whole block is one 2000-byte line that would wedge.
        assert_eq!(with_icrnl.would_discard(&block), None);
        assert_eq!(without.would_discard(&block), Some(2001));
        // A newline ends a line either way.
        assert!(with_icrnl.ends_a_line(b'\n'));
        assert!(without.ends_a_line(b'\n'));
        assert!(with_icrnl.ends_a_line(b'\r'));
        assert!(!without.ends_a_line(b'\r'));
    }

    #[test]
    fn nothing_is_discarded_where_no_line_discipline_applies() {
        // Windows, and any Unix master that will not answer `tcgetattr`.
        assert_eq!(LineDiscipline::Raw.max_line_bytes(), None);
        assert_eq!(LineDiscipline::Unknown.max_line_bytes(), None);
        assert_eq!(
            LineDiscipline::Canonical(macos_default()).max_line_bytes(),
            Some(1024)
        );
    }

    #[test]
    fn terminal_size_never_has_a_zero_dimension() {
        let size = TerminalSize::new(0, 0);
        assert_eq!(size.rows, 1);
        assert_eq!(size.cols, 1);
    }

    #[test]
    fn for_harness_cwd_denotes_the_project_root_and_uses_display_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(root.join(".git")).unwrap();

        let project = Project::discover(&root, None, false).unwrap();
        let command = TerminalCommand::for_harness("some-harness", &project);

        // Exactly the display-root form (process-safe on Windows), ...
        assert_eq!(command.cwd(), project.display_root());
        // ... which denotes the very same filesystem location as the
        // canonical root identity is built on.
        assert!(platform_paths::same_file(command.cwd(), project.root()));
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
    fn a_new_command_records_no_implicit_environment_changes() {
        let cmd = TerminalCommand::new("/bin/sh", "/tmp");
        assert!(cmd.env_overrides().is_empty(), "{:?}", cmd.env_overrides());
        assert!(cmd.env_removals().is_empty(), "{:?}", cmd.env_removals());
    }

    /// A transient allocation failure must not reach the caller: `open_pty`
    /// retries, and the pseudo-terminal it finally returns is a real one.
    ///
    /// Non-vacuity: delete the retry loop and this fails on the first
    /// injected error.
    #[test]
    fn a_transient_pty_allocation_failure_is_retried() {
        use std::cell::Cell;

        let attempts = Cell::new(0u32);
        let pair = open_pty(TerminalSize::default().into(), |size| {
            let attempt = attempts.get() + 1;
            attempts.set(attempt);
            if attempt < 3 {
                // The shape of the real macOS failure: an allocation that
                // reports nothing usable about why it failed.
                anyhow::bail!("simulated transient openpty failure");
            }
            native_pty_system().openpty(size)
        })
        .expect("a retried allocation must succeed");

        assert_eq!(
            attempts.get(),
            3,
            "the failing attempts should have retried"
        );
        // Not merely an `Ok`: a genuinely usable pseudo-terminal came back.
        assert!(pair.master.get_size().is_ok());
    }

    /// Retrying is bounded, and a host that really cannot allocate one gets
    /// the underlying error rather than a synthesized or swallowed one.
    #[test]
    fn pty_allocation_gives_up_after_a_bounded_number_of_attempts() {
        use std::cell::Cell;

        let attempts = Cell::new(0u32);
        let result = open_pty(TerminalSize::default().into(), |_| {
            attempts.set(attempts.get() + 1);
            anyhow::bail!("host is out of pseudo-terminals")
        });
        // `PtyPair` has no `Debug`, so unwrap the error by matching rather
        // than through `expect_err`.
        let Err(err) = result else {
            panic!("an always-failing allocation must fail");
        };

        assert_eq!(attempts.get(), PTY_ALLOCATION_ATTEMPTS);
        let message = format!("{err:#}");
        assert!(
            message.contains("host is out of pseudo-terminals"),
            "the real cause must survive: {message}"
        );
        assert!(
            message.contains("could not open a pseudo-terminal"),
            "the caller-facing context must survive: {message}"
        );
    }

    /// An interrupted read says nothing about the far end, so `next_chunk`
    /// must go back and read again rather than report an ending. Everything
    /// else — end-of-file and every other error kind — still ends the loop.
    ///
    /// Non-vacuity: delete the `Interrupted` arm and the first assertion
    /// fails with `None`, which is the whole defect.
    #[test]
    fn an_interrupted_read_is_retried_and_every_other_error_still_ends_it() {
        use std::io::ErrorKind;

        /// Yields the given results in order, then end-of-file forever.
        struct Scripted(std::collections::VecDeque<std::io::Result<&'static [u8]>>);

        impl Read for Scripted {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                match self.0.pop_front() {
                    Some(Ok(bytes)) => {
                        buf[..bytes.len()].copy_from_slice(bytes);
                        Ok(bytes.len())
                    }
                    Some(Err(error)) => Err(error),
                    None => Ok(0),
                }
            }
        }

        let mut buffer = [0u8; 32];

        // Two interruptions in a row, then real bytes: the signal is not an
        // ending however often it arrives.
        let mut reader = Scripted(
            [
                Err(std::io::Error::from(ErrorKind::Interrupted)),
                Err(std::io::Error::from(ErrorKind::Interrupted)),
                Ok(b"harness output".as_slice()),
            ]
            .into_iter()
            .collect(),
        );
        assert_eq!(
            next_chunk(&mut reader, &mut buffer),
            Some("harness output".len()),
            "an interrupted read must be retried, not reported as an ending"
        );
        assert_eq!(&buffer[..14], b"harness output");

        // A real hangup still ends it.
        let mut reader = Scripted(
            [Err(std::io::Error::from(ErrorKind::BrokenPipe))]
                .into_iter()
                .collect(),
        );
        assert_eq!(
            next_chunk(&mut reader, &mut buffer),
            None,
            "a hangup must still end the reader"
        );

        // So does end-of-file.
        let mut reader = Scripted(std::collections::VecDeque::new());
        assert_eq!(next_chunk(&mut reader, &mut buffer), None);
    }

    /// The window between `spawn_command` and the `PtyProcess` that owns its
    /// child is the one place `PtyProcess::drop` cannot cover, so `spawn`
    /// must end and reap the harness itself before returning an error.
    ///
    /// The assertion is the absence of the process, not the presence of an
    /// error: `kill(pid, 0)` fails with `ESRCH` only once the pid has been
    /// both killed **and** reaped — a zombie is still signallable by its own
    /// parent and would answer `Ok`. That distinction is the point: the two
    /// leaks `PtyProcess::drop`'s comment records are exactly "still running"
    /// and "never reaped", and this rules out both.
    ///
    /// Non-vacuity: revert either error arm in `spawn_with` to a `?` and this
    /// fails with the harness still alive.
    #[cfg(unix)]
    #[test]
    fn a_spawn_that_fails_after_the_child_starts_leaves_no_process_behind() {
        use std::sync::{Arc, Mutex};

        /// Records the pid of whatever it spawns, then hands the child on
        /// untouched — the test's only way to learn the identity of a child
        /// `spawn` is about to abandon.
        struct RecordingSlave {
            inner: Box<dyn portable_pty::SlavePty + Send>,
            pid: Arc<Mutex<Option<u32>>>,
        }

        impl portable_pty::SlavePty for RecordingSlave {
            fn spawn_command(
                &self,
                cmd: CommandBuilder,
            ) -> std::result::Result<Box<dyn portable_pty::Child + Send + Sync>, anyhow::Error>
            {
                let child = self.inner.spawn_command(cmd)?;
                *self.pid.lock().unwrap() = child.process_id();
                Ok(child)
            }
        }

        /// A real master whose reader cannot be cloned — the shape of
        /// `EMFILE` on the `dup()` that `try_clone_reader` performs.
        struct UncloneableMaster(Box<dyn MasterPty + Send>);

        impl MasterPty for UncloneableMaster {
            fn resize(&self, size: PtySize) -> std::result::Result<(), anyhow::Error> {
                self.0.resize(size)
            }
            fn get_size(&self) -> std::result::Result<PtySize, anyhow::Error> {
                self.0.get_size()
            }
            fn try_clone_reader(&self) -> std::result::Result<Box<dyn Read + Send>, anyhow::Error> {
                Err(anyhow::anyhow!("too many open files"))
            }
            fn take_writer(&self) -> std::result::Result<Box<dyn Write + Send>, anyhow::Error> {
                self.0.take_writer()
            }
            fn process_group_leader(&self) -> Option<libc::pid_t> {
                self.0.process_group_leader()
            }
            fn as_raw_fd(&self) -> Option<portable_pty::unix::RawFd> {
                self.0.as_raw_fd()
            }
            fn tty_name(&self) -> Option<PathBuf> {
                self.0.tty_name()
            }
        }

        let pid = Arc::new(Mutex::new(None));
        let recorded = Arc::clone(&pid);

        // A harness that would outlive the test by minutes if nothing ended
        // it, so "gone" cannot be confused with "finished on its own".
        let command = TerminalCommand::new("/bin/sh", "/").args(["-c", "sleep 300"]);

        let result = PtyProcess::spawn_with(command, move |size| {
            let pair = native_pty_system().openpty(size)?;
            Ok(PtyPair {
                slave: Box::new(RecordingSlave {
                    inner: pair.slave,
                    pid: Arc::clone(&recorded),
                }),
                master: Box::new(UncloneableMaster(pair.master)),
            })
        });

        let Err(error) = result else {
            panic!("a master whose reader cannot be cloned must fail the spawn");
        };
        assert!(
            format!("{error:#}").contains("could not read from the pseudo-terminal"),
            "the caller-facing context must survive: {error:#}"
        );

        let pid = pid.lock().unwrap().expect("the child was started");
        // SAFETY: signal 0 sends nothing; it only asks whether the pid is
        // still addressable by this process.
        let alive = unsafe { libc::kill(pid as libc::pid_t, 0) };
        let errno = std::io::Error::last_os_error().raw_os_error();
        assert_eq!(
            (alive, errno),
            (-1, Some(libc::ESRCH)),
            "the abandoned harness (pid {pid}) must be killed and reaped, not left \
             running or left a zombie"
        );
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
