//! One tool call, confined — map line 2455's first caller and map line
//! 2463's per-call half.
//!
//! The invariant: **there is no path through this module that spawns a child
//! outside the sandbox.** Confinement is installed on the `Command` before
//! `output()` is ever reached, and a platform that cannot install one
//! returns a refusal instead of a process — so "unconfined" is not a
//! degraded mode here, it is a refusal like any other.
//!
//! The second invariant is `sandbox-grants.md` §1.4: **a refusal is a
//! value.** Every refusal below is a returned [`PermissionDenied`]; nothing
//! in this module reads from a terminal, asks a question, retries, or ends a
//! turn.
//!
//! The third is the 61D exec-roots ruling
//! (`.agent-runtime/pane/ruling-61d-exec-roots.md`): a tool's own resolved
//! executable is a derived input, so the child is spawned on the **resolved**
//! binary and never on the bare name. The platform applier's executable roots
//! are the fallback for a name that cannot be resolved, and [`ExecGrant`]
//! records when that happened so a caller sees it without reading a log.
//!
//! The fourth is cancellation, and it is deliberately the *weakest* thing
//! that works: a [`CancellationToken`] the caller holds, checked once
//! immediately before the spawn and then at a bounded interval while the
//! child runs. It sits **below** the confinement rather than beside it, so
//! the first invariant is untouched — the only expression that starts a
//! child is still one that a `?` on [`confine`] has already passed.
//!
//! **Nothing model-authored runs here.** The only programs this module can
//! spawn are the four in [`registry::ALL`], each resolved from a name fixed
//! at compile time; there is no argument, no path and no branch through
//! which assistant text selects or becomes a program.

use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{Value, json};

use crate::contract::SessionId;
use crate::glasshouse::{self, Glasshouse};
use crate::sandbox::profile::{Access, PermissionDenied, Profile};
use crate::tools::registry::{self, ArgKind, Argv, Tool};

/// How much of a tool's output reaches the hook payload.
///
/// `runtime-contract.md` §3 caps a *handle preview* at 256 tokens, and that
/// cap belongs to the runtime that renders handles — it does not exist yet.
/// This is the separate, cruder bound on what one hook delivery may carry,
/// stated in bytes because that is what a truncation can actually be
/// performed on.
const PREVIEW_BYTES: usize = 2048;

/// How often a running call asks whether it has been cancelled.
///
/// The bound is on *latency*, not on the child: 20 ms is far below the two
/// seconds the acceptance test allows and far above anything that would make
/// the poll itself measurable next to a process that is doing work.
const CANCEL_POLL: Duration = Duration::from_millis(20);

/// The cancellation facility, and it is the whole of it: a flag whoever
/// started the call may set from another thread.
///
/// The invariant is that **setting it can never widen anything and never
/// starts anything** — every path that observes it either returns before a
/// child exists or kills one that does. It is deliberately not a channel,
/// not a signal handler and not an async runtime: a call is cancellable
/// between calls (the holder checks [`is_cancelled`](Self::is_cancelled)
/// itself) and during one ([`run_cancellable`] checks it), and nothing else
/// is needed for `runtime-contract.md` §5 to render the result, because a
/// cancelled call lands in the shape a refusal already has.
///
/// Cloning is cheap and every clone names the same flag, which is what lets
/// the holder keep one while the call borrows another.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// A fresh, un-cancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancels every call holding this token or a clone of it. Idempotent,
    /// and there is no way back: a token is one call's decision, not a
    /// reusable switch.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// The arguments of one call, by declared name.
///
/// A `BTreeMap<String, String>`: every value is one argv element or one
/// command line, and nothing here parses, splits or expands it. Structured
/// argument types are `runtime-contract.md` §7's "argument types" question
/// and they arrive with the runtime that has something structured to pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Args(BTreeMap<String, String>);

impl Args {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder form, so a call site reads like the contract's own
    /// `read({ path: … })`.
    pub fn with(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.0.insert(name.into(), value.into());
        self
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    /// The call's arguments as the hook protocol's `tool_input`.
    fn as_json(&self) -> Value {
        Value::Object(
            self.0
                .iter()
                .map(|(name, value)| (name.clone(), Value::String(value.clone())))
                .collect(),
        )
    }
}

/// Which binary the child was exec'd on, and whether that was decided here
/// or left to the platform's executable roots.
///
/// The 61D ruling in one type: `binary` is the resolved path pane hands to
/// `execvp`, and `fell_back_to_roots` is `true` exactly when the name could
/// not be resolved and the applier's directory roots are what bound the exec
/// instead. The fallback is logged when it happens **and** recorded here,
/// because a log line is not observable to the caller that has to decide
/// what to say about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecGrant {
    pub binary: PathBuf,
    pub fell_back_to_roots: bool,
}

