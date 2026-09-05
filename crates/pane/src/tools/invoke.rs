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
//! **Nothing model-authored runs here.** The only programs this module can
//! spawn are the four in [`registry::ALL`], each resolved from a name fixed
//! at compile time; there is no argument, no path and no branch through
//! which assistant text selects or becomes a program.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

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

/// Which mechanism confined the child. There is no `Unconfined` variant, and
/// that is the module's first invariant expressed as a type: a platform with
/// nothing to install returns [`PermissionDenied`] instead of a value of this
/// type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confinement {
    /// macOS seatbelt, entered between `fork` and `exec`.
    Seatbelt,
    /// Linux Landlock, installed on the forked child before `exec`.
    Landlock,
}

impl Confinement {
    pub fn as_str(self) -> &'static str {
        match self {
            Confinement::Seatbelt => "seatbelt",
            Confinement::Landlock => "landlock",
        }
    }
}

/// What one call returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
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
/// Two variants, and they are different in kind: [`ToolError::Denied`] is the
/// profile's own answer and is a value the caller is expected to handle
/// (§1.4), while [`ToolError::Spawn`] is the operating system failing to
/// start a program pane had already decided to allow. Collapsing them would
/// report a missing binary as a permission decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    Denied(PermissionDenied),
    Spawn {
        tool: String,
        program: PathBuf,
        error: String,
    },
}

impl ToolError {
    /// The refusal, when this was one.
    pub fn denied(&self) -> Option<&PermissionDenied> {
        match self {
            ToolError::Denied(denied) => Some(denied),
            ToolError::Spawn { .. } => None,
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
    let Some(tool) = registry::lookup(name) else {
        return Err(ToolError::Denied(PermissionDenied {
            tool: name.to_string(),
            path: String::new(),
            rule: format!(
                "no tool named `{name}` is registered; the registry declares {}",
                registry::names().join(", ")
            ),
        }));
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

    let outcome = checked_call(ctx, tool, args);

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

    outcome
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

fn checked_call(ctx: &ToolContext<'_>, tool: &Tool, args: &Args) -> Result<ToolResult, ToolError> {
    let checked = check_arguments(ctx.profile, tool, args)?;
    let argv = build_argv(tool, &checked)?;
    spawn_confined(ctx.profile, tool, &argv)
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
                checked.push((arg.name(), Checked::Path(resolved)));
            }
            // The project root stands in for a missing path, and it is
            // checked rather than trusted: `Profile::check` is what says the
            // root is reachable, and a `deny` rule naming it would refuse
            // here exactly as it would for any other path.
            (ArgKind::Path, None) => {
                let root = profile.root().to_path_buf();
                let resolved = profile.check(tool.name(), Access::Read, &root)?;
                checked.push((arg.name(), Checked::Path(resolved)));
            }
            (ArgKind::Pattern, Some(value)) => {
                checked.push((arg.name(), Checked::Pattern(value.to_string())));
            }
            (ArgKind::CommandLine, Some(value)) => {
                profile.admits_command(value)?;
                checked.push((arg.name(), Checked::CommandLine(value.to_string())));
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
        Argv::FindNamed => {
            let pattern = text(checked, "pattern").ok_or_else(|| missing("pattern"))?;
            let path = resolved_path(checked, "path").ok_or_else(|| missing("path"))?;
            argv.push(path.into());
            argv.push("-name".into());
            argv.push(pattern.into());
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
/// it, and `output()` comes after. A `?` on the confinement is what makes
/// "no unconfined path" mechanical rather than promised: the only expression
/// that runs the child is below a confinement that returned `Ok`.
fn spawn_confined(
    profile: &Profile,
    tool: &Tool,
    argv: &[std::ffi::OsString],
) -> Result<ToolResult, ToolError> {
    let grant = exec_grant(tool.executable());
    let mut command = Command::new(&grant.binary);
    command.args(argv);
    command.current_dir(profile.root());
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let confinement = confine(profile, &grant.binary, tool.name(), &mut command)?;

    let output = command.output().map_err(|error| ToolError::Spawn {
        tool: tool.name().to_string(),
        program: grant.binary.clone(),
        error: error.to_string(),
    })?;

    Ok(ToolResult {
        tool: tool.name().to_string(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code(),
        grant,
        confinement,
    })
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
        let denied = check_arguments(&profile, tool, &args).unwrap_err();
        assert_eq!(denied.path, "depth");
        assert!(denied.rule.contains("declares no argument named `depth`"));
    }

    #[test]
    fn a_missing_required_argument_is_refused() {
        let profile = Profile::compile(std::env::temp_dir(), None);
        let tool = registry::lookup("grep").unwrap();
        let denied = check_arguments(&profile, tool, &Args::new()).unwrap_err();
        assert!(denied.rule.contains("requires an argument named `pattern`"));
    }

    #[test]
    fn a_pattern_never_becomes_an_option_of_the_child() {
        let profile = Profile::compile(std::env::temp_dir(), None);
        let tool = registry::lookup("grep").unwrap();
        let args = Args::new().with("pattern", "-rf");
        let checked = check_arguments(&profile, tool, &args).unwrap();
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
