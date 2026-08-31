//! cmux as an *optional* presentation backend — Phase 17, capability map
//! lines 754–763, and Phase 54, lines 1892–1895.
//!
//! # What this module is, and what it is not
//!
//! cmux is a terminal multiplexer with a documented command-line surface.
//! When Glasshouse is running inside it, a person can ask for a session to
//! be *presented externally*: Glasshouse opens a new cmux workspace in the
//! project root, runs an ordinary Glasshouse launch in that pane, and
//! remembers the pane as the session's presentation metadata. Afterwards the
//! pane can be brought to the front (`glasshouse sessions focus`) and text
//! can reach it through cmux when Glasshouse's own door cannot.
//!
//! That is the whole of it. cmux is a **workspace and presentation backend**
//! (line 763), never the orchestration core: nothing in `session/**` or
//! `shell/**` names it, the session abstraction learns one nullable string
//! (line 762 — see [`crate::session::SessionRecord::presentation_ref`]), and
//! every core function works identically when cmux is absent, changes, or
//! disappears (lines 755, 1894). A tripwire test in
//! `tests/cmux_presentation.rs` scans those layers' sources for the word
//! and fails the moment one of them learns it.
//!
//! # Only the documented surface (line 1893)
//!
//! Every cmux invocation goes through [`Subcommand`], whose variants are the
//! complete list of what this module may run: `ping`, `identify --json`,
//! `workspace create`, `workspace select`, and `send`. All five are named in
//! `cmux --help` and `cmux docs api`; none of them is the socket protocol, an
//! `rpc` call, or a JSON schema copied out of cmux's internals. The same
//! tripwire test checks that no other cmux verb appears in this file's
//! production code, so widening the surface is a deliberate, visible act.
//!
//! # Basic expose-and-focus, and why it stops there (lines 1892, 1895)
//!
//! "Basic expose-and-focus" is exactly three verbs: **open** a pane for a
//! session, **focus** it, and **send** a line to it when the door is not an
//! option. It does not lay panes out, split them, rename them, close them,
//! read their screens, watch their events, or drive cmux's browser. Richer
//! automation is deferred on purpose: line 1892 keeps cmux optional until
//! repeated use proves external-pane workflows essential, and line 1895 says
//! richer automation waits until the basic workflow has proved useful. The
//! evidence that would unlock it is usage, not a design — and until then the
//! allow-list above is the boundary.
//!
//! # Detection is presence *and* an answer (line 754)
//!
//! [`detect`] says cmux is available only when both halves hold: the
//! process is inside a cmux surface (the same `CMUX_SOCKET_PATH` evidence
//! [`super::Discovery`] already reports, corroborated by the surface and
//! workspace variables), *and* `cmux ping` answers. A variable left set in
//! a dead environment — a shell whose cmux has since quit, a copied
//! environment — reads as **absent**, because a backend that cannot answer
//! is not one Glasshouse may hand a session to.
//!
//! # Security
//!
//! - The pane's command names the project root and Glasshouse's own
//!   resolved directories and flags; no credential, token, or provider
//!   value is ever placed in a `cmux` argument. `CMUX_SOCKET_CAPABILITY` is
//!   never read.
//! - A stored presentation reference is an opaque string until it is about
//!   to be handed back to cmux, at which point [`PaneRef::parse`] admits only
//!   the `workspace:N` / `surface:N` shape. A row carrying anything else is
//!   refused by name rather than passed through.
//! - The pane's command is quoted for the login shell cmux runs it under
//!   ([`shell_command`]), so a project root containing a space or a quote
//!   cannot become two words or an injected command.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::IntegrationId;
use crate::platform::exec::{self, ResolveError, ResolvedExecutable};
use crate::session::{SessionId, SessionStore, SessionStoreError};

/// The presentation backends a launch may name with `--presentation`.
///
/// One variant today. The enum exists so the flag has a vocabulary rather
/// than a free string, and so a second backend would be a new variant here
/// rather than a second code path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Cmux,
}

impl Backend {
    /// Every backend, for error messages that list what is known.
    pub const ALL: [Backend; 1] = [Backend::Cmux];

    /// The word `--presentation` takes.
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Cmux => "cmux",
        }
    }

    /// Parse the word a person typed. Unknown words are refused with the
    /// list of known ones, never defaulted.
    pub fn parse(word: &str) -> Result<Backend, UnknownBackend> {
        Backend::ALL
            .into_iter()
            .find(|backend| backend.as_str() == word)
            .ok_or_else(|| UnknownBackend(word.to_owned()))
    }
}

/// `--presentation` named something this build does not know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownBackend(pub String);

impl fmt::Display for UnknownBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let known = Backend::ALL
            .iter()
            .map(|backend| backend.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        write!(
            f,
            "unknown presentation backend `{}`; known backends: {known}",
            self.0
        )
    }
}

impl std::error::Error for UnknownBackend {}

/// A validated reference to a cmux workspace or surface — the only shape
/// Glasshouse will hand back to cmux.
///
/// cmux's own short refs are `workspace:<n>` and `surface:<n>`; the
/// workspace form is what `workspace create` prints and what `workspace
/// select` accepts, so it is the one Glasshouse records. The surface form is
/// accepted for a caller that supplied one by hand, and `send` can target
/// it; `workspace select` cannot, and says so through cmux's own error.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PaneRef(String);