/// Environment variables a confined child never sees.
///
/// The invariant: **a credential is withheld by the shape of its name, and
/// the session says how many.** Two rules and no cleverness — the three
/// variables pane itself reads, and any name that ends in a credential word.
/// A name-shaped rule is predictable in a way an entropy test on values is
/// not, and predictable is what a person needs when a build fails for it.
///
/// The cost is real and stated rather than hidden: a project whose build
/// genuinely wants `GITHUB_TOKEN` cannot get it, because `bg::run` refuses
/// `env` too. That is the safer default to be wrong in.
pub fn is_credential_variable(name: &str) -> bool {
    /// Names with no credential word in them that are still pane's own.
    const PANE_OWN: [&str; 1] = ["ANTHROPIC_BASE_URL"];
    /// A whole underscore-separated segment that makes a name a credential.
    const CREDENTIAL_WORDS: [&str; 8] = [
        "KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "CREDENTIALS",
        "CREDS",
    ];
    let upper = name.to_ascii_uppercase();
    if PANE_OWN.contains(&upper.as_str()) {
        return true;
    }
    // **Whole segments, not substrings.** `AWS_SECRET_ACCESS_KEY` has to
    // match on `SECRET` and `KEY` wherever they sit, so a suffix rule is too
    // narrow; a substring rule is too wide and takes `TOKENIZER` with it.
    upper
        .split('_')
        .any(|segment| CREDENTIAL_WORDS.contains(&segment))
}

/// Which mechanism confined the child. There is no `Unconfined` variant, and
/// that is the module's first invariant expressed as a type: a platform with
/// nothing to install returns [`PermissionDenied`] instead of a value of this
/// type.
/// The in-process state is **not** a hole in the invariant above: a tool
/// declared [`Argv::InProcess`] never becomes a child, so there is no process
/// to confine. Its one argument that touches the filesystem went through
/// `Profile::check` before anything happened, which is the same gate the
/// spawning tools' paths pass and the only one that ever enforced a path
/// rule — the OS layer is directory-granular and never saw them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confinement {
    /// macOS seatbelt, entered between `fork` and `exec`.
    Seatbelt,
    /// Linux Landlock, installed on the forked child before `exec`.
    Landlock,
    /// No child was created. The call ran inside pane, and its path was
    /// checked by `Profile::check` before it did.
    InProcess,
}

impl Confinement {
    pub fn as_str(self) -> &'static str {
        match self {
            Confinement::Seatbelt => "seatbelt",
            Confinement::Landlock => "landlock",
            Confinement::InProcess => "in-process (no child; the path was checked)",
        }
    }
}

/// What one call returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    /// Modification time observed on the admitted read path, after reading.
    pub modified: Option<String>,
    pub tool: String,
    pub stdout: String,
    pub stderr: String,
    /// `None` for a child killed by a signal, which is not an exit status.
    pub exit_code: Option<i32>,
    pub grant: ExecGrant,
    pub confinement: Confinement,
}

impl ToolResult {
    /// The observed output, capped, as the hooks carry it.
    pub fn preview(&self) -> String {
        truncate(&self.stdout, PREVIEW_BYTES)
    }
}

/// Everything that can come back other than a result.
///
/// Three variants, and they are different in kind: [`ToolError::Denied`] is
/// the profile's own answer and is a value the caller is expected to handle
/// (§1.4); [`ToolError::Spawn`] is the operating system failing to start a
/// program pane had already decided to allow; [`ToolError::Cancelled`] is the
/// caller having withdrawn the call. Collapsing any two would report one as
/// the other — a missing binary as a permission decision, or a withdrawal as
/// a refusal a person could fix by editing `settings.json`.
///
/// All three reach the model the same way, and that is the point of not
/// inventing a fourth shape: `runtime-contract.md` §5 already says a throw is
/// a result, so a cancelled call is an `Error` preview in the turn slot a
/// yield would have used and the turn is not retried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    Denied(PermissionDenied),
    Spawn {
        tool: String,
        program: PathBuf,
        error: String,
    },
    Cancelled {
        tool: String,
    },
}

impl ToolError {
    /// The refusal, when this was one.
    ///
    /// A cancellation is **not** one: nothing about the profile decided it,
    /// so a caller reporting `denied()` would name a settings file that has
    /// nothing to do with what happened.
    pub fn denied(&self) -> Option<&PermissionDenied> {
        match self {
            ToolError::Denied(denied) => Some(denied),
            ToolError::Spawn { .. } | ToolError::Cancelled { .. } => None,
        }
    }
}

impl From<PermissionDenied> for ToolError {
    fn from(denied: PermissionDenied) -> Self {
        ToolError::Denied(denied)
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolError::Denied(denied) => write!(f, "{denied}"),
            ToolError::Spawn {
                tool,
                program,
                error,
            } => write!(
                f,
                "{tool}: could not start {}: {error}",
                program.to_string_lossy()
            ),
            ToolError::Cancelled { tool } => {
                write!(f, "Cancelled: {tool}() was cancelled before it completed")
            }
        }
    }
}

impl std::error::Error for ToolError {}

/// What one call needs that is not the call: the session's single profile,
/// the Glasshouse seam the hooks go through, and the session id they carry.
///
/// It borrows rather than owns, which is what keeps `sandbox-grants.md`
/// §1.5 true through this layer: there is no constructor here that compiles
/// a `Profile`, so the one the session built at start-up is the only one a
/// call can be made against.
pub struct ToolContext<'a> {
    pub profile: &'a Profile,
    pub glasshouse: &'a Glasshouse,
    pub session: &'a SessionId,
}

/// One monotonic id per call in this process, so `PreToolUse` and
/// `PostToolUse` of the same call carry the same `tool_use_id` and two
/// concurrent calls never share one.
static CALLS: AtomicU64 = AtomicU64::new(0);

fn next_call_id() -> String {
    let n = CALLS.fetch_add(1, Ordering::Relaxed);
    format!("pane-{}-{n}", std::process::id())
}