impl PaneRef {
    /// Admit `workspace:<digits>` or `surface:<digits>`, nothing else.
    ///
    /// Strict on purpose: this string ends up as a `cmux` argument, and the
    /// validation is what lets a stored value be treated as opaque
    /// everywhere else — see the module doc's security section.
    pub fn parse(text: &str) -> Result<PaneRef, InvalidPaneRef> {
        let text = text.trim();
        let number = text
            .strip_prefix(WORKSPACE_PREFIX)
            .or_else(|| text.strip_prefix(SURFACE_PREFIX));
        let number_ok = number.is_some_and(|number| {
            !number.is_empty()
                && number.len() <= 12
                && number.bytes().all(|byte| byte.is_ascii_digit())
        });
        if number_ok {
            Ok(PaneRef(text.to_owned()))
        } else {
            Err(InvalidPaneRef(text.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this names a workspace, which is what focusing needs.
    pub fn is_workspace(&self) -> bool {
        self.0.starts_with(WORKSPACE_PREFIX)
    }
}

/// The two reference shapes cmux prints and accepts.
const WORKSPACE_PREFIX: &str = "workspace:";
const SURFACE_PREFIX: &str = "surface:";

impl fmt::Display for PaneRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A presentation reference that is not `workspace:N` or `surface:N`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidPaneRef(pub String);

impl fmt::Display for InvalidPaneRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` is not a cmux reference; expected `workspace:<n>` or `surface:<n>`",
            self.0
        )
    }
}

impl std::error::Error for InvalidPaneRef {}

/// What `--presentation-ref` may say: a literal reference, or `caller`,
/// meaning *ask cmux which pane this process is running in*.
///
/// `caller` is what the outer process passes when it opens a pane, because
/// the pane's reference is not known until the pane exists — and by then
/// its command has already been given. The process inside the pane resolves
/// it through `cmux identify`, the documented way for a caller to learn its
/// own surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneRefRequest {
    Caller,
    Given(PaneRef),
}

impl PaneRefRequest {
    pub fn parse(text: &str) -> Result<PaneRefRequest, InvalidPaneRef> {
        if text.trim() == "caller" {
            Ok(PaneRefRequest::Caller)
        } else {
            PaneRef::parse(text).map(PaneRefRequest::Given)
        }
    }
}

/// The complete list of cmux subcommands this module may invoke.
///
/// Adding a variant here is the *only* way to widen the surface, and the
/// tripwire test in `tests/cmux_presentation.rs` checks that no other cmux
/// verb is named anywhere in this file's production code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Subcommand {
    /// `cmux ping` — does the control socket answer?
    Ping,
    /// `cmux identify --json` — which pane is the caller in?
    Identify,
    /// `cmux workspace create` — open a pane in a directory, running a
    /// command.
    WorkspaceCreate,
    /// `cmux workspace select` — bring a workspace to the front.
    WorkspaceSelect,
    /// `cmux send` — type text into a pane.
    Send,
}

impl Subcommand {
    pub const ALL: [Subcommand; 5] = [
        Subcommand::Ping,
        Subcommand::Identify,
        Subcommand::WorkspaceCreate,
        Subcommand::WorkspaceSelect,
        Subcommand::Send,
    ];

    /// The words placed on the command line, before any flags.
    pub fn words(self) -> &'static [&'static str] {
        match self {
            Subcommand::Ping => &["ping"],
            Subcommand::Identify => &["identify", "--json"],
            Subcommand::WorkspaceCreate => &["workspace", "create"],
            Subcommand::WorkspaceSelect => &["workspace", "select"],
            Subcommand::Send => &["send"],
        }
    }
}

/// Something cmux could not do, in cmux's words where it had any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CmuxError {
    /// The `cmux` executable could not be started at all.
    Spawn {
        subcommand: Subcommand,
        message: String,
    },
    /// cmux ran and refused; `output` is its own stdout and stderr,
    /// trimmed.
    Refused {
        subcommand: Subcommand,
        output: String,
    },
    /// cmux answered, but not with anything this module could read.
    Unreadable {
        subcommand: Subcommand,
        output: String,
    },
    /// A stored or given reference cannot be handed to cmux.
    InvalidRef(InvalidPaneRef),
    /// The reference names a surface, and the operation needs a workspace.
    NotAWorkspace(PaneRef),
    /// The payload for `send_line` contains a backslash, which cmux's
    /// escape language cannot carry as data. The message deliberately does
    /// not echo the payload: it may be the very injection this refusal
    /// exists to stop.
    PayloadHasBackslash,
}

impl fmt::Display for CmuxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let words = |subcommand: &Subcommand| subcommand.words().join(" ");
        match self {
            CmuxError::Spawn {
                subcommand,
                message,
            } => write!(f, "could not run `cmux {}`: {message}", words(subcommand)),
            CmuxError::Refused { subcommand, output } => {
                write!(f, "`cmux {}` refused: {output}", words(subcommand))
            }
            CmuxError::Unreadable { subcommand, output } => write!(
                f,
                "`cmux {}` answered something unexpected: {output}",
                words(subcommand)
            ),
            CmuxError::InvalidRef(err) => write!(f, "{err}"),
            CmuxError::NotAWorkspace(pane) => write!(
                f,
                "`{pane}` names a surface; focusing needs a workspace reference"
            ),
            CmuxError::PayloadHasBackslash => write!(
                f,
                "text for `cmux send` cannot contain a backslash: cmux has no \
                 escape-of-escape, so a backslash in the payload cannot be \
                 carried as data"
            ),
        }
    }
}

impl std::error::Error for CmuxError {}

impl From<InvalidPaneRef> for CmuxError {
    fn from(err: InvalidPaneRef) -> Self {
        CmuxError::InvalidRef(err)
    }
}

/// A workspace to open: where, what to run, and whether to switch to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWorkspace {
    /// The title cmux shows. Display only.
    pub name: String,
    /// The directory the pane starts in — the project root, and nothing else.
    pub cwd: PathBuf,
    /// The command the pane runs, already quoted for the shell — see
    /// [`shell_command`].
    pub command: String,
    /// Whether cmux should switch to the new workspace. A person who asked
    /// for a pane wants to see it; an orchestrator spawning a worker does
    /// not want its own view stolen.
    pub focus: bool,
}

/// The five things Glasshouse asks cmux to do, behind a seam so a test can
/// stand in for cmux without a socket, a window, or a process.
///
/// Every production caller goes through [`CmuxCli`]; every test caller goes
/// through a fake that records what it was asked. The trait is the whole of
/// the dependency: a cmux release that changed one of these commands would
/// be met here, in one file, and nowhere else (line 1894).
pub trait CmuxControl {
    /// `cmux ping`. `Ok` means the control socket answered.
    fn ping(&self) -> Result<(), CmuxError>;
    /// `cmux identify --json`, reduced to the caller's workspace reference.
    fn identify_caller(&self) -> Result<PaneRef, CmuxError>;
    /// `cmux workspace create …`, returning the reference cmux printed.
    fn create_workspace(&self, workspace: &NewWorkspace) -> Result<PaneRef, CmuxError>;
    /// `cmux workspace select <ref>`.
    fn select_workspace(&self, pane: &PaneRef) -> Result<(), CmuxError>;
    /// `cmux send … <text>`: one line, submitted with Enter.
    fn send_line(&self, pane: &PaneRef, text: &str) -> Result<(), CmuxError>;
}

/// The real thing: the `cmux` executable, invoked as a child process.
#[derive(Debug, Clone)]
pub struct CmuxCli {
    executable: PathBuf,
}

impl CmuxCli {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        CmuxCli {
            executable: executable.into(),
        }
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Run one allow-listed subcommand with its arguments and return its
    /// trimmed standard output.
    ///
    /// This is the single place in Glasshouse that starts `cmux`. Standard
    /// input is null so a cmux that ever decided to prompt cannot block a
    /// launch; both output streams are captured so a refusal can be quoted
    /// rather than paraphrased.
    fn run(&self, subcommand: Subcommand, args: &[&OsStr]) -> Result<String, CmuxError> {
        let output = Command::new(&self.executable)
            .args(subcommand.words())
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // The legacy-verb deprecation hint is not an answer; cmux
            // documents this variable as the way to keep it off stdout.
            .env("CMUX_QUIET", "1")
            .output()
            .map_err(|err| CmuxError::Spawn {
                subcommand,
                message: err.to_string(),
            })?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if output.status.success() {
            Ok(stdout)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            let combined = match (stdout.is_empty(), stderr.is_empty()) {
                (true, true) => format!("exit status {}", output.status),
                (false, true) => stdout,
                (true, false) => stderr,
                (false, false) => format!("{stdout}\n{stderr}"),
            };
            Err(CmuxError::Refused {
                subcommand,
                output: combined,
            })
        }
    }
}

/// Pull the first `workspace:<n>` (or `surface:<n>`) token out of a cmux
/// answer such as `OK workspace:347` or `OK surface:406 workspace:349`.
fn first_ref(output: &str, prefer: &str) -> Option<PaneRef> {
    let tokens: Vec<&str> = output.split_whitespace().collect();
    tokens
        .iter()
        .find(|token| token.starts_with(prefer))
        .or_else(|| tokens.iter().find(|token| PaneRef::parse(token).is_ok()))
        .and_then(|token| PaneRef::parse(token).ok())
}

/// Read `caller.workspace_ref` out of `cmux identify --json` without a JSON
/// schema: the one key is located by name and its quoted value taken. A
/// full parse would tie this module to the document's shape, which is more
/// of cmux's internals than line 1893 allows Glasshouse to depend on.
fn caller_workspace_ref(json: &str) -> Option<PaneRef> {
    let caller = json.find("\"caller\"")?;
    let rest = &json[caller..];
    let key = rest.find("\"workspace_ref\"")?;
    let after_key = &rest[key + "\"workspace_ref\"".len()..];
    let open = after_key.find('"')?;
    let value = &after_key[open + 1..];
    let close = value.find('"')?;
    PaneRef::parse(&value[..close]).ok()
}

/// The exact argument `cmux send -- …` is given for one line of text, or a
/// refusal if the text cannot be carried as data.
///
/// cmux reads its payload as an escape language — a literal `\r` in it is
/// Enter — so text Glasshouse carries as *data* must never let cmux see one.
/// Doubling backslashes (`\` → `\\`) looks like the standard escape for this,
/// but is not: measured against a live cmux, doubling does not prevent the
/// injection it is meant to stop. cmux advances through the payload one
/// character at a time; an unrecognized backslash is emitted literally
/// (confirmed: `A\\B` renders as `A\\B`, two backslashes, not one), but the
/// very next backslash is still checked on its own, so a doubled backslash
/// immediately followed by `r` — `\\r` — still ends in a recognized `\r` and
/// still submits, while also leaving a spurious extra backslash in the pane.
/// cmux has no escape-of-escape, so a backslash in the payload cannot be
/// carried as data at all: the only correct move is to refuse it, not to
/// transform it. `cli::ApiCommand::Send` promises the text "is not expanded,
/// interpreted, or given to a shell anywhere on its way to the session";
/// this refusal is what keeps that true for a payload doubling cannot save.
fn submitted_line(text: &str) -> Result<String, CmuxError> {
    if text.contains('\\') {
        return Err(CmuxError::PayloadHasBackslash);
    }
    let mut line = String::with_capacity(text.len() + 2);
    line.push_str(text);
    line.push_str("\\r");
    Ok(line)
}

impl CmuxControl for CmuxCli {
    fn ping(&self) -> Result<(), CmuxError> {
        self.run(Subcommand::Ping, &[]).map(|_| ())
    }

    fn identify_caller(&self) -> Result<PaneRef, CmuxError> {
        let output = self.run(Subcommand::Identify, &[])?;
        caller_workspace_ref(&output).ok_or(CmuxError::Unreadable {
            subcommand: Subcommand::Identify,
            output,
        })
    }

    fn create_workspace(&self, workspace: &NewWorkspace) -> Result<PaneRef, CmuxError> {
        let focus = if workspace.focus { "true" } else { "false" };
        let args: [&OsStr; 8] = [
            OsStr::new("--name"),
            OsStr::new(&workspace.name),
            OsStr::new("--cwd"),
            workspace.cwd.as_os_str(),
            OsStr::new("--command"),
            OsStr::new(&workspace.command),
            OsStr::new("--focus"),
            OsStr::new(focus),
        ];
        let output = self.run(Subcommand::WorkspaceCreate, &args)?;
        first_ref(&output, WORKSPACE_PREFIX).ok_or(CmuxError::Unreadable {
            subcommand: Subcommand::WorkspaceCreate,
            output,
        })
    }

    fn select_workspace(&self, pane: &PaneRef) -> Result<(), CmuxError> {
        if !pane.is_workspace() {
            return Err(CmuxError::NotAWorkspace(pane.clone()));
        }
        self.run(Subcommand::WorkspaceSelect, &[OsStr::new(pane.as_str())])
            .map(|_| ())
    }

    fn send_line(&self, pane: &PaneRef, text: &str) -> Result<(), CmuxError> {
        // cmux's documented escape: a literal `\r` in the text is Enter. It
        // is appended rather than a real carriage return because the text
        // travels as a command-line argument, and this is the form the
        // command documents for submitting a line. `submitted_line` refuses
        // a payload containing a backslash rather than trying to escape it —
        // see its doc comment for the measured reason doubling cannot work.
        let line = submitted_line(text)?;
        let target = if pane.is_workspace() {
            "--workspace"
        } else {
            "--surface"
        };
        self.run(
            Subcommand::Send,
            &[
                OsStr::new(target),
                OsStr::new(pane.as_str()),
                OsStr::new("--"),
                OsStr::new(&line),
            ],
        )
        .map(|_| ())
    }
}

/// Why cmux is not available right now. Each is a reason a person can act
/// on, and every one of them means *the session runs embedded*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Absence {
    /// No cmux control environment: `CMUX_SOCKET_PATH` is unset or empty.
    NotInsideCmux,
    /// Inside cmux by the environment's account, but no `cmux` executable
    /// resolves on `PATH`.
    NoExecutable(String),
    /// The executable is there and the environment says cmux, but `cmux
    /// ping` did not answer — a stale environment, or a cmux that quit.
    NotAnswering(String),
}

impl fmt::Display for Absence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Absence::NotInsideCmux => f.write_str("not running inside a cmux surface"),
            Absence::NoExecutable(detail) => {
                write!(f, "no usable `cmux` executable on PATH: {detail}")
            }
            Absence::NotAnswering(detail) => write!(f, "cmux did not answer a ping: {detail}"),
        }
    }
}

/// The answer to *can cmux present a session right now?*
#[derive(Debug)]
pub enum Availability<C = CmuxCli> {
    Available(C),
    Absent(Absence),
}

impl<C> Availability<C> {
    pub fn as_available(&self) -> Option<&C> {
        match self {
            Availability::Available(control) => Some(control),
            Availability::Absent(_) => None,
        }
    }
}

/// Is cmux available to this process? Presence **and** an answer — see the
/// module doc.
///
/// Reads the real environment and the real `PATH`, and pings the real
/// executable. Never fails: every way of not being available is an
/// [`Absence`] with its reason.
pub fn detect() -> Availability {
    detect_with(
        |name| std::env::var(name).ok(),
        exec::resolve,
        |cli: &CmuxCli| cli.ping(),
    )
}

/// Core of [`detect`], with the environment, the executable resolver and
/// the ping injected so tests can walk every branch without touching the
/// process environment or spawning anything.
pub fn detect_with(
    env: impl Fn(&str) -> Option<String>,
    resolve: impl Fn(&str) -> Result<ResolvedExecutable, ResolveError>,
    ping: impl Fn(&CmuxCli) -> Result<(), CmuxError>,
) -> Availability {
    // The same evidence `Discovery` reports for cmux, produced by the same
    // function, so the doctor's "configured" and this module's "inside cmux"
    // cannot come to disagree about what counts as presence.
    if super::presence_without_executable_with(IntegrationId::Cmux, &env).is_empty() {
        return Availability::Absent(Absence::NotInsideCmux);
    }

    let mut tried = Vec::new();
    let mut executable = None;
    for candidate in IntegrationId::Cmux.executable_candidates() {
        match resolve(candidate) {
            Ok(resolved) => {
                executable = Some(resolved);
                break;
            }
            Err(err) => tried.push(format!("{candidate}: {err}")),
        }
    }
    let Some(executable) = executable else {
        return Availability::Absent(Absence::NoExecutable(tried.join("; ")));
    };

    let cli = CmuxCli::new(executable.path());
    match ping(&cli) {
        Ok(()) => Availability::Available(cli),
        Err(err) => Availability::Absent(Absence::NotAnswering(err.to_string())),
    }
}