/// Runs one tool call and returns its result as a value.
///
/// The order is `sandbox-grants.md` §2's, and it is the order because the
/// two questions are different: the arguments are checked (a path through
/// `Profile::check`, a command line through `Profile::admits_command`), the
/// executable is resolved, the child is confined, and only then is it
/// spawned.
///
/// `PreToolUse` fires once for every call that named a registered tool, and
/// `PostToolUse` fires once for the same call **whatever it returned** — a
/// refusal is an observed output, and a firewall that only saw successes
/// would report a program that probed a hundred paths as having done
/// nothing. An unregistered name is not a call: it fires neither, because
/// there is no tool for the event to name.
pub fn run(ctx: &ToolContext<'_>, name: &str, args: &Args) -> Result<ToolResult, ToolError> {
    run_cancellable(ctx, &CancellationToken::default(), name, args)
}

/// The same call, cancellable by whoever holds `token`.
///
/// The invariant this adds is that **a cancelled call reaches exactly the
/// same two hook deliveries a completed one does**: `PreToolUse` fires before
/// the pre-spawn check, so a call cancelled before it started anything still
/// announces itself and still reports what it returned. A firewall that saw
/// only the calls that ran would report an abandoned branch as never having
/// been attempted.
///
/// [`run`] is this function with a token nobody can set, which is why its
/// signature and its behaviour are unchanged.
pub fn run_cancellable(
    ctx: &ToolContext<'_>,
    token: &CancellationToken,
    name: &str,
    args: &Args,
) -> Result<ToolResult, ToolError> {
    run_traced(ctx, token, name, args).outcome
}

/// The arguments of one call as [`check_arguments`] admitted them — the
/// spelling that reached the child, which is what `runtime-contract.md`
/// §9.4's trajectory records. A path is `Profile::check`'s resolved path; a
/// pattern and a command line are the text the profile admitted.
pub type CheckedArgs = BTreeMap<String, String>;

/// One call's result and, beside it, its arguments as checked. A refused
/// call carries only what was admitted before the refusing argument, so a
/// spelling the profile never admitted is never written down as one it did.
pub struct Traced {
    pub outcome: Result<ToolResult, ToolError>,
    pub checked: CheckedArgs,
}

/// [`run_cancellable`], answering with the checked arguments too. The
/// trajectory is the only reader; nothing about the call itself differs.
pub fn run_traced(
    ctx: &ToolContext<'_>,
    token: &CancellationToken,
    name: &str,
    args: &Args,
) -> Traced {
    let mut checked = CheckedArgs::new();
    let Some(tool) = registry::lookup(name) else {
        return Traced {
            outcome: Err(ToolError::Denied(PermissionDenied {
                tool: name.to_string(),
                path: String::new(),
                rule: format!(
                    "no tool named `{name}` is registered; the registry declares {}",
                    registry::names().join(", ")
                ),
            })),
            checked,
        };
    };

    let call = next_call_id();
    let input = args.as_json();
    emit(
        ctx,
        json!({
            "hook_event_name": "PreToolUse",
            "session_id": ctx.session.as_str(),
            "tool_use_id": call,
            "tool_name": tool.name(),
            "tool_input": input,
        }),
    );

    let outcome = checked_call(ctx, token, tool, args, &mut checked);

    emit(
        ctx,
        json!({
            "hook_event_name": "PostToolUse",
            "session_id": ctx.session.as_str(),
            "tool_use_id": call,
            "tool_name": tool.name(),
            "tool_input": input,
            "tool_response": tool_response(tool, &outcome),
        }),
    );

    Traced { outcome, checked }
}

/// The `tool_response` the `PostToolUse` payload carries.
///
/// Two shapes, because the consumer has two: Glasshouse's context-firewall
/// adapter reads a `bash` result as `{stdout, stderr, interrupted,
/// exit_code}` and every other tool as `{type: "text", text}`. A refusal
/// renders in the shape its own tool would have, so the event parses either
/// way and the refusal is the observed output rather than a missing one.
fn tool_response(tool: &Tool, outcome: &Result<ToolResult, ToolError>) -> Value {
    let (stdout, stderr, exit_code) = match outcome {
        Ok(result) => (
            result.preview(),
            truncate(&result.stderr, PREVIEW_BYTES),
            result.exit_code,
        ),
        Err(error) => (
            String::new(),
            truncate(&error.to_string(), PREVIEW_BYTES),
            None,
        ),
    };
    if tool.name().eq_ignore_ascii_case("bash") {
        json!({
            "stdout": stdout,
            "stderr": stderr,
            "interrupted": false,
            "exit_code": exit_code,
        })
    } else {
        let text = if stderr.is_empty() {
            stdout
        } else if stdout.is_empty() {
            stderr
        } else {
            format!("{stdout}{stderr}")
        };
        json!({ "type": "text", "text": text })
    }
}

/// Delivers one hook event.
///
/// **`context-firewall hook`, not `hook`.** `glasshouse.rs` already fixed
/// that distinction — `PostToolUse` is not in Claude Code's `REPORTED_EVENTS`,
/// so a tool event sent to plain `hook` reaches no consumer at all — and this
/// module inherits it rather than re-deciding it.
fn emit(ctx: &ToolContext<'_>, payload: Value) {
    glasshouse::emit_tool_result(ctx.glasshouse, ctx.session, &payload.to_string());
}

/// One checked argument, carrying which of §2's two questions answered it.
#[derive(Debug, Clone)]
enum Checked {
    /// The path `Profile::check` **resolved**, which is the only spelling
    /// that may reach the child.
    Path(PathBuf),
    Pattern(String),
    CommandLine(String),
}

impl Checked {
    /// The spelling the trajectory records: the resolved path, or the text
    /// the profile admitted.
    fn spelling(&self) -> String {
        match self {
            Checked::Path(path) => path.to_string_lossy().into_owned(),
            Checked::Pattern(text) | Checked::CommandLine(text) => text.clone(),
        }
    }
}

fn checked_call(
    ctx: &ToolContext<'_>,
    token: &CancellationToken,
    tool: &Tool,
    args: &Args,
    trace: &mut CheckedArgs,
) -> Result<ToolResult, ToolError> {
    let checked = check_arguments(ctx.profile, tool, args, trace)?;
    // Branching here and not inside `spawn_confined` is what makes
    // `Argv::InProcess`'s claim structural: an in-process tool never reaches
    // a `Command`, an `exec_grant` or a sandbox applier, because the only
    // call site of all three is the other arm.
    if tool.argv() == Argv::InProcess {
        return perform_in_process(ctx.profile, token, tool, &checked);
    }
    let argv = build_argv(tool, &checked)?;
    let mut result = spawn_confined(ctx.profile, token, tool, &argv)?;
    if tool.name() == "read" && result.exit_code == Some(0) {
        result.modified = resolved_path(&checked, "path")
            .and_then(|path| std::fs::metadata(path).ok())
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|time| i64::try_from(time.as_millis()).ok())
            .map(|millis| crate::events::Stamp::from_millis(millis).to_string());
    }
    Ok(result)
}