/// Quote one word for a POSIX shell: single quotes, with any embedded
/// single quote spelled `'\''`.
///
/// cmux runs a workspace's `--command` through the user's login shell, so
/// the command Glasshouse hands it is *shell text*, not an argument vector.
/// Single-quoting is the one form every Bourne-family shell — and fish —
/// reads back as exactly the bytes given, which is what keeps a project root
/// containing a space, a `$`, or a quote from becoming something else.
pub fn shell_quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./:=+@%,".contains(&b))
    {
        return word.to_owned();
    }
    let mut quoted = String::with_capacity(word.len() + 2);
    quoted.push('\'');
    for ch in word.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

/// An argument vector as one line of shell text — see [`shell_quote`].
///
/// A non-UTF-8 argument is spelled with its lossy form: a pane command is
/// text the shell parses, and there is no byte-exact way to hand one a path
/// that is not text. Such a path would already have failed further up, when
/// the project root was resolved.
pub fn shell_command<I, S>(argv: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    argv.into_iter()
        .map(|arg| shell_quote(&arg.as_ref().to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The command a pane runs: this Glasshouse, its resolved directories and
/// project, and the launch it was asked for — with `--presentation-ref
/// caller` so the process inside the pane records where it is.
///
/// `global` are the process-wide flags (`--data-dir`, `--config-dir`,
/// `--scope`, logging) and `launch` is the `launch …` tail, both supplied by
/// the caller that knows them. This function only assembles and quotes.
pub fn pane_command(executable: &Path, global: &[OsString], launch: &[OsString]) -> String {
    let mut argv: Vec<OsString> = Vec::with_capacity(1 + global.len() + launch.len());
    argv.push(executable.as_os_str().to_owned());
    argv.extend(global.iter().cloned());
    argv.extend(launch.iter().cloned());
    shell_command(argv)
}

/// How long a caller that opened a pane waits for the session inside it to
/// record itself before answering without an id. The process in the pane
/// records its session before it starts the harness, so this is a bound on
/// process start-up, not on the harness.
pub const RECORD_WAIT: Duration = Duration::from_secs(5);

/// Which pane, if any, every recorded session named *before* a pane is
/// opened — so the session that names the new pane afterwards can be told
/// from one a reused reference might still name, whether it was minted in
/// the pane or was an existing session the pane's launch continued.
pub fn recorded_panes(
    store: &SessionStore<'_>,
) -> Result<HashMap<SessionId, Option<String>>, SessionStoreError> {
    Ok(store
        .list()?
        .into_iter()
        .map(|record| (record.id, record.presentation_ref))
        .collect())
}

/// Wait, bounded by `timeout`, for a session that names `pane` and did not
/// name it in `before`.
///
/// The process inside the pane records its session before it starts the
/// harness — minting one, or moving the session it continues into the pane
/// — so under normal conditions this returns within a fraction of a
/// second. `None` means the pane exists but nothing has recorded itself in
/// it yet — a fact the caller reports rather than an error, because the
/// pane is real either way and `glasshouse sessions` will show the session
/// once it appears.
pub fn await_session_at(
    store: &SessionStore<'_>,
    pane: &PaneRef,
    before: &HashMap<SessionId, Option<String>>,
    timeout: Duration,
) -> Result<Option<SessionId>, SessionStoreError> {
    const POLL: Duration = Duration::from_millis(50);
    let deadline = Instant::now() + timeout;
    loop {
        let mut fresh: Vec<_> = store
            .list()?
            .into_iter()
            .filter(|record| {
                record.presentation_ref.as_deref() == Some(pane.as_str())
                    && before
                        .get(&record.id)
                        .is_none_or(|old| old.as_deref() != Some(pane.as_str()))
            })
            .collect();
        // Newest first, in the unlikely event two appeared.
        fresh.sort_by_key(|record| std::cmp::Reverse(record.created_at));
        if let Some(record) = fresh.into_iter().next() {
            return Ok(Some(record.id));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(POLL);
    }
}

/// Resolve what `--presentation-ref` asked for into a concrete reference.
///
/// A literal reference needs nothing from cmux — it is metadata the caller
/// asserted, and recording it is not an act on cmux. `caller` needs cmux to
/// answer, and when it cannot, the launch has no pane to name: the caller
/// reports the reason and runs embedded, which is line 755's rule applied
/// inside the pane.
pub fn resolve_pane_ref(
    request: &PaneRefRequest,
    availability: &Availability<impl CmuxControl>,
) -> Result<PaneRef, String> {
    match request {
        PaneRefRequest::Given(pane) => Ok(pane.clone()),
        PaneRefRequest::Caller => match availability {
            Availability::Available(control) => control
                .identify_caller()
                .map_err(|err| format!("cmux could not identify this pane: {err}")),
            Availability::Absent(reason) => Err(format!("cmux is not available ({reason})")),
        },
    }
}

/// Bring a session's pane to the front — line 759. Exactly one `workspace
/// select`, and the stored reference is validated on its way out.
pub fn focus(stored_ref: &str, control: &impl CmuxControl) -> Result<PaneRef, CmuxError> {
    let pane = PaneRef::parse(stored_ref)?;
    control.select_workspace(&pane)?;
    Ok(pane)
}

/// Type one line into a session's pane — line 758's fallback, for a session
/// Glasshouse's own door cannot reach. The stored reference is validated on
/// its way out, exactly as [`focus`] does.
pub fn send_line(
    stored_ref: &str,
    text: &str,
    control: &impl CmuxControl,
) -> Result<PaneRef, CmuxError> {
    let pane = PaneRef::parse(stored_ref)?;
    control.send_line(&pane, text)?;
    Ok(pane)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A cmux that answers what it is told to and remembers what it was
    /// asked, so a test can assert on the exact sequence of calls.
    #[derive(Default)]
    struct FakeCmux {
        calls: RefCell<Vec<String>>,
        ping_ok: bool,
        caller: Option<PaneRef>,
        created: Option<PaneRef>,
    }

    impl FakeCmux {
        fn answering(mut self) -> Self {
            self.ping_ok = true;
            self
        }

        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl CmuxControl for FakeCmux {
        fn ping(&self) -> Result<(), CmuxError> {
            self.calls.borrow_mut().push("ping".to_owned());
            if self.ping_ok {
                Ok(())
            } else {
                Err(CmuxError::Refused {
                    subcommand: Subcommand::Ping,
                    output: "no socket".to_owned(),
                })
            }
        }

        fn identify_caller(&self) -> Result<PaneRef, CmuxError> {
            self.calls.borrow_mut().push("identify --json".to_owned());
            self.caller.clone().ok_or(CmuxError::Unreadable {
                subcommand: Subcommand::Identify,
                output: "{}".to_owned(),
            })
        }

        fn create_workspace(&self, workspace: &NewWorkspace) -> Result<PaneRef, CmuxError> {
            self.calls.borrow_mut().push(format!(
                "workspace create --name {} --cwd {} --command {} --focus {}",
                workspace.name,
                workspace.cwd.display(),
                workspace.command,
                workspace.focus
            ));
            self.created.clone().ok_or(CmuxError::Refused {
                subcommand: Subcommand::WorkspaceCreate,
                output: "refused".to_owned(),
            })
        }

        fn select_workspace(&self, pane: &PaneRef) -> Result<(), CmuxError> {
            self.calls
                .borrow_mut()
                .push(format!("workspace select {pane}"));
            Ok(())
        }

        fn send_line(&self, pane: &PaneRef, text: &str) -> Result<(), CmuxError> {
            self.calls.borrow_mut().push(format!("send {pane} {text}"));
            Ok(())
        }
    }

    fn fake_executable(name: &str) -> Result<ResolvedExecutable, ResolveError> {
        exec::resolve_explicit(&std::env::current_exe().unwrap()).map_err(|_| {
            ResolveError::NotFound {
                name: name.to_owned(),
            }
        })
    }

    fn not_found(name: &str) -> Result<ResolvedExecutable, ResolveError> {
        Err(ResolveError::NotFound {
            name: name.to_owned(),
        })
    }

    // --- line 754: detection is presence AND an answer ----------------------

    #[test]
    fn outside_cmux_is_absent_before_anything_is_resolved_or_pinged() {
        let resolved = RefCell::new(0);
        let pinged = RefCell::new(0);
        let availability = detect_with(
            |_| None,
            |name| {
                *resolved.borrow_mut() += 1;
                fake_executable(name)
            },
            |_| {
                *pinged.borrow_mut() += 1;
                Ok(())
            },
        );
        assert!(matches!(
            availability,
            Availability::Absent(Absence::NotInsideCmux)
        ));
        assert_eq!(*resolved.borrow(), 0, "nothing is resolved outside cmux");
        assert_eq!(*pinged.borrow(), 0, "nothing is pinged outside cmux");
    }

    #[test]
    fn an_empty_socket_path_is_outside_cmux() {
        let availability = detect_with(
            |name| (name == "CMUX_SOCKET_PATH").then(String::new),
            fake_executable,
            |_| Ok(()),
        );
        assert!(matches!(
            availability,
            Availability::Absent(Absence::NotInsideCmux)
        ));
    }

    #[test]
    fn inside_cmux_without_an_executable_is_absent_and_names_what_was_tried() {
        let availability = detect_with(
            |name| (name == "CMUX_SOCKET_PATH").then(|| "/tmp/x.sock".to_owned()),
            not_found,
            |_| Ok(()),
        );
        match availability {
            Availability::Absent(Absence::NoExecutable(detail)) => {
                assert!(detail.contains("cmux"), "{detail}");
            }
            other => panic!("expected NoExecutable, got {other:?}"),
        }
    }

    /// The decision this module exists for: a variable left set in a dead
    /// environment reads as absent. Mutation target — a detection that
    /// ignored the ping would pass every other test here.
    #[test]
    fn a_set_variable_whose_cmux_does_not_answer_reads_as_absent() {
        let availability = detect_with(
            |name| (name == "CMUX_SOCKET_PATH").then(|| "/tmp/x.sock".to_owned()),
            fake_executable,
            |_| {
                Err(CmuxError::Refused {
                    subcommand: Subcommand::Ping,
                    output: "connection refused".to_owned(),
                })
            },
        );
        match availability {
            Availability::Absent(Absence::NotAnswering(detail)) => {
                assert!(detail.contains("connection refused"), "{detail}");
            }
            other => panic!("expected NotAnswering, got {other:?}"),
        }
    }

    #[test]
    fn presence_with_an_executable_and_an_answer_is_available() {
        let availability = detect_with(
            |name| (name == "CMUX_SOCKET_PATH").then(|| "/tmp/x.sock".to_owned()),
            fake_executable,
            |_| Ok(()),
        );
        assert!(availability.as_available().is_some());
    }

    #[test]
    fn every_absence_reads_as_a_reason_a_person_can_act_on() {
        for absence in [
            Absence::NotInsideCmux,
            Absence::NoExecutable("cmux: not found".to_owned()),
            Absence::NotAnswering("timed out".to_owned()),
        ] {
            let text = absence.to_string();
            assert!(!text.is_empty());
            assert!(!text.contains("Absence"), "no Debug leakage: {text}");
        }
    }

    // --- refs: opaque until handed back, then strict -----------------------

    #[test]
    fn only_workspace_and_surface_refs_are_admitted() {
        for ok in ["workspace:1", "surface:406", " workspace:349 "] {
            let pane = PaneRef::parse(ok).unwrap_or_else(|err| panic!("{ok}: {err}"));
            assert_eq!(pane.as_str(), ok.trim());
        }
        for bad in [
            "",
            "workspace",
            "workspace:",
            "workspace:abc",
            "pane:3",
            "window:1",
            "workspace:1; rm -rf /",
            "surface:1 --window window:2",
            "WORKSPACE:1",
            "workspace:1234567890123",
        ] {
            assert!(PaneRef::parse(bad).is_err(), "`{bad}` must be refused");
        }
    }

    #[test]
    fn caller_is_a_request_and_a_literal_ref_is_the_other() {
        assert_eq!(PaneRefRequest::parse("caller"), Ok(PaneRefRequest::Caller));
        assert_eq!(
            PaneRefRequest::parse("workspace:7"),
            Ok(PaneRefRequest::Given(
                PaneRef::parse("workspace:7").unwrap()
            ))
        );
        assert!(PaneRefRequest::parse("mine").is_err());
    }

    #[test]
    fn focus_validates_the_stored_ref_and_issues_exactly_one_select() {
        let cmux = FakeCmux::default().answering();
        let pane = focus("workspace:349", &cmux).unwrap();
        assert_eq!(pane.as_str(), "workspace:349");
        assert_eq!(cmux.calls(), vec!["workspace select workspace:349"]);

        let cmux = FakeCmux::default().answering();
        let err = focus("workspace:1; cmux workspace close workspace:2", &cmux).unwrap_err();
        assert!(matches!(err, CmuxError::InvalidRef(_)), "{err}");
        assert!(cmux.calls().is_empty(), "an invalid ref reaches cmux never");
    }

    #[test]
    fn a_backslash_in_the_payload_is_refused_not_escaped() {
        // No backslash: unchanged pass-through, `\r` appended once.
        assert_eq!(submitted_line("hello there").unwrap(), "hello there\\r");

        // A literal `\r` — the finding's own repro — is refused rather than
        // doubled: doubling does not stop the submission (measured against a
        // live cmux, see `submitted_line`'s doc comment), so refusal is the
        // only correct response.
        assert!(matches!(
            submitted_line("x\\rrm -rf ~"),
            Err(CmuxError::PayloadHasBackslash)
        ));

        // Any backslash at all is refused, not only one that spells `\r` —
        // a Windows path is refused too, since doubling could not carry it
        // as data either.
        assert!(matches!(
            submitted_line(r"C:\reports\x"),
            Err(CmuxError::PayloadHasBackslash)
        ));

        // The refusal's message does not echo the payload.
        let message = submitted_line("x\\rrm -rf ~").unwrap_err().to_string();
        assert!(!message.contains("rm -rf"), "{message}");
    }

    #[test]
    fn send_line_validates_the_stored_ref_and_types_one_line() {
        let cmux = FakeCmux::default().answering();
        send_line("workspace:5", "hello there", &cmux).unwrap();
        assert_eq!(cmux.calls(), vec!["send workspace:5 hello there"]);

        let cmux = FakeCmux::default().answering();
        assert!(send_line("nonsense", "x", &cmux).is_err());
        assert!(cmux.calls().is_empty());
    }

    #[test]
    fn the_real_cli_refuses_to_select_by_surface_before_asking_cmux() {
        // A surface ref is a valid *reference* but not a workspace; the
        // wrapper says so itself rather than spending a process on cmux's
        // "Workspace not found". `CmuxCli` over a path that does not exist:
        // if this ever reached `run`, it would fail as `Spawn`, not as
        // `NotAWorkspace`.
        let cli = CmuxCli::new("/nonexistent/cmux");
        let err = cli
            .select_workspace(&PaneRef::parse("surface:9").unwrap())
            .unwrap_err();
        assert!(matches!(err, CmuxError::NotAWorkspace(_)), "{err}");
    }

    #[test]
    fn resolving_caller_goes_through_identify_and_a_literal_does_not() {
        let cmux = FakeCmux {
            caller: Some(PaneRef::parse("workspace:349").unwrap()),
            ..FakeCmux::default()
        }
        .answering();
        let availability: Availability<FakeCmux> = Availability::Available(cmux);
        let pane = resolve_pane_ref(&PaneRefRequest::Caller, &availability).unwrap();
        assert_eq!(pane.as_str(), "workspace:349");
        assert_eq!(
            availability.as_available().unwrap().calls(),
            vec!["identify --json"]
        );

        let literal = PaneRefRequest::Given(PaneRef::parse("surface:2").unwrap());
        let pane = resolve_pane_ref(&literal, &availability).unwrap();
        assert_eq!(pane.as_str(), "surface:2");
        assert_eq!(
            availability.as_available().unwrap().calls().len(),
            1,
            "a literal ref asks cmux nothing"
        );

        let absent: Availability<FakeCmux> = Availability::Absent(Absence::NotInsideCmux);
        let err = resolve_pane_ref(&PaneRefRequest::Caller, &absent).unwrap_err();
        assert!(err.contains("not running inside a cmux surface"), "{err}");
    }

    // --- parsing cmux's answers --------------------------------------------

    #[test]
    fn a_create_answer_yields_its_workspace_ref() {
        assert_eq!(
            first_ref("OK workspace:347", "workspace:")
                .unwrap()
                .as_str(),
            "workspace:347"
        );
        assert_eq!(
            first_ref("OK surface:406 workspace:349", "workspace:")
                .unwrap()
                .as_str(),
            "workspace:349"
        );
        assert!(first_ref("OK", "workspace:").is_none());
    }

    #[test]
    fn identify_yields_the_callers_workspace_not_the_focused_one() {
        let json = r#"{
  "caller" : {
    "surface_ref" : "surface:406",
    "workspace_ref" : "workspace:349"
  },
  "focused" : {
    "surface_ref" : "surface:360",
    "workspace_ref" : "workspace:304"
  }
}"#;
        assert_eq!(
            caller_workspace_ref(json).unwrap().as_str(),
            "workspace:349"
        );
        assert!(caller_workspace_ref("{}").is_none());
        assert!(caller_workspace_ref(r#"{"caller": {"workspace_ref": "bogus"}}"#).is_none());
    }

    // --- the pane command is shell text, quoted ----------------------------

    #[test]
    fn shell_quoting_keeps_a_word_a_word() {
        assert_eq!(shell_quote("plain-word.1"), "plain-word.1");
        assert_eq!(
            shell_quote("/Users/me/my project"),
            "'/Users/me/my project'"
        );
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("$HOME"), "'$HOME'");
        assert_eq!(shell_quote("a;b"), "'a;b'");
    }

    #[test]
    fn the_pane_command_names_the_executable_then_globals_then_the_launch() {
        let command = pane_command(
            Path::new("/opt/glass house/bin/glasshouse"),
            &[OsString::from("--data-dir"), OsString::from("/tmp/d")],
            &[
                OsString::from("launch"),
                OsString::from("claude-code"),
                OsString::from("--presentation-ref"),
                OsString::from("caller"),
            ],
        );
        assert_eq!(
            command,
            "'/opt/glass house/bin/glasshouse' --data-dir /tmp/d launch claude-code \
             --presentation-ref caller"
        );
    }

    #[test]
    fn every_backend_parses_from_its_own_word_and_nothing_else_does() {
        for backend in Backend::ALL {
            assert_eq!(Backend::parse(backend.as_str()), Ok(backend));
        }
        let err = Backend::parse("tmux").unwrap_err();
        assert!(err.to_string().contains("cmux"), "{err}");
    }

    #[test]
    fn every_subcommand_is_spelled_the_way_cmux_documents_it() {
        let spelled: Vec<String> = Subcommand::ALL
            .iter()
            .map(|subcommand| subcommand.words().join(" "))
            .collect();
        assert_eq!(
            spelled,
            [
                "ping",
                "identify --json",
                "workspace create",
                "workspace select",
                "send"
            ]
        );
    }
}