/// Performs a tool pane does itself. One tool today, and the match is
/// exhaustive on name so a second in-process tool cannot be added without
/// deciding what it does here.
fn perform_in_process(
    profile: &Profile,
    token: &CancellationToken,
    tool: &Tool,
    checked: &[(&'static str, Checked)],
) -> Result<ToolResult, ToolError> {
    let refuse = |rule: String| {
        ToolError::Denied(PermissionDenied {
            tool: tool.name().to_string(),
            path: String::new(),
            rule,
        })
    };
    match tool.name() {
        "glob" => {
            let (Some(root), Some(pattern)) =
                (resolved_path(checked, "path"), text(checked, "pattern"))
            else {
                return Err(refuse("glob needs a checked path and pattern".to_string()));
            };
            let stdout = glob_paths(profile, token, tool.name(), root, pattern)?;
            Ok(ToolResult {
                modified: None,
                tool: tool.name().to_string(),
                stdout,
                stderr: String::new(),
                exit_code: Some(0),
                grant: ExecGrant {
                    binary: PathBuf::new(),
                    fell_back_to_roots: false,
                },
                confinement: Confinement::InProcess,
            })
        }
        "write" => {
            let (Some(path), Some(content)) =
                (resolved_path(checked, "path"), text(checked, "content"))
            else {
                return Err(refuse("write needs a checked path and content".to_string()));
            };
            // The parent is created, because a model that has to `mkdir -p`
            // through `bash` before every `write` gains nothing from having
            // `write`. It is inside the checked path by construction, so it
            // reaches nowhere the write itself could not.
            if let Some(parent) = path.parent()
                && let Err(error) = std::fs::create_dir_all(parent)
            {
                return Err(ToolError::Spawn {
                    tool: tool.name().to_string(),
                    program: PathBuf::from("(in-process)"),
                    error: error.to_string(),
                });
            }
            match std::fs::write(path, content) {
                Ok(()) => Ok(ToolResult {
                    modified: None,
                    tool: tool.name().to_string(),
                    stdout: format!("wrote {} bytes to {}", content.len(), path.display()),
                    stderr: String::new(),
                    exit_code: Some(0),
                    grant: ExecGrant {
                        binary: PathBuf::new(),
                        fell_back_to_roots: false,
                    },
                    confinement: Confinement::InProcess,
                }),
                Err(error) => Err(ToolError::Spawn {
                    tool: tool.name().to_string(),
                    program: PathBuf::from("(in-process)"),
                    error: error.to_string(),
                }),
            }
        }
        other => Err(refuse(format!(
            "`{other}` is declared in-process and nothing here performs it"
        ))),
    }
}

/// Walks a checked root and matches slash-separated patterns against paths
/// relative to it. A denied directory is pruned before `read_dir`, which
/// keeps names beneath it from becoming an enumeration side channel.
fn glob_paths(
    profile: &Profile,
    token: &CancellationToken,
    tool: &str,
    root: &Path,
    pattern: &str,
) -> Result<String, ToolError> {
    const MAX_VISITED: usize = 100_000;

    let normalized_pattern = pattern.replace('\\', "/");
    let pattern: Vec<&str> = normalized_pattern
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if pattern.is_empty() {
        return Ok(String::new());
    }

    let mut pending = vec![root.to_path_buf()];
    let mut matches = Vec::new();
    let mut visited = 0usize;
    while let Some(directory) = pending.pop() {
        if token.is_cancelled() {
            return Err(ToolError::Cancelled {
                tool: tool.to_string(),
            });
        }
        let entries = std::fs::read_dir(&directory).map_err(|error| ToolError::Spawn {
            tool: tool.to_string(),
            program: PathBuf::from("(in-process)"),
            error: error.to_string(),
        })?;
        for entry in entries {
            if token.is_cancelled() {
                return Err(ToolError::Cancelled {
                    tool: tool.to_string(),
                });
            }
            visited += 1;
            if visited > MAX_VISITED {
                return Err(ToolError::Spawn {
                    tool: tool.to_string(),
                    program: PathBuf::from("(in-process)"),
                    error: format!("glob stopped after {MAX_VISITED} directory entries"),
                });
            }
            let entry = entry.map_err(|error| ToolError::Spawn {
                tool: tool.to_string(),
                program: PathBuf::from("(in-process)"),
                error: error.to_string(),
            })?;
            let path = entry.path();
            let Ok(checked) = profile.check(tool, Access::Read, &path) else {
                continue;
            };
            let relative = checked.strip_prefix(root).unwrap_or(&checked);
            let components: Vec<String> = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            if glob_components(&pattern, &components) {
                matches.push(checked.clone());
            }
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                pending.push(checked);
            }
        }
    }
    matches.sort();
    Ok(matches
        .into_iter()
        .map(|path| format!("{}\n", path.display()))
        .collect())
}

fn glob_components(pattern: &[&str], path: &[String]) -> bool {
    let mut matched = vec![vec![false; path.len() + 1]; pattern.len() + 1];
    matched[0][0] = true;
    for (index, part) in pattern.iter().enumerate() {
        if *part == "**" {
            for depth in 0..=path.len() {
                matched[index + 1][depth] |= matched[index][depth];
                if depth < path.len() && matched[index + 1][depth] {
                    matched[index + 1][depth + 1] = true;
                }
            }
        } else {
            for depth in 0..path.len() {
                if matched[index][depth] && glob_segment(part, &path[depth]) {
                    matched[index + 1][depth + 1] = true;
                }
            }
        }
    }
    matched[pattern.len()][path.len()]
}

fn glob_segment(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let mut matched = vec![vec![false; text.len() + 1]; pattern.len() + 1];
    matched[0][0] = true;
    for (index, byte) in pattern.iter().enumerate() {
        for offset in 0..=text.len() {
            if *byte == '*' {
                matched[index + 1][offset] |= matched[index][offset];
                if offset < text.len() && matched[index + 1][offset] {
                    matched[index + 1][offset + 1] = true;
                }
            } else if offset < text.len()
                && matched[index][offset]
                && (*byte == '?' || *byte == text[offset])
            {
                matched[index + 1][offset + 1] = true;
            }
        }
    }
    matched[pattern.len()][text.len()]
}

/// Admits one argument: onto the argv list, and into the trajectory's
/// record of what was checked.
fn admit(
    checked: &mut Vec<(&'static str, Checked)>,
    trace: &mut CheckedArgs,
    name: &'static str,
    value: Checked,
) {
    trace.insert(name.to_string(), value.spelling());
    checked.push((name, value));
}

/// Checks every declared argument, and refuses every undeclared one.
///
/// An undeclared name is refused rather than ignored: a call carrying
/// `path` to a tool that has no `path` would otherwise run against the
/// project root and look like it had honoured the argument.
fn check_arguments(
    profile: &Profile,
    tool: &Tool,
    args: &Args,
    trace: &mut CheckedArgs,
) -> Result<Vec<(&'static str, Checked)>, PermissionDenied> {
    for given in args.names() {
        if !tool.args().iter().any(|arg| arg.name() == given) {
            return Err(PermissionDenied {
                tool: tool.name().to_string(),
                path: given.to_string(),
                rule: format!("`{}` declares no argument named `{given}`", tool.name()),
            });
        }
    }

    let mut checked = Vec::new();
    for arg in tool.args() {
        let given = args.get(arg.name());
        match (arg.kind(), given) {
            (ArgKind::Path, Some(value)) => {
                let resolved = profile.check(tool.name(), Access::Read, Path::new(value))?;
                admit(&mut checked, trace, arg.name(), Checked::Path(resolved));
            }
            // The same check, asked with the other access. A path the profile
            // grants for reading and not for writing is refused here, which
            // is the whole difference between the two kinds.
            (ArgKind::WritePath, Some(value)) => {
                let resolved = profile.check(tool.name(), Access::Write, Path::new(value))?;
                admit(&mut checked, trace, arg.name(), Checked::Path(resolved));
            }
            // The project root stands in for a missing path, and it is
            // checked rather than trusted: `Profile::check` is what says the
            // root is reachable, and a `deny` rule naming it would refuse
            // here exactly as it would for any other path.
            (ArgKind::Path, None) => {
                let root = profile.root().to_path_buf();
                let resolved = profile.check(tool.name(), Access::Read, &root)?;
                admit(&mut checked, trace, arg.name(), Checked::Path(resolved));
            }
            (ArgKind::Pattern, Some(value)) => {
                admit(
                    &mut checked,
                    trace,
                    arg.name(),
                    Checked::Pattern(value.to_string()),
                );
            }
            (ArgKind::CommandLine, Some(value)) => {
                profile.admits_command(value)?;
                admit(
                    &mut checked,
                    trace,
                    arg.name(),
                    Checked::CommandLine(value.to_string()),
                );
            }
            (_, None) => {
                return Err(PermissionDenied {
                    tool: tool.name().to_string(),
                    path: arg.name().to_string(),
                    rule: format!(
                        "`{}` requires an argument named `{}`",
                        tool.name(),
                        arg.name()
                    ),
                });
            }
        }
    }
    Ok(checked)
}

fn resolved_path<'a>(checked: &'a [(&'static str, Checked)], name: &str) -> Option<&'a Path> {
    checked.iter().find_map(|(declared, value)| match value {
        Checked::Path(path) if *declared == name => Some(path.as_path()),
        _ => None,
    })
}

fn text<'a>(checked: &'a [(&'static str, Checked)], name: &str) -> Option<&'a str> {
    checked.iter().find_map(|(declared, value)| match value {
        Checked::Pattern(text) | Checked::CommandLine(text) if *declared == name => {
            Some(text.as_str())
        }
        _ => None,
    })
}

/// The child's argv, built only from checked values.
///
/// Nothing here quotes anything, because nothing here builds a string for a
/// shell: each element is one `execvp` argument. `bash` is the one tool
/// whose argument *is* a command line, and it got there through
/// `Profile::admits_command` rather than through this function.
fn build_argv(
    tool: &Tool,
    checked: &[(&'static str, Checked)],
) -> Result<Vec<std::ffi::OsString>, PermissionDenied> {
    let missing = |what: &str| PermissionDenied {
        tool: tool.name().to_string(),
        path: what.to_string(),
        rule: format!(
            "`{}`'s declaration and its argv shape disagree about `{what}`",
            tool.name()
        ),
    };
    let mut argv: Vec<std::ffi::OsString> = Vec::new();
    match tool.argv() {
        // Unreachable through `checked_call`, which branches first; a direct
        // caller gets an empty argv rather than a panic, and `spawn_confined`
        // refuses it for having no binary.
        Argv::InProcess => {}
        Argv::ReadPath => {
            let path = resolved_path(checked, "path").ok_or_else(|| missing("path"))?;
            argv.push("--".into());
            argv.push(path.into());
        }
        Argv::GrepIn => {
            let pattern = text(checked, "pattern").ok_or_else(|| missing("pattern"))?;
            let path = resolved_path(checked, "path").ok_or_else(|| missing("path"))?;
            argv.push("-r".into());
            argv.push("-n".into());
            argv.push("-e".into());
            argv.push(pattern.into());
            argv.push("--".into());
            argv.push(path.into());
        }
        Argv::ShellCommand => {
            let command = text(checked, "command").ok_or_else(|| missing("command"))?;
            argv.push("-c".into());
            argv.push(command.into());
        }
    }
    Ok(argv)
}

/// Resolves `program` to the binary the child will be exec'd on.
///
/// This is the 61D ruling's mechanism. A name is looked up on `PATH` and
/// then `canonicalize`d, so a `PATH` entry that is a symlink, a relative
/// component or a `..` produces the real path and not the spelling that
/// found it. A name that resolves to nothing is **not** an error here: the
/// call falls back to letting `execvp` search, bounded by the platform
/// applier's executable roots, and says so — in the returned value and in a
/// log line, because the two have different readers.
pub fn exec_grant(program: &str) -> ExecGrant {
    if let Some(binary) = resolve_program(program) {
        return ExecGrant {
            binary,
            fell_back_to_roots: false,
        };
    }
    eprintln!(
        "pane: sandbox: `{program}` did not resolve to a binary on PATH; exec falls back to the \
         platform applier's executable roots (61D exec-roots ruling)"
    );
    ExecGrant {
        binary: PathBuf::from(program),
        fell_back_to_roots: true,
    }
}

fn resolve_program(program: &str) -> Option<PathBuf> {
    let candidate = Path::new(program);
    if candidate.components().count() > 1 {
        return runnable(candidate).then(|| std::fs::canonicalize(candidate).ok())?;
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .filter(|candidate| runnable(candidate))
        .find_map(|candidate| std::fs::canonicalize(candidate).ok())
}

#[cfg(unix)]
fn runnable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn runnable(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.is_file())
        .unwrap_or(false)
}

/// Confines the child and spawns it, in that order and in no other.
///
/// The `Command` is built, [`confine`] installs the platform's mechanism on
/// it, and the spawn comes after. A `?` on the confinement is what makes "no
/// unconfined path" mechanical rather than promised: the only expression that
/// runs the child is below a confinement that returned `Ok`.
///
/// The child is spawned rather than run to completion because a cancellation
/// needs a [`Child`] to kill; the two drain threads are what `output()` did
/// internally, and the poll loop is where `token` is asked. Three outcomes:
///
/// - set before the spawn: no child is ever created, and the check is the
///   last statement before `spawn()`, so "never created" is a property of the
///   control flow and not of a race;
/// - set while the child runs: `kill` then `wait`, because the `wait` is what
///   leaves nothing behind for `init` to reap;
/// - neither: exactly the `ToolResult` this function returned before.
fn spawn_confined(
    profile: &Profile,
    token: &CancellationToken,
    tool: &Tool,
    argv: &[std::ffi::OsString],
) -> Result<ToolResult, ToolError> {
    let cancelled = || ToolError::Cancelled {
        tool: tool.name().to_string(),
    };
    // `Some` by construction: `checked_call` sends every `Argv::InProcess`
    // tool down the other arm, and those are the only ones without a binary.
    let Some(executable) = tool.executable() else {
        return Err(ToolError::Spawn {
            tool: tool.name().to_string(),
            program: PathBuf::new(),
            error: "an in-process tool reached spawn_confined".to_string(),
        });
    };
    let grant = exec_grant(executable);
    let mut command = Command::new(&grant.binary);
    command.args(argv);
    command.current_dir(profile.root());
    // **The child does not inherit this session's credentials.**
    //
    // `sandbox-grants.md` §4.2 makes the OS keyring never-grantable and §4.1
    // denies the network; a provider key sitting in the environment is the
    // same class of secret and was the one route left to it. Measured
    // 2026-09-06: `printenv ANTHROPIC_API_KEY` from inside a cell returned
    // the key, and a cell's output reaches the transcript, the rollout file
    // on disk and every hook payload -- an exfiltration path that needs no
    // network at all.
    //
    // Removed by name rather than by value: a value-matching scrub cannot
    // see a credential this process never read, and `env_clear` would take
    // `PATH` and `HOME` with it and break every tool.
    for name in std::env::vars_os()
        .map(|(name, _)| name)
        .filter(|name| is_credential_variable(&name.to_string_lossy()))
    {
        command.env_remove(name);
    }

    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    // The child leads a process group of its own, so a cancellation can name
    // everything the call started and not only the handle it holds. See
    // [`kill_and_reap`] for why that is the difference between stopping a
    // call and stopping a process.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let confinement = confine(profile, &grant.binary, tool.name(), &mut command)?;

    if token.is_cancelled() {
        return Err(cancelled());
    }

    let spawn_failed = |error: std::io::Error| ToolError::Spawn {
        tool: tool.name().to_string(),
        program: grant.binary.clone(),
        error: error.to_string(),
    };
    let mut child = command.spawn().map_err(spawn_failed)?;

    // A cancellation that landed in the window between the check above and
    // this line now has something to stop, and the poll loop below would not
    // ask the token again for a whole `CANCEL_POLL`. Killing here rather
    // than a tick later is not about the latency: it is that this is the
    // window a caller is *most* likely to cancel in, because a caller that
    // cancels at all usually cancels early.
    if token.is_cancelled() {
        kill_and_reap(&mut child);
        return Err(cancelled());
    }

    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if token.is_cancelled() => {
                kill_and_reap(&mut child);
                return Err(cancelled());
            }
            Ok(None) => std::thread::sleep(CANCEL_POLL),
            // The wait itself failing leaves a running child nothing here
            // can observe again, so it is killed on the way out. Reporting
            // it as `Spawn` is not new lumping: `output()` raised the same
            // variant for its own wait and read failures.
            Err(error) => {
                kill_and_reap(&mut child);
                return Err(spawn_failed(error));
            }
        }
    };

    Ok(ToolResult {
        modified: None,
        tool: tool.name().to_string(),
        stdout: collect(stdout),
        stderr: collect(stderr),
        exit_code: status.code(),
        grant,
        confinement,
    })
}

/// Reads one of the child's pipes to EOF on a thread of its own.
///
/// The invariant is that neither pipe can fill while the other is being
/// read, which is what `Command::output` did internally and what this
/// function restores now that the wait is explicit.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buffer);
        }
        buffer
    })
}

/// What the drain thread read, or nothing if it panicked — which a
/// `read_to_end` into a `Vec` does not do, so the `unwrap_or_default` is a
/// shape and not a fallback.
fn collect(handle: JoinHandle<Vec<u8>>) -> String {
    let bytes = handle.join().unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Kills everything the call started and reaps the child, in that order, so
/// nothing is left for `init`.
///
/// **The group is killed first, and the group is the point.** A [`Child`]
/// handle names the process pane spawned and nothing that process started,
/// so killing the handle alone stops the shell and leaves its background
/// jobs running — a `bash` call that started a server, cancelled, leaves the
/// server spinning at 100% until the machine is rebooted. That is not
/// hypothetical: `tests/tools.rs` reproduces it in one fixture.
///
/// Killing a group is safe here only because [`spawn_confined`] *created*
/// this one: `process_group(0)` makes the child a group leader whose group
/// id is its own pid, so the members are exactly the processes this call
/// started. Killing a group pane did not create is how a cancellation
/// becomes an outage, which is why the group is established at the spawn
/// rather than guessed at the kill. The order matters for the same reason:
/// once `wait` has reaped the child, its pid — and therefore the group id —
/// may be reused, so the group must be signalled while the child is still
/// unreaped.
///
/// Every result is discarded deliberately. A child that exited between the
/// poll and the `kill` is not an error, it is the race this function exists
/// to be indifferent to, and a group that no longer has members answers
/// `ESRCH` for the same reason.
///
/// The two drain threads are *not* joined here. Their output is discarded by
/// a cancelled call, and joining them would make cancellation wait on a
/// grandchild that inherited the pipe — the one thing a bounded cancellation
/// must not do.
fn kill_and_reap(child: &mut Child) {
    #[cfg(unix)]
    kill_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

/// `SIGKILL`, which is 9 on every unix pane builds for.
#[cfg(unix)]
const SIGKILL: i32 = 9;

#[cfg(unix)]
unsafe extern "C" {
    /// POSIX `killpg`. Declared rather than depended on: pane has no `libc`
    /// in its tree, and this is the whole of what it would be used for.
    fn killpg(pgrp: i32, sig: i32) -> i32;
}

/// `SIGKILL`s the process group led by `pid`.
///
/// `pid` is a child [`spawn_confined`] made a group leader, so the group id
/// is the pid and its members are exactly what that call started.
#[cfg(unix)]
fn kill_group(pid: u32) {
    // SAFETY: `killpg` is a POSIX libc call taking two integers and
    // returning one. There is no pointer, no allocation and no state; the
    // only failure it can report is `ESRCH` for a group whose members have
    // all exited, which is the ordinary case and is discarded above.
    unsafe {
        killpg(pid as i32, SIGKILL);
    }
}

/// Installs this platform's confinement on `command`, or refuses.
///
/// A platform with no applier that has ever executed refuses rather than
/// spawning: `sandbox-grants.md` §3 gives Windows a restricted token and an
/// AppContainer, and `sandbox/windows.rs` says in its own documentation that
/// nothing there has been run. Spawning there "for now" would be the one
/// unconfined path this module exists to not have.
#[cfg(target_os = "macos")]
fn confine(
    profile: &Profile,
    binary: &Path,
    tool: &str,
    command: &mut Command,
) -> Result<Confinement, PermissionDenied> {
    crate::sandbox::macos::confine(profile, binary, command)
        .map(|()| Confinement::Seatbelt)
        .map_err(|error| PermissionDenied {
            tool: tool.to_string(),
            path: String::new(),
            rule: format!(
                "the seatbelt profile could not be applied, so nothing was spawned: {error}"
            ),
        })
}

#[cfg(target_os = "linux")]
fn confine(
    profile: &Profile,
    binary: &Path,
    tool: &str,
    command: &mut Command,
) -> Result<Confinement, PermissionDenied> {
    let refused = |rule: String| PermissionDenied {
        tool: tool.to_string(),
        path: String::new(),
        rule,
    };
    match crate::sandbox::linux::confine(profile, binary, command) {
        Ok(true) => Ok(Confinement::Landlock),
        // `linux::confine` returns `Ok(false)` below Landlock ABI 3 and
        // installs nothing. That is a refusal here rather than a warning.
        Ok(false) => Err(refused(
            "this kernel's Landlock ABI is below 3, so no ruleset could be installed and pane \
             does not spawn a tool unconfined (sandbox-grants.md §3)"
                .to_string(),
        )),
        Err(error) => Err(refused(format!(
            "the Landlock ruleset could not be installed, so nothing was spawned: {error}"
        ))),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn confine(
    profile: &Profile,
    binary: &Path,
    tool: &str,
    command: &mut Command,
) -> Result<Confinement, PermissionDenied> {
    let _ = (profile, binary, command);
    Err(PermissionDenied {
        tool: tool.to_string(),
        path: String::new(),
        rule: "pane has no sandbox applier that has ever executed on this platform, and does not \
               spawn a tool unconfined (sandbox-grants.md §3)"
            .to_string(),
    })
}

/// Truncates on a character boundary, marking that it did.
fn truncate(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[{} bytes truncated]", &text[..end], text.len() - end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_tool_name_is_a_refusal_and_not_a_panic() {
        let profile = Profile::compile(std::env::temp_dir(), None);
        let glasshouse = Glasshouse::None;
        let session = SessionId::new("test");
        let ctx = ToolContext {
            profile: &profile,
            glasshouse: &glasshouse,
            session: &session,
        };
        let error = run(&ctx, "webfetch", &Args::new()).unwrap_err();
        let denied = error.denied().expect("a refusal, not a spawn failure");
        assert_eq!(denied.tool, "webfetch");
        assert!(
            denied
                .rule
                .contains("no tool named `webfetch` is registered")
        );
    }

    #[test]
    fn an_undeclared_argument_is_refused_rather_than_ignored() {
        let profile = Profile::compile(std::env::temp_dir(), None);
        let tool = registry::lookup("read").unwrap();
        let args = Args::new().with("path", "x").with("depth", "3");
        let denied = check_arguments(&profile, tool, &args, &mut CheckedArgs::new()).unwrap_err();
        assert_eq!(denied.path, "depth");
        assert!(denied.rule.contains("declares no argument named `depth`"));
    }

    #[test]
    fn a_missing_required_argument_is_refused() {
        let profile = Profile::compile(std::env::temp_dir(), None);
        let tool = registry::lookup("grep").unwrap();
        let denied =
            check_arguments(&profile, tool, &Args::new(), &mut CheckedArgs::new()).unwrap_err();
        assert!(denied.rule.contains("requires an argument named `pattern`"));
    }

    #[test]
    fn a_pattern_never_becomes_an_option_of_the_child() {
        let profile = Profile::compile(std::env::temp_dir(), None);
        let tool = registry::lookup("grep").unwrap();
        let args = Args::new().with("pattern", "-rf");
        let mut trace = CheckedArgs::new();
        let checked = check_arguments(&profile, tool, &args, &mut trace).unwrap();
        // The trajectory records the pattern as admitted, and only that.
        assert_eq!(trace.get("pattern").map(String::as_str), Some("-rf"));
        let argv = build_argv(tool, &checked).unwrap();
        let position = argv.iter().position(|a| a == "-rf").unwrap();
        assert_eq!(argv[position - 1], "-e", "{argv:?}");
    }

    #[test]
    fn truncation_marks_itself_and_keeps_a_character_boundary() {
        let text = "é".repeat(40);
        let cut = truncate(&text, 11);
        assert!(cut.starts_with("ééééé"));
        assert!(cut.contains("bytes truncated"));
        assert_eq!(truncate("short", 11), "short");
    }
}
